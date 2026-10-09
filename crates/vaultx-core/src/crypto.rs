//! Cryptographic primitives: Argon2id key derivation, XChaCha20-Poly1305
//! sealing, randomness and base64 helpers.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// Length of symmetric keys (vault key, key-encryption keys).
pub const KEY_LEN: usize = 32;
/// XChaCha20-Poly1305 nonce length.
pub const NONCE_LEN: usize = 24;
/// Argon2 salt length used for new vaults.
pub const SALT_LEN: usize = 16;

/// A 256-bit secret key that is wiped from memory when dropped.
pub type SecretKey = Zeroizing<[u8; KEY_LEN]>;

/// Argon2id cost parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Default for KdfParams {
    /// m = 64 MiB, t = 3, p = 4.
    fn default() -> Self {
        KdfParams {
            memory_kib: 64 * 1024,
            iterations: 3,
            parallelism: 4,
        }
    }
}

impl KdfParams {
    /// Upper bounds accepted from files (guards against parameters that would
    /// make unlocking a denial of service).
    pub const MAX_MEMORY_KIB: u32 = 1024 * 1024;
    pub const MAX_ITERATIONS: u32 = 20;
    pub const MAX_PARALLELISM: u32 = 16;

    /// Very cheap parameters for unit tests. Never use for real vaults.
    pub fn insecure_for_tests() -> Self {
        KdfParams {
            memory_kib: 64,
            iterations: 1,
            parallelism: 1,
        }
    }

    /// Checks that the parameters are within the supported range.
    pub fn validate(&self) -> Result<()> {
        let ok = (1..=Self::MAX_PARALLELISM).contains(&self.parallelism)
            && (1..=Self::MAX_ITERATIONS).contains(&self.iterations)
            && self.memory_kib <= Self::MAX_MEMORY_KIB
            && self.memory_kib >= 8 * self.parallelism;
        if ok {
            Ok(())
        } else {
            Err(Error::invalid("kdf_params"))
        }
    }
}

/// Derives a 32-byte key from a password with Argon2id.
pub fn derive_key(password: &[u8], salt: &[u8], params: &KdfParams) -> Result<SecretKey> {
    params.validate()?;
    if salt.len() < argon2::MIN_SALT_LEN {
        return Err(Error::invalid("kdf_salt"));
    }
    let argon_params = Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(KEY_LEN),
    )
    .map_err(|e| Error::invalid(format!("kdf_params: {e}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(password, salt, out.as_mut_slice())
        .map_err(|e| Error::Io(std::io::Error::other(format!("key derivation failed: {e}"))))?;
    Ok(out)
}

/// Fills `buf` with cryptographically secure random bytes from the OS.
pub fn fill_random(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|e| {
        Error::Io(std::io::Error::other(format!(
            "OS random number generator failed: {e}"
        )))
    })
}

/// `n` random bytes.
pub fn random_bytes(n: usize) -> Result<Vec<u8>> {
    let mut v = vec![0u8; n];
    fill_random(&mut v)?;
    Ok(v)
}

/// A random fixed-size array.
pub fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut a = [0u8; N];
    fill_random(&mut a)?;
    Ok(a)
}

/// A fresh random 256-bit key.
pub fn random_key() -> Result<SecretKey> {
    let mut k = Zeroizing::new([0u8; KEY_LEN]);
    fill_random(k.as_mut_slice())?;
    Ok(k)
}

/// Uniformly distributed random integer in `0..bound` (rejection sampling,
/// no modulo bias). `bound` must be > 0.
pub fn random_below(bound: u32) -> Result<u32> {
    if bound == 0 {
        return Err(Error::invalid("random_bound"));
    }
    // Largest multiple of `bound` that fits into u32::MAX + 1.
    let zone = u32::MAX - (u32::MAX - bound + 1) % bound;
    loop {
        let mut b = [0u8; 4];
        fill_random(&mut b)?;
        let v = u32::from_le_bytes(b);
        if v <= zone {
            return Ok(v % bound);
        }
    }
}

/// Ciphertext produced by [`seal`] (the Poly1305 tag is appended to `ciphertext`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

/// Encrypts and authenticates `plaintext` with XChaCha20-Poly1305 under a
/// fresh random nonce.
pub fn seal(key: &[u8; KEY_LEN], plaintext: &[u8], aad: &[u8]) -> Result<Sealed> {
    let nonce = random_array::<NONCE_LEN>()?;
    let cipher = XChaCha20Poly1305::new(&(*key).into());
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| Error::Io(std::io::Error::other("encryption failed")))?;
    Ok(Sealed { nonce, ciphertext })
}

/// Verifies and decrypts data produced by [`seal`].
///
/// Returns `Error::Corrupt` if the nonce has the wrong size or the
/// authentication tag does not verify (wrong key, wrong AAD or tampered
/// data). Callers that unwrap keys translate this into `WrongPassword`.
pub fn open(
    key: &[u8; KEY_LEN],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| Error::corrupt("invalid nonce length"))?;
    let cipher = XChaCha20Poly1305::new(&(*key).into());
    cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| Error::corrupt("authentication failed"))
}

