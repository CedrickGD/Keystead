//! Non-secret application settings (`<data_dir>/settings.json`).
//!
//! Loading never fails: a missing or unreadable file yields the defaults and
//! invalid individual values fall back to their default.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::format;
use crate::generator::{self, GeneratorOptions, GeneratorPreset};
use crate::paths;
use crate::util;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    De,
    En,
}

impl Language {
    /// German if the OS locale starts with "de", English otherwise.
    pub fn system_default() -> Self {
        Self::from_locale(sys_locale::get_locale().as_deref().unwrap_or(""))
    }

    fn from_locale(locale: &str) -> Self {
        if locale.trim().to_ascii_lowercase().starts_with("de") {
            Language::De
        } else {
            Language::En
        }
    }
}

impl Default for Language {
    fn default() -> Self {
        Self::system_default()
    }
}

/// Which releases the in-app updater offers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    /// Test versions (pre-releases) and stable releases.
    Beta,
    /// Stable releases only – the default since 2.0.0.
    #[default]
    Stable,
}

/// Upper bounds applied when loading/saving.
const MAX_AUTO_LOCK_MINUTES: u32 = 7 * 24 * 60;
const MAX_CLIPBOARD_CLEAR_SECONDS: u32 = 24 * 60 * 60;
/// Most user generator presets kept.
pub const GENERATOR_PRESETS_MAX: usize = 50;

/// App settings, serialised with camelCase keys (mirrors `Settings` in
/// `apps/desktop/src/lib/types.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme: Theme,
    pub language: Language,
    /// 0 = never.
    pub auto_lock_minutes: u32,
    pub lock_on_system_lock: bool,
    /// 0 = never.
    pub clipboard_clear_seconds: u32,
    pub minimize_to_tray: bool,
    pub start_in_tray: bool,
    pub browser_integration: bool,
    pub last_vault_id: Option<String>,
    /// Fetch missing website icons in the background, directly from the
    /// websites (no icon service). Off: no icon requests at all; stored
    /// icons are still shown.
    pub website_icons: bool,
    /// Look for app updates in the background (at start and every 6 h).
    pub update_check: bool,
    pub update_channel: UpdateChannel,
    /// The user's generator presets (the built-in ones live in code, see
    /// [`generator::builtin_presets`]). Normalised on load/save: invalid or
    /// duplicate ones are dropped, at most [`GENERATOR_PRESETS_MAX`].
    pub generator_presets: Vec<GeneratorPreset>,
    /// Id of the default preset (built-in or user); an unknown id falls
    /// back to [`generator::PRESET_STANDARD`].
    pub generator_default: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Theme::System,
            language: Language::system_default(),
            auto_lock_minutes: 15,
            lock_on_system_lock: true,
            clipboard_clear_seconds: 30,
            minimize_to_tray: true,
            start_in_tray: false,
            browser_integration: true,
            last_vault_id: None,
            website_icons: true,
            update_check: true,
            update_channel: UpdateChannel::Stable,
            generator_presets: Vec::new(),
            generator_default: generator::PRESET_STANDARD.to_owned(),
        }
    }
}

/// Deserialises `key` from `obj` into `target`, keeping the current value if
/// the key is missing or has an invalid value.
fn take<T: serde::de::DeserializeOwned>(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    target: &mut T,
) {
    if let Some(v) = obj.get(key) {
        if let Ok(parsed) = serde_json::from_value(v.clone()) {
            *target = parsed;
        }
    }
}

impl Settings {
    /// Loads from [`paths::settings_path`]; defaults if missing or corrupt.
    pub fn load() -> Settings {
        Self::load_from(&paths::settings_path())
    }

    /// Loads from an explicit path; defaults if missing or corrupt.
    pub fn load_from(path: &Path) -> Settings {
        match std::fs::read(path) {
            Ok(bytes) => Self::from_json_lenient(&String::from_utf8_lossy(&bytes)),
            Err(_) => Settings::default(),
        }
    }

