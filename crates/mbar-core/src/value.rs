//! Value parsers shared by every property, mirroring the C library functions
//! SketchyBar relies on (`strtol(s, NULL, 0)`, `strtoul`, `strtof`) so that odd inputs
//! (`"12px"`, `"0x1f"`, `"010"`, `""`) produce the same numbers.

/// `strtol(s, NULL, 0)` semantics: leading whitespace, optional sign, base prefix
/// (`0x`/`0X` hex, leading `0` octal, otherwise decimal), stops at the first invalid
/// character. Returns 0 if no digits were parsed. Saturates on overflow, then is
/// truncated to `i32` like the `(int)` cast in SketchyBar.
pub fn parse_int(s: &str) -> i32 {
    parse_long(s) as i32
}

/// `strtol(s, NULL, 0)` returning the full `i64` (C `long` on 64-bit macOS).
pub fn parse_long(s: &str) -> i64 {
    let (neg, mag) = parse_unsigned_magnitude(s);
    let mag = mag.min(if neg {
        i64::MAX as u128 + 1
    } else {
        i64::MAX as u128
    });
    if neg {
        (mag as i128).wrapping_neg() as i64
    } else {
        mag as i64
    }
}

/// `strtoul(s, NULL, 0)` truncated to `u32` (used for colors and ids).
/// Like C, a leading `-` negates the value modulo 2^64.
pub fn parse_u32(s: &str) -> u32 {
    let (neg, mag) = parse_unsigned_magnitude(s);
    let mag = mag.min(u64::MAX as u128) as u64;
    let v = if neg { mag.wrapping_neg() } else { mag };
    v as u32
}

fn parse_unsigned_magnitude(s: &str) -> (bool, u128) {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    let mut neg = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        neg = b[i] == b'-';
        i += 1;
    }
    let mut radix = 10u32;
    if i + 1 < b.len() && b[i] == b'0' && (b[i + 1] == b'x' || b[i + 1] == b'X') {
        // "0x" only counts as a prefix if a hex digit follows; otherwise "0" is parsed.
        if i + 2 < b.len() && (b[i + 2] as char).is_ascii_hexdigit() {
            radix = 16;
            i += 2;
        }
    } else if i < b.len() && b[i] == b'0' {
        radix = 8;
    }
    let mut v: u128 = 0;
    while i < b.len() {
        let Some(d) = (b[i] as char).to_digit(radix) else {
            break;
        };
        v = v.saturating_mul(radix as u128).saturating_add(d as u128);
        i += 1;
    }
    (neg, v)
}

/// `strtof(s, NULL)` semantics: parses the longest valid float prefix
/// (decimal, exponent, `inf`, `nan`, hex floats are treated as decimal `0`).
/// Returns 0.0 if nothing parses.
pub fn parse_float(s: &str) -> f32 {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut end = 0;
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let lower = t[i..].to_ascii_lowercase();
    for word in ["infinity", "inf", "nan"] {
        if lower.starts_with(word) {
            return t[..i + word.len()].parse::<f32>().unwrap_or(0.0);
        }
    }
    let mut digits = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits = true;
        end = i;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits = true;
        }
        if digits {
            end = i;
        }
    }
    if digits && i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > start {
            end = j;
        }
    }
    if !digits {
        return 0.0;
    }
    t[..end].parse::<f32>().unwrap_or(0.0)
}

/// SketchyBar's `evaluate_boolean_state`: `on|yes|true|1|!off|!no|!false|!0` → true,
/// `toggle` → `!previous`, everything else → false.
pub fn parse_bool(s: &str, previous: bool) -> bool {
    match s {
        "on" | "yes" | "true" | "1" | "!off" | "!no" | "!false" | "!0" => true,
        "toggle" => !previous,
        _ => false,
    }
}

/// SketchyBar's `format_bool`.
pub fn format_bool(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// SketchyBar's `escape_string` used in query JSON: escapes `"` and newlines only.
pub fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

/// Splits `key=value` at the first `=`. Returns `None` if there is no `=`.
/// An empty value (`key=`) yields `Some((key, ""))`.
pub fn split_key_value(token: &str) -> Option<(&str, &str)> {
    token.split_once('=')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints_follow_strtol() {
        assert_eq!(parse_int("42"), 42);
        assert_eq!(parse_int("  -7"), -7);
        assert_eq!(parse_int("0x1f"), 31);
        assert_eq!(parse_int("010"), 8);
        assert_eq!(parse_int("12px"), 12);
        assert_eq!(parse_int(""), 0);
        assert_eq!(parse_int("abc"), 0);
        assert_eq!(parse_int("0x"), 0);
        assert_eq!(parse_int("09"), 0);
    }

    #[test]
    fn u32_follows_strtoul() {
        assert_eq!(parse_u32("0xffffffff"), 0xffff_ffff);
        assert_eq!(parse_u32("0x40000000"), 0x4000_0000);
        assert_eq!(parse_u32("-1"), u32::MAX);
        assert_eq!(parse_u32("0x1ffffffff"), 0xffff_ffff);
    }

    #[test]
    fn floats_follow_strtof() {
        assert_eq!(parse_float("14.0"), 14.0);
        assert_eq!(parse_float("0.5abc"), 0.5);
        assert_eq!(parse_float(".5"), 0.5);
        assert_eq!(parse_float("1e2"), 100.0);
        assert_eq!(parse_float("1e"), 1.0);
        assert_eq!(parse_float("-3"), -3.0);
        assert_eq!(parse_float("x"), 0.0);
        assert_eq!(parse_float(""), 0.0);
        assert_eq!(parse_float("5."), 5.0);
    }

    #[test]
    fn bools() {
        assert!(parse_bool("on", false));
        assert!(parse_bool("!0", false));
        assert!(!parse_bool("off", true));
        assert!(!parse_bool("garbage", true));
        assert!(parse_bool("toggle", false));
        assert!(!parse_bool("toggle", true));
    }

    #[test]
    fn escaping() {
        assert_eq!(escape_json_string("a\"b\nc"), "a\\\"b\\nc");
    }
}
