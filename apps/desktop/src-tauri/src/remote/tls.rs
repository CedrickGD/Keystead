//! TLS of the server sync – the only place on the phone that builds an
//! HTTP client (a unit test checks that).
//!
//! * [`client`] builds the WebDAV client on a rustls configuration of its
//!   own (`tls_backend_preconfigured`): ring as the crypto provider; the
//!   server certificate is checked by the system's trust store on the
//!   desktop (`rustls-platform-verifier`, as reqwest itself would) and by
//!   Mozilla's root store compiled in (`webpki-roots`) on Android – there
//!   the platform verifier would need JNI set-up. No redirects, connect
//!   timeout 15 s, read timeout 30 s, no total timeout (a large vault file
//!   over a slow line must not be cut off).
//! * A server with a certificate no trust store knows (a NAS, `rclone serve
//!   webdav` with its own certificate): the connection test returns
//!   `sync_server_certificate` and the leaf certificate's facts
//!   ([`fetch_certificate`]: SHA-256, subject, issuer, validity); once the
//!   user confirmed it, [`PinnedVerifier`] accepts exactly that leaf – the
//!   handshake signatures are still verified with it, so only the holder of
//!   its private key passes – and nothing else. A changed certificate is
//!   `sync_server_certificate` again (the user confirms the new one).

use std::io::{Read as _, Write as _};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Url;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use super::{BAD_RESPONSE, CERTIFICATE, OFFLINE};
use crate::error::{AppError, AppResult};

/// Connecting gives up after this long.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A read (of the answer's head or the next part of its body) gives up
/// after this long.
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Which server certificates are accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// The ones the system's (desktop) or Mozilla's (Android) roots vouch
    /// for, for the server's name.
    System,
    /// Exactly the leaf with this SHA-256 (the user confirmed it).
    Pinned([u8; 32]),
}

/// The crypto provider of the server sync: ring (as the updater's).
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The rustls configuration for `trust` (HTTP/1.1, the safe default
/// protocol versions TLS 1.2 and 1.3).
pub fn client_config(trust: Trust) -> AppResult<ClientConfig> {
    let provider = provider();
    let verifier: Arc<dyn ServerCertVerifier> = match trust {
        Trust::System => system_verifier(Arc::clone(&provider))?,
        Trust::Pinned(sha256) => Arc::new(PinnedVerifier::new(sha256, Arc::clone(&provider))),
    };
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| AppError::unsupported(CERTIFICATE))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

/// The desktop: the system's trust store.
#[cfg(desktop)]
fn system_verifier(provider: Arc<CryptoProvider>) -> AppResult<Arc<dyn ServerCertVerifier>> {
    let verifier = rustls_platform_verifier::Verifier::new(provider)
        .map_err(|_| AppError::unsupported(CERTIFICATE))?;
    Ok(Arc::new(verifier))
}

/// Android: Mozilla's root store (no JNI set-up for the platform
/// verifier).
#[cfg(target_os = "android")]
fn system_verifier(provider: Arc<CryptoProvider>) -> AppResult<Arc<dyn ServerCertVerifier>> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let verifier =
        rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .map_err(|_| AppError::unsupported(CERTIFICATE))?;
    Ok(verifier)
}

/// iOS has no server sync yet.
#[cfg(target_os = "ios")]
fn system_verifier(_provider: Arc<CryptoProvider>) -> AppResult<Arc<dyn ServerCertVerifier>> {
    Err(AppError::unsupported(CERTIFICATE))
}

