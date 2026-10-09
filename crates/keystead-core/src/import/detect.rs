//! Import format detection by file content (drag & drop).
//!
//! The JSON formats are told apart by a shallow scan of the top-level keys
//! that skips every value it does not need, so detection neither builds a
//! JSON tree of the file nor keeps any of its secrets.

use std::fmt;
use std::path::Path;

use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{looks_like_csv, parse_legacy_meta, read_file_limited, ImportFormat};
use crate::error::{Error, Result};
use crate::format::{VaultFile, FORMAT_NAME};
use crate::util;

/// What [`detect_import`] found (returned to the UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedImport {
    pub format: ImportFormat,
    /// The file is encrypted: ask for its password before [`super::read_import`].
    pub needs_password: bool,
    /// File name without directory (for display).
    pub file_name: String,
}

/// Detects the format of an import file by its content; the extension only
/// decides which reading is tried first when the content could be either.
///
/// * `{"format":"keystead",…}` → `keystead` (needs password; the header is
///   validated, e.g. `unsupported:vault format version 2`);
/// * VaultX 1.x vault (`Salt`, `Iterations`, `IV`/`Data`, optional `Mac`,
///   keys case-insensitive) → `legacy` (needs password; header validated);
/// * Bitwarden JSON with `"encrypted": true` →
///   `unsupported:bitwarden_encrypted`; with an `items` array and
///   `"encrypted": false` (or `folders`/`collections`) → `bitwarden_json`;
/// * a CSV header row of a supported dialect (',' ';' or tab separated,
///   Excel `sep=` line, UTF-8 with or without BOM, UTF-16 with BOM,
///   Windows-1252) → `csv`;
/// * anything else → `unsupported:unknown_format`.
///
/// Files larger than [`super::IMPORT_MAX_BYTES`] →
/// `unsupported:file_too_large` (not read); a missing file → `not_found`; a
/// directory, another non-regular file or an unreadable file → `io:…`.
pub fn detect_import(path: &Path) -> Result<DetectedImport> {
    let bytes = read_file_limited(path)?;
    let format = detect_bytes(&bytes, path)?;
    Ok(DetectedImport {
        format,
        needs_password: format.needs_password(),
        file_name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    })
}

fn detect_bytes(bytes: &[u8], path: &Path) -> Result<ImportFormat> {
    let text = Zeroizing::new(util::decode_text(bytes));
    let body = util::strip_bom(&text).trim_start();
    let maybe_json = body.starts_with(['{', '[']);
    let csv_extension = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["csv", "tsv", "txt"]
            .iter()
            .any(|x| e.eq_ignore_ascii_case(x))
    });
    // Tie-breaker: a ".csv" starting with "{" is read as CSV first.
    if maybe_json && !csv_extension {
        if let Some(found) = detect_json(body, bytes) {
            return found;
        }
    }
    if !body.is_empty() && !body.contains('\0') && looks_like_csv(body) {
        return Ok(ImportFormat::Csv);
    }
    if maybe_json && csv_extension {
        if let Some(found) = detect_json(body, bytes) {
            return found;
        }
    }
    Err(Error::Unsupported("unknown_format".into()))
}

/// `None` if `text` is not a JSON object of a known kind.
fn detect_json(text: &str, raw: &[u8]) -> Option<Result<ImportFormat>> {
    let probe: JsonProbe = serde_json::from_str(text).ok()?;
    if probe.format.as_deref() == Some(FORMAT_NAME) {
        // Same parser as the import itself (raw bytes, UTF-8 with or
        // without BOM): refuse what it would refuse before asking for a
        // password.
        return Some(VaultFile::parse(raw).map(|_| ImportFormat::Keystead));
    }
    if probe.encrypted == Some(true) {
        return Some(Err(Error::Unsupported("bitwarden_encrypted".into())));
    }
    if probe.items_array && (probe.encrypted == Some(false) || probe.folders_array) {
        return Some(Ok(ImportFormat::BitwardenJson));
    }
    if probe.legacy_salt && probe.legacy_iterations && probe.legacy_payload {
        return Some(parse_legacy_meta(raw).map(|_| ImportFormat::Legacy));
    }
    None
}

/// The top-level keys of a JSON object that identify an import format.
#[derive(Debug, Default)]
struct JsonProbe {
    /// `format` (only when it is a short string).
    format: Option<String>,
    /// Bitwarden `encrypted`.
    encrypted: Option<bool>,
    /// Bitwarden `items` is an array.
    items_array: bool,
    /// Bitwarden `folders` or `collections` is an array.
    folders_array: bool,
    /// VaultX 1.x keys (case-insensitive like PowerShell).
    legacy_salt: bool,
    legacy_iterations: bool,
    /// `IV` or `Data`.
    legacy_payload: bool,
}

impl<'de> Deserialize<'de> for JsonProbe {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct ProbeVisitor;

