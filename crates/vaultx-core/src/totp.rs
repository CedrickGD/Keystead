//! TOTP (RFC 6238) codes from a base32 secret or an `otpauth://totp/` URI.

use data_encoding::{Encoding, Specification};
use hmac::{KeyInit, Mac};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// A TOTP code and its validity window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TotpCode {
    pub code: String,
    /// Period in seconds.
    pub period: u32,
    /// Seconds until the code changes (1..=period).
    pub remaining: u32,
}

/// HMAC hash function of a TOTP seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotpAlgorithm {
    Sha1,
    Sha256,
    Sha512,
}

/// Parsed TOTP parameters.
#[derive(Clone, PartialEq, Eq)]
pub struct TotpParams {
    pub secret: Zeroizing<Vec<u8>>,
    pub algorithm: TotpAlgorithm,
    /// 6..=8
    pub digits: u32,
    /// Seconds, 1..=3600
    pub period: u32,
    pub issuer: Option<String>,
    pub account: Option<String>,
}

impl std::fmt::Debug for TotpParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TotpParams")
            .field("algorithm", &self.algorithm)
            .field("digits", &self.digits)
            .field("period", &self.period)
            .field("issuer", &self.issuer)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

const MAX_PERIOD: u32 = 3600;

fn base32() -> Encoding {
    // RFC 4648 alphabet, no padding, lenient about non-zero trailing bits
    // (some services emit secrets whose length is not a multiple of 8).
    let mut spec = Specification::new();
    spec.symbols.push_str("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567");
    spec.check_trailing_bits = false;
    // The specification above is statically valid; fall back to the strict
    // built-in encoding should that ever change.
    spec.encoding()
        .unwrap_or_else(|_| data_encoding::BASE32_NOPAD.clone())
}

/// Decodes a base32 secret, ignoring spaces, dashes, case and padding.
pub fn decode_base32_secret(s: &str) -> Result<Zeroizing<Vec<u8>>> {
    let cleaned: Zeroizing<String> = Zeroizing::new(
        s.chars()
            .filter(|c| !c.is_whitespace() && *c != '-' && *c != '=')
            .map(|c| c.to_ascii_uppercase())
            .collect(),
    );
    if cleaned.is_empty() {
        return Err(Error::invalid("totp_secret_empty"));
    }
    let bytes = base32()
        .decode(cleaned.as_bytes())
        .map_err(|_| Error::invalid("totp_secret"))?;
    if bytes.is_empty() {
        return Err(Error::invalid("totp_secret"));
    }
    Ok(Zeroizing::new(bytes))
}

/// Parses a TOTP seed: a bare base32 secret or an `otpauth://totp/…` URI.
pub fn parse(seed: &str) -> Result<TotpParams> {
    let seed = seed.trim();
    if seed.len() >= 10 && seed[..10].eq_ignore_ascii_case("otpauth://") {
        return parse_uri(seed);
    }
    Ok(TotpParams {
        secret: decode_base32_secret(seed)?,
        algorithm: TotpAlgorithm::Sha1,
        digits: 6,
        period: 30,
        issuer: None,
        account: None,
    })
}

