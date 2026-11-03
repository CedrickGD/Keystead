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
//!
//! Android apps (the phone's autofill, stage 3): a login names an app with
//! `androidapp://<package>` – or `android://<hash>@<package>`, as Chrome and
//! Google's password manager export it (the hash is not checked). Such a
//! URI matches exactly that package (case-sensitive, never a prefix: `com.x`
//! never matches `com.x.evil`), whatever its match type except `Never`, and
//! never a web page ([`app_uri_matches`]). The page an Android browser
//! reports is only a host; [`web_domain_url`] turns it into the URL the web
//! rules above compare.

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

/// Whether `name` is an Android package name: at least two segments
/// separated by `.`, each a letter followed by letters, digits or `_`; at
/// most 255 bytes.
pub fn is_android_package(name: &str) -> bool {
    name.len() <= 255
        && name.split('.').count() >= 2
        && name.split('.').all(|seg| {
            let mut chars = seg.chars();
            chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The Android package a stored URI names: `androidapp://<package>` or
/// `android://<anything>@<package>` (split at the last `@`), the scheme in
/// any case, at most one trailing `/` – nothing else (no path, query or
/// port). `None` for any other URI.
pub fn android_app_package(stored_uri: &str) -> Option<&str> {
    let raw = stored_uri.trim();
    let (scheme, rest) = raw.split_once("://")?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let package = if scheme.eq_ignore_ascii_case("androidapp") {
        rest
    } else if scheme.eq_ignore_ascii_case("android") {
        rest.rsplit_once('@')?.1
    } else {
        return None;
    };
    is_android_package(package).then_some(package)
}

/// True if the stored login URI names the Android app `package` (see the
/// module docs): exact equality, every match type but `Never`.
pub fn app_uri_matches(stored: &LoginUri, package: &str) -> bool {
    stored.match_type != UriMatch::Never
        && is_android_package(package)
        && android_app_package(&stored.uri) == Some(package)
}

/// `https://<host>/` for the web domain an Android browser reports for a
/// page (a host only), or `None`: one trailing `.` is dropped; an empty
/// host, more than 253 bytes, whitespace or any of `/ \ @ ? # : %` is
/// refused, and the parsed URL must have no user, port, path, query or
/// fragment. International names come out as punycode, in lower case.
pub fn web_domain_url(domain: &str) -> Option<String> {
    let domain = domain.strip_suffix('.').unwrap_or(domain);
    if domain.is_empty()
        || domain.len() > 253
        || domain
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '@' | '?' | '#' | ':' | '%'))
    {
        return None;
    }
    let url = Url::parse(&format!("https://{domain}/")).ok()?;
    let clean = url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.host().is_some();
    clean.then(|| url.to_string())
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

    #[test]
    fn android_packages_and_app_uris() {
        for good in ["com.x", "com.example.app", "de.bank_x.app2", "A.b"] {
            assert!(is_android_package(good), "{good}");
        }
        for bad in [
            "com", "com..x", "1com.x", "com.x/y", "com.x.", ".com.x", "com.-x", "", "com.x y",
        ] {
            assert!(!is_android_package(bad), "{bad}");
        }
        let pkg = android_app_package;
        assert_eq!(pkg("androidapp://com.x"), Some("com.x"));
        assert_eq!(pkg("ANDROIDAPP://com.x/"), Some("com.x"));
        assert_eq!(pkg(" androidapp://com.x "), Some("com.x"));
        assert_eq!(pkg("android://Xk1Q==@com.x/"), Some("com.x"));
        assert_eq!(pkg("android://a@b@com.x"), Some("com.x"));
        for none in [
            "androidapp://com.x//",
            "androidapp://com.x/path",
            "androidapp://com.x?y=1",
            "androidapp://com.x:80",
            "android://com.x",
            "https://com.x",
            "com.x.app",
            "androidapp://",
            "androidapp://com",
        ] {
            assert_eq!(pkg(none), None, "{none}");
        }
        let stored = |u: &str, m: UriMatch| LoginUri {
            uri: u.into(),
            match_type: m,
        };
        for m in [
            UriMatch::Domain,
            UriMatch::Host,
            UriMatch::StartsWith,
            UriMatch::Exact,
        ] {
            assert!(app_uri_matches(&stored("androidapp://com.x", m), "com.x"));
        }
        assert!(!app_uri_matches(
            &stored("androidapp://com.x", UriMatch::Never),
            "com.x"
        ));
        assert!(!app_uri_matches(
            &stored("androidapp://com.x", UriMatch::Domain),
            "com.x.evil"
        ));
        assert!(!app_uri_matches(
            &stored("androidapp://com.x.evil", UriMatch::Domain),
            "com.x"
        ));
        assert!(!app_uri_matches(
            &stored("androidapp://com.x", UriMatch::Domain),
            "COM.X"
        ));
        assert!(!app_uri_matches(
            &stored("androidapp://com.e", UriMatch::Domain),
            "com.x"
        ));
        assert!(!app_uri_matches(
            &stored("com.x.app", UriMatch::Domain),
            "com.x.app"
        ));
        // An app URI never matches a web page, a web URI never an app.
        assert!(!uri_matches(
            &stored("androidapp://com.github", UriMatch::Domain),
            "https://github.com/"
        ));
        assert!(!app_uri_matches(
            &stored("https://github.com", UriMatch::Domain),
            "com.github"
        ));
    }

    #[test]
    fn web_domains_of_android_browsers() {
        let url = web_domain_url;
        assert_eq!(url("github.com").as_deref(), Some("https://github.com/"));
        assert_eq!(url("GitHub.COM.").as_deref(), Some("https://github.com/"));
        assert_eq!(
            url("münchen.de").as_deref(),
            Some("https://xn--mnchen-3ya.de/")
        );
        assert_eq!(
            url("192.168.1.10").as_deref(),
            Some("https://192.168.1.10/")
        );
        for bad in [
            "",
            ".",
            "bank.com@evil.com",
            "evil.com/bank.com",
            "bank.com:443",
            "bank.com?x",
            "bank.com#x",
            "bank com",
            "bank.com\\x",
            "ban%6b.com",
            &"a".repeat(254),
        ] {
            assert_eq!(url(bad), None, "{bad}");
        }
        // The web rules then compare it as usual.
        let stored = LoginUri {
            uri: "https://github.com".into(),
            match_type: UriMatch::Domain,
        };
        assert!(uri_matches(&stored, &url("gist.github.com").unwrap()));
        assert!(!uri_matches(&stored, &url("github.com.evil.io").unwrap()));
    }

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
