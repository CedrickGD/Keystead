//! Website icons stored inside the vault ([`VaultData::icons`]).
//!
//! Pure helpers only – no networking. The desktop app fetches the icons
//! (`apps/desktop/src-tauri/src/icons.rs`) and stores them through
//! [`crate::UnlockedVault::set_icons`].
//!
//! * The key of an icon is the **host** of a login: the lower-case host of
//!   its first http(s) URI (scheme-less URIs count as `https://`), without a
//!   trailing dot and a leading `www.`, port ignored – the same "site" the
//!   import uses for duplicates ([`site_host`]). The UI computes the same
//!   host in TypeScript (`apps/desktop/src/lib/icons.ts`).
//! * Only hosts of non-trashed logins keep an icon ([`prune_icons`]).
//! * Hosts that must never be contacted (IP literals, `localhost`, `.local`,
//!   single-label intranet names, …) are never fetched
//!   ([`is_fetchable_host`]).

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

use zeroize::Zeroize;

use crate::matching;
use crate::model::{IconEntry, ItemType, VaultData, VaultItem};

/// Width and height of a stored icon (pixels).
pub const ICON_SIZE: u32 = 64;
/// A stored icon is fetched again after 30 days.
pub const ICON_REFRESH_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// A failed fetch is retried after 7 days.
pub const ICON_RETRY_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Largest PNG accepted by [`crate::UnlockedVault::set_icons`] (bytes).
pub const ICON_MAX_PNG_BYTES: usize = 48 * 1024;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Host suffixes of names that only exist in local networks.
const LOCAL_SUFFIXES: &[&str] = &[
    ".localhost",
    ".local",
    ".localdomain",
    ".internal",
    ".intranet",
    ".lan",
    ".home",
    ".home.arpa",
    ".corp",
    ".private",
    ".test",
    ".invalid",
    ".example",
    ".onion",
];

/// The site of one stored URI: lower-case host of an http(s) URI
/// (scheme-less = `https://`), without a trailing dot and a leading `www.`.
/// `None` for other schemes and unparsable input.
pub fn site_host(uri: &str) -> Option<String> {
    let url = matching::parse_stored(uri.trim()).filter(matching::is_web)?;
    let host = url.host_str()?.trim_end_matches('.').to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    (!host.is_empty()).then(|| host.to_owned())
}

/// The icon host of an item: [`site_host`] of the first login URI that has
/// one. `None` for other item types and logins without a web address.
pub fn icon_host(item: &VaultItem) -> Option<String> {
    if item.item_type != ItemType::Login {
        return None;
    }
    item.login
        .as_ref()?
        .uris
        .iter()
        .find_map(|u| site_host(&u.uri))
}

/// True if the app may contact `host` for its icon: a DNS name with at
/// least two labels and only `[a-z0-9-]` characters (IDNs arrive as
/// punycode), not an IP literal, not `localhost`, and not a name that only
/// exists in local networks (`.local`, `.lan`, `.internal`, `.home.arpa`, …).
/// The fetcher additionally refuses connections to non-public addresses.
pub fn is_fetchable_host(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    let unbracketed = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if unbracketed.parse::<IpAddr>().is_ok() {
        return false;
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || LOCAL_SUFFIXES.iter().any(|s| lower.ends_with(s)) {
        return false;
    }
    let labels: Vec<&str> = lower.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let valid_label = |l: &&str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if !labels.iter().all(valid_label) {
        return false;
    }
    // All-numeric last label: an IPv4 address in another notation.
    let tld = labels[labels.len() - 1];
    !tld.bytes().all(|b| b.is_ascii_digit())
}

/// `data:image/png;base64,<png>` for a stored icon.
pub fn data_url(png_base64: &str) -> String {
    format!("data:image/png;base64,{png_base64}")
}

/// True if `bytes` look like a PNG file (signature).
pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(PNG_MAGIC)
}

