//! Server sync (stage 2c) – what the phone's link (`mobile::sync`) and the
//! PC's mirror (`mirror`) share: the server as stored ([`SyncServer`], no
//! password) ↔ the WebDAV client's [`WebDavConfig`], connecting
//! ([`connect`]: the connection test must pass, then the password is
//! stored), opening the server for a sync ([`open`]), the pause after
//! server errors ([`Backoff`]) and the commands `webdav_test` and
//! `webdav_create_folder` (both platforms). See "Server sync (WebDAV)" in
//! docs/ARCHITECTURE.md.
//!
//! The WebDAV client drives its requests on a runtime of its own
//! (`Runtime::block_on`): it is only ever used on plain threads – the
//! blocking pool of the commands (`commands::run`), the mirror's worker,
//! the phone's sync and push threads – never inside an async task (a unit
//! test below runs it on Tauri's blocking pool).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use keystead_core::settings::SyncServer;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::commands::{parse, run, CmdResult, Shared};
use crate::error::{AppError, AppResult};
use crate::remote::webdav::{self, WebDavConfig, WebDavRemote, WebDavTest};
use crate::remote::{self, Remote as _};
use crate::state::{log, Core};

/// `invalid_input:webdav_config`: the command's argument is not a server
/// configuration.
pub const BAD_CONFIG: &str = "webdav_config";
/// `skipped`: an automatic sync waits after a server error ([`Backoff`]).
#[cfg(mobile)]
pub const SKIPPED_BACKOFF: &str = "sync_backoff";

/// The pauses of automatic syncs after consecutive server errors: 1, 2, 5,
/// then 15 minutes.
pub const BACKOFF_STEPS: [Duration; 4] = [
    Duration::from_secs(60),
    Duration::from_secs(2 * 60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
];

/// The client's configuration of a stored server with its password.
pub fn config_of(server: &SyncServer, password: Zeroizing<String>) -> WebDavConfig {
    WebDavConfig {
        url: server.url.clone(),
        user: server.user.clone(),
        password,
        folder: server.folder.clone(),
        cert_sha256: server.cert_sha256.clone(),
    }
}

/// What is stored of `cfg` (no password); `cas` as the test found it.
pub fn server_of(cfg: &WebDavConfig, cas: bool) -> SyncServer {
    SyncServer {
        url: cfg.url.trim().to_owned(),
        user: cfg.user.clone(),
        folder: cfg.folder.clone(),
        cert_sha256: cfg.cert_sha256.clone(),
        cas,
    }
}

/// The key of the server's folder (`webdav:<normalized folder URL>`) – what
/// its password, the sync bases and the link are recorded for.
pub fn key_of(server: &SyncServer) -> AppResult<String> {
    Ok(webdav::key_of(&webdav::collection_url(
        &server.url,
        &server.folder,
    )?))
}

/// The server for a sync, with its stored password
/// (`unsupported:sync_credentials_missing` without one).
pub fn open(core: &Core, server: &SyncServer) -> AppResult<WebDavRemote> {
    let key = key_of(server)?;
    let password = core.credentials().require(&core.data_dir(), &key)?;
    WebDavRemote::new(&config_of(server, password))
}

/// A server [`connect`] reached: what is stored of it, its folder's key
/// and what the connection test found.
#[derive(Debug)]
// The PC's `set_sync_server` needs the server only.
#[cfg_attr(not(any(mobile, test)), allow(dead_code))]
pub struct Connected {
    pub server: SyncServer,
    pub key: String,
    pub test: WebDavTest,
}

/// Connects to the server of `cfg`: the connection test must pass (its
/// error code otherwise – the folder missing, an untrusted certificate,
/// …), then the password is stored under the folder's key.
pub fn connect(core: &Core, cfg: &WebDavConfig) -> AppResult<Connected> {
    let test = WebDavRemote::test(cfg);
    if !test.ok {
        return Err(AppError::Code(test.code.clone().unwrap_or_else(|| {
            AppError::unsupported(remote::SERVER_ERROR).code()
        })));
    }
    let server = server_of(cfg, test.cas);
    let key = WebDavRemote::new(cfg)?.key();
    core.credentials()
        .store(&core.data_dir(), &key, &cfg.password)?;
    Ok(Connected { server, key, test })
}

/// The server password of a link or setting that is replaced or removed
/// is forgotten – unless the new one uses the same server folder (`keep`,
/// its key). Logged, never an error (a stale file only costs space).
pub fn forget_password(core: &Core, old_key: &str, keep: Option<&str>) {
    if keep == Some(old_key) {
        return;
    }
    if let Err(e) = core.credentials().remove(&core.data_dir(), old_key) {
        log(format_args!(
            "could not forget the server password: {}",
            e.code()
        ));
    }
}

/// Whether an error (its code) pauses the automatic syncs: the server
/// cannot be reached, failed, answered nonsense or refused the login (a
/// server that locks accounts after failed logins must not be asked every
/// minute), its certificate changed, the folder is gone. Errors of single
/// files do not.
pub fn backs_off(code: &str) -> bool {
    let Some(detail) = code.strip_prefix("unsupported:") else {
        return false;
    };
    [
        remote::OFFLINE,
        remote::SERVER_ERROR,
        remote::BAD_RESPONSE,
        remote::QUOTA,
        remote::AUTH_FAILED,
        remote::CERTIFICATE,
        remote::NOT_WEBDAV,
        remote::FOLDER_MISSING,
    ]
    .contains(&detail)
}

/// The pause of automatic syncs after server errors ([`BACKOFF_STEPS`];
/// a sync the user starts always runs). Both clocks: the monotonic one
/// stands still while a phone sleeps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Backoff {
    failures: usize,
    since: Option<(Instant, SystemTime)>,
}

