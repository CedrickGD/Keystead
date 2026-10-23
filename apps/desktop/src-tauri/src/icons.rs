//! Website icons (see "Website icons" in docs/ARCHITECTURE.md): the stored
//! icons of a vault for the UI (`get_icons`) and the browser bridge, and the
//! loader's view of the vault session ([`IconTracker`], part of `Core`).
//! The loading itself – desktop only, the phone does not load icons – lives in
//! [`fetch`].

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, MutexGuard, PoisonError};

use keystead_core::icons as core_icons;
#[cfg(desktop)]
use keystead_core::model::ItemSummary;
use keystead_core::UnlockedVault;

use crate::commands::{run, CmdResult, Shared};

#[cfg(desktop)]
mod fetch;
#[cfg(desktop)]
pub use fetch::spawn_scheduler;
#[cfg(desktop)]
pub(crate) use fetch::{proxy_matcher, proxy_setup_unclear};
#[cfg(all(desktop, test))]
pub(crate) use fetch::{proxy_unclear_from, SystemProxyHints};

/// `vault://icons` – payload `{}`: new icons were stored, the UI re-reads
/// them (`get_icons`).
#[cfg(desktop)]
pub const EVENT_ICONS: &str = "vault://icons";

/// Icons the browser bridge attaches to `logins_for_url` / `search` rows:
/// only for the first rows of a reply, and only small ones (the reply to
/// the browser is capped at 1 MiB).
#[cfg(desktop)]
pub const BRIDGE_ICON_ROWS: usize = 20;
#[cfg(desktop)]
pub const BRIDGE_ICON_MAX_LEN: usize = 16 * 1024;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Tracker (part of `Core`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    /// A vault was opened (unlock, new vault, switch).
    Opened,
    /// The open vault was closed.
    Closed,
    /// The vault was changed, or the setting turned on: check soon.
    Changed,
}

/// The icon scheduler's view of the vault session, held by [`Core`]: the
/// state hooks (`install_vault_since`, `finish_lock`, `mutate_with`, settings)
/// call it; a run started under an older `generation` stops.
#[derive(Default)]
pub struct IconTracker {
    generation: AtomicU64,
    wake: Mutex<Option<Sender<Signal>>>,
}

impl IconTracker {
    fn send(&self, signal: Signal) {
        if let Some(tx) = lock(&self.wake).as_ref() {
            let _ = tx.send(signal);
        }
    }

    /// Read by the loader's runs (desktop).
    #[cfg(desktop)]
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn cancel_runs(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// A vault was opened: a run for it starts in 5 s; a run for the vault
    /// before (if any) stops.
    pub fn vault_opened(&self) {
        self.cancel_runs();
        self.send(Signal::Opened);
    }

    /// The open vault was closed: a running fetch stops, nothing is written.
    pub fn vault_closed(&self) {
        self.cancel_runs();
        self.send(Signal::Closed);
    }

    /// The open vault changed (maybe a login with a new website). Called by
    /// the vault writers, which the read-only phone does not have yet.
    #[cfg_attr(mobile, allow(dead_code))]
    pub fn vault_changed(&self) {
        self.send(Signal::Changed);
    }

    /// `websiteIcons` was switched: on → run soon, off → stop now.
    pub fn setting_changed(&self, enabled: bool) {
        if enabled {
            self.send(Signal::Changed);
        } else {
            self.cancel_runs();
        }
    }

    /// Disconnects the scheduler (app shutdown).
    pub fn stop(&self) {
        self.cancel_runs();
        *lock(&self.wake) = None;
    }
}

// ---------------------------------------------------------------------------
// Commands and the browser bridge
// ---------------------------------------------------------------------------

/// `get_icons(pageVaultId)` → `{ [host]: "data:image/png;base64,…" }` of
/// the page's vault (`locked` if another vault is open).
#[tauri::command]
pub async fn get_icons(
    core: Shared<'_>,
    page_vault_id: Option<String>,
) -> CmdResult<BTreeMap<String, String>> {
    run(&core, false, move |c| {
        let st = c.state();
        let vault = st.page_vault(page_vault_id.as_deref())?;
        Ok(icon_map(vault))
    })
    .await
}

/// `clear_icons(pageVaultId)` † → number of removed icons.
#[cfg(desktop)]
#[tauri::command]
pub async fn clear_icons(core: Shared<'_>, page_vault_id: Option<String>) -> CmdResult<usize> {
    run(&core, true, move |c| {
        c.mutate_for_page(page_vault_id.as_deref(), |v| v.clear_icons())
    })
    .await
}

fn icon_map(vault: &UnlockedVault) -> BTreeMap<String, String> {
    vault
        .data()
        .icons
        .iter()
        .filter_map(|(host, entry)| {
            Some((host.clone(), core_icons::data_url(entry.png.as_deref()?)))
        })
        .collect()
}

/// Adds the stored icon (data URL) to the first [`BRIDGE_ICON_ROWS`] rows
/// of a bridge reply, if it is at most [`BRIDGE_ICON_MAX_LEN`] long.
#[cfg(desktop)]
pub fn attach_icons(vault: &UnlockedVault, rows: &mut [ItemSummary]) {
    for row in rows.iter_mut().take(BRIDGE_ICON_ROWS) {
        row.icon = vault
            .item(&row.id)
            .and_then(|item| vault.icon_for_item(item))
            .map(core_icons::data_url)
            .filter(|url| url.len() <= BRIDGE_ICON_MAX_LEN);
    }
}
