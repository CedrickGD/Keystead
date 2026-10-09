//! Password strength (zxcvbn) and the vault security report.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model::{now_ms, ItemType, VaultData, VaultItem};

/// zxcvbn result in a UI friendly shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Strength {
    /// 0 (very weak) ..= 4 (very strong).
    pub score: u8,
    /// Estimated offline crack time against a slow hash (English text,
    /// e.g. "3 hours", "centuries").
    pub crack_time: String,
    /// Main weakness (English, may be empty).
    pub warning: String,
    /// Improvement hints (English).
    pub suggestions: Vec<String>,
}

/// Rates a password. `user_inputs` (username, item name, …) count as
/// guessable words.
pub fn strength(password: &str, user_inputs: &[&str]) -> Strength {
    let entropy = zxcvbn::zxcvbn(password, user_inputs);
    let feedback = entropy.feedback();
    Strength {
        score: u8::from(entropy.score()),
        crack_time: entropy
            .crack_times()
            .offline_slow_hashing_1e4_per_second()
            .to_string(),
        warning: feedback
            .and_then(|f| f.warning())
            .map(|w| w.to_string())
            .unwrap_or_default(),
        suggestions: feedback
            .map(|f| f.suggestions().iter().map(ToString::to_string).collect())
            .unwrap_or_default(),
    }
}

/// Security overview of a vault (trash excluded).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    /// Logins in the vault (not trashed).
    pub total_logins: usize,
    /// Logins whose password scores below 3.
    pub weak: Vec<String>,
    /// Groups (≥ 2) of logins sharing the same password.
    pub reused: Vec<Vec<String>>,
    /// Logins whose password was not changed for more than 365 days.
    pub old: Vec<String>,
    /// Logins with a password but without TOTP.
    pub missing_totp_count: usize,
    /// 0..=100, see [`health_report_at`] for the formula.
    pub score: u8,
}

/// Passwords older than this count as "old".
pub const OLD_AFTER_DAYS: i64 = 365;

/// [`health_report_at`] for the current time.
pub fn health_report(data: &VaultData) -> HealthReport {
    health_report_at(data, now_ms())
}

