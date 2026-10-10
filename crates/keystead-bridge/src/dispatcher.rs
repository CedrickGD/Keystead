//! Request dispatch: authentication (pairing), lock state, pairing flow and
//! unlock rate limiting on top of a [`VaultBackend`] implemented by the app.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use keystead_core::generator::GeneratorOptions;
use keystead_core::model::ItemSummary;
use keystead_core::totp::TotpCode;
use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::clients::{ClientStore, PairedClient};
use crate::error::Result;
use crate::protocol::{
    BridgeError, CopyData, CopyField, ExtensionInfo, IdData, ListVaultsData, LoginSecret, PairData,
    Payload, Request, Response, StatusData, UnlockData, VaultSummary, PAIRING_TIMEOUT,
    SEARCH_LIMIT,
};
use crate::server::BridgeHandler;
use crate::util::{lock, log};

/// Maximum length (in characters) of a client name.
pub const MAX_CLIENT_NAME_CHARS: usize = 100;
/// How often a waiting `pair` request checks whether its connection is gone.
const PEER_CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// At most this many pairing requests can wait for the user at once.
const MAX_PENDING_PAIRINGS: usize = 4;

/// What the app provides to the bridge. Called from connection threads,
/// possibly concurrently. Errors are protocol codes; `From<keystead_core::Error>`
/// is implemented for [`BridgeError`] to make `?` convenient.
pub trait VaultBackend: Send + Sync + 'static {
    /// The app version, e.g. `"2.0.0"`.
    fn app_version(&self) -> String;
    /// The unlocked vault, `None` while locked.
    fn unlocked_vault(&self) -> Option<VaultSummary>;
    /// The browser extension the app delivers (version and folder), shown
    /// to paired clients in `status`. Default: none.
    fn extension_info(&self) -> Option<ExtensionInfo> {
        None
    }
    /// All vaults (any order; the dispatcher sorts them) and the id of the
    /// vault used last, if the app remembers one.
    fn list_vaults(&self) -> std::result::Result<(Vec<VaultSummary>, Option<String>), BridgeError>;
    /// Unlocks `vault_id` – or, if `None`, the vault the app would unlock
    /// (the last used one, else the only one) – with `password` and returns
    /// it. `NotFound` for an unknown id (or no vault to pick),
    /// `WrongPassword` on a bad password. If another vault is open, a
    /// successful unlock replaces it (closed like on lock); a failed one
    /// leaves it open. Unlocking the open vault again only checks the
    /// password.
    fn unlock(
        &self,
        vault_id: Option<&str>,
        password: &str,
    ) -> std::result::Result<VaultSummary, BridgeError>;
    /// Locks the vault (no-op if locked).
    fn lock(&self);
    /// Shows and raises the app window.
    fn focus_app(&self);
    /// Asks the user to approve a pairing (emit `bridge://pairing-request`).
    /// Must not block; the answer comes via [`Dispatcher::respond_pairing`].
    fn request_pairing(&self, request_id: &str, client_name: &str, code: &str);
    /// A pairing request announced by [`VaultBackend::request_pairing`] ended
    /// without the user's decision: the browser cancelled it (its connection
    /// closed), a newer request of the same client replaced it, or it timed
    /// out. [`Dispatcher::respond_pairing`] no longer accepts it; e.g. close
    /// the dialog.
    fn pairing_closed(&self, request_id: &str) {
        let _ = request_id;
    }
    /// Logins matching the page URL (favorites first).
    fn logins_for_url(&self, url: &str) -> std::result::Result<Vec<ItemSummary>, BridgeError>;
    /// Search over the unlocked vault (the dispatcher caps the result at 50).
    fn search(&self, query: &str) -> std::result::Result<Vec<ItemSummary>, BridgeError>;
    /// Secrets of one login; `NotFound` for unknown ids and non-logins.
    fn get_login(&self, item_id: &str) -> std::result::Result<LoginSecret, BridgeError>;
    /// Current TOTP code of a login; `NotFound` if it has no TOTP seed.
    fn get_totp(&self, item_id: &str) -> std::result::Result<TotpCode, BridgeError>;
    /// Generates a password (and stores it in the generator history if
    /// unlocked). Invalid options → `InvalidRequest`.
    fn generate_password(
        &self,
        options: GeneratorOptions,
    ) -> std::result::Result<String, BridgeError>;
    /// Creates a new login; returns its id.
    fn save_login(
        &self,
        name: &str,
        url: &str,
        username: &str,
        password: &str,
    ) -> std::result::Result<String, BridgeError>;
    /// Replaces the password of a login; returns its id.
    fn update_password(
        &self,
        item_id: &str,
        password: &str,
    ) -> std::result::Result<String, BridgeError>;
    /// Puts a secret the browser's popup copies (`copy_field`,
    /// `copy_secret`) on the system clipboard: excluded from clipboard
    /// history and cleared after the user's `clipboardClearSeconds`
    /// (0 = never), like the app's own copies – so the extension never
    /// leaves a password on the clipboard indefinitely.
    ///
    /// The default does exactly that with
    /// [`keystead_core::clipboard::copy_secret`] in this (the app's)
    /// process, reading the delay from the settings file
    /// ([`keystead_core::settings::load`]); since the copy goes through the
    /// core clipboard module, the app's "clear on lock" covers it as well.
    /// An app that holds its settings in memory may override it.
    fn copy_secret(&self, text: &str) -> std::result::Result<(), BridgeError> {
        let seconds = keystead_core::settings::load().clipboard_clear_seconds;
        let clear_after = (seconds > 0).then(|| Duration::from_secs(u64::from(seconds)));
        keystead_core::clipboard::copy_secret(text, clear_after).map_err(|e| {
            log(format_args!(
                "could not copy to the clipboard: {}",
                e.code()
            ));
            BridgeError::Internal
        })
    }
    /// `copy_field`: puts a login's password or current TOTP code (without
    /// spaces) on the clipboard like [`VaultBackend::copy_secret`]; returns
    /// the seconds the code stays valid (`None` for a password). The default
    /// reads it with [`VaultBackend::get_login`] / [`VaultBackend::get_totp`]
    /// and copies it with [`VaultBackend::copy_secret`]; an app may override
    /// it to read and copy as one step (the desktop app copies nothing if
    /// the vault was locked or switched in between, so the closed vault's
    /// secret never lands on the clipboard after its clear ran).
    fn copy_field(
        &self,
        item_id: &str,
        field: CopyField,
    ) -> std::result::Result<Option<u32>, BridgeError> {
        let (text, remaining) = match field {
            CopyField::Password => (
                Zeroizing::new(self.get_login(item_id)?.password.clone()),
                None,
            ),
            CopyField::Totp => {
                let code = self.get_totp(item_id)?;
                let digits: String = code.code.chars().filter(|c| !c.is_whitespace()).collect();
                (Zeroizing::new(digits), Some(code.remaining))
            }
        };
        self.copy_secret(&text)?;
        Ok(remaining)
    }
    /// Called after a successful deliberate user action from the browser
    /// (see [`Payload::is_user_action`]), e.g. to reset the auto-lock timer.
    fn on_activity(&self) {}
}

