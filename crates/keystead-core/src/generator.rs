//! Password and passphrase generator, plus the generator presets.
//!
//! All randomness comes from the OS CSPRNG; indices are drawn with
//! rejection sampling ([`crypto::random_below`]) so there is no modulo bias.
//! Passwords meet their minimum counts by construction: the required
//! characters are drawn first (each from its own class), the rest from the
//! union of the enabled classes, then a Fisher–Yates shuffle mixes them.
//! Passphrases use an embedded word list ([`Wordlist`]): the EFF large list
//! (7776 words, ≈12.9 bit per word) or a German list derived from dys2p's
//! `de-7776` (7724 words, ≈12.9 bit per word) – sources and licences in
//! `wordlist/README.md`.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto;
use crate::error::{Error, Result};

const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const DIGITS: &str = "0123456789";
/// The default symbol set: symbols accepted by virtually every site (no
/// quotes, backslash or space).
pub const DEFAULT_SYMBOLS: &str = "!#$%&*+-=?@^_";
/// Characters excluded by `avoid_ambiguous`.
const AMBIGUOUS: &str = "Il1O0";

pub const PASSWORD_MIN_LEN: u32 = 4;
pub const PASSWORD_MAX_LEN: u32 = 128;
pub const PASSPHRASE_MIN_WORDS: u32 = 3;
pub const PASSPHRASE_MAX_WORDS: u32 = 20;
const SEPARATOR_MAX_CHARS: usize = 8;
/// Longest accepted `symbol_set` / `exclude` input (characters, before
/// duplicates are removed).
pub const CHARSET_INPUT_MAX_CHARS: usize = 256;
/// Longest preset name (characters, after trimming).
pub const PRESET_NAME_MAX_CHARS: usize = 40;
/// Longest preset id (ASCII letters, digits, `-`, `_`).
pub const PRESET_ID_MAX_CHARS: usize = 64;

static EFF_LARGE_RAW: &str = include_str!("wordlist/eff_large.txt");
static DE_DYS2P_RAW: &str = include_str!("wordlist/de_dys2p.txt");

fn parse_wordlist(raw: &'static str) -> Vec<&'static str> {
    raw.lines()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .collect()
}

/// The word list of a passphrase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wordlist {
    /// EFF large word list (English, 7776 words).
    #[default]
    En,
    /// German list derived from dys2p `de-7776` (7724 words).
    De,
}

impl Wordlist {
    pub const ALL: [Wordlist; 2] = [Wordlist::En, Wordlist::De];

    /// The words of this list (lowercase ASCII, unique, file order).
    pub fn words(self) -> &'static [&'static str] {
        static EN: OnceLock<Vec<&'static str>> = OnceLock::new();
        static DE: OnceLock<Vec<&'static str>> = OnceLock::new();
        match self {
            Wordlist::En => EN.get_or_init(|| parse_wordlist(EFF_LARGE_RAW)),
            Wordlist::De => DE.get_or_init(|| parse_wordlist(DE_DYS2P_RAW)),
        }
    }

    /// Entropy of one word drawn from this list: log2 of its real size.
    pub fn bits_per_word(self) -> f64 {
        (self.words().len() as f64).log2()
    }
}

/// The embedded EFF large wordlist (same as `Wordlist::En.words()`).
pub fn wordlist() -> &'static [&'static str] {
    Wordlist::En.words()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeneratorKind {
    #[default]
    Password,
    Passphrase,
}

/// Generator settings. Missing JSON fields take the defaults, so partial
/// options (e.g. from the browser extension or an older app) are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeneratorOptions {
    pub kind: GeneratorKind,
    /// Password length, 4..=128.
    pub length: u32,
    pub uppercase: bool,
    pub lowercase: bool,
    pub digits: bool,
    pub symbols: bool,
    /// Minimum number of upper-case letters (only if `uppercase`).
    pub min_uppercase: u32,
    /// Minimum number of lower-case letters (only if `lowercase`).
    pub min_lowercase: u32,
    /// Minimum number of digits (only if `digits`).
    pub min_digits: u32,
    /// Minimum number of symbols (only if `symbols`).
    pub min_symbols: u32,
    /// The symbols to draw from: printable ASCII that is neither a letter,
    /// a digit nor a space; duplicates are ignored. Must not be empty while
    /// `symbols` is on.
    pub symbol_set: String,
    /// Characters removed from every class (any characters; those in no
    /// class have no effect).
    pub exclude: String,
    /// Exclude the look-alike characters `I l 1 O 0`.
    pub avoid_ambiguous: bool,
    /// The first character is a letter.
    pub start_with_letter: bool,
    /// Passphrase word count, 3..=20.
    pub words: u32,
    pub separator: String,
    /// Capitalise the first letter of each word.
    pub capitalize: bool,
    /// Append one random digit to one random word.
    pub include_number: bool,
    /// Word list of a passphrase.
    pub wordlist: Wordlist,
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
            min_uppercase: 1,
            min_lowercase: 1,
            min_digits: 1,
            min_symbols: 1,
            symbol_set: DEFAULT_SYMBOLS.to_owned(),
            exclude: String::new(),
            avoid_ambiguous: false,
            start_with_letter: false,
            words: 5,
            separator: "-".to_owned(),
            capitalize: true,
            include_number: true,
            wordlist: Wordlist::En,
        }
    }
}