    /// Parses settings JSON field by field; unknown keys are ignored and
    /// invalid values replaced by defaults.
    pub fn from_json_lenient(json: &str) -> Settings {
        let mut s = Settings::default();
        let Ok(serde_json::Value::Object(obj)) =
            serde_json::from_str::<serde_json::Value>(util::strip_bom(json))
        else {
            return s;
        };
        take(&obj, "theme", &mut s.theme);
        take(&obj, "language", &mut s.language);
        take(&obj, "autoLockMinutes", &mut s.auto_lock_minutes);
        take(&obj, "lockOnSystemLock", &mut s.lock_on_system_lock);
        take(
            &obj,
            "clipboardClearSeconds",
            &mut s.clipboard_clear_seconds,
        );
        take(&obj, "minimizeToTray", &mut s.minimize_to_tray);
        take(&obj, "startInTray", &mut s.start_in_tray);
        take(&obj, "browserIntegration", &mut s.browser_integration);
        take(&obj, "lastVaultId", &mut s.last_vault_id);
        // `showIcons` of older versions is ignored: it was never shown in
        // the UI, so its `false` was the old default, not a choice.
        take(&obj, "websiteIcons", &mut s.website_icons);
        take(&obj, "updateCheck", &mut s.update_check);
        take(&obj, "updateChannel", &mut s.update_channel);
        if let Some(serde_json::Value::Array(presets)) = obj.get("generatorPresets") {
            s.generator_presets = presets.iter().filter_map(preset_lenient).collect();
        }
        take(&obj, "generatorDefault", &mut s.generator_default);
        s.normalized()
    }

    /// Clamps numeric values to sane ranges, clears an empty
    /// `last_vault_id`, keeps only valid generator presets
    /// ([`GeneratorPreset::normalized`]; the first of an id, at most
    /// [`GENERATOR_PRESETS_MAX`]) and resets an unknown `generator_default`
    /// to [`generator::PRESET_STANDARD`].
    pub fn normalized(mut self) -> Settings {
        self.auto_lock_minutes = self.auto_lock_minutes.min(MAX_AUTO_LOCK_MINUTES);
        self.clipboard_clear_seconds = self
            .clipboard_clear_seconds
            .min(MAX_CLIPBOARD_CLEAR_SECONDS);
        if self
            .last_vault_id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
        {
            self.last_vault_id = None;
        }
        let mut presets: Vec<GeneratorPreset> = Vec::new();
        for preset in &self.generator_presets {
            if presets.len() == GENERATOR_PRESETS_MAX {
                break;
            }
            if let Ok(p) = preset.normalized() {
                if !presets.iter().any(|q| q.id == p.id) {
                    presets.push(p);
                }
            }
        }
        self.generator_presets = presets;
        let default = self.generator_default.trim();
        self.generator_default =
            if generator::preset_options(default, &self.generator_presets).is_some() {
                default.to_owned()
            } else {
                generator::PRESET_STANDARD.to_owned()
            };
        self
    }

    /// The options of the default generator preset
    /// ([`Settings::generator_default`]); the defaults if it is unknown.
    pub fn default_generator_options(&self) -> GeneratorOptions {
        generator::preset_options(&self.generator_default, &self.generator_presets)
            .unwrap_or_default()
    }

    /// Saves atomically to [`paths::settings_path`].
    pub fn save(&self) -> Result<()> {
        self.save_to(&paths::settings_path())
    }

    /// Saves atomically to an explicit path (parent directories are created).
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let mut json = serde_json::to_vec_pretty(&self.clone().normalized())?;
        json.push(b'\n');
        format::write_atomic(path, &json, false)
    }
}

/// One user preset of a `settings.json`, parsed leniently: `id` and `name`
/// must be strings, the options are taken field by field
/// ([`GeneratorOptions::from_json_lenient`]). Validated later by
/// [`Settings::normalized`].
fn preset_lenient(value: &serde_json::Value) -> Option<GeneratorPreset> {
    let obj = value.as_object()?;
    Some(GeneratorPreset {
        id: obj.get("id")?.as_str()?.to_owned(),
        name: obj.get("name")?.as_str()?.to_owned(),
        options: obj
            .get("options")
            .map(GeneratorOptions::from_json_lenient)
            .unwrap_or_default(),
    })
}

/// Same as [`Settings::load`].
pub fn load() -> Settings {
    Settings::load()
}

