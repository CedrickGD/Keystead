//! WebDAV `PROPFIND` answers (`207 Multi-Status`) – untrusted input from
//! the server, parsed defensively:
//!
//! * no DTD: a `<!DOCTYPE` is refused (no entity expansion of any kind;
//!   only the five predefined entities and character references are
//!   resolved),
//! * at most [`BODY_MAX`] bytes (the caller stops reading there too),
//!   [`RESPONSES_MAX`] `<response>` elements and [`DEPTH_MAX`] levels,
//! * an `href` must name the folder that was asked for or one entry right
//!   in it ([`child_name`]): same scheme, host and port; no `..`, `.`,
//!   empty segments, `/`, `\` or NUL in a name – also not percent-encoded.
//!
//! Anything else is `unsupported:sync_server_response`.

use std::borrow::Cow;

use percent_encoding::percent_decode_str;
use quick_xml::events::Event;
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::NsReader;
use reqwest::Url;

use super::BAD_RESPONSE;
use crate::error::{AppError, AppResult};

/// Largest answer read (bytes): 8 MiB.
pub const BODY_MAX: usize = 8 * 1024 * 1024;
/// Most `<response>` elements in one answer.
pub const RESPONSES_MAX: usize = 10_000;
/// Deepest element nesting.
pub const DEPTH_MAX: usize = 32;

const DAV: &str = "DAV:";
const OC: &str = "http://owncloud.org/ns";

/// The `PROPFIND` body: exactly the properties the sync uses.
pub const PROPFIND_BODY: &str = concat!(
    r#"<?xml version="1.0" encoding="utf-8"?>"#,
    r#"<d:propfind xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns"><d:prop>"#,
    "<d:resourcetype/><d:getcontentlength/><d:getetag/><d:getlastmodified/>",
    "<oc:permissions/></d:prop></d:propfind>"
);

fn bad() -> AppError {
    AppError::unsupported(BAD_RESPONSE)
}

/// One `<response>`: its `href` and the properties of its `200` propstat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DavEntry {
    /// As sent (entities resolved, still percent-encoded).
    pub href: String,
    pub collection: bool,
    pub size: Option<u64>,
    /// As sent (quotes included; weak ones too – see [`strong_etag`]).
    pub etag: Option<String>,
    /// `getlastmodified`, ms since the epoch.
    pub modified: Option<i64>,
    /// ownCloud/Nextcloud permissions (`oc:permissions`), e.g. `RGDNVCK`.
    pub permissions: Option<String>,
}

/// The ETag if it is a strong one (`If-Match` compares strongly; a weak
/// one would fail every write).
pub fn strong_etag(etag: &str) -> Option<String> {
    let etag = etag.trim();
    (etag.len() >= 2 && etag.starts_with('"') && etag.ends_with('"') && !etag.contains('\0'))
        .then(|| etag.to_owned())
}

/// What is being read right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Href,
    Status,
    Length,
    Etag,
    Modified,
    Permissions,
}

#[derive(Default)]
struct Propstat {
    status_ok: bool,
    collection: bool,
    size: Option<String>,
    etag: Option<String>,
    modified: Option<String>,
    permissions: Option<String>,
}