/// The HTTP client of the server sync (see the module docs). The only
/// place on the phone that builds one.
pub fn client(trust: Trust) -> AppResult<reqwest::Client> {
    // `rustls-no-provider`: reqwest wants a process-wide provider even with
    // a configuration of its own (the updater installs ring the same way).
    if CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    reqwest::Client::builder()
        .tls_backend_preconfigured(client_config(trust)?)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("Keystead/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| AppError::unsupported(CERTIFICATE))
}

/// SHA-256 of a DER certificate.
pub fn sha256_of(der: &[u8]) -> [u8; 32] {
    Sha256::digest(der).into()
}

/// `[u8; 32]` as lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A SHA-256 as the settings keep it (hex, colons and case ignored).
pub fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|b| *b != b':')
        .map(|b| b.to_ascii_lowercase())
        .collect();
    if digits.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in digits.chunks(2).enumerate() {
        let pair = std::str::from_utf8(pair).ok()?;
        out[i] = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

/// Accepts exactly one leaf certificate (the one the user confirmed): its
/// SHA-256 must match; chain, name and validity are not looked at (the user
/// trusted this certificate for this server). The handshake signatures are
/// verified with that leaf's key as usual.
#[derive(Debug)]
pub struct PinnedVerifier {
    sha256: [u8; 32],
    provider: Arc<CryptoProvider>,
}

impl PinnedVerifier {
    pub fn new(sha256: [u8; 32], provider: Arc<CryptoProvider>) -> Self {
        PinnedVerifier { sha256, provider }
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if sha256_of(end_entity) == self.sha256 {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

// ---------------------------------------------------------------------------
// The leaf certificate for "trust this certificate"
// ---------------------------------------------------------------------------

/// What the user is shown before trusting a server's certificate. No
/// secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertInfo {
    /// SHA-256 of the leaf certificate (DER), lower-case hex – what is
    /// stored as `certSha256` once confirmed.
    pub sha256: String,
    /// `CN=…, O=…` of the subject and the issuer (as far as they say).
    pub subject: String,
    pub issuer: String,
    /// Validity (ms since the epoch), if readable.
    pub not_before: Option<i64>,
    pub not_after: Option<i64>,
}

impl CertInfo {
    /// The facts of a DER certificate.
    pub fn of(der: &[u8]) -> CertInfo {
        let parsed = der_cert::parse(der);
        CertInfo {
            sha256: hex(&sha256_of(der)),
            subject: parsed
                .as_ref()
                .map(|p| p.subject.clone())
                .unwrap_or_default(),
            issuer: parsed
                .as_ref()
                .map(|p| p.issuer.clone())
                .unwrap_or_default(),
            not_before: parsed.as_ref().and_then(|p| p.not_before),
            not_after: parsed.as_ref().and_then(|p| p.not_after),
        }
    }
}

/// Records the leaf certificate and stops the handshake there: nothing is
/// sent to a server that is not trusted yet.
#[derive(Debug)]
struct Capture {
    leaf: Mutex<Option<Vec<u8>>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Capture {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self
            .leaf
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(end_entity.to_vec());
        Err(rustls::Error::InvalidCertificate(
            CertificateError::ApplicationVerificationFailure,
        ))
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("not used".into()))
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("not used".into()))
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The leaf certificate the server at `url` (https) presents, for the user
/// to confirm. A TLS handshake up to the certificate only (no request);
/// connect and read time out like the client's. Errors:
/// `unsupported:sync_offline` (no connection), `unsupported:sync_server_response`
/// (no certificate came).
pub fn fetch_certificate(url: &Url) -> AppResult<CertInfo> {
    let offline = || AppError::unsupported(OFFLINE);
    let host = url.host_str().ok_or_else(offline)?.to_owned();
    let port = url.port_or_known_default().ok_or_else(offline)?;
    let name = ServerName::try_from(host.trim_matches(['[', ']']).to_owned())
        .map_err(|_| AppError::invalid("server_url"))?;
    let addrs: Vec<_> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| offline())?
        .collect();
    let mut stream = addrs
        .iter()
        .find_map(|addr| TcpStream::connect_timeout(addr, CONNECT_TIMEOUT).ok())
        .ok_or_else(offline)?;
    stream.set_read_timeout(Some(READ_TIMEOUT)).ok();
    stream.set_write_timeout(Some(READ_TIMEOUT)).ok();

    let provider = provider();
    let capture = Arc::new(Capture {
        leaf: Mutex::new(None),
        provider: Arc::clone(&provider),
    });
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| AppError::unsupported(CERTIFICATE))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::clone(&capture) as Arc<dyn ServerCertVerifier>)
        .with_no_client_auth();
    let mut conn = rustls::ClientConnection::new(Arc::new(config), name)
        .map_err(|_| AppError::unsupported(CERTIFICATE))?;
    // Ends with the capture's refusal (or the server giving up).
    let mut tls = rustls::Stream::new(&mut conn, &mut stream);
    let _ = tls.write_all(b"");
    let _ = tls.flush();
    let mut byte = [0u8; 1];
    let _ = tls.read(&mut byte);
    let leaf = capture
        .leaf
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
        .ok_or_else(|| AppError::unsupported(BAD_RESPONSE))?;
    Ok(CertInfo::of(&leaf))
}