impl GeneratorOptions {
    /// Checks the options exactly as [`generate`] does, without generating.
    pub fn validate(&self) -> Result<()> {
        self.normalized().map(|_| ())
    }

    /// A validated copy for storing (e.g. in a preset). For passwords the
    /// `symbol_set` and `exclude` lose duplicate characters (first
    /// occurrence kept). Only the fields of `kind` are checked, as in
    /// [`generate`]; the others are kept unchanged.
    pub fn normalized(&self) -> Result<GeneratorOptions> {
        let mut out = self.clone();
        match self.kind {
            GeneratorKind::Password => {
                let plan = password_plan(self)?;
                out.symbol_set = plan.symbol_set;
                out.exclude = plan.exclude;
            }
            GeneratorKind::Passphrase => check_passphrase(self)?,
        }
        Ok(out)
    }

    /// Parses options field by field: a missing field or one with an
    /// invalid value gets its default; unknown keys are ignored. Used for
    /// presets in `settings.json` (a value a later version wrote must not
    /// cost the whole preset). Not validated – see [`Self::normalized`].
    pub fn from_json_lenient(value: &serde_json::Value) -> GeneratorOptions {
        let serde_json::Value::Object(input) = value else {
            return GeneratorOptions::default();
        };
        let Ok(serde_json::Value::Object(mut base)) =
            serde_json::to_value(GeneratorOptions::default())
        else {
            return GeneratorOptions::default();
        };
        for (key, v) in input {
            if !base.contains_key(key) {
                continue;
            }
            let mut candidate = base.clone();
            candidate.insert(key.clone(), v.clone());
            let candidate = serde_json::Value::Object(candidate);
            if serde_json::from_value::<GeneratorOptions>(candidate.clone()).is_ok() {
                if let serde_json::Value::Object(map) = candidate {
                    base = map;
                }
            }
        }
        serde_json::from_value(serde_json::Value::Object(base)).unwrap_or_default()
    }
}

/// Generates a password or passphrase.
pub fn generate(opts: &GeneratorOptions) -> Result<String> {
    match opts.kind {
        GeneratorKind::Password => generate_password(opts),
        GeneratorKind::Passphrase => generate_passphrase(opts),
    }
}

/// Validates a symbol set: printable ASCII that is neither a letter, a
/// digit nor a space (`invalid_input:symbol_set` otherwise, also above
/// [`CHARSET_INPUT_MAX_CHARS`]). Returns it without duplicates, first
/// occurrence kept. Empty input gives an empty set.
pub fn normalize_symbol_set(input: &str) -> Result<String> {
    if input.chars().count() > CHARSET_INPUT_MAX_CHARS {
        return Err(Error::invalid("symbol_set"));
    }
    let mut out = String::new();
    for c in input.chars() {
        if !c.is_ascii_graphic() || c.is_ascii_alphanumeric() {
            return Err(Error::invalid("symbol_set"));
        }
        if !out.contains(c) {
            out.push(c);
        }
    }
    Ok(out)
}

