//! State of the password generator screen.

use vaultx_core::generator::{
    self, GeneratorKind, GeneratorOptions, PASSPHRASE_MAX_WORDS, PASSPHRASE_MIN_WORDS,
    PASSWORD_MAX_LEN, PASSWORD_MIN_LEN,
};
use vaultx_core::health;
use zeroize::Zeroizing;

/// Separators offered for passphrases (cycled with Space/Left/Right).
pub const SEPARATORS: &[&str] = &["-", " ", ".", "_"];

/// One option row of the generator screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenRow {
    Mode,
    Length,
    Uppercase,
    Lowercase,
    Digits,
    Symbols,
    AvoidAmbiguous,
    Words,
    Separator,
    Capitalize,
    IncludeNumber,
}

const PASSWORD_ROWS: &[GenRow] = &[
    GenRow::Mode,
    GenRow::Length,
    GenRow::Uppercase,
    GenRow::Lowercase,
    GenRow::Digits,
    GenRow::Symbols,
    GenRow::AvoidAmbiguous,
];

const PASSPHRASE_ROWS: &[GenRow] = &[
    GenRow::Mode,
    GenRow::Words,
    GenRow::Separator,
    GenRow::Capitalize,
    GenRow::IncludeNumber,
];

/// Generator options plus the current result.
#[derive(Debug)]
pub struct GeneratorState {
    pub options: GeneratorOptions,
    /// Index into [`GeneratorState::rows`].
    pub row: usize,
    pub value: Zeroizing<String>,
    /// zxcvbn score of `value`.
    pub score: Option<u8>,
    pub error: Option<vaultx_core::Error>,
}

impl Default for GeneratorState {
    fn default() -> Self {
        GeneratorState {
            options: GeneratorOptions::default(),
            row: 0,
            value: Zeroizing::new(String::new()),
            score: None,
            error: None,
        }
    }
}

impl GeneratorState {
    /// Option rows for the current mode.
    pub fn rows(&self) -> &'static [GenRow] {
        match self.options.kind {
            GeneratorKind::Password => PASSWORD_ROWS,
            GeneratorKind::Passphrase => PASSPHRASE_ROWS,
        }
    }

    pub fn current_row(&self) -> GenRow {
        let rows = self.rows();
        rows.get(self.row).copied().unwrap_or(GenRow::Mode)
    }

    pub fn move_row(&mut self, delta: isize) {
        let max = self.rows().len().saturating_sub(1);
        self.row = self.row.saturating_add_signed(delta).min(max);
    }

    /// Generates a new value with the current options.
    pub fn regenerate(&mut self) {
        match generator::generate(&self.options) {
            Ok(v) => {
                self.score = Some(health::strength(&v, &[]).score);
                self.value = Zeroizing::new(v);
                self.error = None;
            }
            Err(e) => {
                self.value = Zeroizing::new(String::new());
                self.score = None;
                self.error = Some(e);
            }
        }
    }

    /// Forgets the generated value (on lock / leaving the screen).
    pub fn clear_value(&mut self) {
        self.value = Zeroizing::new(String::new());
        self.score = None;
        self.error = None;
    }

    /// Changes the password length or passphrase word count by `delta`.
    pub fn adjust_size(&mut self, delta: i32) {
        match self.options.kind {
            GeneratorKind::Password => {
                self.options.length = step(
                    self.options.length,
                    delta,
                    PASSWORD_MIN_LEN,
                    PASSWORD_MAX_LEN,
                );
            }
            GeneratorKind::Passphrase => {
                self.options.words = step(
                    self.options.words,
                    delta,
                    PASSPHRASE_MIN_WORDS,
                    PASSPHRASE_MAX_WORDS,
                );
            }
        }
        self.regenerate();
    }

    /// Left/Right on the current row: numbers step, everything else toggles
    /// or cycles. Returns false if the change was refused (see
    /// [`GeneratorState::toggle_row`]).
    pub fn adjust_row(&mut self, delta: i32) -> bool {
        match self.current_row() {
            GenRow::Length | GenRow::Words => self.adjust_size(delta),
            GenRow::Separator => self.cycle_separator(delta),
            _ => return self.toggle_row(),
        }
        true
    }

    /// Space on the current row. Returns false if the change was refused
    /// (disabling the last character set).
    pub fn toggle_row(&mut self) -> bool {
        let row = self.current_row();
        let o = &mut self.options;
        match row {
            GenRow::Mode => {
                o.kind = match o.kind {
                    GeneratorKind::Password => GeneratorKind::Passphrase,
                    GeneratorKind::Passphrase => GeneratorKind::Password,
                };
                self.row = 0;
            }
            GenRow::Uppercase | GenRow::Lowercase | GenRow::Digits | GenRow::Symbols => {
                let enabled = [o.uppercase, o.lowercase, o.digits, o.symbols]
                    .iter()
                    .filter(|b| **b)
                    .count();
                let flag = match row {
                    GenRow::Uppercase => &mut o.uppercase,
                    GenRow::Lowercase => &mut o.lowercase,
                    GenRow::Digits => &mut o.digits,
                    _ => &mut o.symbols,
                };
                if *flag && enabled == 1 {
                    return false;
                }
                *flag = !*flag;
            }
            GenRow::AvoidAmbiguous => o.avoid_ambiguous = !o.avoid_ambiguous,
            GenRow::Capitalize => o.capitalize = !o.capitalize,
            GenRow::IncludeNumber => o.include_number = !o.include_number,
            GenRow::Separator => {
                self.cycle_separator(1);
                return true;
            }
            GenRow::Length | GenRow::Words => return true,
        }
        self.regenerate();
        true
    }

    fn cycle_separator(&mut self, delta: i32) {
        let len = SEPARATORS.len();
        let current = SEPARATORS
            .iter()
            .position(|s| *s == self.options.separator)
            .unwrap_or(0);
        let next = if delta < 0 {
            (current + len - 1) % len
        } else {
            (current + 1) % len
        };
        self.options.separator = SEPARATORS[next].to_owned();
        self.regenerate();
    }
}