/// The icon hosts of all non-trashed logins.
pub fn login_hosts(data: &VaultData) -> BTreeSet<String> {
    data.items
        .iter()
        .filter(|i| !i.is_trashed())
        .filter_map(icon_host)
        .collect()
}

/// Whether the icon of a host should be fetched (again) at `now`: none
/// stored yet, the last attempt is 30 days old, or the last failure is 7
/// days old.
pub fn needs_fetch(entry: Option<&IconEntry>, now: i64) -> bool {
    let Some(entry) = entry else {
        return true;
    };
    if let Some(failed) = entry.failed_at {
        return now.saturating_sub(failed) >= ICON_RETRY_MS;
    }
    entry.png.is_none() || now.saturating_sub(entry.fetched_at) >= ICON_REFRESH_MS
}

/// Hosts of non-trashed logins whose icon should be fetched at `now`
/// ([`needs_fetch`], only [`is_fetchable_host`] hosts): hosts without any
/// icon first, then due refreshes, each in alphabetical order.
pub fn hosts_needing_fetch(data: &VaultData, now: i64) -> Vec<String> {
    let mut missing = Vec::new();
    let mut due = Vec::new();
    for host in login_hosts(data) {
        if !is_fetchable_host(&host) {
            continue;
        }
        match data.icons.get(&host) {
            None => missing.push(host),
            Some(entry) if needs_fetch(Some(entry), now) => {
                if entry.png.is_some() {
                    due.push(host);
                } else {
                    missing.push(host);
                }
            }
            Some(_) => {}
        }
    }
    missing.extend(due);
    missing
}

/// Removes icons whose host belongs to no non-trashed login. Returns how
/// many were removed.
pub fn prune_icons(data: &mut VaultData) -> usize {
    if data.icons.is_empty() {
        return 0;
    }
    let hosts = login_hosts(data);
    let before = data.icons.len();
    let mut removed: Vec<String> = data
        .icons
        .keys()
        .filter(|h| !hosts.contains(*h))
        .cloned()
        .collect();
    for host in &removed {
        data.icons.remove(host);
    }
    wipe_hosts(&mut removed);
    before - data.icons.len()
}

/// Stores fetch results (`None` = the fetch failed) for hosts that belong
/// to a non-trashed login; others are ignored. A PNG that is not one or is
/// larger than [`ICON_MAX_PNG_BYTES`] counts as a failure. Returns how many
/// entries were written.
pub(crate) fn apply_icons(
    data: &mut VaultData,
    results: &[(String, Option<Vec<u8>>)],
    now: i64,
) -> usize {
    let hosts = login_hosts(data);
    let mut written = 0;
    for (host, png) in results {
        if !hosts.contains(host) {
            continue;
        }
        let png = png
            .as_deref()
            .filter(|p| is_png(p) && p.len() <= ICON_MAX_PNG_BYTES);
        let entry = data.icons.entry(host.clone()).or_default();
        entry.fetched_at = now;
        match png {
            Some(bytes) => {
                entry.png = Some(crate::crypto::b64_encode(bytes));
                entry.failed_at = None;
            }
            None => entry.failed_at = Some(now),
        }
        written += 1;
    }
    written
}

/// Overwrites the host names of the icon map before it is freed (the hosts
/// reveal the user's accounts; the images themselves are public).
pub(crate) fn wipe_icons(icons: &mut BTreeMap<String, IconEntry>) {
    let mut hosts: Vec<String> = std::mem::take(icons).into_keys().collect();
    wipe_hosts(&mut hosts);
}

