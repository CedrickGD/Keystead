//! In-app updates (see "In-app updates" in docs/ARCHITECTURE.md).
//!
//! `tauri-plugin-updater` fetches a `latest.json` from GitHub – the endpoint
//! is chosen at runtime by `Settings.updateChannel` – and verifies the
//! minisign signature of the downloaded NSIS setup (or AppImage) against the
//! public key in `tauri.conf.json` before anything runs. The UI talks to the
//! commands here, never to the plugin's JS API (no plugin permission is
//! granted to the webview).
//!
//! * `check_update` / the background check (15 s after start, then every
//!   6 h while `updateCheck` is on; `update://available` once per version);
//! * `install_update`: download with `update://progress`, `update://ready`,
//!   stop the browser bridge, lock the vault (no unlock until the process
//!   ends) and clear a copied secret, then install: on Windows the plugin
//!   runs our `on_before_exit` hook, starts the setup (passive mode, restarts
//!   the app) and exits the process, elsewhere the app restarts itself.
//!
//! Only installed copies update themselves (`canInstall`): the portable
//! build gets a link to the release page instead.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use keystead_core::settings::UpdateChannel;
use semver::Version;
use serde::Serialize;
use tauri::{AppHandle, Manager as _, Url};
use tauri_plugin_updater::{Error as UpdaterError, RemoteRelease, Update, Updater, UpdaterExt};

use crate::error::{AppError, AppResult};
use crate::platform;
use crate::state::{log, Core, LockReason};

/// `update://available` – payload `UpdateInfo`, once per newly found version.
pub const EVENT_AVAILABLE: &str = "update://available";
/// `update://progress` – payload `{ downloaded, total }` (bytes).
pub const EVENT_PROGRESS: &str = "update://progress";
/// `update://ready` – payload `{ version }`: downloaded and verified, the
/// app installs it and restarts.
pub const EVENT_READY: &str = "update://ready";

/// The repository the releases come from.
pub const REPO_URL: &str = "https://github.com/CedrickGD/Keystead";
/// `latest.json` of the beta channel: a fixed pre-release (`updater-beta`)
/// whose asset CI overwrites with every published build (betas and stable
/// releases). Being a pre-release it is never GitHub's "latest" release,
/// which VaultX 1.x follows.
pub const BETA_ENDPOINT: &str =
    "https://github.com/CedrickGD/Keystead/releases/download/updater-beta/latest.json";
/// `latest.json` of the stable channel: an asset of GitHub's "latest"
/// (newest non-pre-release) release.
pub const STABLE_ENDPOINT: &str =
    "https://github.com/CedrickGD/Keystead/releases/latest/download/latest.json";

/// First background check after start.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(15);
/// Background check interval.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// Timeout of the `latest.json` request (not of the download).
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// `update://progress` is sent at most this often.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// The `latest.json` URL of a channel.
pub fn endpoint(channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Beta => BETA_ENDPOINT,
        UpdateChannel::Stable => STABLE_ENDPOINT,
    }
}

/// The release page of `version` (tag `v<version>`), or the release list.
pub fn release_url(version: Option<&str>) -> String {
    match version {
        Some(v) => format!("{REPO_URL}/releases/tag/v{v}"),
        None => format!("{REPO_URL}/releases"),
    }
}

/// The version the app shows (About page, bridge `status`): CI's
/// `KEYSTEAD_VERSION_LABEL` if set, else the version from `tauri.conf.json`
/// (which CI also sets per build, e.g. `2.0.0-beta.42`).
pub fn version_label(package_version: &str) -> String {
    option_env!("KEYSTEAD_VERSION_LABEL")
        .filter(|label| !label.is_empty())
        .unwrap_or(package_version)
        .to_owned()
}

/// Result of `check_update`, payload of `update://available`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// A newer version than the running one is offered.
    pub available: bool,
    pub current_version: String,
    /// The version `latest.json` names (`null` if the channel has none yet).
    pub version: Option<String>,
    pub notes: Option<String>,
    /// Release date, RFC 3339.
    pub date: Option<String>,
    /// This copy can install it (installed copy with a build for this
    /// platform); otherwise the UI links to `release_url`.
    pub can_install: bool,
    pub release_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    downloaded: u64,
    total: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadyPayload {
    version: String,
}