fn step(value: u32, delta: i32, min: u32, max: u32) -> u32 {
    let v = i64::from(value) + i64::from(delta);
    u32::try_from(v.clamp(i64::from(min), i64::from(max))).unwrap_or(min)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_and_mode() {
        let mut g = GeneratorState::default();
        g.regenerate();
        assert_eq!(g.value.chars().count(), 20);
        g.adjust_size(5);
        assert_eq!(g.value.chars().count(), 25);
        g.adjust_size(-1000);
        assert_eq!(g.options.length, PASSWORD_MIN_LEN);
        g.adjust_size(1000);
        assert_eq!(g.options.length, PASSWORD_MAX_LEN);

        assert_eq!(g.current_row(), GenRow::Mode);
        assert!(g.toggle_row());
        assert_eq!(g.options.kind, GeneratorKind::Passphrase);
        g.move_row(2);
        assert_eq!(g.current_row(), GenRow::Separator);
        // Words of the EFF list may contain "-", never a space.
        g.toggle_row();
        assert_eq!(g.options.separator, " ");
        assert_eq!(g.value.split(' ').count(), 5);
        g.adjust_size(1);
        assert_eq!(g.value.split(' ').count(), 6);
        g.adjust_row(-1);
        assert_eq!(g.options.separator, "-");
        g.move_row(100);
        assert_eq!(g.current_row(), GenRow::IncludeNumber);
    }

    #[test]
    fn last_character_set_cannot_be_disabled() {
        let mut g = GeneratorState::default();
        for row in 2..=4 {
            g.row = row;
            assert!(g.toggle_row());
        }
        g.row = 5; // symbols, the last one enabled
        assert_eq!(g.current_row(), GenRow::Symbols);
        assert!(!g.toggle_row());
        assert!(g.options.symbols);
        g.regenerate();
        assert!(g.error.is_none());
        assert!(g.value.chars().all(|c| !c.is_ascii_alphanumeric()));
    }
}
