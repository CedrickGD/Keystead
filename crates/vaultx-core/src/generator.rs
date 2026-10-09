//! Password and passphrase generator.
//!
//! All randomness comes from the OS CSPRNG; indices are drawn with
//! rejection sampling ([`crypto::random_below`]) so there is no modulo bias.
//! Passphrases use the EFF large wordlist (7776 words, ≈12.9 bit per word).

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto;
use crate::error::{Error, Result};

const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const DIGITS: &str = "0123456789";
/// Symbols accepted by virtually every site (no quotes, backslash or space).
const SYMBOLS: &str = "!#$%&*+-=?@^_";
/// Characters excluded by `avoid_ambiguous`.
const AMBIGUOUS: &str = "Il1O0";

pub const PASSWORD_MIN_LEN: u32 = 5;
pub const PASSWORD_MAX_LEN: u32 = 128;
pub const PASSPHRASE_MIN_WORDS: u32 = 3;
pub const PASSPHRASE_MAX_WORDS: u32 = 20;
const SEPARATOR_MAX_CHARS: usize = 8;

static WORDLIST_RAW: &str = include_str!("wordlist/eff_large.txt");

/// The embedded EFF large wordlist.
pub fn wordlist() -> &'static [&'static str] {
    static WORDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| {
        WORDLIST_RAW
            .lines()
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .collect()
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeneratorKind {
    #[default]
    Password,
    Passphrase,
}

/// Generator settings. Missing JSON fields take the defaults, so partial
/// options (e.g. from the browser extension) are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeneratorOptions {
    pub kind: GeneratorKind,
    /// Password length, 5..=128.
    pub length: u32,
    pub uppercase: bool,
    pub lowercase: bool,
    pub digits: bool,
    pub symbols: bool,
    /// Minimum number of digits (only if `digits`).
    pub min_digits: u32,
    /// Minimum number of symbols (only if `symbols`).
    pub min_symbols: u32,
    /// Exclude the look-alike characters `I l 1 O 0`.
    pub avoid_ambiguous: bool,
    /// Passphrase word count, 3..=20.
    pub words: u32,
    pub separator: String,
    /// Capitalise the first letter of each word.
    pub capitalize: bool,
    /// Append one random digit to one random word.
    pub include_number: bool,
}

impl Default for GeneratorOptions {
    fn default() -> Self {
        GeneratorOptions {
            kind: GeneratorKind::Password,
            length: 20,
            uppercase: true,
            lowercase: true,
            digits: true,
            symbols: true,
            min_digits: 1,
            min_symbols: 1,
            avoid_ambiguous: false,
            words: 5,
            separator: "-".to_owned(),
            capitalize: true,
            include_number: true,
        }
    }
}

/// Generates a password or passphrase.
pub fn generate(opts: &GeneratorOptions) -> Result<String> {
    match opts.kind {
        GeneratorKind::Password => generate_password(opts),
        GeneratorKind::Passphrase => generate_passphrase(opts),
    }
}

fn charset(set: &str, avoid_ambiguous: bool) -> Vec<char> {
    set.chars()
        .filter(|c| !avoid_ambiguous || !AMBIGUOUS.contains(*c))
        .collect()
}

fn pick<T: Copy>(items: &[T]) -> Result<T> {
    let len = u32::try_from(items.len()).map_err(|_| Error::invalid("charset"))?;
    let idx = crypto::random_below(len)?;
    items
        .get(idx as usize)
        .copied()
        .ok_or_else(|| Error::invalid("charset"))
}

fn shuffle<T>(v: &mut [T]) -> Result<()> {
    // Fisher–Yates.
    for i in (1..v.len()).rev() {
        let bound = u32::try_from(i + 1).map_err(|_| Error::invalid("length"))?;
        let j = crypto::random_below(bound)? as usize;
        v.swap(i, j);
    }
    Ok(())
}

fn generate_password(opts: &GeneratorOptions) -> Result<String> {
    if !(PASSWORD_MIN_LEN..=PASSWORD_MAX_LEN).contains(&opts.length) {
        return Err(Error::invalid("length"));
    }
    // (character set, minimum count) per enabled class.
    let mut classes: Vec<(Vec<char>, u32)> = Vec::new();
    if opts.uppercase {
        classes.push((charset(UPPER, opts.avoid_ambiguous), 1));
    }
    if opts.lowercase {
        classes.push((charset(LOWER, opts.avoid_ambiguous), 1));
    }
    if opts.digits {
        classes.push((charset(DIGITS, opts.avoid_ambiguous), opts.min_digits));
    }
    if opts.symbols {
        classes.push((charset(SYMBOLS, opts.avoid_ambiguous), opts.min_symbols));
    }
    if classes.is_empty() {
        return Err(Error::invalid("no_character_set"));
    }
    let required: u64 = classes.iter().map(|(_, min)| u64::from(*min)).sum();
    if required > u64::from(opts.length) {
        return Err(Error::invalid("minimums_exceed_length"));
    }
    let all: Vec<char> = classes.iter().flat_map(|(set, _)| set.iter().copied()).collect();

    let mut chars: Zeroizing<Vec<char>> = Zeroizing::new(Vec::with_capacity(opts.length as usize));
    for (set, min) in &classes {
        for _ in 0..*min {
            chars.push(pick(set)?);
        }
    }
    while chars.len() < opts.length as usize {
        chars.push(pick(&all)?);
    }
    shuffle(&mut chars)?;
    Ok(chars.iter().collect())
}

fn capitalize_word(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn generate_passphrase(opts: &GeneratorOptions) -> Result<String> {
    if !(PASSPHRASE_MIN_WORDS..=PASSPHRASE_MAX_WORDS).contains(&opts.words) {
        return Err(Error::invalid("words"));
    }
    if opts.separator.chars().count() > SEPARATOR_MAX_CHARS {
        return Err(Error::invalid("separator"));
    }
    let list = wordlist();
    let mut words: Vec<Zeroizing<String>> = Vec::with_capacity(opts.words as usize);
    for _ in 0..opts.words {
        let w = pick(list)?;
        words.push(Zeroizing::new(if opts.capitalize {
            capitalize_word(w)
        } else {
            w.to_owned()
        }));
    }
    if opts.include_number {
        let len = u32::try_from(words.len()).map_err(|_| Error::invalid("words"))?;
        let idx = crypto::random_below(len)? as usize;
        let digit = crypto::random_below(10)?;
        if let Some(w) = words.get_mut(idx) {
            w.push(char::from(b'0' + digit as u8));
        }
    }
    let joined = words
        .iter()
        .map(|w| w.as_str())
        .collect::<Vec<_>>()
        .join(&opts.separator);
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn wordlist_has_7776_unique_words() {
        let list = wordlist();
        assert_eq!(list.len(), 7776);
        let unique: HashSet<_> = list.iter().collect();
        assert_eq!(unique.len(), 7776);
        assert!(list.iter().all(|w| w
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '-')));
        assert_eq!(list.first(), Some(&"abacus"));
        assert_eq!(list.last(), Some(&"zoom"));
    }

    #[test]
    fn defaults_match_contract() {
        let o = GeneratorOptions::default();
        assert_eq!(o.kind, GeneratorKind::Password);
        assert_eq!(o.length, 20);
        assert!(o.uppercase && o.lowercase && o.digits && o.symbols);
        assert_eq!((o.min_digits, o.min_symbols), (1, 1));
        assert!(!o.avoid_ambiguous);
        assert_eq!(o.words, 5);
        assert_eq!(o.separator, "-");
        assert!(o.capitalize && o.include_number);
    }

    #[test]
    fn partial_json_uses_defaults() {
        let o: GeneratorOptions = serde_json::from_str(r#"{"length":32,"symbols":false}"#).unwrap();
        assert_eq!(o.length, 32);
        assert!(!o.symbols);
        assert!(o.uppercase);
        assert_eq!(o.words, 5);
        let o: GeneratorOptions = serde_json::from_str(r#"{"kind":"passphrase"}"#).unwrap();
        assert_eq!(o.kind, GeneratorKind::Passphrase);
        let o: GeneratorOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(o, GeneratorOptions::default());
        let v = serde_json::to_value(GeneratorOptions::default()).unwrap();
        assert_eq!(v["minDigits"], 1);
        assert_eq!(v["includeNumber"], true);
        assert_eq!(v["kind"], "password");
    }

    #[test]
    fn password_respects_options() {
        for _ in 0..200 {
            let o = GeneratorOptions {
                length: 12,
                min_digits: 3,
                min_symbols: 2,
                ..Default::default()
            };
            let p = generate(&o).unwrap();
            assert_eq!(p.chars().count(), 12);
            assert!(p.chars().filter(char::is_ascii_digit).count() >= 3);
            assert!(p.chars().filter(|c| SYMBOLS.contains(*c)).count() >= 2);
            assert!(p.chars().any(|c| c.is_ascii_uppercase()));
            assert!(p.chars().any(|c| c.is_ascii_lowercase()));
        }
        let o = GeneratorOptions {
            length: 128,
            uppercase: false,
            symbols: false,
            avoid_ambiguous: true,
            ..Default::default()
        };
        for _ in 0..20 {
            let p = generate(&o).unwrap();
            assert_eq!(p.len(), 128);
            assert!(p
                .chars()
                .all(|c| (c.is_ascii_lowercase() || c.is_ascii_digit()) && !AMBIGUOUS.contains(c)));
        }
        let digits_only = GeneratorOptions {
            length: 6,
            uppercase: false,
            lowercase: false,
            symbols: false,
            min_digits: 0,
            ..Default::default()
        };
        assert!(generate(&digits_only)
            .unwrap()
            .chars()
            .all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn password_rejects_bad_options() {
        let bad = [
            GeneratorOptions {
                length: 4,
                ..Default::default()
            },
            GeneratorOptions {
                length: 129,
                ..Default::default()
            },
            GeneratorOptions {
                uppercase: false,
                lowercase: false,
                digits: false,
                symbols: false,
                ..Default::default()
            },
            GeneratorOptions {
                length: 5,
                min_digits: 3,
                min_symbols: 3,
                ..Default::default()
            },
            GeneratorOptions {
                min_digits: u32::MAX,
                min_symbols: u32::MAX,
                ..Default::default()
            },
        ];
        for o in bad {
            assert!(matches!(generate(&o), Err(Error::InvalidInput(_))), "{o:?}");
        }
        // Minimums of disabled classes are ignored.
        let o = GeneratorOptions {
            length: 5,
            digits: false,
            min_digits: 50,
            ..Default::default()
        };
        assert!(generate(&o).is_ok());
    }

    #[test]
    fn passwords_are_not_repeated_and_roughly_uniform() {
        let o = GeneratorOptions {
            length: 64,
            uppercase: false,
            lowercase: false,
            symbols: false,
            min_digits: 0,
            ..Default::default()
        };
        let mut counts = [0u32; 10];
        let mut seen = HashSet::new();
        for _ in 0..200 {
            let p = generate(&o).unwrap();
            assert!(seen.insert(p.clone()));
            for c in p.chars() {
                counts[(c as u8 - b'0') as usize] += 1;
            }
        }
        // 12800 samples, expected 1280 each; allow a generous ±20 %.
        for c in counts {
            assert!((1024..=1536).contains(&c), "{counts:?}");
        }
    }

    #[test]
    fn passphrase() {
        let o = GeneratorOptions {
            kind: GeneratorKind::Passphrase,
            words: 6,
            separator: " ".into(),
            capitalize: false,
            include_number: false,
            ..Default::default()
        };
        let p = generate(&o).unwrap();
        let parts: Vec<&str> = p.split(' ').collect();
        assert_eq!(parts.len(), 6);
        let list: HashSet<&str> = wordlist().iter().copied().collect();
        assert!(parts.iter().all(|w| list.contains(w)));

        let o = GeneratorOptions {
            kind: GeneratorKind::Passphrase,
            ..Default::default()
        };
        for _ in 0..50 {
            let p = generate(&o).unwrap();
            // Words may themselves contain '-', so count capitals instead.
            let caps = p.chars().filter(char::is_ascii_uppercase).count();
            assert!(caps >= 5, "{p}");
            assert_eq!(p.chars().filter(char::is_ascii_digit).count(), 1, "{p}");
        }
        for words in [2, 21] {
            let o = GeneratorOptions {
                kind: GeneratorKind::Passphrase,
                words,
                ..Default::default()
            };
            assert!(generate(&o).is_err());
        }
        let o = GeneratorOptions {
            kind: GeneratorKind::Passphrase,
            separator: "-----------".into(),
            ..Default::default()
        };
        assert!(generate(&o).is_err());
    }
}