/// The release `latest.json` announced (independent of the platform).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseFacts {
    pub version: Version,
    pub notes: Option<String>,
    pub date: Option<String>,
}

impl ReleaseFacts {
    fn of(release: &RemoteRelease) -> Self {
        ReleaseFacts {
            version: release.version.clone(),
            notes: release.notes.clone().filter(|n| !n.trim().is_empty()),
            date: release.pub_date.and_then(|d| {
                d.format(&time::format_description::well_known::Rfc3339)
                    .ok()
            }),
        }
    }
}

/// Builds the [`UpdateInfo`] for the running version `current`, the release
/// the channel announces (if any) and whether this copy could install it.
pub fn info_from(
    current: &Version,
    release: Option<&ReleaseFacts>,
    can_install: bool,
) -> UpdateInfo {
    let available = release.is_some_and(|r| r.version > *current);
    let version = release.map(|r| r.version.to_string());
    UpdateInfo {
        available,
        current_version: current.to_string(),
        release_url: release_url(version.as_deref()),
        version,
        notes: release.and_then(|r| r.notes.clone()),
        date: release.and_then(|r| r.date.clone()),
        can_install: available && can_install,
    }
}

/// What remains of a failed `latest.json` check.
#[derive(Debug, PartialEq, Eq)]
pub enum CheckFailure {
    /// The channel has no `latest.json` (yet, HTTP 404/410): nothing to
    /// offer.
    NoRelease,
    /// The release has no build for this platform: show it, but only as a
    /// download link.
    NoBuildForPlatform,
    /// Network/HTTP/parse error (`io:<detail>`).
    Other(String),
}

/// Classifies an updater error. `ReleaseNotFound` is only a candidate for
/// [`CheckFailure::NoRelease`]: the plugin also reports it for a 403 or 5xx
/// answer, see [`missing_release_cause`].
pub fn classify(error: &UpdaterError) -> CheckFailure {
    match error {
        UpdaterError::ReleaseNotFound => CheckFailure::NoRelease,
        UpdaterError::TargetNotFound(_) | UpdaterError::TargetsNotFound(_) => {
            CheckFailure::NoBuildForPlatform
        }
        other => CheckFailure::Other(other.to_string()),
    }
}

/// Why the plugin found no release, judged by the status a second request
/// for `latest.json` gets. The plugin treats every non-2xx answer as "no
/// release" (`ReleaseNotFound`) and drops the status, so a rate limit (403,
/// 429) or a server error (5xx) would read as "up to date".
///
/// Only 404/410 mean "nothing to offer": the channel has no `latest.json`
/// (yet), e.g. the stable channel while GitHub's "latest" release is still a
/// VaultX 1.x one. A success now means the earlier answer was a passing error.
pub fn missing_release_cause(status: reqwest::StatusCode) -> CheckFailure {
    use reqwest::StatusCode;
    if status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
        CheckFailure::NoRelease
    } else if status.is_success() {
        CheckFailure::Other("the update server answered with an error, try again".into())
    } else {
        CheckFailure::Other(format!("HTTP {status}"))
    }
}