        impl<'de> Visitor<'de> for ProbeVisitor {
            type Value = JsonProbe;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<JsonProbe, A::Error> {
                let mut p = JsonProbe::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "format" => {
                            if let Shallow::Str(s) = map.next_value()? {
                                p.format = Some(s);
                            }
                        }
                        "encrypted" => {
                            if let Shallow::Bool(b) = map.next_value()? {
                                p.encrypted = Some(b);
                            }
                        }
                        "items" => p.items_array = matches!(map.next_value()?, Shallow::Array),
                        "folders" | "collections" => {
                            p.folders_array |= matches!(map.next_value()?, Shallow::Array);
                        }
                        k => {
                            if k.eq_ignore_ascii_case("Salt") {
                                p.legacy_salt = true;
                            } else if k.eq_ignore_ascii_case("Iterations") {
                                p.legacy_iterations = true;
                            } else if k.eq_ignore_ascii_case("IV") || k.eq_ignore_ascii_case("Data")
                            {
                                p.legacy_payload = true;
                            }
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(p)
            }
        }

        d.deserialize_map(ProbeVisitor)
    }
}

/// The kind of a JSON value; strings are kept only when short (the
/// `format` marker), arrays and objects are skipped without storing them.
enum Shallow {
    Bool(bool),
    Str(String),
    Array,
    Other,
}

const SHALLOW_STR_MAX: usize = 64;

impl<'de> Deserialize<'de> for Shallow {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct ShallowVisitor;

        impl<'de> Visitor<'de> for ShallowVisitor {
            type Value = Shallow;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any JSON value")
            }

            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Shallow, E> {
                Ok(Shallow::Bool(v))
            }

            fn visit_i64<E: de::Error>(self, _: i64) -> std::result::Result<Shallow, E> {
                Ok(Shallow::Other)
            }

            fn visit_u64<E: de::Error>(self, _: u64) -> std::result::Result<Shallow, E> {
                Ok(Shallow::Other)
            }

            fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<Shallow, E> {
                Ok(Shallow::Other)
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Shallow, E> {
                Ok(if v.len() <= SHALLOW_STR_MAX {
                    Shallow::Str(v.to_owned())
                } else {
                    Shallow::Other
                })
            }

            fn visit_unit<E: de::Error>(self) -> std::result::Result<Shallow, E> {
                Ok(Shallow::Other)
            }

            fn visit_none<E: de::Error>(self) -> std::result::Result<Shallow, E> {
                Ok(Shallow::Other)
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Shallow, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Shallow::Array)
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Shallow, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Shallow::Other)
            }
        }

        d.deserialize_any(ShallowVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(text: &str) -> Option<JsonProbe> {
        serde_json::from_str(text).ok()
    }

    #[test]
    fn probe_reads_only_top_level_markers() {
        let p = probe(
            r#"{"encrypted":false,"folders":[{"id":"x"}],"items":[{"login":{"password":"pw"}}],
                "nested":{"format":"keystead"}}"#,
        )
        .unwrap();
        assert_eq!(p.encrypted, Some(false));
        assert!(p.items_array && p.folders_array);
        assert_eq!(p.format, None, "nested keys are ignored");

        let p = probe(r#"{"salt":"a","ITERATIONS":1,"Data":"","Mac":null}"#).unwrap();
        assert!(p.legacy_salt && p.legacy_iterations && p.legacy_payload);

        let p = probe(r#"{"format":5,"items":{"a":1},"encrypted":"yes"}"#).unwrap();
        assert_eq!(p.format, None);
        assert_eq!(p.encrypted, None);
        assert!(!p.items_array);

        let long = format!(r#"{{"format":"{}"}}"#, "k".repeat(100));
        assert_eq!(probe(&long).unwrap().format, None);

        assert!(probe("[1,2]").is_none());
        assert!(probe("\"x\"").is_none());
        assert!(probe("{\"a\":").is_none());
    }

    #[test]
    fn csv_extension_breaks_ties() {
        // Valid CSV header that starts with "{": JSON fails, CSV wins either way.
        let text = b"{x},name,password\n1,a,b\n";
        assert_eq!(
            detect_bytes(text, Path::new("a.csv")).unwrap(),
            ImportFormat::Csv
        );
        assert_eq!(
            detect_bytes(text, Path::new("a.json")).unwrap(),
            ImportFormat::Csv
        );
        let json = br#"{"encrypted":false,"items":[]}"#;
        assert_eq!(
            detect_bytes(json, Path::new("export.csv")).unwrap(),
            ImportFormat::BitwardenJson
        );
        assert!(matches!(
            detect_bytes(b"", Path::new("a.csv")),
            Err(Error::Unsupported(d)) if d == "unknown_format"
        ));
        assert!(matches!(
            detect_bytes(b"name\0,password\n", Path::new("a.csv")),
            Err(Error::Unsupported(d)) if d == "unknown_format"
        ));
    }
}
