//! JankyBorders' argument grammar (`docs/spec/borders.md` §2.3, `src/parse.c`), including
//! the C `sscanf` leniency it depends on (BR-PARSE-02).

use super::{BorderColor, BorderOrder, BorderSettings, BorderStyle, GradientDirection, UpdateMask};

/// Why [`parse_arg`] rejected an argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgError {
    /// No rule matched (rule 14).
    InvalidArgument,
    /// A color key matched but its value did not parse; holds the text after the key,
    /// including the leading `=` (BR-PARSE-05).
    InvalidColor(String),
}

impl ArgError {
    /// The text JankyBorders prints on stdout (`[?] Borders: …\n`).
    pub fn jankyborders_text(&self, arg: &str) -> String {
        match self {
            ArgError::InvalidArgument => format!("[?] Borders: Invalid argument '{arg}'\n"),
            ArgError::InvalidColor(rest) => {
                format!("[?] Borders: Invalid color argument color{rest}\n")
            }
        }
    }

    /// The same message in mbar's error convention (`[!] Borders: …\n`).
    pub fn mbar_text(&self, arg: &str) -> String {
        let text = self.jankyborders_text(arg);
        format!("[!]{}", &text[3..])
    }
}

/// Applies one argument to `settings` (`parse_settings`, spec §2.3): the rules are tried in
/// order and the first match wins. Returns the update bits of the match.
pub fn parse_arg(settings: &mut BorderSettings, arg: &str) -> Result<UpdateMask, ArgError> {
    if let Some(rest) = arg.strip_prefix("active_color") {
        settings.active = parse_color(rest)?;
        return Ok(UpdateMask(UpdateMask::ACTIVE));
    }
    if let Some(rest) = arg.strip_prefix("inactive_color") {
        settings.inactive = parse_color(rest)?;
        return Ok(UpdateMask(UpdateMask::INACTIVE));
    }
    if let Some(rest) = arg.strip_prefix("background_color") {
        let color = parse_color(rest)?;
        // BR-PARSE-07: the alpha of the `color` union member; a gradient aliases
        // `direction` (0 or 1), so it never shows.
        settings.show_background = match color {
            BorderColor::Solid(c) | BorderColor::Glow(c) => c & 0xff00_0000 != 0,
            BorderColor::Gradient { .. } => false,
        };
        settings.background = color;
        return Ok(UpdateMask(UpdateMask::ALL));
    }
    if let Some(rest) = arg.strip_prefix("blacklist=") {
        settings.blacklist = parse_list(rest);
        return Ok(UpdateMask(UpdateMask::RECREATE_ALL));
    }
    if let Some(rest) = arg.strip_prefix("whitelist=") {
        settings.whitelist = parse_list(rest);
        return Ok(UpdateMask(UpdateMask::RECREATE_ALL));
    }
    if let Some(width) = arg.strip_prefix("width=").and_then(scan_float) {
        settings.width = width;
        return Ok(UpdateMask(UpdateMask::ALL));
    }
    if let Some(c) = arg.strip_prefix("order=").and_then(scan_char) {
        settings.order = if c == 'a' {
            BorderOrder::Above
        } else {
            BorderOrder::Below
        };
        return Ok(UpdateMask(UpdateMask::ALL));
    }
    if let Some(c) = arg.strip_prefix("style=").and_then(scan_char) {
        // BR-PARSE-03: drawing only tests 's' and 'u'; every other char draws round.
        settings.style = match c {
            's' => BorderStyle::Square,
            'u' => BorderStyle::Uniform,
            _ => BorderStyle::Round,
        };
        return Ok(UpdateMask(UpdateMask::ALL));
    }
    match arg {
        "hidpi=on" | "hidpi=off" => {
            settings.hidpi = arg == "hidpi=on";
            return Ok(UpdateMask(UpdateMask::RECREATE_ALL));
        }
        "ax_focus=on" | "ax_focus=off" => {
            settings.ax_focus = Some(arg == "ax_focus=on");
            return Ok(UpdateMask(UpdateMask::SETTING));
        }
        _ => {}
    }
    if arg.strip_prefix("apply-to=").and_then(scan_int).is_some() {
        // The window id itself is handled by `BordersState::apply`, which needs it to
        // route the message (BR-IPC-07); the settings are untouched.
        return Ok(UpdateMask(UpdateMask::SETTING));
    }
    Err(ArgError::InvalidArgument)
}