/// The `exclude` input without duplicates (first occurrence kept);
/// `invalid_input:exclude` above [`CHARSET_INPUT_MAX_CHARS`].
fn normalize_exclude(input: &str) -> Result<String> {
    if input.chars().count() > CHARSET_INPUT_MAX_CHARS {
        return Err(Error::invalid("exclude"));
    }
    let mut out = String::new();
    for c in input.chars() {
        if !out.contains(c) {
            out.push(c);
        }
    }
    Ok(out)
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

/// The validated character classes of a password.
struct PasswordPlan {
    /// (characters, minimum) per enabled class.
    classes: Vec<(Vec<char>, u32)>,
    /// Union of all enabled classes.
    all: Vec<char>,
    /// Union of the enabled letter classes; set only with
    /// `start_with_letter`.
    letters: Vec<char>,
    /// `start_with_letter` without any letter minimum: one letter is drawn
    /// as an extra required character.
    extra_letter: bool,
    symbol_set: String,
    exclude: String,
}

fn password_plan(opts: &GeneratorOptions) -> Result<PasswordPlan> {
    if !(PASSWORD_MIN_LEN..=PASSWORD_MAX_LEN).contains(&opts.length) {
        return Err(Error::invalid("length"));
    }
    let symbol_set = normalize_symbol_set(&opts.symbol_set)?;
    if opts.symbols && symbol_set.is_empty() {
        return Err(Error::invalid("symbol_set"));
    }
    let exclude = normalize_exclude(&opts.exclude)?;
    let charset = |set: &str| -> Vec<char> {
        set.chars()
            .filter(|c| !exclude.contains(*c))
            .filter(|c| !opts.avoid_ambiguous || !AMBIGUOUS.contains(*c))
            .collect()
    };
    let mut classes: Vec<(Vec<char>, u32)> = Vec::new();
    let mut letters: Vec<char> = Vec::new();
    let mut letter_minimum = false;
    for (enabled, set, min, letter) in [
        (opts.uppercase, UPPER, opts.min_uppercase, true),
        (opts.lowercase, LOWER, opts.min_lowercase, true),
        (opts.digits, DIGITS, opts.min_digits, false),
        (opts.symbols, symbol_set.as_str(), opts.min_symbols, false),
    ] {
        if !enabled {
            // Minimums of disabled classes are ignored (older clients send
            // e.g. `minSymbols: 1` with `symbols: false`).
            continue;
        }
        let chars = charset(set);
        if chars.is_empty() {
            // Everything of an enabled class was excluded.
            return Err(Error::invalid("no_character_set"));
        }
        if letter {
            letters.extend_from_slice(&chars);
            letter_minimum |= min > 0;
        }
        classes.push((chars, min));
    }
    if classes.is_empty() {
        return Err(Error::invalid("no_character_set"));
    }
    if opts.start_with_letter && letters.is_empty() {
        return Err(Error::invalid("start_with_letter"));
    }
    let extra_letter = opts.start_with_letter && !letter_minimum;
    let required: u64 =
        classes.iter().map(|(_, min)| u64::from(*min)).sum::<u64>() + u64::from(extra_letter);
    if required > u64::from(opts.length) {
        return Err(Error::invalid("minimums_exceed_length"));
    }
    let all: Vec<char> = classes
        .iter()
        .flat_map(|(set, _)| set.iter().copied())
        .collect();
    if !opts.start_with_letter {
        letters.clear();
    }
    Ok(PasswordPlan {
        classes,
        all,
        letters,
        extra_letter,
        symbol_set,
        exclude,
    })
}

fn generate_password(opts: &GeneratorOptions) -> Result<String> {
    let plan = password_plan(opts)?;
    let length = opts.length as usize;
    let mut chars: Zeroizing<Vec<char>> = Zeroizing::new(Vec::with_capacity(length));
    for (set, min) in &plan.classes {
        for _ in 0..*min {
            chars.push(pick(set)?);
        }
    }
    if plan.extra_letter {
        chars.push(pick(&plan.letters)?);
    }
    while chars.len() < length {
        chars.push(pick(&plan.all)?);
    }
    shuffle(&mut chars)?;
    if opts.start_with_letter && !chars.first().is_some_and(char::is_ascii_alphabetic) {
        // Swap a uniformly chosen letter to the front: every arrangement of
        // the drawn characters that starts with a letter stays equally
        // likely. There is at least one letter (minimum or extra letter).
        let positions: Zeroizing<Vec<usize>> = Zeroizing::new(
            chars
                .iter()
                .enumerate()
                .filter(|(_, c)| c.is_ascii_alphabetic())
                .map(|(i, _)| i)
                .collect(),
        );
        let pos = pick(&positions)?;
        chars.swap(0, pos);
    }
    Ok(chars.iter().collect())
}

fn capitalize_word(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn check_passphrase(opts: &GeneratorOptions) -> Result<()> {
    if !(PASSPHRASE_MIN_WORDS..=PASSPHRASE_MAX_WORDS).contains(&opts.words) {
        return Err(Error::invalid("words"));
    }
    if opts.separator.chars().count() > SEPARATOR_MAX_CHARS {
        return Err(Error::invalid("separator"));
    }
    Ok(())
}

fn generate_passphrase(opts: &GeneratorOptions) -> Result<String> {
    check_passphrase(opts)?;
    let list = opts.wordlist.words();
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

/// Entropy of a passphrase in bits: `words × log2(list size)`, plus
/// `log2(10 × words)` for the appended digit. An upper bound with an empty
/// separator (word boundaries are then ambiguous). Invalid options →
/// the error of [`generate`].
pub fn passphrase_entropy_bits(opts: &GeneratorOptions) -> Result<f64> {
    check_passphrase(opts)?;
    let words = f64::from(opts.words);
    let mut bits = words * opts.wordlist.bits_per_word();
    if opts.include_number {
        bits += (10.0 * words).log2();
    }
    Ok(bits)
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

/// Id of the built-in preset "Standard" (the defaults).
pub const PRESET_STANDARD: &str = "standard";
/// Id of the built-in preset "Letters and digits only".
pub const PRESET_ALNUM: &str = "alnum";
/// Id of the built-in preset "PIN" (6 digits).
pub const PRESET_PIN: &str = "pin";
/// Id of the built-in preset "Long and strong" (32 characters).
pub const PRESET_LONG: &str = "long";
/// Id of the built-in preset "Passphrase".
pub const PRESET_PASSPHRASE: &str = "passphrase";
/// Ids of the built-in presets, in display order. User presets cannot use
/// them.
pub const BUILTIN_PRESET_IDS: [&str; 5] = [
    PRESET_STANDARD,
    PRESET_ALNUM,
    PRESET_PIN,
    PRESET_LONG,
    PRESET_PASSPHRASE,
];

/// Named generator options. Built-in presets live in code
/// ([`builtin_presets`]; their `name` is an English fallback – the UIs show
/// their own translation by id), user presets in `settings.json`
/// (`Settings::generator_presets`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeneratorPreset {
    pub id: String,
    pub name: String,
    pub options: GeneratorOptions,
}

impl GeneratorPreset {
    /// A validated copy for storing as a user preset: the name trimmed,
    /// 1..=40 characters ([`validate_preset_name`]); the id 1..=64 ASCII
    /// letters, digits, `-` or `_` and no built-in id
    /// (`invalid_input:preset_id`); the options normalised
    /// ([`GeneratorOptions::normalized`]).
    pub fn normalized(&self) -> Result<GeneratorPreset> {
        let id = self.id.trim();
        if id.is_empty()
            || id.len() > PRESET_ID_MAX_CHARS
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            || is_builtin_preset(id)
        {
            return Err(Error::invalid("preset_id"));
        }
        Ok(GeneratorPreset {
            id: id.to_owned(),
            name: validate_preset_name(&self.name)?,
            options: self.options.normalized()?,
        })
    }
}

/// Trims a preset name and checks its length: empty →
/// `invalid_input:preset_name_required`, more than
/// [`PRESET_NAME_MAX_CHARS`] characters → `invalid_input:preset_name_too_long`.
pub fn validate_preset_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Error::invalid("preset_name_required"));
    }
    if trimmed.chars().count() > PRESET_NAME_MAX_CHARS {
        return Err(Error::invalid("preset_name_too_long"));
    }
    Ok(trimmed.to_owned())
}

