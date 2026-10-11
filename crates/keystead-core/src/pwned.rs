//! Leaked-password check against Have I Been Pwned's "Pwned Passwords"
//! range API (k-anonymity): only the first 5 hex characters of a password's
//! SHA-1 ever leave the device; the service answers with every known hash
//! suffix under that prefix and its breach count, and the match happens
//! here. This module does the hashing, the vault-side grouping and the
//! response parsing. No networking – the desktop app sends the request
//! (`https://api.pwnedpasswords.com/range/<PREFIX>`).

use std::collections::HashMap;

use data_encoding::HEXUPPER;
use sha1::{Digest, Sha1};
use subtle::{ConditionallySelectable, ConstantTimeEq};
use zeroize::{Zeroize, Zeroizing};

use crate::model::{ItemType, VaultData};

/// Hex characters of the prefix sent to the service.
pub const PREFIX_LEN: usize = 5;
/// Hex characters of the suffix kept on the device (SHA-1 is 40 hex digits).
pub const SUFFIX_LEN: usize = 40 - PREFIX_LEN;

/// A password's SHA-1 split the way the range API uses it. `Debug` prints
/// only the prefix – prefix and suffix together are the full hash, which
/// is brute-forceable for weak passwords.
#[derive(Clone, PartialEq, Eq)]
pub struct PwnedHash {
    /// First 5 hex characters (upper-case). This is what goes online.
    pub prefix: String,
    /// Remaining 35 hex characters (upper-case). Never leaves the device.
    pub suffix: Zeroizing<String>,
}

impl std::fmt::Debug for PwnedHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PwnedHash")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

/// Hashes a password for the range API.
pub fn pwned_hash(password: &str) -> PwnedHash {
    let mut digest = Sha1::digest(password.as_bytes());
    let hex = Zeroizing::new(HEXUPPER.encode(&digest));
    digest[..].zeroize();
    PwnedHash {
        prefix: hex[..PREFIX_LEN].to_owned(),
        suffix: Zeroizing::new(hex[PREFIX_LEN..].to_owned()),
    }
}

/// Number of breaches the password behind `hash` appeared in according to
/// a range API response `body` (lines `SUFFIX:COUNT`, CRLF or LF). 0 when
/// the suffix is not listed – or listed as a padding line with count 0.
/// Suffixes are compared case-insensitively and in constant time; every
/// line is visited so the position of a match does not show in the timing.
/// Malformed lines are ignored.
pub fn pwned_count(body: &str, hash: &PwnedHash) -> u64 {
    let wanted = hash.suffix.as_bytes();
    let mut found = 0u64;
    for line in body.lines() {
        let Some((suffix, count)) = line.trim().split_once(':') else {
            continue;
        };
        let suffix = suffix.trim();
        if suffix.len() != SUFFIX_LEN || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(count) = count.trim().parse::<u64>() else {
            continue;
        };
        let upper = suffix.to_ascii_uppercase();
        let matched = upper.as_bytes().ct_eq(wanted);
        found = u64::conditional_select(&found, &count, matched);
    }
    found
}

/// One distinct password of the vault and the logins using it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreachCandidate {
    pub hash: PwnedHash,
    /// Ids of the non-trashed logins with exactly this password, sorted.
    pub item_ids: Vec<String>,
}