/// The window id of an `apply-to=<wid>` argument (`%d` into a `uint32_t`, so negative
/// values wrap).
pub fn apply_to_window(arg: &str) -> Option<u32> {
    arg.strip_prefix("apply-to=").and_then(scan_int)
}

/// Splits the `borders` client's arguments into the ones the primary would accept and the
/// error lines JankyBorders' client prints for the others (`[?] Borders: …\n`). The mbar
/// extension `drawing=on|off` is accepted too.
pub fn validate_args(args: &[String]) -> (Vec<String>, Vec<String>) {
    let mut scratch = BorderSettings::default();
    let mut valid = Vec::new();
    let mut errors = Vec::new();
    for arg in args {
        if arg.starts_with("drawing=") {
            valid.push(arg.clone());
            continue;
        }
        match parse_arg(&mut scratch, arg) {
            Ok(_) => valid.push(arg.clone()),
            Err(e) => errors.push(e.jankyborders_text(arg)),
        }
    }
    (valid, errors)
}

/// `parse_color` (BR-PARSE-05); `token` includes the leading `=`.
fn parse_color(token: &str) -> Result<BorderColor, ArgError> {
    if let Some(c) = token
        .strip_prefix("=0x")
        .and_then(|t| scan_hex(t).map(|(v, _)| v))
    {
        return Ok(BorderColor::Solid(c));
    }
    if let Some(c) = token
        .strip_prefix("=glow(0x")
        .and_then(|t| scan_hex(t).map(|(v, _)| v))
    {
        return Ok(BorderColor::Glow(c));
    }
    let gradients = [
        (
            "=gradient(top_left=0x",
            ",bottom_right=0x",
            GradientDirection::TopLeftToBottomRight,
        ),
        (
            "=gradient(top_right=0x",
            ",bottom_left=0x",
            GradientDirection::TopRightToBottomLeft,
        ),
    ];
    for (head, middle, direction) in gradients {
        let parsed = token.strip_prefix(head).and_then(|t| {
            let (color1, rest) = scan_hex(t)?;
            let (color2, _) = scan_hex(rest.strip_prefix(middle)?)?;
            Some((color1, color2))
        });
        if let Some((color1, color2)) = parsed {
            return Ok(BorderColor::Gradient {
                direction,
                color1,
                color2,
            });
        }
    }
    Err(ArgError::InvalidColor(token.to_string()))
}

/// `parse_list` (BR-PARSE-08): split on `,`, skip empty entries, keep the rest verbatim.
fn parse_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn skip_ws(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_ascii_whitespace())
}

