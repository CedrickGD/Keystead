//! Native messaging host registration for Chromium-based browsers (see
//! "Native host registration" in docs/ARCHITECTURE.md).
//!
//! The manifest `com.keystead.bridge.json` is always written to
//! `<data_dir>/native-host/`. Then per browser:
//! * Windows: `HKCU\Software\<vendor>\NativeMessagingHosts\com.keystead.bridge`
//!   (default value = path of that manifest). Detected = the browser's
//!   `%LOCALAPPDATA%\...\User Data` directory exists.
//! * Linux/macOS: a copy of the manifest in the browser's
//!   `NativeMessagingHosts` directory. Detected = the browser's config
//!   directory exists.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use keystead_core::paths;

use crate::error::Result;
use crate::util::write_atomic;

/// Native messaging host name.
pub const HOST_NAME: &str = "com.keystead.bridge";
/// ID of the Keystead extension (fixed by the public key in its manifest).
pub const EXTENSION_ID: &str = "imfndemblnaalppnmdplagajjielnaok";
/// File name of the host manifest.
pub const MANIFEST_FILE_NAME: &str = "com.keystead.bridge.json";

/// A supported browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserId {
    Chrome,
    Edge,
    Brave,
    Chromium,
    Vivaldi,
}

impl BrowserId {
    pub const ALL: [BrowserId; 5] = [
        BrowserId::Chrome,
        BrowserId::Edge,
        BrowserId::Brave,
        BrowserId::Chromium,
        BrowserId::Vivaldi,
    ];

    /// The id used in the protocol / UI, e.g. `"edge"`.
    pub const fn as_str(self) -> &'static str {
        match self {
            BrowserId::Chrome => "chrome",
            BrowserId::Edge => "edge",
            BrowserId::Brave => "brave",
            BrowserId::Chromium => "chromium",
            BrowserId::Vivaldi => "vivaldi",
        }
    }

    /// Human-readable product name.
    pub const fn display_name(self) -> &'static str {
        match self {
            BrowserId::Chrome => "Google Chrome",
            BrowserId::Edge => "Microsoft Edge",
            BrowserId::Brave => "Brave",
            BrowserId::Chromium => "Chromium",
            BrowserId::Vivaldi => "Vivaldi",
        }
    }
}

impl fmt::Display for BrowserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error of parsing an unknown browser id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown browser id: {0}")]
pub struct UnknownBrowser(pub String);

impl FromStr for BrowserId {
    type Err = UnknownBrowser;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        BrowserId::ALL
            .into_iter()
            .find(|b| b.as_str().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| UnknownBrowser(s.to_owned()))
    }
}

/// Browser state for the settings page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserInfo {
    pub id: BrowserId,
    pub name: String,
    /// The browser seems to be installed (profile/config directory exists).
    pub detected: bool,
    /// A Keystead host manifest is registered for this browser.
    pub registered: bool,
}

/// Content of `com.keystead.bridge.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostManifest {
    pub name: String,
    pub description: String,
    /// Absolute path of the host executable.
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub allowed_origins: Vec<String>,
}

impl HostManifest {
    /// The manifest for host executable `exe_path` (should be absolute).
    pub fn new(exe_path: &Path) -> HostManifest {
        HostManifest {
            name: HOST_NAME.to_owned(),
            description: "Keystead browser bridge".to_owned(),
            path: exe_path.to_string_lossy().into_owned(),
            kind: "stdio".to_owned(),
            allowed_origins: vec![allowed_origin()],
        }
    }
}

/// `chrome-extension://<EXTENSION_ID>/`.
pub fn allowed_origin() -> String {
    format!("chrome-extension://{EXTENSION_ID}/")
}

/// `<data_dir>/native-host`.
pub fn manifest_dir() -> PathBuf {
    paths::data_dir().join("native-host")
}

/// `<data_dir>/native-host/com.keystead.bridge.json`.
pub fn manifest_path() -> PathBuf {
    manifest_dir().join(MANIFEST_FILE_NAME)
}

/// Detection and registration state of all supported browsers.
pub fn browsers() -> Vec<BrowserInfo> {
    browsers_in(&Layout::current())
}

/// Registers the native host for `ids` with `exe_path` (made absolute) as
/// host executable. Other browsers are left untouched.
pub fn register(ids: &[BrowserId], exe_path: &Path) -> Result<()> {
    register_in(&Layout::current(), ids, exe_path)
}

/// Removes the registration for `ids`. The shared manifest in the data
/// directory is removed once no browser uses it anymore.
pub fn unregister(ids: &[BrowserId]) -> Result<()> {
    unregister_in(&Layout::current(), ids)
}