/// The distinct passwords of all non-trashed logins (empty passwords
/// skipped), each with the items using it – one range request per entry.
/// Ordered by their first item id so the result is deterministic.
pub fn breach_candidates(data: &VaultData) -> Vec<BreachCandidate> {
    let mut by_password: HashMap<&str, Vec<String>> = HashMap::new();
    for item in &data.items {
        if item.is_trashed() || item.item_type != ItemType::Login {
            continue;
        }
        let password = item.password();
        if password.is_empty() {
            continue;
        }
        by_password
            .entry(password)
            .or_default()
            .push(item.id.clone());
    }
    let mut candidates: Vec<BreachCandidate> = by_password
        .into_iter()
        .map(|(password, mut item_ids)| {
            item_ids.sort();
            BreachCandidate {
                hash: pwned_hash(password),
                item_ids,
            }
        })
        .collect();
    // Groups are disjoint, so the sorted id lists order them strictly.
    candidates.sort_by(|a, b| a.item_ids.cmp(&b.item_ids));
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LoginData, VaultItem};

    /// SHA-1("password") = 5BAA61E4C9B93F3F0682250B6CF8331B7EE68FD8.
    const PREFIX: &str = "5BAA6";
    const SUFFIX: &str = "1E4C9B93F3F0682250B6CF8331B7EE68FD8";

    fn login(id: &str, password: &str) -> VaultItem {
        VaultItem {
            id: id.into(),
            item_type: ItemType::Login,
            name: format!("Item {id}"),
            login: Some(LoginData {
                username: "user".into(),
                password: password.into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn hash_matches_known_vector() {
        let hash = pwned_hash("password");
        assert_eq!(hash.prefix, PREFIX);
        assert_eq!(hash.suffix.as_str(), SUFFIX);
        assert_eq!(hash.prefix.len(), PREFIX_LEN);
        assert_eq!(hash.suffix.len(), SUFFIX_LEN);
        // Upper-case hex throughout, also for other inputs.
        let other = pwned_hash("Tr0ub4dor&3");
        assert!(other
            .prefix
            .chars()
            .chain(other.suffix.chars())
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
        assert_ne!(other, hash);
        assert_eq!(pwned_hash("password"), hash);
    }

    #[test]
    fn debug_has_no_suffix() {
        let hash = pwned_hash("password");
        let debug = format!("{hash:?}");
        assert!(debug.contains(PREFIX), "{debug}");
        assert!(!debug.contains(SUFFIX), "{debug}");
        assert!(!debug.contains(&SUFFIX[..10]), "{debug}");
        let candidate = BreachCandidate {
            hash,
            item_ids: vec!["a".into()],
        };
        let debug = format!("{candidate:?}");
        assert!(debug.contains(PREFIX) && debug.contains("\"a\""), "{debug}");
        assert!(!debug.contains(SUFFIX), "{debug}");
    }

    #[test]
    fn count_parses_crlf_and_lf_bodies() {
        let hash = pwned_hash("password");
        let crlf = format!(
            "0018A45C4D1DEF81644B54AB7F969B88D65:1\r\n{SUFFIX}:3861493\r\n00D4F6E8FA6EECAD2A3AA415EEC418D38EC:2\r\n"
        );
        assert_eq!(pwned_count(&crlf, &hash), 3_861_493);
        let lf = crlf.replace("\r\n", "\n");
        assert_eq!(pwned_count(&lf, &hash), 3_861_493);
        // Without a trailing line break and as the only line.
        assert_eq!(pwned_count(&format!("{SUFFIX}:7"), &hash), 7);
    }

    #[test]
    fn count_is_case_insensitive() {
        let hash = pwned_hash("password");
        let lower = format!(
            "0018a45c4d1def81644b54ab7f969b88d65:1\n{}:42\n",
            SUFFIX.to_ascii_lowercase()
        );
        assert_eq!(pwned_count(&lower, &hash), 42);
    }

    #[test]
    fn count_zero_when_missing_or_padded() {
        let hash = pwned_hash("password");
        assert_eq!(pwned_count("", &hash), 0);
        assert_eq!(
            pwned_count("0018A45C4D1DEF81644B54AB7F969B88D65:1\r\n", &hash),
            0
        );
        // Padding lines carry a count of 0.
        let padded = format!("0018A45C4D1DEF81644B54AB7F969B88D65:0\r\n{SUFFIX}:0\r\n");
        assert_eq!(pwned_count(&padded, &hash), 0);
        // A different password's suffix under the same body is not found.
        assert_eq!(
            pwned_count(&format!("{SUFFIX}:5\r\n"), &pwned_hash("Password")),
            0
        );
    }

    #[test]
    fn count_ignores_garbage_lines() {
        let hash = pwned_hash("password");
        let short = &SUFFIX[..SUFFIX_LEN - 1];
        let lines = [
            String::new(),
            "not a line".into(),
            SUFFIX.into(),
            format!("{SUFFIX}:"),
            format!("{SUFFIX}:abc"),
            format!("{SUFFIX}:-1"),
            format!("{short}:9"),
            format!("{SUFFIX}XX:9"),
            "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ:9".into(),
            "<html>oops</html>".into(),
            ":5".into(),
            format!("  {SUFFIX} : 12 "),
            "0018A45C4D1DEF81644B54AB7F969B88D65:1".into(),
        ];
        let body = lines.join("\r\n");
        // Only the well-formed (whitespace-trimmed) line counts.
        assert_eq!(pwned_count(&body, &hash), 12);
        assert_eq!(pwned_count("\u{fffd}\u{fffd}:3\n", &hash), 0);
    }

    #[test]
    fn candidates_group_dedupe_and_skip() {
        let mut data = VaultData::default();
        data.items.push(login("b", "password"));
        data.items.push(login("a", "password"));
        data.items.push(login("c", "Tr0ub4dor&3"));
        // Skipped: trashed, empty password, non-logins.
        let mut trashed = login("d", "password");
        trashed.deleted_at = Some(1);
        data.items.push(trashed);
        data.items.push(login("e", ""));
        data.items.push(VaultItem::new(ItemType::Note, "note"));
        let mut card = VaultItem::new(ItemType::Card, "card");
        card.id = "f".into();
        data.items.push(card);

        let candidates = breach_candidates(&data);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].item_ids, ["a", "b"]);
        assert_eq!(candidates[0].hash, pwned_hash("password"));
        assert_eq!(candidates[1].item_ids, ["c"]);
        assert_eq!(candidates[1].hash, pwned_hash("Tr0ub4dor&3"));
        assert!(breach_candidates(&VaultData::default()).is_empty());
    }
}