fn wipe_hosts(hosts: &mut [String]) {
    for h in hosts {
        h.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LoginData, LoginUri, UriMatch};

    fn login(name: &str, uris: &[&str]) -> VaultItem {
        VaultItem {
            id: format!("id-{name}"),
            item_type: ItemType::Login,
            name: name.into(),
            login: Some(LoginData {
                uris: uris
                    .iter()
                    .map(|u| LoginUri {
                        uri: (*u).into(),
                        match_type: UriMatch::Domain,
                    })
                    .collect(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn png() -> Vec<u8> {
        let mut v = PNG_MAGIC.to_vec();
        v.extend_from_slice(b"rest");
        v
    }

    #[test]
    fn host_normalisation_matches_the_import_site() {
        let host = |u: &str| icon_host(&login("x", &[u]));
        assert_eq!(
            host("https://www.GitHub.com/login").as_deref(),
            Some("github.com")
        );
        assert_eq!(host("github.com").as_deref(), Some("github.com"));
        assert_eq!(host("http://Example.com.:8080/x").as_deref(), Some("example.com"));
        assert_eq!(host("https://www2.example.com").as_deref(), Some("www2.example.com"));
        assert_eq!(host("münchen.de").as_deref(), Some("xn--mnchen-3ya.de"));
        assert_eq!(host("192.168.0.1").as_deref(), Some("192.168.0.1"));
        assert_eq!(host("ftp://example.com"), None);
        assert_eq!(host("androidapp://com.example"), None);
        assert_eq!(host(""), None);
        // The first URI with a web host counts.
        let item = login("x", &["androidapp://a", "", "https://www.second.example/"]);
        assert_eq!(icon_host(&item).as_deref(), Some("second.example"));
        // Only logins.
        let mut note = login("n", &["https://example.com"]);
        note.item_type = ItemType::Note;
        assert_eq!(icon_host(&note), None);
    }

    #[test]
    fn fetchable_hosts() {
        for ok in [
            "github.com",
            "accounts.google.com",
            "xn--mnchen-3ya.de",
            "my-bank.co.uk",
            "a.b.c.d.example.org",
            "example.com.",
        ] {
            assert!(is_fetchable_host(ok), "{ok}");
        }
        for bad in [
            "",
            "localhost",
            "app.localhost",
            "nas",
            "router",
            "printer.local",
            "fritz.box.local",
            "server.lan",
            "x.home.arpa",
            "db.internal",
            "192.168.0.1",
            "10.0.0.1",
            "[::1]",
            "::1",
            "127.1",
            "0x7f.1",
            "under_score.example.com",
            "-bad.example.com",
            "a..b.com",
            "space here.com",
            "user@example.com",
            "example.com:443",
        ] {
            assert!(!is_fetchable_host(bad), "{bad}");
        }
    }

    #[test]
    fn needs_fetch_rules() {
        let now = 100 * ICON_REFRESH_MS;
        assert!(needs_fetch(None, now));
        let ok = IconEntry {
            png: Some("x".into()),
            fetched_at: now - ICON_REFRESH_MS + 1,
            failed_at: None,
        };
        assert!(!needs_fetch(Some(&ok), now));
        let stale = IconEntry {
            fetched_at: now - ICON_REFRESH_MS,
            ..ok.clone()
        };
        assert!(needs_fetch(Some(&stale), now));
        let failed = IconEntry {
            png: None,
            fetched_at: now - ICON_RETRY_MS + 1,
            failed_at: Some(now - ICON_RETRY_MS + 1),
        };
        assert!(!needs_fetch(Some(&failed), now));
        let failed_long_ago = IconEntry {
            failed_at: Some(now - ICON_RETRY_MS),
            ..failed.clone()
        };
        assert!(needs_fetch(Some(&failed_long_ago), now));
        // A failed refresh keeps the old icon and waits 7 days, not 30.
        let refresh_failed = IconEntry {
            png: Some("old".into()),
            fetched_at: now - 1000,
            failed_at: Some(now - 1000),
        };
        assert!(!needs_fetch(Some(&refresh_failed), now));
        // A clock that went backwards does not cause a fetch storm.
        let future = IconEntry {
            fetched_at: now + 5000,
            ..ok
        };
        assert!(!needs_fetch(Some(&future), now));
    }

    #[test]
    fn hosts_needing_fetch_orders_missing_first_and_skips_local_hosts() {
        let now = 50 * ICON_REFRESH_MS;
        let mut trashed = login("Old", &["https://trashed.example.org"]);
        trashed.deleted_at = Some(1);
        let mut data = VaultData {
            items: vec![
                login("B", &["https://b.example.com"]),
                login("A", &["https://www.a.example.com"]),
                login("A2", &["a.example.com/other"]),
                login("Stale", &["https://stale.example.com"]),
                login("Fresh", &["https://fresh.example.com"]),
                login("Router", &["http://192.168.178.1"]),
                login("Nas", &["http://nas:5000"]),
                login("None", &[]),
                trashed,
            ],
            ..Default::default()
        };
        data.icons.insert(
            "stale.example.com".into(),
            IconEntry {
                png: Some("x".into()),
                fetched_at: now - ICON_REFRESH_MS - 1,
                failed_at: None,
            },
        );
        data.icons.insert(
            "fresh.example.com".into(),
            IconEntry {
                png: Some("x".into()),
                fetched_at: now - 1,
                failed_at: None,
            },
        );
        assert_eq!(
            hosts_needing_fetch(&data, now),
            vec!["a.example.com", "b.example.com", "stale.example.com"]
        );
    }

    #[test]
    fn apply_and_prune() {
        let now = 1_000;
        let mut data = VaultData {
            items: vec![
                login("A", &["https://a.example.com"]),
                login("B", &["https://b.example.com"]),
            ],
            ..Default::default()
        };
        let written = apply_icons(
            &mut data,
            &[
                ("a.example.com".into(), Some(png())),
                ("b.example.com".into(), None),
                ("not-a-login.example.com".into(), Some(png())),
            ],
            now,
        );
        assert_eq!(written, 2);
        let a = &data.icons["a.example.com"];
        assert_eq!(a.png.as_deref(), Some(crate::crypto::b64_encode(&png()).as_str()));
        assert_eq!((a.fetched_at, a.failed_at), (now, None));
        let b = &data.icons["b.example.com"];
        assert_eq!((b.png.as_deref(), b.failed_at), (None, Some(now)));
        assert!(!data.icons.contains_key("not-a-login.example.com"));

        // A failed refresh keeps the icon; a non-PNG or oversized blob is a
        // failure.
        apply_icons(&mut data, &[("a.example.com".into(), None)], now + 1);
        assert!(data.icons["a.example.com"].png.is_some());
        assert_eq!(data.icons["a.example.com"].failed_at, Some(now + 1));
        apply_icons(
            &mut data,
            &[("b.example.com".into(), Some(b"GIF89a".to_vec()))],
            now + 2,
        );
        assert_eq!(data.icons["b.example.com"].png, None);
        let mut huge = png();
        huge.resize(ICON_MAX_PNG_BYTES + 1, 0);
        apply_icons(&mut data, &[("b.example.com".into(), Some(huge))], now + 3);
        assert_eq!(data.icons["b.example.com"].failed_at, Some(now + 3));

        // Trashing B's login drops its icon.
        data.items[1].deleted_at = Some(5);
        assert_eq!(prune_icons(&mut data), 1);
        assert_eq!(
            data.icons.keys().collect::<Vec<_>>(),
            vec!["a.example.com"]
        );
        assert_eq!(prune_icons(&mut data), 0);
    }

    #[test]
    fn icon_entry_debug_has_no_image_data() {
        let entry = IconEntry {
            png: Some("iVBORw0KGgoAAAANSUhEUgAAAEAAAABA".into()),
            fetched_at: 1,
            failed_at: None,
        };
        let text = format!("{entry:?}");
        assert!(!text.contains("iVBOR"), "{text}");
        assert!(text.contains("png_len: Some(32)"), "{text}");
        let mut data = VaultData::default();
        data.icons.insert("example.com".into(), entry);
        assert!(!format!("{data:?}").contains("iVBOR"));
    }
}