/// True if some browser has a Keystead registration that does not point to
/// `exe_path` (the portable exe was moved) or whose manifest is missing or
/// broken. Re-register [`registered_browsers`] in that case.
pub fn needs_reregister(exe_path: &Path) -> bool {
    needs_reregister_in(&Layout::current(), exe_path)
}

/// Browsers with any Keystead registration (valid or broken).
pub fn registered_browsers() -> Vec<BrowserId> {
    registered_browsers_in(&Layout::current())
}

// ---------------------------------------------------------------------------
// Implementation (platform-independent part)
// ---------------------------------------------------------------------------

/// Where things live; tests substitute temporary directories.
#[derive(Debug, Clone)]
struct Layout {
    /// The shared manifest in the data directory.
    manifest_path: PathBuf,
    /// Windows: `%LOCALAPPDATA%`; Linux: `~/.config`; macOS:
    /// `~/Library/Application Support`.
    base: Option<PathBuf>,
}

impl Layout {
    fn current() -> Layout {
        Layout {
            manifest_path: manifest_path(),
            base: platform_base(),
        }
    }
}

#[cfg(windows)]
fn platform_base() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::data_local_dir)
}

#[cfg(not(windows))]
fn platform_base() -> Option<PathBuf> {
    dirs::config_dir()
}

/// Relative path (below the platform base) whose existence means the
/// browser is installed.
fn browser_dir_parts(id: BrowserId) -> &'static [&'static str] {
    if cfg!(windows) {
        match id {
            BrowserId::Chrome => &["Google", "Chrome", "User Data"],
            BrowserId::Edge => &["Microsoft", "Edge", "User Data"],
            BrowserId::Brave => &["BraveSoftware", "Brave-Browser", "User Data"],
            BrowserId::Chromium => &["Chromium", "User Data"],
            BrowserId::Vivaldi => &["Vivaldi", "User Data"],
        }
    } else if cfg!(target_os = "macos") {
        match id {
            BrowserId::Chrome => &["Google", "Chrome"],
            BrowserId::Edge => &["Microsoft Edge"],
            BrowserId::Brave => &["BraveSoftware", "Brave-Browser"],
            BrowserId::Chromium => &["Chromium"],
            BrowserId::Vivaldi => &["Vivaldi"],
        }
    } else {
        match id {
            BrowserId::Chrome => &["google-chrome"],
            BrowserId::Edge => &["microsoft-edge"],
            BrowserId::Brave => &["BraveSoftware", "Brave-Browser"],
            BrowserId::Chromium => &["chromium"],
            BrowserId::Vivaldi => &["vivaldi"],
        }
    }
}

fn browser_dir(layout: &Layout, id: BrowserId) -> Option<PathBuf> {
    let mut dir = layout.base.clone()?;
    dir.extend(browser_dir_parts(id));
    Some(dir)
}

/// The browser's directory exists and holds more than what registering
/// created there itself (on Linux/macOS registering creates
/// `<dir>/NativeMessagingHosts`, which must not count as "installed").
fn is_detected(layout: &Layout, id: BrowserId) -> bool {
    let Some(dir) = browser_dir(layout, id) else {
        return false;
    };
    fs::read_dir(dir).is_ok_and(|mut entries| {
        entries.any(|e| e.is_ok_and(|e| e.file_name() != "NativeMessagingHosts"))
    })
}

/// Registration state of one browser.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Registration {
    None,
    /// A readable Keystead manifest is registered.
    Ours(HostManifest),
    /// Registered, but the manifest is missing or unreadable.
    Broken,
}

fn read_manifest(path: &Path) -> Option<HostManifest> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice::<HostManifest>(&bytes)
        .ok()
        .filter(|m| m.name == HOST_NAME)
}

fn browsers_in(layout: &Layout) -> Vec<BrowserInfo> {
    BrowserId::ALL
        .into_iter()
        .map(|id| BrowserInfo {
            id,
            name: id.display_name().to_owned(),
            detected: is_detected(layout, id),
            registered: matches!(registration(layout, id), Registration::Ours(_)),
        })
        .collect()
}

fn registered_browsers_in(layout: &Layout) -> Vec<BrowserId> {
    BrowserId::ALL
        .into_iter()
        .filter(|id| registration(layout, *id) != Registration::None)
        .collect()
}

fn needs_reregister_in(layout: &Layout, exe_path: &Path) -> bool {
    let exe = std::path::absolute(exe_path).unwrap_or_else(|_| exe_path.to_path_buf());
    BrowserId::ALL
        .into_iter()
        .any(|id| match registration(layout, id) {
            Registration::None => false,
            Registration::Broken => true,
            Registration::Ours(manifest) => !same_path(Path::new(&manifest.path), &exe),
        })
}

fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
        norm(a) == norm(b)
    } else {
        a == b
    }
}

fn manifest_bytes(exe_path: &Path) -> Result<Vec<u8>> {
    let exe = std::path::absolute(exe_path)?;
    let mut json = serde_json::to_vec_pretty(&HostManifest::new(&exe))?;
    json.push(b'\n');
    Ok(json)
}

fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Linux / macOS: manifest copies in the browsers' config directories
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn host_manifest_path(layout: &Layout, id: BrowserId) -> Option<PathBuf> {
    Some(
        browser_dir(layout, id)?
            .join("NativeMessagingHosts")
            .join(MANIFEST_FILE_NAME),
    )
}

#[cfg(not(windows))]
fn registration(layout: &Layout, id: BrowserId) -> Registration {
    let Some(path) = host_manifest_path(layout, id) else {
        return Registration::None;
    };
    if !path.exists() {
        return Registration::None;
    }
    read_manifest(&path).map_or(Registration::Broken, Registration::Ours)
}

#[cfg(not(windows))]
fn register_in(layout: &Layout, ids: &[BrowserId], exe_path: &Path) -> Result<()> {
    let json = manifest_bytes(exe_path)?;
    write_atomic(&layout.manifest_path, &json, false)?;
    for &id in ids {
        let path = host_manifest_path(layout, id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "cannot determine the configuration directory of {}",
                    id.display_name()
                ),
            )
        })?;
        write_atomic(&path, &json, false)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn unregister_in(layout: &Layout, ids: &[BrowserId]) -> Result<()> {
    for &id in ids {
        if let Some(path) = host_manifest_path(layout, id) {
            remove_file_if_exists(&path)?;
        }
    }
    if registered_browsers_in(layout).is_empty() {
        remove_file_if_exists(&layout.manifest_path)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Windows: HKCU registry keys pointing at the shared manifest
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn registry_key(id: BrowserId) -> String {
    let vendor = match id {
        BrowserId::Chrome => r"Google\Chrome",
        BrowserId::Edge => r"Microsoft\Edge",
        BrowserId::Brave => r"BraveSoftware\Brave-Browser",
        BrowserId::Chromium => "Chromium",
        BrowserId::Vivaldi => "Vivaldi",
    };
    format!(r"Software\{vendor}\NativeMessagingHosts\{HOST_NAME}")
}

#[cfg(windows)]
fn hkcu() -> winreg::RegKey {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
}

#[cfg(windows)]
fn registration(_layout: &Layout, id: BrowserId) -> Registration {
    let Ok(key) = hkcu().open_subkey(registry_key(id)) else {
        return Registration::None;
    };
    match key.get_value::<std::ffi::OsString, _>("") {
        Ok(value) if !value.is_empty() => {
            read_manifest(Path::new(&value)).map_or(Registration::Broken, Registration::Ours)
        }
        _ => Registration::Broken,
    }
}

#[cfg(windows)]
fn register_in(layout: &Layout, ids: &[BrowserId], exe_path: &Path) -> Result<()> {
    let json = manifest_bytes(exe_path)?;
    write_atomic(&layout.manifest_path, &json, false)?;
    let value = layout.manifest_path.as_os_str().to_os_string();
    let root = hkcu();
    for &id in ids {
        let (key, _) = root.create_subkey(registry_key(id))?;
        key.set_value("", &value)?;
    }
    Ok(())
}

#[cfg(windows)]
fn unregister_in(layout: &Layout, ids: &[BrowserId]) -> Result<()> {
    let root = hkcu();
    for &id in ids {
        match root.delete_subkey_all(registry_key(id)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    if registered_browsers_in(layout).is_empty() {
        remove_file_if_exists(&layout.manifest_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_id_serde_and_parse() {
        assert_eq!(serde_json::to_string(&BrowserId::Edge).unwrap(), "\"edge\"");
        let all: Vec<BrowserId> =
            serde_json::from_str(r#"["chrome","edge","brave","chromium","vivaldi"]"#).unwrap();
        assert_eq!(all, BrowserId::ALL);
        assert_eq!("Brave".parse::<BrowserId>(), Ok(BrowserId::Brave));
        assert!("firefox".parse::<BrowserId>().is_err());
        assert!(serde_json::from_str::<BrowserId>("\"firefox\"").is_err());
    }

    #[test]
    fn browser_info_json_shape() {
        let info = BrowserInfo {
            id: BrowserId::Chrome,
            name: "Google Chrome".into(),
            detected: true,
            registered: false,
        };
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            serde_json::json!({"id":"chrome","name":"Google Chrome","detected":true,"registered":false})
        );
    }

    #[test]
    fn manifest_content() {
        let exe = std::path::absolute("Keystead").unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&manifest_bytes(&exe).unwrap()).unwrap();
        assert_eq!(json["name"], "com.keystead.bridge");
        assert_eq!(json["type"], "stdio");
        assert_eq!(json["path"], exe.to_string_lossy().as_ref());
        assert_eq!(
            json["allowed_origins"],
            serde_json::json!(["chrome-extension://imfndemblnaalppnmdplagajjielnaok/"])
        );
        assert!(json["description"].is_string());
        // Relative paths are made absolute.
        let json: serde_json::Value =
            serde_json::from_slice(&manifest_bytes(Path::new("rel/Keystead")).unwrap()).unwrap();
        assert!(Path::new(json["path"].as_str().unwrap()).is_absolute());
    }

    #[cfg(not(windows))]
    #[test]
    fn register_detect_unregister_in_temp_layout() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("config");
        let layout = Layout {
            manifest_path: dir.path().join("data/native-host").join(MANIFEST_FILE_NAME),
            base: Some(base.clone()),
        };
        // Pretend Chrome and Brave are installed (they have profile data);
        // an empty Chromium directory does not count.
        let chrome_dir = browser_dir(&layout, BrowserId::Chrome).unwrap();
        fs::create_dir_all(chrome_dir.join("Default")).unwrap();
        let brave_dir = browser_dir(&layout, BrowserId::Brave).unwrap();
        fs::create_dir_all(&brave_dir).unwrap();
        fs::write(brave_dir.join("Local State"), b"{}").unwrap();
        fs::create_dir_all(browser_dir(&layout, BrowserId::Chromium).unwrap()).unwrap();

        let info = browsers_in(&layout);
        assert_eq!(info.len(), 5);
        let detected: Vec<_> = info.iter().filter(|b| b.detected).map(|b| b.id).collect();
        assert_eq!(detected, vec![BrowserId::Chrome, BrowserId::Brave]);
        assert!(info.iter().all(|b| !b.registered));
        assert!(!needs_reregister_in(&layout, Path::new("/opt/Keystead")));

        let exe = Path::new("/opt/keystead/Keystead");
        register_in(&layout, &[BrowserId::Chrome, BrowserId::Edge], exe).unwrap();
        assert!(layout.manifest_path.exists());
        let chrome_manifest = chrome_dir
            .join("NativeMessagingHosts")
            .join(MANIFEST_FILE_NAME);
        let m = read_manifest(&chrome_manifest).unwrap();
        assert_eq!(m, HostManifest::new(exe));
        // Edge was registered although not installed; the directories this
        // created do not make it "detected".
        let info = browsers_in(&layout);
        let registered: Vec<_> = info.iter().filter(|b| b.registered).map(|b| b.id).collect();
        assert_eq!(registered, vec![BrowserId::Chrome, BrowserId::Edge]);
        let detected: Vec<_> = info.iter().filter(|b| b.detected).map(|b| b.id).collect();
        assert_eq!(detected, vec![BrowserId::Chrome, BrowserId::Brave]);
        assert_eq!(registered_browsers_in(&layout), registered);

        assert!(!needs_reregister_in(&layout, exe));
        assert!(needs_reregister_in(&layout, Path::new("/elsewhere/Keystead")));

        // A broken manifest also asks for re-registration.
        fs::write(&chrome_manifest, b"{broken").unwrap();
        assert!(needs_reregister_in(&layout, exe));
        assert!(!browsers_in(&layout)[0].registered);
        register_in(&layout, &registered_browsers_in(&layout), exe).unwrap();
        assert!(!needs_reregister_in(&layout, exe));

        unregister_in(&layout, &[BrowserId::Chrome]).unwrap();
        assert!(!chrome_manifest.exists());
        assert!(layout.manifest_path.exists(), "still used by Edge");
        unregister_in(&layout, &BrowserId::ALL).unwrap();
        assert!(!layout.manifest_path.exists());
        assert!(registered_browsers_in(&layout).is_empty());
        // Unregistering twice is fine.
        unregister_in(&layout, &BrowserId::ALL).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn no_base_dir_means_nothing_detected() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout {
            manifest_path: dir.path().join(MANIFEST_FILE_NAME),
            base: None,
        };
        assert!(browsers_in(&layout)
            .iter()
            .all(|b| !b.detected && !b.registered));
        assert!(register_in(&layout, &[BrowserId::Chrome], Path::new("/x")).is_err());
    }
}
