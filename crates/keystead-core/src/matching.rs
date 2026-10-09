//! URL matching for autofill suggestions.
//!
//! Rules:
//! * `Domain` (default): same registrable domain ("eTLD+1" via the public
//!   suffix list), e.g. `example.com` matches `login.example.com`; IP
//!   addresses, `localhost` and hosts without a registrable domain (intranet
//!   names) must be equal. Ports are ignored.
//! * `Host`: same host name and same port (default ports are equivalent to
//!   no port, the scheme itself is not compared).
//! * `StartsWith` / `Exact`: the (normalised) page URL starts with / equals
//!   the (normalised) stored URI.
//! * `Never`: never matches.
//!
//! Stored URIs without a scheme (`example.com`, `example.com:8080/login`)
//! are treated as web addresses. `Domain` and `Host` only ever match
//! `http(s)` page URLs.

use std::net::IpAddr;

use url::{Host, Url};

use crate::model::{LoginUri, UriMatch};

/// Registrable domain ("eTLD+1") of a host name: `login.example.co.uk` →
/// `example.co.uk`. IP addresses and `localhost` are returned as-is. `None`
/// for public suffixes themselves (`co.uk`) and unparsable input.
pub fn registrable_domain(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.');
    if host.is_empty() {
        return None;
    }
    let unbracketed = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = unbracketed.parse::<IpAddr>() {
        return Some(ip.to_string());
    }
    let ascii = match Host::parse(host).ok()? {
        Host::Domain(d) => d,
        Host::Ipv4(ip) => return Some(ip.to_string()),
        Host::Ipv6(ip) => return Some(ip.to_string()),
    };
    let ascii = ascii.trim_end_matches('.').to_ascii_lowercase();
    if ascii == "localhost" {
        return Some(ascii);
    }
    psl::domain_str(&ascii).map(str::to_owned)
}

/// True if the stored login URI should be suggested for `page_url`.
pub fn uri_matches(stored: &LoginUri, page_url: &str) -> bool {
    let stored_raw = stored.uri.trim();
    let page_raw = page_url.trim();
    if stored_raw.is_empty() || page_raw.is_empty() {
        return false;
    }
    match stored.match_type {
        UriMatch::Never => false,
        UriMatch::Exact => compare_urls(stored_raw, page_raw, |s, p| s == p),
        UriMatch::StartsWith => compare_urls(stored_raw, page_raw, |s, p| p.starts_with(s)),
        UriMatch::Host | UriMatch::Domain => {
            let Some(page) = Url::parse(page_raw).ok().filter(is_web) else {
                return false;
            };
            let Some(stored_url) = parse_stored(stored_raw).filter(is_web) else {
                return false;
            };
            let (Some(sh), Some(ph)) = (stored_url.host(), page.host()) else {
                return false;
            };
            if stored.match_type == UriMatch::Host {
                same_host(&sh, &ph) && stored_url.port() == page.port()
            } else {
                same_domain(&sh, &ph)
            }
        }
    }
}

pub(crate) fn is_web(u: &Url) -> bool {
    matches!(u.scheme(), "http" | "https")
}

fn has_scheme(raw: &str) -> bool {
    raw.contains("://")
}

/// Parses a stored URI; scheme-less input is read as `https://<input>`.
pub(crate) fn parse_stored(raw: &str) -> Option<Url> {
    if has_scheme(raw) {
        Url::parse(raw).ok()
    } else {
        Url::parse(&format!("https://{raw}")).ok()
    }
}

/// Compares normalised forms for `Exact`/`StartsWith`. A scheme-less stored
/// URI matches both `http` and `https` pages. Falls back to raw string
/// comparison when either side is not a parsable absolute URL.
fn compare_urls(stored_raw: &str, page_raw: &str, cmp: impl Fn(&str, &str) -> bool) -> bool {
    let Ok(mut page) = Url::parse(page_raw) else {
        return cmp(stored_raw, page_raw);
    };
    let Some(stored_url) = parse_stored(stored_raw) else {
        return cmp(stored_raw, page_raw);
    };
    if has_scheme(stored_raw) {
        return cmp(stored_url.as_str(), page.as_str());
    }
    if !is_web(&page) {
        return false;
    }
    if page.scheme() == "http" {
        let port = page.port();
        if page.set_scheme("https").is_err() {
            return false;
        }
        // Keep an explicit port (":80" stays distinguishable from none).
        if port.is_none() && page.port().is_some() && page.set_port(None).is_err() {
            return false;
        }
    }
    cmp(stored_url.as_str(), page.as_str())
}