/// Builds the report as of `now` (Unix ms).
///
/// Only non-trashed logins with a non-empty password are rated. With
/// `n` = number of such logins:
///
/// ```text
/// score = 100 − 50·(weak / n) − 35·(reused / n) − 15·(old / n)
/// ```
///
/// where `reused` counts every login that belongs to a reuse group. The
/// result is rounded and clamped to 0..=100; a vault without passwords
/// scores 100. Missing TOTP is informational and not penalised (many sites
/// do not offer it).
pub fn health_report_at(data: &VaultData, now: i64) -> HealthReport {
    let logins: Vec<&VaultItem> = data
        .items
        .iter()
        .filter(|i| !i.is_trashed() && i.item_type == ItemType::Login)
        .collect();
    let rated: Vec<&VaultItem> = logins
        .iter()
        .copied()
        .filter(|i| !i.password().is_empty())
        .collect();

    let mut weak = Vec::new();
    let mut old = Vec::new();
    let mut missing_totp_count = 0;
    let mut by_password: HashMap<&str, Vec<&VaultItem>> = HashMap::new();
    let old_before = now - OLD_AFTER_DAYS * 24 * 60 * 60 * 1000;

    for item in &rated {
        let Some(login) = item.login.as_ref() else {
            continue;
        };
        let inputs: Vec<&str> = [login.username.as_str(), item.name.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        if strength(&login.password, &inputs).score < 3 {
            weak.push(item.id.clone());
        }
        let changed = login.password_revised_at.unwrap_or(item.created_at);
        if changed < old_before {
            old.push(item.id.clone());
        }
        if login.totp.trim().is_empty() {
            missing_totp_count += 1;
        }
        by_password
            .entry(login.password.as_str())
            .or_default()
            .push(item);
    }

    let mut reused: Vec<Vec<&VaultItem>> =
        by_password.into_values().filter(|g| g.len() >= 2).collect();
    for g in &mut reused {
        g.sort_by_cached_key(|i| (i.name.to_lowercase(), i.id.clone()));
    }
    // Biggest groups first, then alphabetical by first member.
    reused.sort_by_cached_key(|g| {
        (
            std::cmp::Reverse(g.len()),
            g.first()
                .map(|i| (i.name.to_lowercase(), i.id.clone()))
                .unwrap_or_default(),
        )
    });
    let reused: Vec<Vec<String>> = reused
        .into_iter()
        .map(|g| g.into_iter().map(|i| i.id.clone()).collect())
        .collect();

    let score = if rated.is_empty() {
        100
    } else {
        let n = rated.len() as f64;
        let reused_count: usize = reused.iter().map(Vec::len).sum();
        let s = 100.0
            - 50.0 * weak.len() as f64 / n
            - 35.0 * reused_count as f64 / n
            - 15.0 * old.len() as f64 / n;
        s.round().clamp(0.0, 100.0) as u8
    };

    HealthReport {
        total_logins: logins.len(),
        weak,
        reused,
        old,
        missing_totp_count,
        score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LoginData, VaultItem};

    const DAY: i64 = 24 * 60 * 60 * 1000;
    const NOW: i64 = 1_800_000_000_000;

    fn login(id: &str, name: &str, password: &str, created: i64) -> VaultItem {
        VaultItem {
            id: id.into(),
            item_type: ItemType::Login,
            name: name.into(),
            login: Some(LoginData {
                username: "user".into(),
                password: password.into(),
                ..Default::default()
            }),
            created_at: created,
            ..Default::default()
        }
    }

    #[test]
    fn strength_scores() {
        let weak = strength("password", &[]);
        assert_eq!(weak.score, 0);
        assert!(!weak.warning.is_empty());
        assert!(!weak.crack_time.is_empty());
        let strong = strength("correct-Horse-battery-staple-91!x", &[]);
        assert_eq!(strong.score, 4);
        assert!(strong.warning.is_empty());
        assert_eq!(strength("", &[]).score, 0);
        // User inputs lower the score.
        let with = strength("maximilianmusterm", &["maximilianmusterm"]);
        let without = strength("maximilianmusterm", &[]);
        assert!(with.score <= without.score);
        let v = serde_json::to_value(&weak).unwrap();
        assert!(v.get("crackTime").is_some());
    }

    #[test]
    fn empty_vault_scores_100() {
        let r = health_report_at(&VaultData::default(), NOW);
        assert_eq!(r.score, 100);
        assert_eq!(r.total_logins, 0);
    }

    #[test]
    fn report_classifies_items() {
        let strong_a = "Vq8#mZ2!rT6x@Lp4wN9e";
        let mut data = VaultData::default();
        data.items.push(login("1", "Alpha", "123456", NOW - DAY));
        data.items.push(login("2", "Beta", strong_a, NOW - DAY));
        data.items.push(login("3", "Gamma", strong_a, NOW - DAY));
        data.items
            .push(login("4", "Delta", "Hk3$uP9!zQ7#mW2x", NOW - 400 * DAY));
        let mut revised = login("5", "Eps", "Zr5%nB8@kD1!qF6v", NOW - 800 * DAY);
        if let Some(l) = revised.login.as_mut() {
            l.password_revised_at = Some(NOW - 10 * DAY);
            l.totp = "JBSWY3DPEHPK3PXP".into();
        }
        data.items.push(revised);
        // Trashed and non-login items are ignored; empty passwords not rated.
        let mut trashed = login("6", "Trash", "123456", NOW - 900 * DAY);
        trashed.deleted_at = Some(NOW);
        data.items.push(trashed);
        data.items.push(login("7", "NoPw", "", NOW - 900 * DAY));
        data.items.push(VaultItem::new(ItemType::Note, "note"));

        let r = health_report_at(&data, NOW);
        assert_eq!(r.total_logins, 6);
        assert_eq!(r.weak, vec!["1".to_string()]);
        assert_eq!(r.reused, vec![vec!["2".to_string(), "3".to_string()]]);
        assert_eq!(r.old, vec!["4".to_string()]);
        assert_eq!(r.missing_totp_count, 4);
        // n = 5: 100 − 50·1/5 − 35·2/5 − 15·1/5 = 73
        assert_eq!(r.score, 73);
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("missingTotpCount").is_some());
        assert!(v.get("totalLogins").is_some());
    }

    #[test]
    fn worst_case_is_zero() {
        let mut data = VaultData::default();
        for i in 0..3 {
            data.items
                .push(login(&i.to_string(), "x", "password", NOW - 1000 * DAY));
        }
        let r = health_report_at(&data, NOW);
        assert_eq!(r.score, 0);
        assert_eq!(r.reused.len(), 1);
        assert_eq!(r.reused[0].len(), 3);
    }
}