/// True for the id of a built-in preset.
pub fn is_builtin_preset(id: &str) -> bool {
    BUILTIN_PRESET_IDS.contains(&id)
}

/// The built-in preset `id`, if any.
pub fn builtin_preset(id: &str) -> Option<GeneratorPreset> {
    let d = GeneratorOptions::default();
    let (name, options) = match id {
        PRESET_STANDARD => ("Standard", d),
        PRESET_ALNUM => (
            "Letters and digits only",
            GeneratorOptions {
                symbols: false,
                min_symbols: 0,
                ..d
            },
        ),
        PRESET_PIN => (
            "PIN",
            GeneratorOptions {
                length: 6,
                uppercase: false,
                lowercase: false,
                symbols: false,
                min_uppercase: 0,
                min_lowercase: 0,
                min_digits: 0,
                min_symbols: 0,
                ..d
            },
        ),
        PRESET_LONG => (
            "Long and strong",
            GeneratorOptions {
                length: 32,
                min_uppercase: 2,
                min_lowercase: 2,
                min_digits: 2,
                min_symbols: 2,
                ..d
            },
        ),
        PRESET_PASSPHRASE => (
            "Passphrase",
            GeneratorOptions {
                kind: GeneratorKind::Passphrase,
                ..d
            },
        ),
        _ => return None,
    };
    Some(GeneratorPreset {
        id: id.to_owned(),
        name: name.to_owned(),
        options,
    })
}

/// All built-in presets, in display order.
pub fn builtin_presets() -> Vec<GeneratorPreset> {
    BUILTIN_PRESET_IDS
        .iter()
        .filter_map(|id| builtin_preset(id))
        .collect()
}