impl Backoff {
    /// The current pause, `None` without one.
    pub fn delay(&self) -> Option<Duration> {
        let step = self.failures.checked_sub(1)?;
        Some(BACKOFF_STEPS[step.min(BACKOFF_STEPS.len() - 1)])
    }

    /// Whether an automatic sync may run at (`now`, `wall`).
    pub fn allows(&self, now: Instant, wall: SystemTime) -> bool {
        let (Some(delay), Some((at, at_wall))) = (self.delay(), self.since) else {
            return true;
        };
        let elapsed = now
            .saturating_duration_since(at)
            .max(wall.duration_since(at_wall).unwrap_or(Duration::ZERO));
        elapsed >= delay
    }

    /// [`Backoff::allows`] now.
    pub fn allows_now(&self) -> bool {
        self.allows(Instant::now(), SystemTime::now())
    }

    /// Records a sync's outcome: `None` (it reached the server) or an
    /// error code that does not pause ends the pause; one that does
    /// ([`backs_off`]) starts or lengthens it.
    pub fn record(&mut self, error: Option<&str>, now: Instant, wall: SystemTime) {
        match error {
            Some(code) if backs_off(code) => {
                self.failures += 1;
                self.since = Some((now, wall));
            }
            _ => self.reset(),
        }
    }

    /// [`Backoff::record`] now.
    pub fn record_now(&mut self, error: Option<&str>) {
        self.record(error, Instant::now(), SystemTime::now());
    }

    pub fn reset(&mut self) {
        *self = Backoff::default();
    }

    /// When the pause ends (ms since the epoch), `None` without one.
    pub fn retry_at_ms(&self) -> Option<i64> {
        let (delay, (_, wall)) = (self.delay()?, self.since?);
        let at = (wall + delay).duration_since(UNIX_EPOCH).ok()?;
        i64::try_from(at.as_millis()).ok()
    }