/// Opens a sealed 32-byte key.
pub fn open_key(
    kek: &[u8; KEY_LEN],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<SecretKey> {
    let plain = open(kek, nonce, ciphertext, aad)?;
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    if plain.len() != KEY_LEN {
        return Err(Error::corrupt("wrapped key has wrong length"));
    }
    key.copy_from_slice(&plain);
    Ok(key)
}

/// Standard base64 with padding.
pub fn b64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Decodes standard base64 (with or without padding).
pub fn b64_decode(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(s))
        .map_err(|_| Error::corrupt("invalid base64"))
}

/// Constant-time byte slice comparison.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_params() {
        let p = KdfParams::default();
        assert_eq!((p.memory_kib, p.iterations, p.parallelism), (65536, 3, 4));
        assert!(p.validate().is_ok());
        assert!(KdfParams::insecure_for_tests().validate().is_ok());
    }

    #[test]
    fn rejects_absurd_params() {
        let bad = [
            KdfParams {
                memory_kib: 2 * 1024 * 1024,
                iterations: 3,
                parallelism: 4,
            },
            KdfParams {
                memory_kib: 65536,
                iterations: 21,
                parallelism: 4,
            },
            KdfParams {
                memory_kib: 65536,
                iterations: 3,
                parallelism: 17,
            },
            KdfParams {
                memory_kib: 65536,
                iterations: 0,
                parallelism: 4,
            },
            KdfParams {
                memory_kib: 65536,
                iterations: 3,
                parallelism: 0,
            },
            KdfParams {
                memory_kib: 16,
                iterations: 3,
                parallelism: 4,
            },
        ];
        for p in bad {
            assert!(p.validate().is_err(), "{p:?}");
            assert!(derive_key(b"pw", &[0u8; 16], &p).is_err());
        }
    }

    #[test]
    fn derive_is_deterministic_and_salted() {
        let p = KdfParams::insecure_for_tests();
        let a = derive_key(b"password", &[1u8; 16], &p).unwrap();
        let b = derive_key(b"password", &[1u8; 16], &p).unwrap();
        let c = derive_key(b"password", &[2u8; 16], &p).unwrap();
        let d = derive_key(b"Password", &[1u8; 16], &p).unwrap();
        assert_eq!(*a, *b);
        assert_ne!(*a, *c);
        assert_ne!(*a, *d);
        assert!(derive_key(b"pw", &[0u8; 4], &p).is_err());
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn argon2id_known_answers() {
        // Expected values computed independently with OpenSSL's Argon2id
        // (Python `cryptography` 50): password "password", salt "somesalt".
        let k = derive_key(
            b"password",
            b"somesalt",
            &KdfParams {
                memory_kib: 64,
                iterations: 1,
                parallelism: 1,
            },
        )
        .unwrap();
        assert_eq!(
            hex(k.as_slice()),
            "729c7a54441bc13559bdca71348c4e554599e719c08a952601ed5c83618c1bbd"
        );
        let k = derive_key(
            b"password",
            b"somesalt",
            &KdfParams {
                memory_kib: 256,
                iterations: 2,
                parallelism: 4,
            },
        )
        .unwrap();
        assert_eq!(
            hex(k.as_slice()),
            "be29d1c497593959cd701e5ceefe8a6fbda26d9b3892c08cff261e0a94bab2b1"
        );
    }

    #[test]
    fn seal_open_round_trip_and_tamper() {
        let key = random_key().unwrap();
        let sealed = seal(&key, b"secret data", b"aad").unwrap();
        let plain = open(&key, &sealed.nonce, &sealed.ciphertext, b"aad").unwrap();
        assert_eq!(plain.as_slice(), b"secret data");

        assert!(open(&key, &sealed.nonce, &sealed.ciphertext, b"other").is_err());
        let mut ct = sealed.ciphertext.clone();
        ct[0] ^= 1;
        assert!(open(&key, &sealed.nonce, &ct, b"aad").is_err());
        let mut nonce = sealed.nonce;
        nonce[5] ^= 1;
        assert!(open(&key, &nonce, &sealed.ciphertext, b"aad").is_err());
        assert!(open(&key, &nonce[..10], &sealed.ciphertext, b"aad").is_err());
        let other = random_key().unwrap();
        assert!(open(&other, &sealed.nonce, &sealed.ciphertext, b"aad").is_err());
    }

    #[test]
    fn nonces_are_fresh() {
        let key = random_key().unwrap();
        let a = seal(&key, b"x", b"").unwrap();
        let b = seal(&key, b"x", b"").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn random_below_is_in_range_and_covers() {
        let mut seen = [false; 7];
        for _ in 0..500 {
            let v = random_below(7).unwrap();
            assert!(v < 7);
            seen[v as usize] = true;
        }
        assert!(seen.iter().all(|s| *s));
        assert_eq!(random_below(1).unwrap(), 0);
        assert!(random_below(0).is_err());
        let _ = random_below(u32::MAX).unwrap();
    }

    #[test]
    fn base64_helpers() {
        assert_eq!(b64_encode(b"hi"), "aGk=");
        assert_eq!(b64_decode("aGk=").unwrap(), b"hi");
        assert_eq!(b64_decode("aGk").unwrap(), b"hi");
        assert!(b64_decode("!!").is_err());
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