/// Tunables (defaults per contract; tests use short timeouts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatcherConfig {
    /// How long `pair` waits for the user (default 120 s).
    pub pairing_timeout: Duration,
    /// Wrong passwords before unlocking is suspended (default 5): summed
    /// over all vaults; a success clears only those of the vault it opened.
    pub max_unlock_failures: u32,
    /// How long unlocking is suspended (default 30 s).
    pub unlock_lockout: Duration,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        DispatcherConfig {
            pairing_timeout: PAIRING_TIMEOUT,
            max_unlock_failures: 5,
            unlock_lockout: Duration::from_secs(30),
        }
    }
}

/// A pairing request waiting for the user's decision (payload of the
/// `bridge://pairing-request` event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingRequest {
    pub request_id: String,
    pub client_name: String,
    pub code: String,
}

/// Wrong passwords of `unlock`, counted per vault: the limit applies to
/// their sum (all vaults together), but a success only clears the failures
/// of the vault it opened or re-checked – so knowing the password of one
/// vault never resets the count for another.
#[derive(Debug, Default)]
struct UnlockLimiter {
    /// Key: the requested vault id, `""` for a request without one (the
    /// backend's default vault). Only ids of existing vaults get here
    /// (unknown ids are `not_found`, which does not count).
    failures: HashMap<String, u32>,
    blocked_until: Option<Instant>,
}