fn host_string(h: &Host<&str>) -> String {
    match h {
        Host::Domain(d) => d.trim_end_matches('.').to_ascii_lowercase(),
        Host::Ipv4(ip) => ip.to_string(),
        Host::Ipv6(ip) => ip.to_string(),
    }
}

fn same_host(a: &Host<&str>, b: &Host<&str>) -> bool {
    host_string(a) == host_string(b)
}

fn is_ip_or_localhost(h: &Host<&str>) -> bool {
    match h {
        Host::Ipv4(_) | Host::Ipv6(_) => true,
        Host::Domain(d) => d.trim_end_matches('.').eq_ignore_ascii_case("localhost"),
    }
}

fn same_domain(a: &Host<&str>, b: &Host<&str>) -> bool {
    if is_ip_or_localhost(a) || is_ip_or_localhost(b) {
        return same_host(a, b);
    }
    let (ha, hb) = (host_string(a), host_string(b));
    match (registrable_domain(&ha), registrable_domain(&hb)) {
        (Some(da), Some(db)) => da == db,
        // Intranet names etc.: fall back to exact host comparison.
        _ => ha == hb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(u: &str, m: UriMatch) -> LoginUri {
        LoginUri {
            uri: u.to_owned(),
            match_type: m,
        }
    }

    fn check(stored: &str, m: UriMatch, page: &str) -> bool {
        uri_matches(&uri(stored, m), page)
    }

    #[test]
    fn registrable_domains() {
        let rd = |h: &str| registrable_domain(h);
        assert_eq!(rd("example.com").as_deref(), Some("example.com"));
        assert_eq!(rd("login.example.com").as_deref(), Some("example.com"));
        assert_eq!(rd("WWW.Example.COM.").as_deref(), Some("example.com"));
        assert_eq!(rd("a.b.example.co.uk").as_deref(), Some("example.co.uk"));
        assert_eq!(rd("user.github.io").as_deref(), Some("user.github.io"));
        assert_eq!(rd("co.uk"), None);
        assert_eq!(rd("com"), None);
        assert_eq!(rd(""), None);
        assert_eq!(rd("192.168.1.10").as_deref(), Some("192.168.1.10"));
        assert_eq!(rd("[::1]").as_deref(), Some("::1"));
        assert_eq!(rd("::1").as_deref(), Some("::1"));
        assert_eq!(rd("localhost").as_deref(), Some("localhost"));
        assert_eq!(rd("münchen.de").as_deref(), Some("xn--mnchen-3ya.de"));
        assert_eq!(rd("bad host"), None);
    }

    #[test]
    fn domain_match() {
        use UriMatch::Domain as D;
        assert!(check("https://example.com", D, "https://example.com/login"));
        assert!(check(
            "https://example.com",
            D,
            "https://login.example.com/x?y=1"
        ));
        assert!(check("https://www.example.com/a", D, "http://example.com"));
        assert!(check("example.com", D, "https://accounts.example.com/"));
        assert!(check("example.com", D, "http://example.com:8080/"));
        assert!(check("Example.COM", D, "https://EXAMPLE.com"));
        assert!(check(
            "https://a.example.co.uk",
            D,
            "https://b.example.co.uk"
        ));
        assert!(!check("https://example.com", D, "https://example.org"));
        assert!(!check(
            "https://example.com",
            D,
            "https://example.com.evil.org"
        ));
        assert!(!check("https://example.com", D, "https://notexample.com"));
        assert!(!check("https://a.example.co.uk", D, "https://other.co.uk"));
        assert!(!check(
            "https://alice.github.io",
            D,
            "https://bob.github.io"
        ));
        assert!(check("münchen.de", D, "https://www.xn--mnchen-3ya.de/"));
    }

    #[test]
    fn non_web_pages_never_match_domain_or_host() {
        for m in [UriMatch::Domain, UriMatch::Host] {
            assert!(!check("example.com", m, "ftp://example.com/"));
            assert!(!check("example.com", m, "chrome://settings"));
            assert!(!check("example.com", m, "file:///C:/example.com/x.html"));
            assert!(!check("example.com", m, "about:blank"));
            assert!(!check("example.com", m, "not a url"));
            assert!(!check("androidapp://com.example", m, "https://example.com"));
        }
    }

    #[test]
    fn ip_and_localhost_compare_by_host() {
        use UriMatch::Domain as D;
        assert!(check("192.168.1.1", D, "http://192.168.1.1/admin"));
        assert!(check("http://192.168.1.1:8080", D, "https://192.168.1.1/"));
        assert!(!check("192.168.1.1", D, "http://192.168.1.2/"));
        assert!(!check("10.0.0.1", D, "http://1.0.0.1/"));
        assert!(check("localhost:3000", D, "http://localhost:5173/"));
        assert!(!check("localhost", D, "http://example.com/"));
        assert!(!check("http://app.localhost", D, "http://localhost/"));
        assert!(check("http://[::1]:8080/", D, "http://[::1]/"));
        assert!(check("nas", D, "http://nas:5000/"));
        assert!(!check("nas", D, "http://nas2:5000/"));
    }

    #[test]
    fn host_match_includes_port() {
        use UriMatch::Host as H;
        assert!(check("https://example.com", H, "https://example.com/path"));
        assert!(check("example.com", H, "http://example.com/"));
        assert!(check("https://example.com:443", H, "https://example.com/"));
        assert!(!check(
            "https://example.com",
            H,
            "https://login.example.com"
        ));
        assert!(!check(
            "https://example.com",
            H,
            "https://example.com:8443/"
        ));
        assert!(check("example.com:8443", H, "https://example.com:8443/"));
        assert!(!check("example.com:8443", H, "https://example.com:9443/"));
        assert!(check("192.168.0.5:8080", H, "http://192.168.0.5:8080/x"));
        assert!(!check("192.168.0.5:8080", H, "http://192.168.0.5/x"));
    }

    #[test]
    fn starts_with() {
        use UriMatch::StartsWith as S;
        assert!(check(
            "https://example.com/app",
            S,
            "https://example.com/app/login"
        ));
        assert!(check("https://example.com", S, "https://example.com/x"));
        assert!(!check(
            "https://example.com",
            S,
            "https://example.com.evil.org/"
        ));
        assert!(!check("https://example.com/app", S, "https://example.com/"));
        assert!(!check(
            "https://example.com/app",
            S,
            "http://example.com/app"
        ));
        assert!(check("example.com/app", S, "http://example.com/app/1"));
        assert!(check("example.com/app", S, "https://example.com/app/1"));
        assert!(!check("example.com/app", S, "ftp://example.com/app/1"));
        // Non-http page URLs are allowed for StartsWith/Exact.
        assert!(check(
            "ftp://files.example.com/",
            S,
            "ftp://files.example.com/a"
        ));
        assert!(check("custom-thing", S, "custom-thing-and-more"));
    }

    #[test]
    fn exact() {
        use UriMatch::Exact as E;
        assert!(check(
            "https://example.com/login",
            E,
            "https://example.com/login"
        ));
        assert!(check("https://example.com", E, "https://example.com/"));
        assert!(check("https://EXAMPLE.com/a", E, "https://example.com/a"));
        assert!(!check(
            "https://example.com/login",
            E,
            "https://example.com/login?x"
        ));
        assert!(!check(
            "https://example.com/login",
            E,
            "https://example.com/"
        ));
        assert!(check("example.com/login", E, "http://example.com/login"));
        assert!(!check(
            "example.com:80/login",
            E,
            "http://example.com/login"
        ));
        assert!(check("chrome://settings", E, "chrome://settings"));
    }

    #[test]
    fn never_and_empty() {
        assert!(!check(
            "https://example.com",
            UriMatch::Never,
            "https://example.com"
        ));
        assert!(!check("", UriMatch::Domain, "https://example.com"));
        assert!(!check("   ", UriMatch::Exact, "   "));
        assert!(!check("https://example.com", UriMatch::Domain, ""));
    }
}