/// Parses a `207 Multi-Status` body.
pub fn parse_multistatus(body: &[u8]) -> AppResult<Vec<DavEntry>> {
    if body.len() > BODY_MAX {
        return Err(bad());
    }
    let mut reader = NsReader::from_reader(body);
    {
        let config = reader.config_mut();
        config.expand_empty_elements = true;
        config.check_end_names = true;
        config.trim_text(true);
    }
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut entries = Vec::new();
    let mut saw_multistatus = false;
    // The open DAV/oc elements, by local name ("" for any other).
    let mut stack: Vec<(bool, String)> = Vec::new();
    let mut entry: Option<(Option<String>, Vec<Propstat>)> = None;
    let mut propstat: Option<Propstat> = None;
    let mut field: Option<Field> = None;
    let mut text = String::new();

    loop {
        let (ns, event) = reader
            .read_resolved_event_into(&mut buf)
            .map_err(|_| bad())?;
        let ns_is = |want: &str| matches!(ns, ResolveResult::Bound(Namespace(n)) if n == want);
        match event {
            Event::DocType(_) => return Err(bad()),
            Event::Start(e) => {
                depth += 1;
                if depth > DEPTH_MAX {
                    return Err(bad());
                }
                let local = e.local_name().as_ref().to_owned();
                let dav = ns_is(DAV);
                let oc = ns_is(OC);
                let parent = stack.last().map(|(d, n)| (*d, n.as_str()));
                match (dav, oc, local.as_str(), parent) {
                    (true, _, "multistatus", None) => saw_multistatus = true,
                    (true, _, "response", Some((true, "multistatus"))) => {
                        if entries.len() >= RESPONSES_MAX {
                            return Err(bad());
                        }
                        entry = Some((None, Vec::new()));
                    }
                    (true, _, "href", Some((true, "response"))) => field = Some(Field::Href),
                    (true, _, "propstat", Some((true, "response"))) => {
                        propstat = Some(Propstat::default());
                    }
                    (true, _, "status", Some((true, "propstat"))) => field = Some(Field::Status),
                    (true, _, "collection", Some((true, "resourcetype"))) => {
                        if let Some(p) = propstat.as_mut() {
                            p.collection = true;
                        }
                    }
                    (true, _, "getcontentlength", Some((true, "prop"))) => {
                        field = Some(Field::Length);
                    }
                    (true, _, "getetag", Some((true, "prop"))) => field = Some(Field::Etag),
                    (true, _, "getlastmodified", Some((true, "prop"))) => {
                        field = Some(Field::Modified);
                    }
                    (_, true, "permissions", Some((true, "prop"))) => {
                        field = Some(Field::Permissions);
                    }
                    _ => {}
                }
                if !saw_multistatus {
                    return Err(bad());
                }
                text.clear();
                stack.push((dav, if dav || oc { local } else { String::new() }));
            }
            Event::Text(t) => {
                if field.is_some() {
                    push_text(&mut text, &t.xml10_content())?;
                }
            }
            Event::CData(t) => {
                if field.is_some() {
                    push_text(&mut text, &t)?;
                }
            }
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref().map_err(|_| bad())? {
                    Some(c) => c,
                    None => match &*r {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        // Anything a DTD would have to define.
                        _ => return Err(bad()),
                    },
                };
                if field.is_some() {
                    push_text(&mut text, resolved.encode_utf8(&mut [0; 4]))?;
                }
            }
            Event::End(_) => {
                let Some((dav, local)) = stack.pop() else {
                    return Err(bad());
                };
                depth -= 1;
                let value = std::mem::take(&mut text).trim().to_owned();
                if let Some(f) = field.take() {
                    match f {
                        Field::Href => {
                            if let Some((href, _)) = entry.as_mut() {
                                if href.is_some() {
                                    return Err(bad());
                                }
                                *href = Some(value);
                            }
                        }
                        Field::Status => {
                            if let Some(p) = propstat.as_mut() {
                                p.status_ok = value.split_whitespace().nth(1) == Some("200");
                            }
                        }
                        Field::Length => set(&mut propstat, |p| p.size = Some(value)),
                        Field::Etag => set(&mut propstat, |p| p.etag = Some(value)),
                        Field::Modified => set(&mut propstat, |p| p.modified = Some(value)),
                        Field::Permissions => {
                            set(&mut propstat, |p| p.permissions = Some(value));
                        }
                    }
                }
                if dav && local == "propstat" {
                    if let (Some(p), Some((_, list))) = (propstat.take(), entry.as_mut()) {
                        list.push(p);
                    }
                }
                if dav && local == "response" {
                    if let Some((href, list)) = entry.take() {
                        entries.push(entry_of(href.ok_or_else(bad)?, list)?);
                    }
                }
            }
            Event::Eof => break,
            Event::Empty(_) | Event::Comment(_) | Event::Decl(_) | Event::PI(_) => {}
        }
        buf.clear();
    }
    if !saw_multistatus || depth != 0 {
        return Err(bad());
    }
    Ok(entries)
}

fn set(propstat: &mut Option<Propstat>, f: impl FnOnce(&mut Propstat)) {
    if let Some(p) = propstat.as_mut() {
        f(p);
    }
}

/// A property's text, at most 8 KiB.
fn push_text(text: &mut String, more: &str) -> AppResult<()> {
    if text.len() + more.len() > 8 * 1024 {
        return Err(bad());
    }
    text.push_str(more);
    Ok(())
}

fn entry_of(href: String, propstats: Vec<Propstat>) -> AppResult<DavEntry> {
    let mut entry = DavEntry {
        href,
        ..DavEntry::default()
    };
    for p in propstats.into_iter().filter(|p| p.status_ok) {
        entry.collection |= p.collection;
        if let Some(size) = p.size {
            entry.size = Some(size.parse().map_err(|_| bad())?);
        }
        if let Some(etag) = p.etag.filter(|e| !e.is_empty()) {
            entry.etag = Some(etag);
        }
        if let Some(modified) = p.modified {
            entry.modified = http_date(&modified);
        }
        if let Some(permissions) = p.permissions {
            entry.permissions = Some(permissions);
        }
    }
    Ok(entry)
}