/// The HTTP client for [`missing_release`]: like the plugin's (system proxy,
/// redirects followed, timeout).
fn probe_client() -> Result<reqwest::Client, String> {
    // `rustls-no-provider`: building a client without a process-wide crypto
    // provider panics. The plugin's check installs ring the same way.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    reqwest::Client::builder()
        .user_agent(concat!("Keystead/", env!("CARGO_PKG_VERSION")))
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

/// Requests `url` once more after the plugin reported `ReleaseNotFound` and
/// classifies the answer ([`missing_release_cause`]); a network error is
/// [`CheckFailure::Other`].
pub async fn missing_release(client: &reqwest::Client, url: &str) -> CheckFailure {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await;
    match response {
        Ok(response) => missing_release_cause(response.status()),
        Err(e) => CheckFailure::Other(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Installed or portable?
// ---------------------------------------------------------------------------

/// Normalises a Windows directory for comparison: no surrounding quotes or
/// whitespace, no `\\?\` prefix, backslashes, no trailing separator,
/// lowercase (NTFS paths are case-insensitive).
#[cfg_attr(not(windows), allow(dead_code))]
fn normalize_windows_dir(dir: &str) -> String {
    let dir = dir.trim().trim_matches('"').trim();
    let dir = dir.strip_prefix(r"\\?\").unwrap_or(dir);
    dir.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Whether the exe in `exe_dir` is an installed copy (Windows paths): it
/// lives in the per-user NSIS install directory (`%LOCALAPPDATA%\Keystead`)
/// or in a directory an uninstall entry of the installer names
/// (`InstallLocation`). Everything else is a portable copy, which must not
/// run the installer (it would install a second copy elsewhere).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn is_installed_copy(exe_dir: &str, per_user_install_dir: &str, registered: &[String]) -> bool {
    let exe_dir = normalize_windows_dir(exe_dir);
    if exe_dir.is_empty() {
        return false;
    }
    std::iter::once(per_user_install_dir)
        .chain(registered.iter().map(String::as_str))
        .map(normalize_windows_dir)
        .any(|dir| !dir.is_empty() && dir == exe_dir)
}

/// The directories the installer's uninstall entry
/// (`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Keystead`,
/// written by the Tauri NSIS template as `UNINSTKEY` = product name) names:
/// `InstallLocation` and the folder of `UninstallString`.
#[cfg(windows)]
fn registered_install_dirs() -> Vec<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Keystead";
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(UNINSTALL_KEY) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    if let Ok(location) = key.get_value::<String, _>("InstallLocation") {
        dirs.push(location);
    }
    if let Ok(uninstaller) = key.get_value::<String, _>("UninstallString") {
        let path = uninstaller.trim().trim_matches('"').to_owned();
        if let Some(dir) = std::path::Path::new(&path).parent() {
            dirs.push(dir.to_string_lossy().into_owned());
        }
    }
    dirs
}

/// Whether this copy can install updates itself: Windows – an installed
/// copy (see [`is_installed_copy`]); Linux – an AppImage; macOS – never
/// (no builds are published).
pub fn can_install(app: &AppHandle) -> bool {
    static CAN_INSTALL: OnceLock<bool> = OnceLock::new();
    *CAN_INSTALL.get_or_init(|| {
        #[cfg(windows)]
        {
            let _ = app;
            let Some(exe_dir) = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|d| d.to_string_lossy().into_owned()))
            else {
                return false;
            };
            let per_user = keystead_core::paths::default_data_dir();
            is_installed_copy(
                &exe_dir,
                &per_user.to_string_lossy(),
                &registered_install_dirs(),
            )
        }
        #[cfg(target_os = "linux")]
        {
            app.env().appimage.is_some()
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = app;
            false
        }
    })
}

// ---------------------------------------------------------------------------
// Checking
// ---------------------------------------------------------------------------

fn build_updater(
    app: &AppHandle,
    channel: UpdateChannel,
    seen: Arc<Mutex<Option<ReleaseFacts>>>,
) -> Result<Updater, UpdaterError> {
    let url = Url::parse(endpoint(channel)).map_err(|_| UpdaterError::EmptyEndpoints)?;
    let exit_app = app.clone();
    app.updater_builder()
        .endpoints(vec![url])?
        .timeout(CHECK_TIMEOUT)
        // Windows: runs once the setup is written, right before it is started
        // and the process ends with `exit(0)` – no `RunEvent::Exit`, so no
        // `Core::shutdown`. Replaces the plugin's default hook, whose
        // `cleanup_before_exit` (tray icon, resources) is kept. The vault was
        // closed and the bridge stopped before `install`; this repeats it
        // for anything copied since (idempotent). If starting the setup
        // fails, `install_inner` undoes it.
        .on_before_exit(move || {
            if let Some(core) = exit_app.try_state::<Arc<Core>>() {
                core.lock_for_update();
            }
            exit_app.cleanup_before_exit();
        })
        // Called with the parsed `latest.json` before the platform entry is
        // looked up: remembers the release even if it has no build for this
        // platform (then shown as a download link).
        .version_comparator(move |current, release| {
            let newer = release.version > current;
            *seen.lock().unwrap_or_else(|p| p.into_inner()) = Some(ReleaseFacts::of(&release));
            newer
        })
        .build()
}

/// Checks the channel. Returns the info for the UI and the installable
/// update (only when `info.can_install`). No `latest.json` (404/410) → not
/// available; any other HTTP error, a network error or an unreadable
/// manifest → `io:<detail>`.
pub async fn check(
    app: &AppHandle,
    channel: UpdateChannel,
) -> AppResult<(UpdateInfo, Option<Update>)> {
    let current = app.package_info().version.clone();
    let installable = can_install(app);
    let seen = Arc::new(Mutex::new(None));
    let updater = build_updater(app, channel, Arc::clone(&seen))
        .map_err(|e| AppError::io(format!("updater: {e}")))?;
    let result = updater.check().await;
    let facts = seen.lock().unwrap_or_else(|p| p.into_inner()).take();
    match result {
        Ok(update) => {
            let info = info_from(&current, facts.as_ref(), installable && update.is_some());
            let update = update.filter(|_| info.can_install);
            Ok((info, update))
        }
        Err(e) => {
            let failure = match classify(&e) {
                // Any non-2xx answer: ask again to tell "no latest.json"
                // (404) from a rate limit or server error.
                CheckFailure::NoRelease => match probe_client() {
                    Ok(client) => missing_release(&client, endpoint(channel)).await,
                    Err(detail) => CheckFailure::Other(detail),
                },
                other => other,
            };
            match failure {
                CheckFailure::NoRelease => Ok((info_from(&current, None, false), None)),
                CheckFailure::NoBuildForPlatform => {
                    Ok((info_from(&current, facts.as_ref(), false), None))
                }
                CheckFailure::Other(detail) => Err(AppError::io(detail)),
            }
        }
    }
}

/// `check_update`: checks the configured channel now and remembers the
/// result (a found update is not announced again by the background check).
pub async fn check_now(app: &AppHandle, core: &Arc<Core>) -> AppResult<UpdateInfo> {
    let channel = core.state().settings.update_channel;
    let (info, _) = check(app, channel).await?;
    core.remember_update(&info);
    Ok(info)
}

/// Background checks: [`FIRST_CHECK_DELAY`] after start, then every
/// [`CHECK_INTERVAL`] and whenever [`Core::wake_update_checker`] is called
/// (setting turned on, channel changed), while `updateCheck` is on. A newly
/// found version is announced with `update://available`; errors are only
/// logged.
pub fn spawn_background_checks(app: AppHandle, core: &Arc<Core>) {
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    core.set_update_wake(wake_tx);
    let weak: Weak<Core> = Arc::downgrade(core);
    let spawned = std::thread::Builder::new()
        .name("keystead-update-check".into())
        .spawn(move || {
            if matches!(
                wake_rx.recv_timeout(FIRST_CHECK_DELAY),
                Err(RecvTimeoutError::Disconnected)
            ) {
                return;
            }
            loop {
                {
                    let Some(core) = weak.upgrade() else { return };
                    if core.is_exiting() {
                        return;
                    }
                    let (enabled, channel) = {
                        let st = core.state();
                        (st.settings.update_check, st.settings.update_channel)
                    };
                    if enabled && !core.update_installing() {
                        match tauri::async_runtime::block_on(check(&app, channel)) {
                            Ok((info, _)) => {
                                if core.remember_update(&info) {
                                    core.emit(EVENT_AVAILABLE, info);
                                }
                            }
                            Err(e) => log(format_args!("update check failed: {}", e.code())),
                        }
                    }
                }
                match wake_rx.recv_timeout(CHECK_INTERVAL) {
                    Ok(()) => {
                        // Several wake-ups in a row (settings toggled) → one check.
                        while wake_rx.try_recv().is_ok() {}
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
    if let Err(e) = spawned {
        log(format_args!("could not start the update check: {e}"));
    }
}

// ---------------------------------------------------------------------------
// Installing
// ---------------------------------------------------------------------------

/// `install_update`: downloads the update of the configured channel,
/// verifies its signature (inside the plugin), locks the vault, clears a
/// copied secret and installs it. On Windows the plugin then starts the
/// setup and ends this process (the setup restarts the app); elsewhere the
/// app restarts. Only returns on failure (or once the restart is under way).
pub async fn install(app: &AppHandle, core: &Arc<Core>) -> AppResult<()> {
    if !core.begin_update_install() {
        return Err(AppError::invalid("update_in_progress"));
    }
    let result = install_inner(app, core).await;
    if result.is_err() {
        core.end_update_install();
    }
    result
}

async fn install_inner(app: &AppHandle, core: &Arc<Core>) -> AppResult<()> {
    if !can_install(app) {
        return Err(AppError::unsupported("update_portable"));
    }
    // The setup ends every running Keystead.exe without asking – also a
    // terminal UI (`--cli`), losing its unsaved input and leaving a secret
    // it copied on the clipboard. Checked again right before installing.
    refuse_while_terminal_ui_runs()?;
    let channel = core.state().settings.update_channel;
    let (info, update) = check(app, channel).await?;
    core.remember_update(&info);
    let Some(update) = update else {
        return Err(if info.available {
            AppError::unsupported("update_platform")
        } else {
            keystead_core::Error::NotFound("update".into()).into()
        });
    };

    let mut downloaded: u64 = 0;
    let mut last_emit: Option<Instant> = None;
    let progress_core = Arc::clone(core);
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                let due = last_emit.is_none_or(|t| t.elapsed() >= PROGRESS_INTERVAL);
                if due || total == Some(downloaded) {
                    last_emit = Some(Instant::now());
                    progress_core.emit(EVENT_PROGRESS, ProgressPayload { downloaded, total });
                }
            },
            || {},
        )
        .await
        .map_err(|e| AppError::io(format!("update: {e}")))?;
    core.emit(
        EVENT_READY,
        ReadyPayload {
            version: update.version.clone(),
        },
    );

    refuse_while_terminal_ui_runs()?;
    // Nothing decrypted may outlive the process, and the setup replaces the
    // executable: stop the bridge, close the vault (and keep it closed) and
    // clear a copied secret first.
    let was_open = core.lock_for_update();
    log(format_args!("installing update {}", update.version));
    match update.install(&bytes) {
        Ok(()) => {
            // Windows never gets here (the plugin exits the process once the
            // setup runs). AppImage: the file was replaced – restart into it
            // (`RunEvent::Exit` → `Core::shutdown`).
            app.request_restart();
            Ok(())
        }
        Err(e) => {
            core.resume_after_failed_update();
            if was_open {
                // The page still shows the vault: back to the unlock screen.
                core.emit_locked(LockReason::Manual);
            }
            Err(AppError::io(format!("update: {e}")))
        }
    }
}

/// `invalid_input:update_tui_running` while a terminal UI runs (Windows;
/// see [`platform::terminal_ui_running`]).
fn refuse_while_terminal_ui_runs() -> AppResult<()> {
    if platform::terminal_ui_running() {
        return Err(AppError::invalid("update_tui_running"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn facts(version: &str) -> ReleaseFacts {
        ReleaseFacts {
            version: v(version),
            notes: Some("Neu: Updates in der App".into()),
            date: Some("2026-10-09T12:00:00Z".into()),
        }
    }

    #[test]
    fn endpoint_follows_the_channel() {
        assert_eq!(
            endpoint(UpdateChannel::Beta),
            "https://github.com/CedrickGD/Keystead/releases/download/updater-beta/latest.json"
        );
        assert_eq!(
            endpoint(UpdateChannel::Stable),
            "https://github.com/CedrickGD/Keystead/releases/latest/download/latest.json"
        );
        // Both are valid https URLs (the updater refuses anything else in
        // release builds).
        for channel in [UpdateChannel::Beta, UpdateChannel::Stable] {
            assert_eq!(Url::parse(endpoint(channel)).unwrap().scheme(), "https");
        }
    }

    #[test]
    fn semver_orders_betas_before_the_release() {
        // CI versions every build: 2.0.0-beta.<run>, then 2.0.0.
        assert!(v("2.0.0-beta.4") < v("2.0.0-beta.5"));
        assert!(v("2.0.0-beta.9") < v("2.0.0-beta.10"));
        assert!(v("2.0.0-beta.10") < v("2.0.0"));
        assert!(v("2.0.0") < v("2.0.1-beta.1"));
    }

    #[test]
    fn info_for_a_newer_release() {
        let info = info_from(&v("2.0.0-beta.4"), Some(&facts("2.0.0-beta.5")), true);
        assert_eq!(
            info,
            UpdateInfo {
                available: true,
                current_version: "2.0.0-beta.4".into(),
                version: Some("2.0.0-beta.5".into()),
                notes: Some("Neu: Updates in der App".into()),
                date: Some("2026-10-09T12:00:00Z".into()),
                can_install: true,
                release_url: "https://github.com/CedrickGD/Keystead/releases/tag/v2.0.0-beta.5"
                    .into(),
            }
        );
        let json = serde_json::to_value(&info).unwrap();
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys.len(),
            7,
            "available, currentVersion, version, notes, date, canInstall, releaseUrl: {keys:?}"
        );
        assert_eq!(json["canInstall"], true);
        assert_eq!(json["currentVersion"], "2.0.0-beta.4");
    }

    #[test]
    fn info_without_an_update() {
        // Same or older version: not available, never installable.
        let same = info_from(&v("2.0.0-beta.5"), Some(&facts("2.0.0-beta.5")), true);
        assert!(!same.available && !same.can_install);
        assert_eq!(same.version.as_deref(), Some("2.0.0-beta.5"));
        let older = info_from(&v("2.0.0"), Some(&facts("2.0.0-beta.9")), true);
        assert!(!older.available);
        // No release on the channel yet.
        let none = info_from(&v("2.0.0"), None, true);
        assert!(!none.available && !none.can_install);
        assert_eq!(none.version, None);
        assert_eq!(
            none.release_url,
            "https://github.com/CedrickGD/Keystead/releases"
        );
        // Portable copy: available, but only as a download.
        let portable = info_from(&v("2.0.0-beta.4"), Some(&facts("2.0.0")), false);
        assert!(portable.available && !portable.can_install);
    }

    #[test]
    fn errors_are_classified() {
        assert_eq!(
            classify(&UpdaterError::ReleaseNotFound),
            CheckFailure::NoRelease
        );
        assert_eq!(
            classify(&UpdaterError::TargetsNotFound(vec!["linux-x86_64".into()])),
            CheckFailure::NoBuildForPlatform
        );
        assert_eq!(
            classify(&UpdaterError::TargetNotFound("linux-x86_64".into())),
            CheckFailure::NoBuildForPlatform
        );
        assert!(matches!(
            classify(&UpdaterError::Network("offline".into())),
            CheckFailure::Other(_)
        ));
    }

    #[test]
    fn only_404_and_410_mean_no_release() {
        use reqwest::StatusCode;
        for status in [StatusCode::NOT_FOUND, StatusCode::GONE] {
            assert_eq!(missing_release_cause(status), CheckFailure::NoRelease);
        }
        // Rate limits and server errors must not read as "up to date".
        for (status, detail) in [
            (StatusCode::FORBIDDEN, "HTTP 403 Forbidden"),
            (StatusCode::TOO_MANY_REQUESTS, "HTTP 429 Too Many Requests"),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "HTTP 500 Internal Server Error",
            ),
            (StatusCode::BAD_GATEWAY, "HTTP 502 Bad Gateway"),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "HTTP 503 Service Unavailable",
            ),
            (StatusCode::UNAUTHORIZED, "HTTP 401 Unauthorized"),
        ] {
            assert_eq!(
                missing_release_cause(status),
                CheckFailure::Other(detail.into())
            );
        }
        // Answered now, not a moment ago: still an error, not "no release".
        assert!(matches!(
            missing_release_cause(StatusCode::OK),
            CheckFailure::Other(_)
        ));
    }

    /// A one-path-per-status HTTP server on 127.0.0.1: `/<status>` answers
    /// with that status, `/redirect/<status>` with a 302 to `/<status>` (like
    /// GitHub's release asset redirect).
    fn status_server() -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                let _ = reader.read_line(&mut request_line);
                let mut header = String::new();
                while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
                    header.clear();
                }
                let path = request_line.split(' ').nth(1).unwrap_or("/").to_owned();
                let response = match path.strip_prefix("/redirect/") {
                    Some(status) => format!(
                        "HTTP/1.1 302 Found\r\nLocation: /{status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    ),
                    None => format!(
                        "HTTP/1.1 {} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}",
                        path.trim_start_matches('/')
                    ),
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        base
    }

    fn test_client() -> reqwest::Client {
        // The production client must build (crypto provider installed) …
        probe_client().unwrap();
        // … the tests' one ignores proxies from the environment.
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
    }

    #[test]
    fn a_failed_check_asks_the_server_why() {
        let base = status_server();
        let client = test_client();
        let ask = |path: &str| {
            tauri::async_runtime::block_on(missing_release(&client, &format!("{base}{path}")))
        };
        assert_eq!(ask("/404"), CheckFailure::NoRelease);
        assert_eq!(ask("/410"), CheckFailure::NoRelease);
        assert_eq!(ask("/redirect/404"), CheckFailure::NoRelease);
        assert_eq!(
            ask("/403"),
            CheckFailure::Other("HTTP 403 Forbidden".into())
        );
        assert_eq!(
            ask("/503"),
            CheckFailure::Other("HTTP 503 Service Unavailable".into())
        );
        assert_eq!(
            ask("/redirect/429"),
            CheckFailure::Other("HTTP 429 Too Many Requests".into())
        );
        assert!(matches!(ask("/200"), CheckFailure::Other(_)));

        // Nobody listening: a network error.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/latest.json", closed.local_addr().unwrap());
        drop(closed);
        assert!(matches!(
            tauri::async_runtime::block_on(missing_release(&client, &url)),
            CheckFailure::Other(detail) if !detail.is_empty()
        ));
    }

    #[test]
    fn installed_or_portable() {
        let per_user = r"C:\Users\Ann\AppData\Local\Keystead";
        // The per-user setup's default location.
        assert!(is_installed_copy(per_user, per_user, &[]));
        assert!(is_installed_copy(
            r"c:\users\ann\appdata\local\keystead\",
            per_user,
            &[]
        ));
        assert!(is_installed_copy(
            r"\\?\C:\Users\Ann\AppData\Local\Keystead",
            per_user,
            &[]
        ));
        // Installed elsewhere: the uninstall entry names the folder (the
        // NSIS template writes InstallLocation in quotes).
        let registered = vec![r#""D:\Apps\Keystead""#.to_owned()];
        assert!(is_installed_copy(
            r"D:\Apps\Keystead",
            per_user,
            &registered
        ));
        assert!(is_installed_copy(
            "D:/Apps/Keystead/",
            per_user,
            &registered
        ));
        // Portable: unzipped anywhere else.
        assert!(!is_installed_copy(
            r"C:\Users\Ann\Downloads\Keystead-2.0.0",
            per_user,
            &registered
        ));
        assert!(!is_installed_copy(r"E:\Keystead", per_user, &[]));
        // Below the install folder is not the installed exe either.
        assert!(!is_installed_copy(
            r"C:\Users\Ann\AppData\Local\Keystead\portable",
            per_user,
            &[]
        ));
        // Empty values never match.
        assert!(!is_installed_copy("", "", &[String::new()]));
        assert!(!is_installed_copy(r"D:\x", per_user, &[r#""""#.to_owned()]));
    }

    #[test]
    fn release_urls() {
        assert_eq!(
            release_url(Some("2.0.0")),
            "https://github.com/CedrickGD/Keystead/releases/tag/v2.0.0"
        );
        assert_eq!(
            release_url(None),
            "https://github.com/CedrickGD/Keystead/releases"
        );
    }
}
