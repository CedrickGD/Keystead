//! Environment-variable dependent behaviour of `paths`, `settings`,
//! `VaultStore::open_default` and `legacy_scan`. Everything runs in a single
//! test so no other thread observes the temporary environment changes.

use std::fs;
use std::path::Path;

use vaultx_core::import::legacy_scan;
use vaultx_core::paths::{
    self, data_dir, is_portable, legacy_dir, settings_path, vaults_dir, DATA_DIR_ENV,
    LEGACY_DIR_ENV,
};
use vaultx_core::settings::{self, Settings, Theme};
use vaultx_core::{KdfParams, VaultStore};

#[test]
fn environment_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let legacy = dir.path().join("legacy");

    // --- VAULTX_DATA_DIR -------------------------------------------------
    std::env::set_var(DATA_DIR_ENV, &data);
    assert_eq!(data_dir(), data);
    assert_eq!(vaults_dir(), data.join("vaults"));
    assert_eq!(settings_path(), data.join("settings.json"));
    assert!(!is_portable());

    // Settings: defaults when missing, atomic save, load.
    assert_eq!(settings::load(), Settings::default());
    let s = Settings {
        theme: Theme::Dark,
        auto_lock_minutes: 5,
        ..Settings::default()
    };
    settings::save(&s).unwrap();
    assert!(settings_path().exists());
    assert_eq!(Settings::load(), s);

    // Store in the default location.
    let store = VaultStore::open_default().unwrap();
    assert_eq!(store.root(), data.as_path());
    assert!(data.join("vaults").is_dir());
    let v = store
        .create_vault_with_params("Env", "pw", KdfParams::insecure_for_tests())
        .unwrap();
    assert!(v.path().starts_with(&data));
    assert_eq!(store.list_vaults().unwrap().len(), 1);

    // An empty value counts as unset.
    std::env::set_var(DATA_DIR_ENV, "");
    assert_ne!(data_dir(), data);
    std::env::remove_var(DATA_DIR_ENV);
    let resolved = data_dir();
    if is_portable() {
        assert_eq!(Some(resolved), paths::portable_dir());
    } else {
        assert_eq!(resolved, paths::default_data_dir());
    }

    // --- VAULTX_LEGACY_DIR -----------------------------------------------
    std::env::set_var(LEGACY_DIR_ENV, &legacy);
    assert_eq!(legacy_dir().as_deref(), Some(legacy.as_path()));
    assert!(
        legacy_scan().is_empty(),
        "missing directory → nothing found"
    );
    fs::create_dir_all(&legacy).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("legacy_v1.json");
    fs::copy(&fixture, legacy.join("vault_Alt_0BADF00D.json")).unwrap();
    let found = legacy_scan();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "Alt");

    std::env::remove_var(LEGACY_DIR_ENV);
    if cfg!(windows) {
        assert!(legacy_dir().is_some_and(|d| d.ends_with("VaultX")));
    } else {
        assert_eq!(legacy_dir(), None);
        assert!(legacy_scan().is_empty());
    }
}