impl UnlockLimiter {
    fn total(&self) -> u32 {
        self.failures
            .values()
            .fold(0u32, |sum, n| sum.saturating_add(*n))
    }
}

/// The [`BridgeHandler`] of the app. Share it as `Arc<Dispatcher>`: one
/// clone goes to [`start_server`](crate::server::start_server), the app
/// keeps another for [`Dispatcher::respond_pairing`], `clients` and `revoke`.
pub struct Dispatcher {
    backend: Arc<dyn VaultBackend>,
    clients: Mutex<ClientStore>,
    pairing: PairingBroker,
    /// Held for the whole unlock attempt, so concurrent attempts cannot
    /// slip past the rate limit.
    unlock_limiter: Mutex<UnlockLimiter>,
    config: DispatcherConfig,
}

impl Dispatcher {
    /// A dispatcher with the default configuration.
    pub fn new(backend: Arc<dyn VaultBackend>, clients: ClientStore) -> Dispatcher {
        Dispatcher::with_config(backend, clients, DispatcherConfig::default())
    }

    pub fn with_config(
        backend: Arc<dyn VaultBackend>,
        clients: ClientStore,
        config: DispatcherConfig,
    ) -> Dispatcher {
        Dispatcher {
            backend,
            clients: Mutex::new(clients),
            pairing: PairingBroker::default(),
            unlock_limiter: Mutex::new(UnlockLimiter::default()),
            config,
        }
    }

    /// Delivers the user's decision for a pending pairing request. Returns
    /// false if the request is unknown, already answered or timed out.
    pub fn respond_pairing(&self, request_id: &str, approve: bool) -> bool {
        self.pairing.respond(request_id, approve)
    }

    /// Pairing requests currently waiting for the user (e.g. to re-show the
    /// dialog after the window was reopened).
    pub fn pending_pairings(&self) -> Vec<PairingRequest> {
        self.pairing.pending()
    }

    /// All paired clients.
    pub fn clients(&self) -> Vec<PairedClient> {
        lock(&self.clients).list()
    }

    /// Removes a paired client; returns whether it existed.
    pub fn revoke(&self, client_id: &str) -> Result<bool> {
        lock(&self.clients).revoke(client_id)
    }

    /// Answers one request (same as [`BridgeHandler::handle`]).
    pub fn dispatch(&self, request: Request) -> Response {
        self.dispatch_for_peer(request, &|| false)
    }

    /// Answers one request of a connection; a `pair` request is withdrawn as
    /// soon as `peer_gone()` reports that the requester hung up (see
    /// [`BridgeHandler::handle_for_peer`]).
    pub fn dispatch_for_peer(&self, request: Request, peer_gone: &dyn Fn() -> bool) -> Response {
        let Request {
            id,
            client_id,
            token,
            payload,
        } = request;
        let user_action = payload.is_user_action();
        let result = self.execute(client_id.as_deref(), token.as_deref(), payload, peer_gone);
        if result.is_ok() && user_action {
            self.backend.on_activity();
        }
        match result {
            Ok(data) => Response {
                id,
                ok: true,
                data,
                error: None,
            },
            Err(e) => Response::error(id, e),
        }
    }