fn parse_uri(seed: &str) -> Result<TotpParams> {
    let url = url::Url::parse(seed).map_err(|_| Error::invalid("totp_uri"))?;
    match url.host_str().map(str::to_ascii_lowercase).as_deref() {
        Some("totp") => {}
        Some("hotp") => return Err(Error::Unsupported("hotp".into())),
        _ => return Err(Error::invalid("totp_uri")),
    }
    let mut secret = None;
    let mut algorithm = TotpAlgorithm::Sha1;
    let mut digits = 6;
    let mut period = 30;
    let mut issuer = None;
    for (k, v) in url.query_pairs() {
        match k.to_ascii_lowercase().as_str() {
            "secret" => secret = Some(decode_base32_secret(&v)?),
            "algorithm" => {
                algorithm = match v.to_ascii_uppercase().replace('-', "").as_str() {
                    "SHA1" => TotpAlgorithm::Sha1,
                    "SHA256" => TotpAlgorithm::Sha256,
                    "SHA512" => TotpAlgorithm::Sha512,
                    _ => return Err(Error::Unsupported(format!("totp algorithm {v}"))),
                }
            }
            "digits" => {
                digits = v
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|d| (6..=8).contains(d))
                    .ok_or_else(|| Error::invalid("totp_digits"))?;
            }
            "period" => {
                period = v
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|p| (1..=MAX_PERIOD).contains(p))
                    .ok_or_else(|| Error::invalid("totp_period"))?;
            }
            "issuer" if !v.trim().is_empty() => issuer = Some(v.trim().to_owned()),
            _ => {}
        }
    }
    let secret = secret.ok_or_else(|| Error::invalid("totp_secret_empty"))?;
    // Label: "Issuer:account" or "account" (percent-decoded).
    let label = percent_decode(url.path().trim_start_matches('/'));
    let (label_issuer, account) = match label.split_once(':') {
        Some((i, a)) => (Some(i.trim().to_owned()), a.trim().to_owned()),
        None => (None, label.trim().to_owned()),
    };
    Ok(TotpParams {
        secret,
        algorithm,
        digits,
        period,
        issuer: issuer.or(label_issuer.filter(|i| !i.is_empty())),
        account: (!account.is_empty()).then_some(account),
    })
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

fn hmac_digest(alg: TotpAlgorithm, key: &[u8], msg: &[u8]) -> Result<Vec<u8>> {
    fn run<M: Mac + KeyInit>(key: &[u8], msg: &[u8]) -> Result<Vec<u8>> {
        let mut mac =
            <M as KeyInit>::new_from_slice(key).map_err(|_| Error::invalid("totp_secret"))?;
        mac.update(msg);
        Ok(mac.finalize().into_bytes().to_vec())
    }
    match alg {
        TotpAlgorithm::Sha1 => run::<hmac::Hmac<sha1::Sha1>>(key, msg),
        TotpAlgorithm::Sha256 => run::<hmac::Hmac<sha2::Sha256>>(key, msg),
        TotpAlgorithm::Sha512 => run::<hmac::Hmac<sha2::Sha512>>(key, msg),
    }
}

impl TotpParams {
    /// The code valid at `unix_seconds`.
    pub fn code_at(&self, unix_seconds: u64) -> Result<TotpCode> {
        let period = u64::from(self.period.max(1));
        let counter = unix_seconds / period;
        let digest = hmac_digest(self.algorithm, &self.secret, &counter.to_be_bytes())?;
        // Dynamic truncation (RFC 4226 §5.3).
        let offset = usize::from(digest.last().copied().unwrap_or(0) & 0x0F);
        let slice = digest
            .get(offset..offset + 4)
            .ok_or_else(|| Error::invalid("totp_secret"))?;
        let bin = u32::from_be_bytes([slice[0] & 0x7F, slice[1], slice[2], slice[3]]);
        let modulus = 10u32.pow(self.digits.clamp(6, 8));
        let code = format!(
            "{:0width$}",
            bin % modulus,
            width = self.digits.clamp(6, 8) as usize
        );
        Ok(TotpCode {
            code,
            period: self.period,
            remaining: (period - unix_seconds % period) as u32,
        })
    }
}

/// The current code for `seed`.
pub fn totp_now(seed: &str) -> Result<TotpCode> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    totp_at(seed, now)
}