/// A minimal DER reader for the facts [`CertInfo`] shows: subject, issuer
/// (common name, organisation, unit, country) and validity. Anything it
/// does not understand is left out – the SHA-256 is what the user confirms.
mod der_cert {
    pub struct Parsed {
        pub subject: String,
        pub issuer: String,
        pub not_before: Option<i64>,
        pub not_after: Option<i64>,
    }

    /// One TLV: (tag, content, rest).
    fn tlv(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
        let (&tag, rest) = data.split_first()?;
        let (&first, rest) = rest.split_first()?;
        let (len, rest) = if first < 0x80 {
            (usize::from(first), rest)
        } else {
            let n = usize::from(first & 0x7f);
            if n == 0 || n > 4 || rest.len() < n {
                return None;
            }
            let len = rest[..n]
                .iter()
                .fold(0usize, |acc, b| (acc << 8) | usize::from(*b));
            (len, &rest[n..])
        };
        (rest.len() >= len).then(|| (tag, &rest[..len], &rest[len..]))
    }

    pub fn parse(der: &[u8]) -> Option<Parsed> {
        let (0x30, cert, _) = tlv(der)? else {
            return None;
        };
        let (0x30, tbs, _) = tlv(cert)? else {
            return None;
        };
        let mut rest = tbs;
        let (tag, _, after) = tlv(rest)?;
        if tag == 0xa0 {
            rest = after; // version
        }
        let (_, _, after) = tlv(rest)?; // serial number
        let (_, _, after) = tlv(after)?; // signature algorithm
        let (_, issuer, after) = tlv(after)?;
        let (_, validity, after) = tlv(after)?;
        let (_, subject, _) = tlv(after)?;
        let (before_tag, before, rest) = tlv(validity)?;
        let (after_tag, after, _) = tlv(rest)?;
        Some(Parsed {
            subject: name(subject),
            issuer: name(issuer),
            not_before: time(before_tag, before),
            not_after: time(after_tag, after),
        })
    }

    /// `CN=…, O=…` of a Name (the attributes shown, in their order).
    fn name(mut data: &[u8]) -> String {
        let mut parts = Vec::new();
        while let Some((0x31, set, rest)) = tlv(data) {
            data = rest;
            let mut set = set;
            while let Some((0x30, attr, more)) = tlv(set) {
                set = more;
                let Some((0x06, oid, value)) = tlv(attr) else {
                    continue;
                };
                let label = match oid {
                    [0x55, 0x04, 0x03] => "CN",
                    [0x55, 0x04, 0x0a] => "O",
                    [0x55, 0x04, 0x0b] => "OU",
                    [0x55, 0x04, 0x06] => "C",
                    _ => continue,
                };
                if let Some((tag, text, _)) = tlv(value) {
                    if let Some(text) = string(tag, text) {
                        parts.push(format!("{label}={text}"));
                    }
                }
            }
        }
        parts.join(", ")
    }