    /// Checks the credentials and updates `lastSeenAt` on success.
    fn authenticate(&self, client_id: Option<&str>, token: Option<&str>) -> bool {
        let (Some(client_id), Some(token)) = (client_id, token) else {
            return false;
        };
        let mut clients = lock(&self.clients);
        if !clients.verify(client_id, token) {
            return false;
        }
        if let Err(e) = clients.touch(client_id) {
            log(format_args!("could not update the paired client list: {e}"));
        }
        true
    }

    fn execute(
        &self,
        client_id: Option<&str>,
        token: Option<&str>,
        payload: Payload,
        peer_gone: &dyn Fn() -> bool,
    ) -> std::result::Result<Value, BridgeError> {
        let paired = self.authenticate(client_id, token);
        if payload.needs_pairing() && !paired {
            return Err(BridgeError::NotPaired);
        }
        if payload.needs_unlocked() && self.backend.unlocked_vault().is_none() {
            return Err(BridgeError::Locked);
        }
        let backend = &self.backend;
        match payload {
            Payload::Status => {
                let vault = backend.unlocked_vault();
                let unlocked = vault.is_some();
                let (vault_id, vault_name) =
                    vault.filter(|_| paired).map(|v| (v.id, v.name)).unzip();
                let (extension_version, extension_dir) = paired
                    .then(|| backend.extension_info())
                    .flatten()
                    .map(|e| (e.version, e.dir))
                    .unzip();
                json(&StatusData {
                    app_version: backend.app_version(),
                    paired,
                    unlocked,
                    vault_name,
                    vault_id,
                    extension_version,
                    extension_dir,
                })
            }
            Payload::Pair { client_name, code } => {
                json(&self.pair(&client_name, &code, peer_gone)?)
            }
            Payload::ListVaults => json(&self.list_vaults()?),
            Payload::Unlock { password, vault_id } => {
                if vault_id.as_deref() == Some("") {
                    return Err(BridgeError::InvalidRequest);
                }
                let vault = self.unlock(vault_id.as_deref(), &password)?;
                json(&UnlockData {
                    vault_name: vault.name,
                    vault_id: vault.id,
                })
            }
            Payload::Lock => {
                backend.lock();
                Ok(Value::Null)
            }
            Payload::FocusApp => {
                backend.focus_app();
                Ok(Value::Null)
            }
            Payload::LoginsForUrl { url } => json(&backend.logins_for_url(&url)?),
            Payload::Search { query } => {
                let mut results = backend.search(&query)?;
                results.truncate(SEARCH_LIMIT);
                json(&results)
            }
            Payload::GetLogin { item_id } => json(&backend.get_login(&item_id)?),
            Payload::GetTotp { item_id } => json(&backend.get_totp(&item_id)?),
            Payload::GeneratePassword { options } => {
                json(&backend.generate_password(options.unwrap_or_default())?)
            }
            Payload::SaveLogin {
                name,
                url,
                username,
                password,
            } => json(&IdData {
                id: backend.save_login(&name, &url, &username, &password)?,
            }),
            Payload::UpdatePassword { item_id, password } => json(&IdData {
                id: backend.update_password(&item_id, &password)?,
            }),
            Payload::CheckLoginPassword { item_id, password } => {
                let login = backend.get_login(&item_id)?;
                json(&same_secret(login.password.as_bytes(), password.as_bytes()))
            }
            Payload::CopyField { item_id, field } => {
                let remaining = backend.copy_field(&item_id, field)?;
                json(&CopyData { remaining })
            }
            Payload::CopySecret { text } => {
                if text.is_empty() {
                    return Err(BridgeError::InvalidRequest);
                }
                backend.copy_secret(&text)?;
                Ok(Value::Null)
            }
        }
    }