/// `%x` into an `unsigned`: whitespace, an optional sign, an optional `0x`, hex digits.
/// Overflow saturates (`strtoul`) and the low 32 bits are kept. Returns the rest.
fn scan_hex(s: &str) -> Option<(u32, &str)> {
    let s = skip_ws(s);
    let (negative, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let s = match s.get(..2) {
        Some("0x" | "0X") if s.as_bytes().get(2).is_some_and(|b| b.is_ascii_hexdigit()) => &s[2..],
        _ => s,
    };
    let digits = s.bytes().take_while(u8::is_ascii_hexdigit).count();
    if digits == 0 {
        return None;
    }
    let mut value: u64 = 0;
    for b in s[..digits].bytes() {
        let d = (b as char).to_digit(16).unwrap_or(0) as u64;
        value = value
            .checked_mul(16)
            .and_then(|v| v.checked_add(d))
            .unwrap_or(u64::MAX);
    }
    if negative {
        value = value.wrapping_neg();
    }
    Some((value as u32, &s[digits..]))
}

/// `%d` into a `uint32_t`: whitespace, an optional sign, decimal digits; wraps.
fn scan_int(s: &str) -> Option<u32> {
    let s = skip_ws(s);
    let (negative, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let mut value: i64 = 0;
    for b in s[..digits].bytes() {
        value = value.saturating_mul(10).saturating_add(i64::from(b - b'0'));
    }
    if negative {
        value = -value;
    }
    Some(value as u32)
}

/// `%c`: exactly one char, which may be whitespace; nothing left is a failure (EOF).
fn scan_char(s: &str) -> Option<char> {
    s.chars().next()
}

/// `%f`: the longest prefix `strtof` accepts after leading whitespace (decimal with an
/// optional exponent, hex floats, `inf`/`infinity`, `nan`). Trailing text is ignored.
fn scan_float(s: &str) -> Option<f32> {
    let s = skip_ws(s);
    let bytes = s.as_bytes();
    let mut i = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        i = 1;
    }
    let negative = bytes.first() == Some(&b'-');
    let body = &s[i..];
    let lower = body.get(..8).unwrap_or(body).to_ascii_lowercase();
    if lower.starts_with("inf") {
        return Some(if negative {
            f32::NEG_INFINITY
        } else {
            f32::INFINITY
        });
    }
    if lower.starts_with("nan") {
        return Some(f32::NAN);
    }
    if lower.starts_with("0x") {
        return scan_hex_float(&body[2..]).map(|v| if negative { -v } else { v });
    }
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let int_digits = digits(i);
    let mut end = i + int_digits;
    let mut frac_digits = 0;
    if bytes.get(end) == Some(&b'.') {
        frac_digits = digits(end + 1);
        end += 1 + frac_digits;
    }
    if int_digits + frac_digits == 0 {
        return None;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut j = end + 1;
        if matches!(bytes.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exp_digits = digits(j);
        if exp_digits > 0 {
            end = j + exp_digits;
        }
    }
    s[..end].parse::<f32>().ok()
}

/// Hex float body after `0x`: hex digits, an optional fraction, an optional `p` exponent.
fn scan_hex_float(s: &str) -> Option<f32> {
    let bytes = s.as_bytes();
    let mut mantissa: f64 = 0.0;
    let mut i = 0;
    let mut any = false;
    while let Some(d) = bytes.get(i).and_then(|b| (*b as char).to_digit(16)) {
        mantissa = mantissa * 16.0 + f64::from(d);
        i += 1;
        any = true;
    }
    let mut scale = 0i32;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        while let Some(d) = bytes.get(i).and_then(|b| (*b as char).to_digit(16)) {
            mantissa = mantissa * 16.0 + f64::from(d);
            scale -= 4;
            i += 1;
            any = true;
        }
    }
    if !any {
        return None;
    }
    if matches!(bytes.get(i), Some(b'p' | b'P')) {
        let rest = &s[i + 1..];
        let (sign, digits) = match rest.as_bytes().first() {
            Some(b'-') => (-1, &rest[1..]),
            Some(b'+') => (1, &rest[1..]),
            _ => (1, rest),
        };
        let n = digits.bytes().take_while(u8::is_ascii_digit).count();
        if n > 0 {
            let exp: i32 = digits[..n].parse().unwrap_or(i32::MAX / 2);
            scale = scale.saturating_add(sign * exp);
        }
    }
    Some((mantissa * 2f64.powi(scale)) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arg: &str) -> (BorderSettings, Result<UpdateMask, ArgError>) {
        let mut s = BorderSettings::default();
        let r = parse_arg(&mut s, arg);
        (s, r)
    }

    #[test]
    fn solid_glow_and_gradient_colors() {
        let (s, r) = parse("active_color=0xff00ff00");
        assert_eq!(r, Ok(UpdateMask(UpdateMask::ACTIVE)));
        assert_eq!(s.active, BorderColor::Solid(0xff00ff00));
        let (s, _) = parse("inactive_color=glow(0x8000ff00)");
        assert_eq!(s.inactive, BorderColor::Glow(0x8000ff00));
        let (s, _) = parse("active_color=gradient(top_left=0xff111111,bottom_right=0xff222222)");
        assert_eq!(
            s.active,
            BorderColor::Gradient {
                direction: GradientDirection::TopLeftToBottomRight,
                color1: 0xff111111,
                color2: 0xff222222
            }
        );
        let (s, _) = parse("active_color=gradient(top_right=0x1,bottom_left=0x2)");
        assert_eq!(
            s.active,
            BorderColor::Gradient {
                direction: GradientDirection::TopRightToBottomLeft,
                color1: 1,
                color2: 2
            }
        );
    }

    #[test]
    fn color_leniency() {
        // A second 0x prefix, missing closing parens, trailing garbage.
        assert_eq!(
            parse("active_color=0x0xff00ff00").0.active,
            BorderColor::Solid(0xff00ff00)
        );
        assert_eq!(
            parse("active_color=glow(0xff00ff00").0.active,
            BorderColor::Glow(0xff00ff00)
        );
        assert_eq!(
            parse("active_color=0xffzz").0.active,
            BorderColor::Solid(0xff)
        );
        // Low 32 bits of `strtoul`, which saturates past 64 bits.
        assert_eq!(
            parse("active_color=0x123456789abc").0.active,
            BorderColor::Solid(0x5678_9abc)
        );
        assert_eq!(
            parse("active_color=0x1234567890abcdef12").0.active,
            BorderColor::Solid(0xffff_ffff)
        );
    }

    #[test]
    fn color_errors_and_prefix_quirk() {
        let (_, r) = parse("active_color=red");
        let e = r.unwrap_err();
        assert_eq!(
            e.jankyborders_text("active_color=red"),
            "[?] Borders: Invalid color argument color=red\n"
        );
        assert_eq!(
            e.mbar_text("active_color=red"),
            "[!] Borders: Invalid color argument color=red\n"
        );
        // BQ2: a longer key starting with a color key is a color error.
        assert_eq!(
            parse("active_colorful=1").1,
            Err(ArgError::InvalidColor("ful=1".into()))
        );
        // Gradient keys in the wrong order are invalid.
        assert!(
            parse("active_color=gradient(bottom_right=0x1,top_left=0x2)")
                .1
                .is_err()
        );
    }

    #[test]
    fn background_visibility() {
        let (s, r) = parse("background_color=0x80000000");
        assert_eq!(r, Ok(UpdateMask(UpdateMask::ALL)));
        assert!(s.show_background);
        assert!(!parse("background_color=0x00ffffff").0.show_background);
        assert!(
            !parse("background_color=gradient(top_left=0xff000000,bottom_right=0xff000000)")
                .0
                .show_background
        );
    }

    #[test]
    fn numbers_and_chars_follow_sscanf() {
        assert_eq!(parse("width=5px").0.width, 5.0);
        assert_eq!(parse("width= -3").0.width, -3.0);
        assert_eq!(parse("width=2.5e1").0.width, 25.0);
        assert!(parse("width=inf").0.width.is_infinite());
        assert_eq!(parse("width=0x1p2").0.width, 4.0);
        assert_eq!(parse("width=").1, Err(ArgError::InvalidArgument));
        assert_eq!(parse("width=abc").1, Err(ArgError::InvalidArgument));
        assert_eq!(parse("order=above").0.order, BorderOrder::Above);
        assert_eq!(parse("order=x").0.order, BorderOrder::Below);
        assert_eq!(parse("style=square").0.style, BorderStyle::Square);
        assert_eq!(parse("style=uniform").0.style, BorderStyle::Uniform);
        assert_eq!(parse("style=Round").0.style, BorderStyle::Round);
        assert_eq!(parse("style=").1, Err(ArgError::InvalidArgument));
        assert_eq!(apply_to_window("apply-to=12abc"), Some(12));
        assert_eq!(apply_to_window("apply-to=-1"), Some(u32::MAX));
        assert_eq!(apply_to_window("apply-to=x"), None);
    }

    #[test]
    fn exact_switches_and_lists() {
        let (s, r) = parse("hidpi=on");
        assert!(s.hidpi);
        assert_eq!(r, Ok(UpdateMask(UpdateMask::RECREATE_ALL)));
        assert_eq!(parse("hidpi=yes").1, Err(ArgError::InvalidArgument));
        assert_eq!(parse("ax_focus=off").0.ax_focus, Some(false));
        let (s, r) = parse("blacklist=Safari, kitty,,");
        assert_eq!(
            s.blacklist,
            vec!["Safari".to_string(), " kitty".to_string()]
        );
        assert_eq!(r, Ok(UpdateMask(UpdateMask::RECREATE_ALL)));
        assert!(parse("whitelist=").0.whitelist.is_empty());
        assert_eq!(parse("blur_radius=5").1, Err(ArgError::InvalidArgument));
    }

    #[test]
    fn validate_splits_client_arguments() {
        let args: Vec<String> = ["width=5.0", "glow=1", "drawing=off", "active_color=x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (valid, errors) = validate_args(&args);
        assert_eq!(
            valid,
            vec!["width=5.0".to_string(), "drawing=off".to_string()]
        );
        assert_eq!(
            errors,
            vec![
                "[?] Borders: Invalid argument 'glow=1'\n".to_string(),
                "[?] Borders: Invalid color argument color=x\n".to_string()
            ]
        );
    }
}