    fn string(tag: u8, bytes: &[u8]) -> Option<String> {
        let text = match tag {
            // UTF8String, PrintableString, IA5String, T61String.
            0x0c | 0x13 | 0x16 | 0x14 => String::from_utf8_lossy(bytes).into_owned(),
            // BMPString (UTF-16BE).
            0x1e => {
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&units)
            }
            _ => return None,
        };
        Some(text.chars().filter(|c| !c.is_control()).take(200).collect())
    }

    /// UTCTime / GeneralizedTime (`…Z`) → ms since the epoch.
    fn time(tag: u8, bytes: &[u8]) -> Option<i64> {
        let text = std::str::from_utf8(bytes).ok()?.strip_suffix('Z')?;
        let digits = |s: &str| -> Option<i64> {
            s.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| s.parse().ok())?
        };
        let (year, rest) = match tag {
            0x17 if text.len() == 12 => {
                let yy = digits(&text[..2])?;
                (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &text[2..])
            }
            0x18 if text.len() == 14 => (digits(&text[..4])?, &text[4..]),
            _ => return None,
        };
        let field = |i: usize| digits(&rest[i..i + 2]);
        let (month, day) = (field(0)?, field(2)?);
        let (hour, minute, second) = (field(4)?, field(6)?, field(8)?);
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
            return None;
        }
        // Days since 1970-01-01 (proleptic Gregorian, H. Hinnant's formula).
        let y = if month <= 2 { year - 1 } else { year };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (month + 9) % 12;
        let doy = (153 * mp + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        Some(((days * 24 + hour) * 60 + minute) * 60_000 + second.min(60) * 1000)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn times() {
            assert_eq!(super::time(0x17, b"700101000000Z"), Some(0));
            assert_eq!(
                super::time(0x18, b"20261023140509Z"),
                Some(1_792_764_309_000)
            );
            assert_eq!(super::time(0x17, b"491231235959Z"), Some(2_524_607_999_000));
            assert_eq!(super::time(0x17, b"7001010000Z"), None);
            assert_eq!(super::time(0x18, b"20261323140509Z"), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// On Android nothing but [`client`] builds an HTTP client (anything
    /// else would use reqwest's own TLS set-up – the platform verifier,
    /// which needs JNI there). The other clients are in modules the phone
    /// does not compile, or in desktop-only or test code.
    #[test]
    fn on_the_phone_only_this_module_builds_an_http_client() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let read = |path: &Path| std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
        // Modules the phone does not compile.
        assert!(read(&src.join("lib.rs")).contains("#[cfg(desktop)]\nmod pwned;"));
        assert!(read(&src.join("icons.rs")).contains("#[cfg(desktop)]\nmod fetch;"));
        let desktop_modules = ["pwned.rs", "icons/fetch.rs"];

        fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&src, &mut files);
        let patterns = [
            "Client::builder",
            "ClientBuilder::new",
            "Client::new(",
            "reqwest::blocking",
            "use_preconfigured_tls",
            "tls_backend_preconfigured",
        ];
        let mut found = 0;
        for file in files {
            let rel = file
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let text = read(&file);
            let test_start = text.find("#[cfg(test)]\nmod tests").unwrap_or(text.len());
            for (at, _) in patterns.iter().flat_map(|p| text.match_indices(p)) {
                found += 1;
                if rel == "remote/tls.rs" || desktop_modules.contains(&rel.as_str()) {
                    continue;
                }
                if at > test_start {
                    continue;
                }
                // Otherwise only in a function compiled for the desktop.
                let before = &text[..at];
                let function = before
                    .rfind("\nfn ")
                    .max(before.rfind("\npub fn "))
                    .max(before.rfind("\npub async fn "));
                let attributes =
                    function.map(|f| &before[before[..f].rfind("\n\n").unwrap_or(0)..f]);
                assert!(
                    attributes.is_some_and(|a| a.contains("#[cfg(desktop)]")),
                    "{rel}: an HTTP client outside remote::tls the phone compiles"
                );
            }
        }
        assert!(found >= 5, "the patterns still match the code ({found})");
    }

    #[test]
    fn sha256_text_round_trip() {
        let sha = sha256_of(b"cert");
        assert_eq!(parse_sha256(&hex(&sha)), Some(sha));
        let colons: Vec<String> = sha.iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(parse_sha256(&colons.join(":")), Some(sha));
        assert_eq!(parse_sha256("abc"), None);
        assert_eq!(parse_sha256(&"g".repeat(64)), None);
    }

    #[test]
    fn the_facts_of_a_certificate() {
        let mut params = rcgen::CertificateParams::new(vec!["nas.local".to_owned()]).unwrap();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "nas.local");
        params
            .distinguished_name
            .push(rcgen::DnType::OrganizationName, "Familie Muster");
        params.not_before = rcgen::date_time_ymd(2026, 1, 2);
        params.not_after = rcgen::date_time_ymd(2036, 1, 2);
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let info = CertInfo::of(cert.der());
        assert_eq!(info.sha256, hex(&sha256_of(cert.der())));
        assert_eq!(info.subject, "CN=nas.local, O=Familie Muster");
        assert_eq!(info.issuer, info.subject, "self-signed");
        assert_eq!(info.not_before, Some(1_767_312_000_000));
        assert_eq!(info.not_after, Some(2_082_844_800_000));
        // Not a certificate: only the fingerprint.
        let junk = CertInfo::of(b"\x30\x03\x02\x01");
        assert_eq!(junk.subject, "");
        assert_eq!(junk.not_after, None);
    }
}