/// Same as [`Settings::save`].
pub fn save(settings: &Settings) -> Result<()> {
    settings.save()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_contract() {
        let s = Settings::default();
        assert_eq!(s.theme, Theme::System);
        assert_eq!(s.auto_lock_minutes, 15);
        assert!(s.lock_on_system_lock);
        assert_eq!(s.clipboard_clear_seconds, 30);
        assert!(s.minimize_to_tray);
        assert!(!s.start_in_tray);
        assert!(s.browser_integration);
        assert_eq!(s.last_vault_id, None);
        assert!(s.website_icons);
        assert!(s.update_check);
        assert_eq!(s.update_channel, UpdateChannel::Stable);
    }

    #[test]
    fn settings_of_older_versions_get_update_defaults() {
        // A settings.json written before the updater existed.
        let old = r#"{
          "theme": "dark", "language": "en", "autoLockMinutes": 5,
          "lockOnSystemLock": false, "clipboardClearSeconds": 10,
          "minimizeToTray": false, "startInTray": true,
          "browserIntegration": false, "lastVaultId": "v-1", "showIcons": true
        }"#;
        let s = Settings::from_json_lenient(old);
        assert_eq!(s.theme, Theme::Dark);
        assert_eq!(s.auto_lock_minutes, 5);
        assert_eq!(s.last_vault_id.as_deref(), Some("v-1"));
        assert!(s.website_icons);
        assert!(s.update_check);
        assert_eq!(s.update_channel, UpdateChannel::Stable);
        // The strict deserializer (Tauri `save_settings` argument) as well.
        let strict: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(strict, s);
    }

    #[test]
    fn website_icons_default_on_and_ignore_the_old_show_icons() {
        // Old files: `showIcons` (never in the UI, always false) is ignored.
        let s = Settings::from_json_lenient(r#"{"showIcons":false}"#);
        assert!(s.website_icons);
        let s = Settings::from_json_lenient(r#"{"showIcons":true,"websiteIcons":false}"#);
        assert!(!s.website_icons);
        let s = Settings::from_json_lenient(r#"{"websiteIcons":"yes"}"#);
        assert!(s.website_icons);
        let strict: Settings = serde_json::from_str(r#"{"showIcons":false}"#).unwrap();
        assert!(strict.website_icons);
    }

    #[test]
    fn update_fields_parse_and_fall_back() {
        let s = Settings::from_json_lenient(r#"{"updateCheck":false,"updateChannel":"stable"}"#);
        assert!(!s.update_check);
        assert_eq!(s.update_channel, UpdateChannel::Stable);
        let s = Settings::from_json_lenient(r#"{"updateCheck":"no","updateChannel":"nightly"}"#);
        assert!(s.update_check);
        assert_eq!(s.update_channel, UpdateChannel::Stable);
        assert!(serde_json::from_str::<Settings>(r#"{"updateChannel":"nightly"}"#).is_err());
    }

    fn preset(id: &str, name: &str) -> GeneratorPreset {
        GeneratorPreset {
            id: id.into(),
            name: name.into(),
            options: GeneratorOptions {
                length: 24,
                symbol_set: "-_".into(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn settings_without_generator_presets_get_defaults() {
        let s = Settings::from_json_lenient(r#"{"theme":"dark"}"#);
        assert!(s.generator_presets.is_empty());
        assert_eq!(s.generator_default, generator::PRESET_STANDARD);
        assert_eq!(s.default_generator_options(), GeneratorOptions::default());
        let strict: Settings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(strict, s);
    }

    #[test]
    fn generator_presets_round_trip_and_choose_the_default() {
        let s = Settings {
            generator_presets: vec![preset("p-1", "Bank"), preset("p_2", "PIN Karte")],
            generator_default: "p_2".into(),
            ..Default::default()
        }
        .normalized();
        assert_eq!(s.generator_presets.len(), 2);
        assert_eq!(s.generator_default, "p_2");
        assert_eq!(s.default_generator_options().length, 24);
        let json = serde_json::to_string(&s).unwrap();
        let back = Settings::from_json_lenient(&json);
        assert_eq!(back, s);
        let strict: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(strict, s);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["generatorPresets"][0]["id"], "p-1");
        assert_eq!(v["generatorPresets"][0]["name"], "Bank");
        assert_eq!(v["generatorPresets"][0]["options"]["length"], 24);
        assert_eq!(v["generatorPresets"][0]["options"]["symbolSet"], "-_");
        // A built-in default.
        let s = Settings {
            generator_default: generator::PRESET_PIN.into(),
            ..Default::default()
        }
        .normalized();
        assert_eq!(s.generator_default, "pin");
        assert_eq!(s.default_generator_options().length, 6);
        // An unknown default (e.g. a deleted preset) falls back.
        let s = Settings::from_json_lenient(r#"{"generatorDefault":"gone"}"#);
        assert_eq!(s.generator_default, "standard");
        let s = Settings::from_json_lenient(r#"{"generatorDefault":7}"#);
        assert_eq!(s.generator_default, "standard");
    }

    #[test]
    fn invalid_generator_presets_are_dropped() {
        let json = serde_json::json!({
            "generatorPresets": [
                {"id": "ok-1", "name": "  Gut  ", "options": {"length": 12, "symbolSet": "--++"}},
                {"id": "ok-1", "name": "Doppelt"},
                {"id": "standard", "name": "Eingebaut"},
                {"id": "", "name": "Ohne Id"},
                {"id": "no-name", "name": "   "},
                {"id": "long-name", "name": "x".repeat(41)},
                {"id": "bad-options", "name": "Kaputt", "options": {"length": 2}},
                {"id": "lenient", "name": "Neuer", "options": {"length": 30, "wordlist": "fr", "future": 1}},
                {"name": "Keine Id"},
                "kein Objekt",
                {"id": 5, "name": "Zahl"}
            ],
            "generatorDefault": "lenient"
        })
        .to_string();
        let s = Settings::from_json_lenient(&json);
        let ids: Vec<&str> = s.generator_presets.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["ok-1", "lenient"]);
        assert_eq!(s.generator_presets[0].name, "Gut");
        assert_eq!(s.generator_presets[0].options.length, 12);
        assert_eq!(s.generator_presets[0].options.symbol_set, "-+");
        // A value of a later version costs only that field.
        assert_eq!(s.generator_presets[1].options.length, 30);
        assert_eq!(
            s.generator_presets[1].options.wordlist,
            generator::Wordlist::En
        );
        assert_eq!(s.generator_default, "lenient");
        // Not an array → none.
        let s = Settings::from_json_lenient(r#"{"generatorPresets":{"a":1}}"#);
        assert!(s.generator_presets.is_empty());
        // At most GENERATOR_PRESETS_MAX are kept.
        let many = Settings {
            generator_presets: (0..60).map(|i| preset(&format!("p{i}"), "x")).collect(),
            ..Default::default()
        }
        .normalized();
        assert_eq!(many.generator_presets.len(), GENERATOR_PRESETS_MAX);
        assert_eq!(many.generator_presets[49].id, "p49");
    }

    #[test]
    fn language_from_locale() {
        assert_eq!(Language::from_locale("de-DE"), Language::De);
        assert_eq!(Language::from_locale("de_AT.UTF-8"), Language::De);
        assert_eq!(Language::from_locale("DE"), Language::De);
        assert_eq!(Language::from_locale("en-US"), Language::En);
        assert_eq!(Language::from_locale("fr-FR"), Language::En);
        assert_eq!(Language::from_locale(""), Language::En);
    }

    #[test]
    fn json_shape() {
        let s = Settings {
            language: Language::De,
            last_vault_id: Some("abc".into()),
            ..Default::default()
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["theme"], "system");
        assert_eq!(v["language"], "de");
        assert_eq!(v["autoLockMinutes"], 15);
        assert_eq!(v["clipboardClearSeconds"], 30);
        assert_eq!(v["lastVaultId"], "abc");
        assert_eq!(v["websiteIcons"], true);
        assert!(v.get("showIcons").is_none());
        assert_eq!(v["updateCheck"], true);
        assert_eq!(v["updateChannel"], "stable");
        assert_eq!(v["generatorPresets"], serde_json::json!([]));
        assert_eq!(v["generatorDefault"], "standard");
        assert_eq!(v.as_object().unwrap().len(), 14);
    }

    #[test]
    fn lenient_parsing() {
        let s = Settings::from_json_lenient(
            "\u{feff}{\"theme\":\"dark\",\"language\":\"klingon\",\"autoLockMinutes\":-5,\"clipboardClearSeconds\":999999,\"websiteIcons\":false,\"extra\":1,\"lastVaultId\":\"\"}",
        );
        assert_eq!(s.theme, Theme::Dark);
        assert_eq!(s.language, Language::system_default());
        assert_eq!(s.auto_lock_minutes, 15);
        assert_eq!(s.clipboard_clear_seconds, MAX_CLIPBOARD_CLEAR_SECONDS);
        assert!(!s.website_icons);
        assert_eq!(s.last_vault_id, None);
        assert_eq!(Settings::from_json_lenient("garbage"), Settings::default());
        assert_eq!(Settings::from_json_lenient("[1,2]"), Settings::default());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        assert_eq!(Settings::load_from(&path), Settings::default());
        let s = Settings {
            theme: Theme::Light,
            language: Language::En,
            auto_lock_minutes: 0,
            lock_on_system_lock: false,
            clipboard_clear_seconds: 0,
            minimize_to_tray: false,
            start_in_tray: true,
            browser_integration: false,
            last_vault_id: Some("id-1".into()),
            website_icons: false,
            update_check: false,
            update_channel: UpdateChannel::Stable,
            generator_presets: vec![preset("p-1", "Bank")],
            generator_default: "p-1".into(),
        };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        std::fs::write(&path, b"{corrupt").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
    }
}