    fn pair(
        &self,
        client_name: &str,
        code: &str,
        peer_gone: &dyn Fn() -> bool,
    ) -> std::result::Result<PairData, BridgeError> {
        let name = sanitize_client_name(client_name).ok_or(BridgeError::InvalidRequest)?;
        if !is_pairing_code(code) {
            return Err(BridgeError::InvalidRequest);
        }
        let (ticket, superseded) = self.pairing.begin(&name, code)?;
        for request_id in &superseded {
            self.backend.pairing_closed(request_id);
        }
        let request_id = ticket.request_id.clone();
        self.backend.request_pairing(&request_id, &name, code);
        match ticket.wait(self.config.pairing_timeout, peer_gone) {
            PairingOutcome::Approved => {}
            PairingOutcome::Denied => return Err(BridgeError::PairingDenied),
            PairingOutcome::Closed => {
                self.backend.pairing_closed(&request_id);
                return Err(BridgeError::PairingDenied);
            }
        }
        let (client, token) = lock(&self.clients).add(&name).map_err(|e| {
            log(format_args!("could not store the paired client: {e}"));
            BridgeError::Internal
        })?;
        Ok(PairData {
            client_id: client.id,
            token,
        })
    }

    /// The vaults sorted by name (case-insensitive), the open one and the
    /// last used one (only if it still exists).
    fn list_vaults(&self) -> std::result::Result<ListVaultsData, BridgeError> {
        let (mut vaults, last_vault_id) = self.backend.list_vaults()?;
        vaults.sort_by_cached_key(|v| (v.name.to_lowercase(), v.id.clone()));
        let current_vault_id = self.backend.unlocked_vault().map(|v| v.id);
        let last_vault_id = last_vault_id.filter(|id| vaults.iter().any(|v| &v.id == id));
        Ok(ListVaultsData {
            vaults,
            current_vault_id,
            last_vault_id,
        })
    }

    fn unlock(
        &self,
        vault_id: Option<&str>,
        password: &str,
    ) -> std::result::Result<VaultSummary, BridgeError> {
        let mut limiter = lock(&self.unlock_limiter);
        if limiter
            .blocked_until
            .is_some_and(|until| Instant::now() < until)
        {
            return Err(BridgeError::WrongPassword);
        }
        let result = self.backend.unlock(vault_id, password);
        match &result {
            Ok(opened) => {
                // Only the failures of this vault: a right password for one
                // vault (e.g. re-checking the open one) says nothing about
                // the guesses against another.
                limiter.failures.remove(&opened.id);
                if vault_id.is_none() {
                    limiter.failures.remove("");
                }
                if limiter.failures.is_empty() {
                    limiter.blocked_until = None;
                }
            }
            Err(BridgeError::WrongPassword) => {
                let count = limiter
                    .failures
                    .entry(vault_id.unwrap_or("").to_owned())
                    .or_default();
                *count = count.saturating_add(1);
                // Once the limit is reached, every further failure (after the
                // lockout expired) suspends unlocking again.
                if limiter.total() >= self.config.max_unlock_failures {
                    limiter.blocked_until = Some(Instant::now() + self.config.unlock_lockout);
                }
            }
            Err(_) => {}
        }
        result
    }
}

impl BridgeHandler for Dispatcher {
    fn handle(&self, request: Request) -> Response {
        self.dispatch(request)
    }

    fn handle_for_peer(&self, request: Request, peer_gone: &dyn Fn() -> bool) -> Response {
        self.dispatch_for_peer(request, peer_gone)
    }
}

impl Drop for Dispatcher {
    fn drop(&mut self) {
        if let Err(e) = lock(&self.clients).flush() {
            log(format_args!("could not save the paired client list: {e}"));
        }
    }
}

fn json<T: Serialize + ?Sized>(data: &T) -> std::result::Result<Value, BridgeError> {
    serde_json::to_value(data).map_err(|_| BridgeError::Internal)
}