/// An HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`) → ms since the epoch.
pub fn http_date(text: &str) -> Option<i64> {
    let mut parts = text.split_whitespace();
    let _weekday = parts.next()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = parts.next()?.parse().ok()?;
    let mut hms = parts.next()?.split(':').map(|p| p.parse::<i64>().ok());
    let (h, m, s) = (hms.next()??, hms.next()??, hms.next()??);
    if parts.next()? != "GMT" || !(1..=31).contains(&day) || h > 23 || m > 59 || s > 60 {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(((days * 24 + h) * 60 + m) * 60_000 + s * 1000)
}

/// The decoded segments of a URL path (`/a/b%20c/` → `["a", "b c"]`): no
/// empty segment except a trailing slash, no `.`/`..`, no `/`, `\` or NUL –
/// also not percent-encoded.
fn segments(path: &str) -> AppResult<Vec<String>> {
    let rest = path.strip_prefix('/').ok_or_else(bad)?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    rest.split('/')
        .map(|raw| {
            let lower = raw.to_ascii_lowercase();
            if raw.is_empty() || ["%2f", "%5c", "%00"].iter().any(|e| lower.contains(e)) {
                return Err(bad());
            }
            let decoded: Cow<'_, str> = percent_decode_str(raw).decode_utf8().map_err(|_| bad())?;
            if decoded == "." || decoded == ".." || decoded.contains(['/', '\\', '\0']) {
                return Err(bad());
            }
            Ok(decoded.into_owned())
        })
        .collect()
}

/// The name of the entry `href` names in the folder `collection` (a URL
/// whose path ends with `/`): `None` for the folder itself, `Some(name)`
/// for an entry right in it. Any other href → `sync_server_response`.
pub fn child_name(href: &str, collection: &Url) -> AppResult<Option<String>> {
    if href.contains(['?', '#']) || href.chars().any(char::is_control) {
        return Err(bad());
    }
    let path = if href.starts_with('/') {
        href
    } else {
        // An absolute URL: the same origin, its path taken as sent (a URL
        // parser would resolve `..` away).
        let (scheme, rest) = href.split_once("://").ok_or_else(bad)?;
        let slash = rest.find('/').ok_or_else(bad)?;
        let origin = Url::parse(&format!("{scheme}://{}/", &rest[..slash])).map_err(|_| bad())?;
        if origin.scheme() != collection.scheme()
            || origin.host() != collection.host()
            || origin.port_or_known_default() != collection.port_or_known_default()
            || !origin.username().is_empty()
            || origin.password().is_some()
        {
            return Err(bad());
        }
        &rest[slash..]
    };
    let here = segments(collection.path())?;
    let there = segments(path)?;
    if there == here {
        return Ok(None);
    }
    if there.len() == here.len() + 1 && there[..here.len()] == here[..] {
        return Ok(there.last().cloned());
    }
    Err(bad())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multistatus(responses: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns" xmlns:s="http://sabredav.org/ns">{responses}</d:multistatus>"#
        )
    }

    const NEXTCLOUD: &str = r#"<d:response><d:href>/remote.php/dav/files/alice/Keystead/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype><d:getetag>&quot;6531a0&quot;</d:getetag><d:getlastmodified>Fri, 23 Oct 2026 14:05:09 GMT</d:getlastmodified><oc:permissions>RGDNVCK</oc:permissions></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat><d:propstat><d:prop><d:getcontentlength/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response><d:response><d:href>/remote.php/dav/files/alice/Keystead/abc.keystead</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>1234</d:getcontentlength><d:getetag>"e1"</d:getetag><d:getlastmodified>Fri, 23 Oct 2026 14:05:10 GMT</d:getlastmodified></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#;

    #[test]
    fn a_nextcloud_answer() {
        let entries = parse_multistatus(multistatus(NEXTCLOUD).as_bytes()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].href, "/remote.php/dav/files/alice/Keystead/");
        assert!(entries[0].collection);
        assert_eq!(entries[0].etag.as_deref(), Some("\"6531a0\""));
        assert_eq!(entries[0].size, None, "a 404 propstat says nothing");
        assert_eq!(entries[0].permissions.as_deref(), Some("RGDNVCK"));
        assert_eq!(entries[0].modified, Some(1_792_764_309_000));
        assert!(!entries[1].collection);
        assert_eq!(entries[1].size, Some(1234));
        assert_eq!(entries[1].etag.as_deref(), Some("\"e1\""));
    }

    #[test]
    fn other_prefixes_and_a_default_namespace() {
        let body = r#"<multistatus xmlns="DAV:"><response><href>/dav/x/</href><propstat><prop><resourcetype><collection/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response><D:response xmlns:D="DAV:"><D:href>/dav/x/a.keystead</D:href><D:propstat><D:prop><D:getcontentlength>5</D:getcontentlength><D:getetag>W/"weak"</D:getetag></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></multistatus>"#;
        let entries = parse_multistatus(body.as_bytes()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].collection);
        assert_eq!(entries[1].size, Some(5));
        assert_eq!(strong_etag(entries[1].etag.as_deref().unwrap()), None);
        assert_eq!(strong_etag("\"s\"").as_deref(), Some("\"s\""));
    }

    #[test]
    fn hostile_answers_are_refused() {
        let refused = |body: &str| {
            parse_multistatus(body.as_bytes()).unwrap_err().code()
                == "unsupported:sync_server_response"
        };
        // A DTD (entity expansion, external entities).
        assert!(refused(&format!(
            r#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;">]>{}"#,
            multistatus(NEXTCLOUD)
        )));
        // An entity nothing defines.
        assert!(refused(&multistatus(
            "<d:response><d:href>&xxe;</d:href></d:response>"
        )));
        // Too deep.
        let deep = format!("{}{}", "<x>".repeat(40), "</x>".repeat(40));
        assert!(refused(&multistatus(&deep)));
        // Too many responses.
        let many = "<d:response><d:href>/a/</d:href></d:response>".repeat(RESPONSES_MAX + 1);
        assert!(refused(&multistatus(&many)));
        // Too large.
        assert!(parse_multistatus(&vec![b' '; BODY_MAX + 1]).is_err());
        // No multistatus, broken XML, two hrefs, a bad size.
        assert!(refused("<html><body>Login</body></html>"));
        assert!(refused(&multistatus("<d:response><d:href>/a/")));
        assert!(refused(&multistatus(
            "<d:response><d:href>/a/</d:href><d:href>/b/</d:href></d:response>"
        )));
        assert!(refused(&multistatus(
            "<d:response><d:href>/a/</d:href><d:propstat><d:prop><d:getcontentlength>-1</d:getcontentlength></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
        )));
        // Character references are fine.
        let ok = parse_multistatus(
            multistatus("<d:response><d:href>/a/&#x62;</d:href></d:response>").as_bytes(),
        )
        .unwrap();
        assert_eq!(ok[0].href, "/a/b");
    }

    #[test]
    fn hrefs_name_the_folder_or_an_entry_in_it() {
        let collection =
            Url::parse("https://cloud.example/remote.php/dav/files/alice/Kee%20stead/").unwrap();
        let name = |href: &str| child_name(href, &collection);
        assert_eq!(
            name("/remote.php/dav/files/alice/Kee%20stead/").unwrap(),
            None
        );
        assert_eq!(name("/remote.php/dav/files/alice/Kee stead").unwrap(), None);
        assert_eq!(
            name("/remote.php/dav/files/alice/Kee%20stead/abc.keystead").unwrap(),
            Some("abc.keystead".into())
        );
        assert_eq!(
            name("https://cloud.example/remote.php/dav/files/alice/Kee%20stead/abc.keystead.attachments/")
                .unwrap(),
            Some("abc.keystead.attachments".into())
        );
        assert_eq!(
            name("https://CLOUD.example:443/remote.php/dav/files/alice/Kee%20stead/x").unwrap(),
            Some("x".into())
        );
        for bad in [
            "https://evil.example/remote.php/dav/files/alice/Kee%20stead/x",
            "http://cloud.example/remote.php/dav/files/alice/Kee%20stead/x",
            "https://cloud.example:8443/remote.php/dav/files/alice/Kee%20stead/x",
            "https://user@cloud.example/remote.php/dav/files/alice/Kee%20stead/x",
            "/remote.php/dav/files/alice/Kee%20stead/../x",
            "/remote.php/dav/files/alice/Kee%20stead/%2e%2e",
            "/remote.php/dav/files/alice/Kee%20stead/a%2Fb",
            "/remote.php/dav/files/alice/Kee%20stead/a%5cb",
            "/remote.php/dav/files/alice/Kee%20stead/a%00",
            "/remote.php/dav/files/alice/Kee%20stead/a\\b",
            "/remote.php/dav/files/alice/Kee%20stead//x",
            "/remote.php/dav/files/alice/Kee%20stead/a/b",
            "/remote.php/dav/files/alice/",
            "/remote.php/dav/files/alice/Other/x",
            "/remote.php/dav/files/alice/Kee%20stead/x?y",
            "abc.keystead",
            "/remote.php/dav/files/alice/Kee%20stead/%ff",
        ] {
            assert!(name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn http_dates() {
        assert_eq!(http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(
            http_date("Fri, 23 Oct 2026 14:05:09 GMT"),
            Some(1_792_764_309_000)
        );
        assert_eq!(http_date("Fri, 23 Foo 2026 14:05:09 GMT"), None);
        assert_eq!(http_date("2026-10-23T14:05:09Z"), None);
    }
}
