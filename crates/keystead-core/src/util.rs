//! Small internal helpers: BOM handling, ISO-8601 timestamps, ids.

/// Removes a leading UTF-8 byte order mark.
pub(crate) fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// Removes a leading UTF-8 byte order mark from raw bytes.
pub(crate) fn strip_bom_bytes(b: &[u8]) -> &[u8] {
    b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)
}

/// Windows-1252 code points for bytes 0x80..=0x9F (undefined bytes map to
/// the C1 control of the same value, like Windows does).
const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

/// Decodes a text file: UTF-8 (with or without BOM), UTF-16 LE/BE with BOM,
/// otherwise Windows-1252 (what Excel on a German Windows writes for "CSV").
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |rest: &[u8], le: bool| -> String {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes
            .iter()
            .map(|&b| match b {
                0x80..=0x9F => char::from_u32(u32::from(CP1252_HIGH[usize::from(b - 0x80)]))
                    .unwrap_or('\u{FFFD}'),
                _ => char::from(b),
            })
            .collect(),
    }
}

/// A fresh random UUID v4 as lowercase hyphenated string.
pub(crate) fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

const MS_PER_DAY: i64 = 86_400_000;

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`]: (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    // Both values are small and positive by construction.
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Formats Unix milliseconds as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub(crate) fn format_iso8601_ms(ms: i64) -> String {
    let days = ms.div_euclid(MS_PER_DAY);
    let rem = ms.rem_euclid(MS_PER_DAY);
    let (y, m, d) = civil_from_days(days);
    let secs = rem / 1000;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60,
        rem % 1000
    )
}

/// Minimal cursor over ASCII bytes for the timestamp parser.
struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn digits(&mut self, n: usize) -> Option<u32> {
        let slice = self.b.get(self.pos..self.pos + n)?;
        let mut v = 0u32;
        for c in slice {
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + u32::from(c - b'0');
        }
        self.pos += n;
        Some(v)
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.b.get(self.pos) == Some(&c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    fn done(&self) -> bool {
        self.pos >= self.b.len()
    }
}

/// Parses common timestamp spellings into Unix milliseconds:
/// ISO-8601 / RFC 3339 (`2024-01-15`, `2024-01-15T10:30:00`, fractional
/// seconds, `Z` or `±HH:MM` offsets – no offset means UTC), and the .NET JSON
/// form `/Date(1700000000000)/` written by Windows PowerShell.
pub(crate) fn parse_timestamp_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Some(inner) = s
        .strip_prefix("/Date(")
        .and_then(|r| r.strip_suffix(")/"))
        .or_else(|| {
            s.strip_prefix("\\/Date(")
                .and_then(|r| r.strip_suffix(")\\/"))
        })
    {
        // An optional "+0100" suffix only describes the original time zone.
        let digits_end = inner
            .char_indices()
            .skip(1)
            .find(|(_, c)| *c == '+' || *c == '-')
            .map_or(inner.len(), |(i, _)| i);
        return inner[..digits_end].parse::<i64>().ok();
    }
    parse_iso8601_ms(s)
}

fn parse_iso8601_ms(s: &str) -> Option<i64> {
    let mut c = Cursor {
        b: s.as_bytes(),
        pos: 0,
    };
    let year = i64::from(c.digits(4)?);
    if !c.eat(b'-') {
        return None;
    }
    let month = c.digits(2)?;
    if !c.eat(b'-') {
        return None;
    }
    let day = c.digits(2)?;
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let mut ms = days_from_civil(year, month, day) * MS_PER_DAY;
    if c.done() {
        return Some(ms);
    }
    if !(c.eat(b'T') || c.eat(b't') || c.eat(b' ')) {
        return None;
    }
    let hour = c.digits(2)?;
    if !c.eat(b':') {
        return None;
    }
    let minute = c.digits(2)?;
    let mut second = 0;
    if c.eat(b':') {
        second = c.digits(2)?;
    }
    // 60 tolerates leap seconds.
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    ms += i64::from(hour * 3600 + minute * 60 + second) * 1000;
    if c.eat(b'.') || c.eat(b',') {
        let mut frac_ms = 0i64;
        let mut digits = 0;
        while let Some(d) = c.peek().filter(u8::is_ascii_digit) {
            if digits < 3 {
                frac_ms = frac_ms * 10 + i64::from(d - b'0');
            }
            digits += 1;
            c.pos += 1;
        }
        if digits == 0 {
            return None;
        }
        for _ in digits..3 {
            frac_ms *= 10;
        }
        ms += frac_ms;
    }
    if c.done() || c.eat(b'Z') || c.eat(b'z') {
        return c.done().then_some(ms);
    }
    let sign: i64 = if c.eat(b'+') {
        1
    } else if c.eat(b'-') {
        -1
    } else {
        return None;
    };
    let oh = c.digits(2)?;
    c.eat(b':');
    let om = c.digits(2).unwrap_or(0);
    if !c.done() || oh > 23 || om > 59 {
        return None;
    }
    Some(ms - sign * i64::from(oh * 3600 + om * 60) * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom_is_stripped() {
        assert_eq!(strip_bom("\u{feff}{}"), "{}");
        assert_eq!(strip_bom("{}"), "{}");
        assert_eq!(strip_bom_bytes(b"\xEF\xBB\xBFab"), b"ab");
    }

    #[test]
    fn text_decoding() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFgr\xC3\xBC\xC3\x9F"), "grüß");
        assert_eq!(decode_text("grüß".as_bytes()), "grüß");
        // Windows-1252: ü = 0xFC, € = 0x80
        assert_eq!(decode_text(b"gr\xFC\xDF \x80"), "grüß €");
        assert_eq!(decode_text(b"\xFF\xFEa\x00\xFC\x00"), "aü");
        assert_eq!(decode_text(b"\xFE\xFF\x00a\x00\xFC"), "aü");
    }

    #[test]
    fn iso_round_trip() {
        for ms in [
            0i64,
            1_700_000_000_123,
            951_782_400_000, /* 2000-02-29 */
            -86_400_001,
            4_102_444_799_999,
        ] {
            let s = format_iso8601_ms(ms);
            assert_eq!(parse_timestamp_ms(&s), Some(ms), "{s}");
        }
        assert_eq!(format_iso8601_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            format_iso8601_ms(1_700_000_000_123),
            "2023-11-14T22:13:20.123Z"
        );
    }

    #[test]
    fn parses_variants() {
        let base = 1_705_314_600_000; // 2024-01-15T10:30:00Z
        assert_eq!(parse_timestamp_ms("2024-01-15T10:30:00"), Some(base));
        assert_eq!(parse_timestamp_ms("2024-01-15T10:30:00Z"), Some(base));
        assert_eq!(parse_timestamp_ms("2024-01-15 10:30"), Some(base));
        assert_eq!(parse_timestamp_ms("2024-01-15T11:30:00+01:00"), Some(base));
        assert_eq!(parse_timestamp_ms("2024-01-15T05:30:00-0500"), Some(base));
        assert_eq!(
            parse_timestamp_ms("2024-01-15T10:30:00.1234567Z"),
            Some(base + 123)
        );
        assert_eq!(
            parse_timestamp_ms("2024-01-15"),
            Some(base - (10 * 3600 + 1800) * 1000)
        );
        assert_eq!(parse_timestamp_ms("/Date(1705314600000)/"), Some(base));
        assert_eq!(parse_timestamp_ms("/Date(1705314600000+0100)/"), Some(base));
        assert_eq!(parse_timestamp_ms("/Date(-1000)/"), Some(-1000));
    }

    #[test]
    fn rejects_garbage() {
        for s in [
            "",
            "yesterday",
            "2024-13-01",
            "2023-02-29",
            "2024-01-15T25:00",
            "2024-01-15T10:30:00Q",
            "2024-01-15T10:30:00.",
            "/Date(abc)/",
        ] {
            assert_eq!(parse_timestamp_ms(s), None, "{s}");
        }
    }
}