/// The code for `seed` at `unix_seconds`.
pub fn totp_at(seed: &str, unix_seconds: u64) -> Result<TotpCode> {
    parse(seed)?.code_at(unix_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b32(bytes: &[u8]) -> String {
        data_encoding::BASE32_NOPAD.encode(bytes)
    }

    const SEED20: &[u8] = b"12345678901234567890";
    const SEED32: &[u8] = b"12345678901234567890123456789012";
    const SEED64: &[u8] = b"1234567890123456789012345678901234567890123456789012345678901234";

    /// RFC 6238 Appendix B.
    #[test]
    fn rfc6238_vectors() {
        let vectors: [(u64, &str, &str, &str); 6] = [
            (59, "94287082", "46119246", "90693936"),
            (1_111_111_109, "07081804", "68084774", "25091201"),
            (1_111_111_111, "14050471", "67062674", "99943326"),
            (1_234_567_890, "89005924", "91819424", "93441116"),
            (2_000_000_000, "69279037", "90698825", "38618901"),
            (20_000_000_000, "65353130", "77737706", "47863826"),
        ];
        for (t, sha1, sha256, sha512) in vectors {
            let u1 = format!("otpauth://totp/x?secret={}&digits=8", b32(SEED20));
            let u256 = format!(
                "otpauth://totp/x?secret={}&digits=8&algorithm=SHA256",
                b32(SEED32)
            );
            let u512 = format!(
                "otpauth://totp/x?secret={}&digits=8&algorithm=SHA512",
                b32(SEED64)
            );
            assert_eq!(totp_at(&u1, t).unwrap().code, sha1, "sha1 t={t}");
            assert_eq!(totp_at(&u256, t).unwrap().code, sha256, "sha256 t={t}");
            assert_eq!(totp_at(&u512, t).unwrap().code, sha512, "sha512 t={t}");
        }
    }

    #[test]
    fn six_digit_and_remaining() {
        let secret = b32(SEED20);
        let c = totp_at(&secret, 59).unwrap();
        assert_eq!(c.code, "287082");
        assert_eq!(c.period, 30);
        assert_eq!(c.remaining, 1);
        let c = totp_at(&secret, 60).unwrap();
        assert_eq!(c.remaining, 30);
        let now = totp_now(&secret).unwrap();
        assert_eq!(now.code.len(), 6);
        assert!((1..=30).contains(&now.remaining));
    }

    #[test]
    fn bare_secret_spellings() {
        let canonical = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        let expected = totp_at(canonical, 1_234_567_890).unwrap();
        for s in [
            "gezd gnbv gy3t qojq gezd gnbv gy3t qojq",
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ====",
            "  gezdgnbvgy3tqojqgezdgnbvgy3tqojq\n",
            "GEZD-GNBV-GY3T-QOJQ-GEZD-GNBV-GY3T-QOJQ",
        ] {
            assert_eq!(totp_at(s, 1_234_567_890).unwrap(), expected, "{s}");
        }
        // Length not a multiple of 8 and non-zero trailing bits are accepted.
        assert!(totp_at("JBSWY3DPEHPK3PXPJBSWY3DPEX", 0).is_ok());
        assert!(totp_at("JBSWY3DPEHPK3PXP", 0).is_ok());
    }

    #[test]
    fn otpauth_parameters() {
        let p = parse(
            "otpauth://totp/ACME%20Co:john.doe@example.com?secret=HXDMVJECJJWSRB3HWIZR4IFUGFTMXBOZ&issuer=ACME%20Co&algorithm=SHA256&digits=7&period=60",
        )
        .unwrap();
        assert_eq!(p.algorithm, TotpAlgorithm::Sha256);
        assert_eq!(p.digits, 7);
        assert_eq!(p.period, 60);
        assert_eq!(p.issuer.as_deref(), Some("ACME Co"));
        assert_eq!(p.account.as_deref(), Some("john.doe@example.com"));
        let c = p.code_at(120).unwrap();
        assert_eq!(c.code.len(), 7);
        assert_eq!(c.period, 60);
        assert_eq!(c.remaining, 60);

        let p = parse("OTPAUTH://TOTP/Example:alice?secret=JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(p.issuer.as_deref(), Some("Example"));
        assert_eq!(p.account.as_deref(), Some("alice"));
        assert_eq!((p.digits, p.period), (6, 30));
        assert_eq!(p.algorithm, TotpAlgorithm::Sha1);
        assert!(!format!("{p:?}").contains("secret: "));
    }

    #[test]
    fn rejects_invalid_seeds() {
        for s in [
            "",
            "   ",
            "not base32 !!!",
            "A",
            "otpauth://totp/x",
            "otpauth://totp/x?secret=",
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=5",
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=9",
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=0",
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=abc",
            "otpauth://foo/x?secret=JBSWY3DPEHPK3PXP",
        ] {
            assert!(
                matches!(totp_now(s), Err(Error::InvalidInput(_))),
                "{s}: {:?}",
                totp_now(s)
            );
        }
        assert!(matches!(
            totp_now("otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP&counter=1"),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            totp_now("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&algorithm=MD5"),
            Err(Error::Unsupported(_))
        ));
    }
}