    /// Unit tests: the pause is over.
    #[cfg(test)]
    pub fn expire(&mut self) {
        if let Some((at, wall)) = self.since {
            let back = BACKOFF_STEPS[BACKOFF_STEPS.len() - 1];
            self.since = Some((
                at.checked_sub(back).unwrap_or(at),
                wall.checked_sub(back).unwrap_or(wall),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// The phone's link to a server
// ---------------------------------------------------------------------------

/// Where the phone's sync goes for a link.
#[cfg(any(mobile, test))]
pub enum LinkTarget {
    /// A cloud app's folder (Android: through the folder plugin): its tree
    /// URI.
    Folder(String),
    /// A server's folder.
    Server(Box<crate::remote::folder::RemoteDirFolder<WebDavRemote>>),
}

/// The target of `link` (a server's is opened with its stored password;
/// one whose folder key no longer matches the link is refused).
#[cfg(any(mobile, test))]
pub fn target(core: &Core, link: &crate::folder_sync::LinkState) -> AppResult<LinkTarget> {
    use crate::folder_sync::LinkKind;
    match link.kind {
        LinkKind::Folder => Ok(LinkTarget::Folder(link.tree_uri.clone())),
        LinkKind::Webdav => {
            let server = link
                .server
                .as_ref()
                .ok_or_else(|| AppError::invalid("server_link"))?;
            let remote = open(core, server)?;
            if remote.key() != link.tree_uri {
                return Err(AppError::invalid("server_link"));
            }
            Ok(LinkTarget::Server(Box::new(
                crate::remote::folder::RemoteDirFolder::new(remote),
            )))
        }
    }
}

/// `link_webdav` without the phone's glue: connects ([`connect`]) and
/// returns the new link (the caller replaces the previous one and forgets
/// its password, [`forget_password`]).
#[cfg(any(mobile, test))]
pub fn link_server(
    core: &Core,
    cfg: &WebDavConfig,
    now: i64,
) -> AppResult<crate::folder_sync::LinkState> {
    let Connected { server, key, test } = connect(core, cfg)?;
    Ok(crate::folder_sync::LinkState::new_server(
        key,
        server,
        now,
        test.writable,
    ))
}

// ---------------------------------------------------------------------------
// Commands (both platforms)
// ---------------------------------------------------------------------------

/// `webdav_test(config)` → [`WebDavTest`]: the settings form's connection
/// test (server, certificate, login, folder). Nothing is stored; the
/// password is not echoed.
#[tauri::command]
pub async fn webdav_test(core: Shared<'_>, config: Value) -> CmdResult<WebDavTest> {
    run(&core, true, move |_| {
        let cfg: WebDavConfig = parse(config, BAD_CONFIG)?;
        Ok(WebDavRemote::test(&cfg))
    })
    .await
}

/// `webdav_create_folder(config)` → [`WebDavTest`]: "Ordner anlegen" – the
/// server's sync folder is created (one that exists is fine) and the
/// connection tested again. Never done by a sync.
#[tauri::command]
pub async fn webdav_create_folder(core: Shared<'_>, config: Value) -> CmdResult<WebDavTest> {
    run(&core, true, move |_| {
        let cfg: WebDavConfig = parse(config, BAD_CONFIG)?;
        WebDavRemote::new(&cfg)?.create_folder(&cfg)?;
        Ok(WebDavRemote::test(&cfg))
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use keystead_core::settings::Settings;
    use keystead_core::{KdfParams, VaultStore};

    use super::*;
    use crate::folder_sync::{self, LinkKind, LinkState, Outcome, RemoteFolder as _};
    use crate::remote::test_server::TestServer;

    fn cfg(server: &TestServer, password: &str) -> WebDavConfig {
        WebDavConfig {
            url: server.base.clone(),
            user: "alice".into(),
            password: Zeroizing::new(password.into()),
            folder: "Keystead".into(),
            cert_sha256: None,
        }
    }

    fn core(dir: &std::path::Path) -> Arc<Core> {
        Core::for_tests(VaultStore::new(dir.join("phone")), Settings::default())
    }

    #[test]
    fn the_backoff_grows_1_2_5_15_minutes_and_ends_with_a_success() {
        let t0 = Instant::now();
        let w0 = SystemTime::now();
        let mut b = Backoff::default();
        assert!(b.allows(t0, w0));
        assert_eq!(b.retry_at_ms(), None);
        let offline = AppError::unsupported(remote::OFFLINE).code();
        let minutes = |m: u64| Duration::from_secs(m * 60);
        for (n, expected) in [1, 2, 5, 15, 15, 15].into_iter().enumerate() {
            b.record(Some(&offline), t0, w0);
            assert_eq!(b.delay(), Some(minutes(expected)), "failure {}", n + 1);
            let just_before = minutes(expected) - Duration::from_secs(1);
            assert!(!b.allows(t0 + just_before, w0 + just_before));
            assert!(b.allows(t0 + minutes(expected), w0));
            // The wall clock counts too (the monotonic one stands still
            // while the phone sleeps).
            assert!(b.allows(t0, w0 + minutes(expected)));
        }
        let at = w0 + minutes(15);
        assert_eq!(
            b.retry_at_ms(),
            Some(at.duration_since(UNIX_EPOCH).unwrap().as_millis() as i64)
        );
        // A sync that reached the server ends it – also one with an error
        // of a single file.
        b.record(None, t0, w0);
        assert_eq!(b, Backoff::default());
        for code in [
            remote::SERVER_ERROR,
            remote::AUTH_FAILED,
            remote::CERTIFICATE,
            remote::FOLDER_MISSING,
        ] {
            b.record(Some(&AppError::unsupported(code).code()), t0, w0);
            assert_eq!(b.delay(), Some(minutes(1)), "{code}");
            b.reset();
        }
        b.record(Some(&offline), t0, w0);
        b.record(
            Some(&AppError::unsupported(remote::TOO_LARGE).code()),
            t0,
            w0,
        );
        assert!(b.allows(t0, w0));
        for code in [
            AppError::unsupported(crate::credentials::MISSING).code(),
            AppError::invalid(remote::OFFLINE).code(),
            "io:disk".to_owned(),
        ] {
            assert!(!backs_off(&code), "{code}");
        }
        b.record(Some(&offline), t0, w0);
        b.expire();
        assert!(b.allows_now());
    }

    #[test]
    fn connecting_stores_the_password_and_the_server_without_it() {
        let dir = tempfile::tempdir().unwrap();
        let core = core(dir.path());
        let server = TestServer::http("alice", "pw");
        // The folder is missing: nothing is stored.
        let e = connect(&core, &cfg(&server, "pw")).unwrap_err();
        assert_eq!(e.code(), "unsupported:sync_server_folder_missing");
        server.mkdir("Keystead");
        let e = connect(&core, &cfg(&server, "wrong")).unwrap_err();
        assert_eq!(e.code(), "unsupported:sync_auth_failed");
        assert!(!crate::credentials::dir(&core.data_dir()).exists());

        let Connected {
            server: stored,
            key,
            test,
        } = connect(&core, &cfg(&server, "pw")).unwrap();
        assert!(test.ok && test.writable && test.cas, "{test:?}");
        assert_eq!(key, format!("webdav:{}Keystead/", server.base));
        assert_eq!(key_of(&stored).unwrap(), key);
        assert_eq!(stored.url, server.base);
        assert_eq!(stored.user, "alice");
        assert_eq!(stored.folder, "Keystead");
        assert!(stored.cas, "the test server honours If-Match");
        assert_eq!(
            core.credentials()
                .require(&core.data_dir(), &key)
                .unwrap()
                .as_str(),
            "pw"
        );
        assert!(!serde_json::to_string(&stored).unwrap().contains("pw\""));
        // Opened for a sync with the stored password.
        let remote = open(&core, &stored).unwrap();
        assert!(remote.probe().unwrap().exists);
        core.credentials().remove(&core.data_dir(), &key).unwrap();
        assert_eq!(
            open(&core, &stored).unwrap_err().code(),
            "unsupported:sync_credentials_missing"
        );
    }

    #[test]
    fn the_client_runs_on_the_commands_blocking_pool() {
        // `commands::run` executes on Tauri's blocking pool – inside the
        // async runtime's context; the client's own runtime must work there.
        let server = TestServer::http("alice", "pw");
        server.mkdir("Keystead");
        let config = cfg(&server, "pw");
        let test =
            tauri::async_runtime::block_on(tauri::async_runtime::spawn_blocking(move || {
                WebDavRemote::test(&config)
            }))
            .unwrap();
        assert!(test.ok, "{test:?}");
    }

    #[test]
    fn a_phone_linked_to_a_server_syncs_through_it() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::http("alice", "pw");
        server.mkdir("Keystead");
        // The PC's vault on the server.
        let pc = VaultStore::new(dir.path().join("pc"));
        let v = pc
            .create_vault_with_params("Privat", "pc-pw", KdfParams::insecure_for_tests())
            .unwrap();
        let name = format!("{}.keystead", v.id());
        server.put(&format!("Keystead/{name}"), &fs::read(v.path()).unwrap());

        let core = core(dir.path());
        // An earlier folder link: its target is the plugin's tree.
        let folder_link = LinkState::new("content://tree/x".into(), "X".into(), 1, true);
        assert!(matches!(
            target(&core, &folder_link).unwrap(),
            LinkTarget::Folder(tree) if tree == "content://tree/x"
        ));
        let mut link = link_server(&core, &cfg(&server, "pw"), 2).unwrap();
        assert_eq!(link.kind, LinkKind::Webdav);
        assert!(link.writable);
        assert_eq!(link.tree_uri, format!("webdav:{}Keystead/", server.base));
        let LinkTarget::Server(folder) = target(&core, &link).unwrap() else {
            panic!("a server link goes to the server");
        };
        assert!(folder.writable().unwrap());
        assert!(folder_sync::sync_folder(&core, &*folder, &mut link, 3).unwrap());
        let result = link.files.iter().find(|f| f.file_name == name).unwrap();
        assert_eq!(result.outcome, Outcome::Imported);
        assert_eq!(
            fs::read(core.store().vault_path(v.id())).unwrap(),
            fs::read(v.path()).unwrap()
        );

        // Another server: the first one's password is forgotten once the
        // new link is in place (not for the same server folder).
        let other = TestServer::http("alice", "pw2");
        other.mkdir("Keystead");
        let first_key = link.tree_uri.clone();
        let second = link_server(&core, &cfg(&other, "pw2"), 4).unwrap();
        assert_ne!(second.tree_uri, first_key);
        forget_password(&core, &first_key, Some(&first_key));
        assert!(core
            .credentials()
            .load(&core.data_dir(), &first_key)
            .unwrap()
            .is_some());
        forget_password(&core, &first_key, Some(&second.tree_uri));
        assert_eq!(
            core.credentials()
                .load(&core.data_dir(), &first_key)
                .unwrap(),
            None
        );
        assert!(core
            .credentials()
            .load(&core.data_dir(), &second.tree_uri)
            .unwrap()
            .is_some());
        // A link whose server no longer matches its key is refused.
        let mut forged = second.clone();
        forged.tree_uri = first_key;
        assert_eq!(
            target(&core, &forged).err().unwrap().code(),
            "invalid_input:server_link"
        );
    }
}