/// Compares two secrets without an early exit on the first difference.
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Trimmed name with control characters replaced; `None` if empty or longer
/// than [`MAX_CLIENT_NAME_CHARS`].
fn sanitize_client_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let count = cleaned.chars().count();
    (count > 0 && count <= MAX_CLIENT_NAME_CHARS).then_some(cleaned)
}

fn is_pairing_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// Pairing broker
// ---------------------------------------------------------------------------

struct PendingPairing {
    client_name: String,
    code: String,
    reply: SyncSender<bool>,
}

type PendingMap = Arc<Mutex<HashMap<String, PendingPairing>>>;

/// Pending pairing requests: request id → reply channel of the waiting
/// connection thread.
#[derive(Default)]
struct PairingBroker {
    pending: PendingMap,
}

/// Held by the waiting connection thread; withdraws the request from the
/// pending map when dropped (also if the thread unwinds).
struct PairingTicket {
    request_id: String,
    reply: Receiver<bool>,
    pending: PendingMap,
}

/// How a pairing request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairingOutcome {
    /// The user approved it.
    Approved,
    /// The user denied it, or a newer request of the same client replaced it.
    Denied,
    /// Withdrawn without a decision: timed out, or the requester hung up.
    Closed,
}

impl PairingTicket {
    /// Blocks until the user decided, `timeout` passed or `peer_gone()`
    /// reports that the requester hung up (checked every
    /// [`PEER_CHECK_INTERVAL`]). A request whose requester is gone is never
    /// approved: nobody would receive the token.
    fn wait(self, timeout: Duration, peer_gone: &dyn Fn() -> bool) -> PairingOutcome {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.reply.recv_timeout(left.min(PEER_CHECK_INTERVAL)) {
                Ok(true) => return PairingOutcome::Approved,
                Ok(false) | Err(RecvTimeoutError::Disconnected) => return PairingOutcome::Denied,
                Err(RecvTimeoutError::Timeout) => {
                    if peer_gone() {
                        lock(&self.pending).remove(&self.request_id);
                        return PairingOutcome::Closed;
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                }
            }
        }
        let mut pending = lock(&self.pending);
        if pending.remove(&self.request_id).is_some() {
            PairingOutcome::Closed
        } else {
            // Answered between the timeout and taking the lock.
            match self.reply.try_recv() {
                Ok(true) => PairingOutcome::Approved,
                _ => PairingOutcome::Denied,
            }
        }
    }
}

impl Drop for PairingTicket {
    fn drop(&mut self) {
        lock(&self.pending).remove(&self.request_id);
    }
}