/// The options of preset `id`: a built-in one or one of `user`.
pub fn preset_options(id: &str, user: &[GeneratorPreset]) -> Option<GeneratorOptions> {
    builtin_preset(id)
        .map(|p| p.options)
        .or_else(|| user.iter().find(|p| p.id == id).map(|p| p.options.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn invalid(o: &GeneratorOptions) -> String {
        match generate(o) {
            Err(Error::InvalidInput(detail)) => {
                // `validate` agrees with `generate`.
                assert!(
                    matches!(o.validate(), Err(Error::InvalidInput(d)) if d == detail),
                    "{o:?}"
                );
                detail
            }
            other => panic!("expected invalid_input for {o:?}, got {other:?}"),
        }
    }

    fn count(p: &str, f: impl Fn(char) -> bool) -> usize {
        p.chars().filter(|c| f(*c)).count()
    }

    #[test]
    fn wordlist_has_7776_unique_words() {
        let list = wordlist();
        assert_eq!(list.len(), 7776);
        let unique: HashSet<_> = list.iter().collect();
        assert_eq!(unique.len(), 7776);
        assert!(list
            .iter()
            .all(|w| w.chars().all(|c| c.is_ascii_lowercase() || c == '-')));
        assert_eq!(list.first(), Some(&"abacus"));
        assert_eq!(list.last(), Some(&"zoom"));
        assert_eq!(Wordlist::En.words(), list);
    }

    #[test]
    fn german_wordlist_has_7724_unique_words() {
        let list = Wordlist::De.words();
        assert_eq!(list.len(), 7724);
        let unique: HashSet<_> = list.iter().collect();
        assert_eq!(unique.len(), 7724);
        // Lowercase ASCII only (no umlauts, no ß), 5..=19 characters.
        assert!(list
            .iter()
            .all(|w| w.chars().all(|c| c.is_ascii_lowercase()) && (5..=19).contains(&w.len())));
        assert_eq!(list.first(), Some(&"aalen"));
        assert_eq!(list.last(), Some(&"zypressen"));
        // The words removed from the upstream list (see wordlist/README.md)
        // stay out.
        for w in ["arsch", "kotzen", "endsieg", "ermorden", "nackt"] {
            assert!(!list.contains(&w), "{w}");
        }
        assert!((Wordlist::De.bits_per_word() - 7724f64.log2()).abs() < 1e-12);
        assert!((Wordlist::En.bits_per_word() - 7776f64.log2()).abs() < 1e-12);
    }

    #[test]
    fn defaults_match_contract() {
        let o = GeneratorOptions::default();
        assert_eq!(o.kind, GeneratorKind::Password);
        assert_eq!(o.length, 20);
        assert!(o.uppercase && o.lowercase && o.digits && o.symbols);
        assert_eq!(
            (
                o.min_uppercase,
                o.min_lowercase,
                o.min_digits,
                o.min_symbols
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(o.symbol_set, DEFAULT_SYMBOLS);
        assert_eq!(o.exclude, "");
        assert!(!o.avoid_ambiguous && !o.start_with_letter);
        assert_eq!(o.words, 5);
        assert_eq!(o.separator, "-");
        assert!(o.capitalize && o.include_number);
        assert_eq!(o.wordlist, Wordlist::En);
        assert_eq!(o.normalized().unwrap(), o);
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
        // Options of an older extension (every field it knows).
        let old = r#"{"kind":"password","length":16,"uppercase":true,"lowercase":true,
            "digits":true,"symbols":false,"minDigits":2,"minSymbols":1,"avoidAmbiguous":true,
            "words":5,"separator":"-","capitalize":true,"includeNumber":true}"#;
        let o: GeneratorOptions = serde_json::from_str(old).unwrap();
        assert_eq!((o.min_uppercase, o.min_lowercase), (1, 1));
        assert_eq!(o.symbol_set, DEFAULT_SYMBOLS);
        assert!(!o.start_with_letter);
        assert_eq!(o.wordlist, Wordlist::En);
        // `minSymbols: 1` with `symbols: false` is fine (ignored).
        let p = generate(&o).unwrap();
        assert_eq!(p.chars().count(), 16);
        assert!(count(&p, |c| c.is_ascii_digit()) >= 2);
        assert_eq!(count(&p, |c| !c.is_ascii_alphanumeric()), 0);

        let v = serde_json::to_value(GeneratorOptions::default()).unwrap();
        assert_eq!(v["minDigits"], 1);
        assert_eq!(v["minUppercase"], 1);
        assert_eq!(v["minLowercase"], 1);
        assert_eq!(v["symbolSet"], DEFAULT_SYMBOLS);
        assert_eq!(v["exclude"], "");
        assert_eq!(v["startWithLetter"], false);
        assert_eq!(v["wordlist"], "en");
        assert_eq!(v["includeNumber"], true);
        assert_eq!(v["kind"], "password");
        assert_eq!(v.as_object().unwrap().len(), 19);
        let o: GeneratorOptions = serde_json::from_str(r#"{"wordlist":"de"}"#).unwrap();
        assert_eq!(o.wordlist, Wordlist::De);
        assert!(serde_json::from_str::<GeneratorOptions>(r#"{"wordlist":"fr"}"#).is_err());
    }

    #[test]
    fn lenient_json_keeps_valid_fields() {
        let v = serde_json::json!({
            "length": 24, "wordlist": "fr", "symbols": "yes", "minDigits": -1,
            "exclude": "xyz", "startWithLetter": true, "future": 1
        });
        let o = GeneratorOptions::from_json_lenient(&v);
        assert_eq!(o.length, 24);
        assert_eq!(o.wordlist, Wordlist::En);
        assert!(o.symbols);
        assert_eq!(o.min_digits, 1);
        assert_eq!(o.exclude, "xyz");
        assert!(o.start_with_letter);
        assert_eq!(
            GeneratorOptions::from_json_lenient(&serde_json::json!([1])),
            GeneratorOptions::default()
        );
        let full = serde_json::to_value(builtin_preset(PRESET_PIN).unwrap().options).unwrap();
        assert_eq!(
            GeneratorOptions::from_json_lenient(&full),
            builtin_preset(PRESET_PIN).unwrap().options
        );
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
            assert!(count(&p, |c| c.is_ascii_digit()) >= 3);
            assert!(count(&p, |c| DEFAULT_SYMBOLS.contains(c)) >= 2);
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
    fn minimums_up_to_the_length_are_met_exactly() {
        // The minimums fill the whole length: exactly that many of each.
        let o = GeneratorOptions {
            length: 10,
            min_uppercase: 4,
            min_lowercase: 3,
            min_digits: 2,
            min_symbols: 1,
            ..Default::default()
        };
        for _ in 0..200 {
            let p = generate(&o).unwrap();
            assert_eq!(count(&p, |c| c.is_ascii_uppercase()), 4, "{p}");
            assert_eq!(count(&p, |c| c.is_ascii_lowercase()), 3, "{p}");
            assert_eq!(count(&p, |c| c.is_ascii_digit()), 2, "{p}");
            assert_eq!(count(&p, |c| DEFAULT_SYMBOLS.contains(c)), 1, "{p}");
        }
        // One class may require the whole length.
        let o = GeneratorOptions {
            length: 8,
            min_uppercase: 0,
            min_lowercase: 0,
            min_digits: 8,
            min_symbols: 0,
            ..Default::default()
        };
        assert!(generate(&o).unwrap().chars().all(|c| c.is_ascii_digit()));
        // Larger minimums of every class.
        let o = GeneratorOptions {
            length: 64,
            min_uppercase: 10,
            min_lowercase: 10,
            min_digits: 10,
            min_symbols: 10,
            ..Default::default()
        };
        for _ in 0..50 {
            let p = generate(&o).unwrap();
            assert!(count(&p, |c| c.is_ascii_uppercase()) >= 10);
            assert!(count(&p, |c| c.is_ascii_lowercase()) >= 10);
            assert!(count(&p, |c| c.is_ascii_digit()) >= 10);
            assert!(count(&p, |c| DEFAULT_SYMBOLS.contains(c)) >= 10);
        }
        // Sum above the length.
        let o = GeneratorOptions {
            length: 10,
            min_uppercase: 4,
            min_lowercase: 3,
            min_digits: 2,
            min_symbols: 2,
            ..Default::default()
        };
        assert_eq!(invalid(&o), "minimums_exceed_length");
    }

    #[test]
    fn password_rejects_bad_options() {
        let cases = [
            (
                GeneratorOptions {
                    length: 3,
                    ..Default::default()
                },
                "length",
            ),
            (
                GeneratorOptions {
                    length: 129,
                    ..Default::default()
                },
                "length",
            ),
            (
                GeneratorOptions {
                    uppercase: false,
                    lowercase: false,
                    digits: false,
                    symbols: false,
                    ..Default::default()
                },
                "no_character_set",
            ),
            (
                GeneratorOptions {
                    length: 5,
                    min_digits: 3,
                    min_symbols: 3,
                    ..Default::default()
                },
                "minimums_exceed_length",
            ),
            (
                GeneratorOptions {
                    min_uppercase: u32::MAX,
                    min_lowercase: u32::MAX,
                    min_digits: u32::MAX,
                    min_symbols: u32::MAX,
                    ..Default::default()
                },
                "minimums_exceed_length",
            ),
        ];
        for (o, detail) in cases {
            assert_eq!(invalid(&o), detail, "{o:?}");
        }
        // Minimums of disabled classes are ignored.
        let o = GeneratorOptions {
            length: 4,
            digits: false,
            min_digits: 50,
            uppercase: false,
            min_uppercase: 50,
            ..Default::default()
        };
        let p = generate(&o).unwrap();
        assert_eq!(p.chars().count(), 4);
        assert!(!p
            .chars()
            .any(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
        // The shortest length (a PIN).
        let pin = GeneratorOptions {
            length: PASSWORD_MIN_LEN,
            ..builtin_preset(PRESET_PIN).unwrap().options
        };
        assert_eq!(generate(&pin).unwrap().len(), 4);
    }

    #[test]
    fn symbol_set_is_validated_and_used() {
        assert_eq!(normalize_symbol_set("--__.").unwrap(), "-_.");
        assert_eq!(normalize_symbol_set("").unwrap(), "");
        let all_ascii_symbols: String = (0x21u8..=0x7e)
            .map(char::from)
            .filter(|c| !c.is_ascii_alphanumeric())
            .collect();
        assert_eq!(all_ascii_symbols.len(), 32);
        assert_eq!(
            normalize_symbol_set(&all_ascii_symbols).unwrap(),
            all_ascii_symbols
        );
        for bad in ["abc", "-a", "-1", "- ", "\t", "ä", "€", "-\u{7f}"] {
            let o = GeneratorOptions {
                symbol_set: bad.into(),
                ..Default::default()
            };
            assert_eq!(invalid(&o), "symbol_set", "{bad:?}");
        }
        let too_long = GeneratorOptions {
            symbol_set: "-".repeat(CHARSET_INPUT_MAX_CHARS + 1),
            ..Default::default()
        };
        assert_eq!(invalid(&too_long), "symbol_set");
        let at_limit = GeneratorOptions {
            symbol_set: "-".repeat(CHARSET_INPUT_MAX_CHARS),
            ..Default::default()
        };
        assert_eq!(at_limit.normalized().unwrap().symbol_set, "-");
        // Empty while symbols are on → error; with symbols off → fine.
        let empty = GeneratorOptions {
            symbol_set: String::new(),
            ..Default::default()
        };
        assert_eq!(invalid(&empty), "symbol_set");
        let off = GeneratorOptions {
            symbols: false,
            min_symbols: 0,
            ..empty
        };
        assert!(generate(&off).is_ok());
        // Only the chosen symbols appear, and every one of them does.
        let o = GeneratorOptions {
            length: 64,
            uppercase: false,
            lowercase: false,
            digits: false,
            min_uppercase: 0,
            min_lowercase: 0,
            min_digits: 0,
            symbol_set: "()[]{}<>\"'\\`".into(),
            ..Default::default()
        };
        let mut seen = HashSet::new();
        for _ in 0..50 {
            let p = generate(&o).unwrap();
            assert!(p.chars().all(|c| o.symbol_set.contains(c)), "{p}");
            seen.extend(p.chars());
        }
        assert_eq!(seen.len(), 12);
        assert_eq!(o.normalized().unwrap().symbol_set, o.symbol_set);
    }

    #[test]
    fn exclude_removes_characters_from_every_class() {
        let o = GeneratorOptions {
            exclude: "aeiouAEIOU01-_".into(),
            symbol_set: "-_.!".into(),
            length: 64,
            ..Default::default()
        };
        let mut counts: HashMap<char, u32> = HashMap::new();
        for _ in 0..300 {
            let p = generate(&o).unwrap();
            for c in p.chars() {
                assert!(!o.exclude.contains(c), "{p}");
                *counts.entry(c).or_default() += 1;
            }
        }
        // Allowed: 21 + 21 upper/lower consonants, 8 digits, 2 symbols.
        let allowed: HashSet<char> = UPPER
            .chars()
            .chain(LOWER.chars())
            .chain(DIGITS.chars())
            .chain("-_.!".chars())
            .filter(|c| !o.exclude.contains(*c))
            .collect();
        assert_eq!(allowed.len(), 52);
        assert_eq!(counts.keys().copied().collect::<HashSet<_>>(), allowed);
        // 19200 draws: no allowed character is rare.
        assert!(counts.values().all(|n| *n > 100), "{counts:?}");
        // Duplicates are dropped when normalising; unknown characters are
        // harmless.
        let o = GeneratorOptions {
            exclude: "xxyy€ ".into(),
            ..Default::default()
        };
        assert_eq!(o.normalized().unwrap().exclude, "xy€ ");
        let p = generate(&o).unwrap();
        assert!(!p.contains('x') && !p.contains('y'));
        // Excluding a whole enabled class.
        let o = GeneratorOptions {
            exclude: DIGITS.into(),
            ..Default::default()
        };
        assert_eq!(invalid(&o), "no_character_set");
        let o = GeneratorOptions {
            exclude: "!#$%&*+-=?@^_".into(),
            ..Default::default()
        };
        assert_eq!(invalid(&o), "no_character_set");
        // … is fine while the class is off.
        let o = GeneratorOptions {
            exclude: DIGITS.into(),
            digits: false,
            ..Default::default()
        };
        assert!(generate(&o).is_ok());
        // Together with avoid_ambiguous.
        let o = GeneratorOptions {
            exclude: "23456789".into(),
            avoid_ambiguous: true,
            ..Default::default()
        };
        assert_eq!(invalid(&o), "no_character_set");
        let too_long = GeneratorOptions {
            exclude: "x".repeat(CHARSET_INPUT_MAX_CHARS + 1),
            ..Default::default()
        };
        assert_eq!(invalid(&too_long), "exclude");
    }

    #[test]
    fn start_with_letter() {
        let o = GeneratorOptions {
            start_with_letter: true,
            ..Default::default()
        };
        let mut first = HashSet::new();
        for _ in 0..3000 {
            let p = generate(&o).unwrap();
            let c = p.chars().next().unwrap();
            assert!(c.is_ascii_alphabetic(), "{p}");
            first.insert(c);
            assert!(count(&p, |c| c.is_ascii_digit()) >= 1);
            assert!(count(&p, |c| DEFAULT_SYMBOLS.contains(c)) >= 1);
        }
        // Every letter can come first.
        assert_eq!(first.len(), 52);

        // No letter minimum: one letter is added, the length still holds.
        let o = GeneratorOptions {
            length: 6,
            lowercase: false,
            min_uppercase: 0,
            min_digits: 3,
            min_symbols: 2,
            start_with_letter: true,
            ..Default::default()
        };
        for _ in 0..200 {
            let p = generate(&o).unwrap();
            assert_eq!(p.len(), 6);
            assert!(p.starts_with(|c: char| c.is_ascii_uppercase()), "{p}");
            assert_eq!(count(&p, |c| c.is_ascii_uppercase()), 1, "{p}");
            assert_eq!(count(&p, |c| c.is_ascii_digit()), 3, "{p}");
        }
        // … which counts against the length.
        let o = GeneratorOptions { min_digits: 4, ..o };
        assert_eq!(invalid(&o), "minimums_exceed_length");
        // No letter class.
        let o = GeneratorOptions {
            start_with_letter: true,
            ..builtin_preset(PRESET_PIN).unwrap().options
        };
        assert_eq!(invalid(&o), "start_with_letter");
        // Letters all excluded: the class itself is the error.
        let o = GeneratorOptions {
            start_with_letter: true,
            lowercase: false,
            exclude: UPPER.into(),
            ..Default::default()
        };
        assert_eq!(invalid(&o), "no_character_set");
        // With a long password the letters stay spread over all positions.
        let o = GeneratorOptions {
            length: 8,
            min_uppercase: 0,
            min_lowercase: 1,
            min_digits: 0,
            min_symbols: 0,
            uppercase: false,
            start_with_letter: true,
            ..Default::default()
        };
        let mut letter_at = [0u32; 8];
        for _ in 0..2000 {
            let p = generate(&o).unwrap();
            for (i, c) in p.chars().enumerate() {
                if c.is_ascii_lowercase() {
                    letter_at[i] += 1;
                }
            }
        }
        assert_eq!(letter_at[0], 2000);
        assert!(letter_at[1..].iter().all(|n| *n > 500), "{letter_at:?}");
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
            assert_eq!(invalid(&o), "words");
        }
        let o = GeneratorOptions {
            kind: GeneratorKind::Passphrase,
            separator: "-----------".into(),
            ..Default::default()
        };
        assert_eq!(invalid(&o), "separator");
        // Password-only fields do not matter for a passphrase.
        let o = GeneratorOptions {
            kind: GeneratorKind::Passphrase,
            length: 0,
            symbol_set: "abc".into(),
            start_with_letter: true,
            ..Default::default()
        };
        assert!(generate(&o).is_ok());
        assert_eq!(o.normalized().unwrap(), o);
    }

    #[test]
    fn german_passphrase() {
        let o = GeneratorOptions {
            kind: GeneratorKind::Passphrase,
            wordlist: Wordlist::De,
            words: 4,
            separator: ".".into(),
            capitalize: false,
            include_number: false,
            ..Default::default()
        };
        let list: HashSet<&str> = Wordlist::De.words().iter().copied().collect();
        for _ in 0..50 {
            let p = generate(&o).unwrap();
            let parts: Vec<&str> = p.split('.').collect();
            assert_eq!(parts.len(), 4, "{p}");
            assert!(parts.iter().all(|w| list.contains(w)), "{p}");
        }
        let o = GeneratorOptions {
            capitalize: true,
            include_number: true,
            ..o
        };
        let p = generate(&o).unwrap();
        let parts: Vec<&str> = p.split('.').collect();
        assert!(parts
            .iter()
            .all(|w| w.starts_with(|c: char| c.is_ascii_uppercase())));
        assert_eq!(count(&p, |c| c.is_ascii_digit()), 1);
    }

    #[test]
    fn passphrase_entropy_uses_the_real_list_size() {
        let o = GeneratorOptions {
            kind: GeneratorKind::Passphrase,
            words: 5,
            include_number: false,
            ..Default::default()
        };
        let en = passphrase_entropy_bits(&o).unwrap();
        assert!((en - 5.0 * 7776f64.log2()).abs() < 1e-9, "{en}");
        let de = passphrase_entropy_bits(&GeneratorOptions {
            wordlist: Wordlist::De,
            ..o.clone()
        })
        .unwrap();
        assert!((de - 5.0 * 7724f64.log2()).abs() < 1e-9, "{de}");
        assert!(de < en);
        let with_number = passphrase_entropy_bits(&GeneratorOptions {
            include_number: true,
            ..o.clone()
        })
        .unwrap();
        assert!((with_number - en - 50f64.log2()).abs() < 1e-9);
        assert!(passphrase_entropy_bits(&GeneratorOptions { words: 2, ..o }).is_err());
    }

    #[test]
    fn builtin_presets_are_valid() {
        let presets = builtin_presets();
        assert_eq!(
            presets.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            BUILTIN_PRESET_IDS
        );
        for p in &presets {
            assert!(!p.name.is_empty());
            assert_eq!(p.options.normalized().unwrap(), p.options, "{}", p.id);
            assert!(generate(&p.options).is_ok(), "{}", p.id);
            assert!(is_builtin_preset(&p.id));
            assert_eq!(preset_options(&p.id, &[]), Some(p.options.clone()));
        }
        assert_eq!(
            builtin_preset(PRESET_STANDARD).unwrap().options,
            GeneratorOptions::default()
        );
        let pin = builtin_preset(PRESET_PIN).unwrap().options;
        let p = generate(&pin).unwrap();
        assert_eq!(p.len(), 6);
        assert!(p.chars().all(|c| c.is_ascii_digit()));
        let alnum = generate(&builtin_preset(PRESET_ALNUM).unwrap().options).unwrap();
        assert!(alnum.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(alnum.len(), 20);
        let long = generate(&builtin_preset(PRESET_LONG).unwrap().options).unwrap();
        assert_eq!(long.len(), 32);
        assert_eq!(
            builtin_preset(PRESET_PASSPHRASE).unwrap().options.kind,
            GeneratorKind::Passphrase
        );
        assert!(builtin_preset("custom").is_none());
        assert!(!is_builtin_preset("custom"));
    }

    #[test]
    fn user_presets_are_validated() {
        let p = GeneratorPreset {
            id: " 3f0c1d2e-aaaa-4bbb-8ccc-123456789abc ".into(),
            name: "  Bank  ".into(),
            options: GeneratorOptions {
                symbol_set: "--!!".into(),
                ..Default::default()
            },
        };
        let n = p.normalized().unwrap();
        assert_eq!(n.id, "3f0c1d2e-aaaa-4bbb-8ccc-123456789abc");
        assert_eq!(n.name, "Bank");
        assert_eq!(n.options.symbol_set, "-!");
        assert_eq!(
            preset_options(&n.id, std::slice::from_ref(&n)),
            Some(n.options.clone())
        );
        assert_eq!(preset_options("other", std::slice::from_ref(&n)), None);

        let long_name = "x".repeat(41);
        let name_cases = [
            ("", "preset_name_required"),
            ("   ", "preset_name_required"),
            (long_name.as_str(), "preset_name_too_long"),
        ];
        for (name, detail) in name_cases {
            let bad = GeneratorPreset {
                name: name.to_owned(),
                ..p.clone()
            };
            assert!(
                matches!(bad.normalized(), Err(Error::InvalidInput(d)) if d == detail),
                "{name:?}"
            );
        }
        assert_eq!(
            validate_preset_name(&"ä".repeat(40))
                .unwrap()
                .chars()
                .count(),
            40
        );
        let long_id = "a".repeat(65);
        for id in [
            "",
            "  ",
            "standard",
            "pin",
            "a b",
            "a/b",
            "ä",
            long_id.as_str(),
        ] {
            let bad = GeneratorPreset {
                id: id.to_owned(),
                ..p.clone()
            };
            assert!(
                matches!(bad.normalized(), Err(Error::InvalidInput(d)) if d == "preset_id"),
                "{id:?}"
            );
        }
        let bad = GeneratorPreset {
            options: GeneratorOptions {
                length: 2,
                ..Default::default()
            },
            ..p.clone()
        };
        assert!(matches!(bad.normalized(), Err(Error::InvalidInput(d)) if d == "length"));
        // JSON shape.
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(v["id"], n.id);
        assert_eq!(v["name"], "Bank");
        assert_eq!(v["options"]["symbolSet"], "-!");
    }
}