impl PairingBroker {
    /// Registers a request. A still pending request with the same client
    /// name is superseded (answered as denied): only the newest code shown
    /// in the extension can be approved. Returns the ticket and the ids of
    /// the superseded requests.
    fn begin(
        &self,
        client_name: &str,
        code: &str,
    ) -> std::result::Result<(PairingTicket, Vec<String>), BridgeError> {
        let mut pending = lock(&self.pending);
        let mut superseded = Vec::new();
        pending.retain(|id, p| {
            if p.client_name == client_name {
                let _ = p.reply.try_send(false);
                superseded.push(id.clone());
                false
            } else {
                true
            }
        });
        if pending.len() >= MAX_PENDING_PAIRINGS {
            return Err(BridgeError::PairingDenied);
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::sync_channel(1);
        pending.insert(
            request_id.clone(),
            PendingPairing {
                client_name: client_name.to_owned(),
                code: code.to_owned(),
                reply: tx,
            },
        );
        Ok((
            PairingTicket {
                request_id,
                reply: rx,
                pending: Arc::clone(&self.pending),
            },
            superseded,
        ))
    }

    /// Sends the decision while holding the lock, so [`PairingTicket::wait`]
    /// either sees the entry (and withdraws it) or finds the answer queued.
    fn respond(&self, request_id: &str, approve: bool) -> bool {
        let mut pending = lock(&self.pending);
        match pending.remove(request_id) {
            Some(p) => p.reply.try_send(approve).is_ok(),
            None => false,
        }
    }

    fn pending(&self) -> Vec<PairingRequest> {
        lock(&self.pending)
            .iter()
            .map(|(id, p)| PairingRequest {
                request_id: id.clone(),
                client_name: p.client_name.clone(),
                code: p.code.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_name_and_code_validation() {
        assert_eq!(
            sanitize_client_name("  Chrome – PC "),
            Some("Chrome – PC".into())
        );
        assert_eq!(sanitize_client_name("a\nb"), Some("a b".into()));
        assert_eq!(sanitize_client_name("   "), None);
        assert_eq!(sanitize_client_name(&"x".repeat(101)), None);
        assert!(sanitize_client_name(&"ä".repeat(100)).is_some());
        assert!(is_pairing_code("012345"));
        assert!(!is_pairing_code("12345"));
        assert!(!is_pairing_code("1234567"));
        assert!(!is_pairing_code("12a456"));
        assert!(!is_pairing_code("١٢٣٤٥٦"));
    }

    #[test]
    fn broker_timeout_and_late_answer() {
        let broker = PairingBroker::default();
        let (ticket, superseded) = broker.begin("a", "123456").unwrap();
        assert!(superseded.is_empty());
        let id = ticket.request_id.clone();
        assert_eq!(
            ticket.wait(Duration::from_millis(10), &|| false),
            PairingOutcome::Closed
        );
        assert!(
            !broker.respond(&id, true),
            "answer after timeout is rejected"
        );
        assert!(broker.pending().is_empty());
    }

    #[test]
    fn broker_withdraws_the_request_when_the_requester_hangs_up() {
        let broker = PairingBroker::default();
        let (ticket, _) = broker.begin("a", "123456").unwrap();
        let id = ticket.request_id.clone();
        let started = Instant::now();
        assert_eq!(
            ticket.wait(Duration::from_secs(30), &|| true),
            PairingOutcome::Closed
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "not the full timeout"
        );
        assert!(broker.pending().is_empty());
        assert!(
            !broker.respond(&id, true),
            "a request whose requester is gone cannot be approved"
        );

        // A decision that arrives while the requester is still there wins.
        let (ticket, _) = broker.begin("b", "123456").unwrap();
        assert!(broker.respond(&ticket.request_id, true));
        assert_eq!(
            ticket.wait(Duration::from_secs(30), &|| false),
            PairingOutcome::Approved
        );
    }

    #[test]
    fn broker_supersedes_same_name_and_caps_pending() {
        let broker = PairingBroker::default();
        let (first, _) = broker.begin("a", "111111").unwrap();
        let first_id = first.request_id.clone();
        let (second, superseded) = broker.begin("a", "222222").unwrap();
        assert_eq!(superseded, vec![first_id]);
        // The first one was answered "denied" immediately.
        assert_eq!(
            first.wait(Duration::from_secs(5), &|| false),
            PairingOutcome::Denied
        );
        assert_eq!(broker.pending().len(), 1);
        assert_eq!(broker.pending()[0].code, "222222");
        assert!(broker.respond(&second.request_id, true));
        assert_eq!(
            second.wait(Duration::from_secs(5), &|| false),
            PairingOutcome::Approved
        );
        assert!(broker.pending().is_empty());

        let tickets: Vec<_> = (0..MAX_PENDING_PAIRINGS)
            .map(|i| broker.begin(&format!("c{i}"), "123456").unwrap())
            .collect();
        assert!(matches!(
            broker.begin("one too many", "123456"),
            Err(BridgeError::PairingDenied)
        ));
        // Dropped tickets (e.g. a panicking thread) free their slots.
        drop(tickets);
        assert!(broker.pending().is_empty());
        assert!(broker.begin("again", "123456").is_ok());
    }
}
