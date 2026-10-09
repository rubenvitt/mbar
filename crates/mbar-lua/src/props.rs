//! Conversion of Lua property tables into `key=value` command tokens.
//!
//! Rules (see `docs/LUA.md`):
//! * nested tables flatten to dot keys: `{ icon = { font = { size = 14 } } }` →
//!   `icon.font.size=14`;
//! * the positional (array) part of a nested table is the value of the key itself,
//!   joined with `,`: `{ space = { 1, 2 } }` → `space=1,2`,
//!   `{ background = { image = { "app.Safari", scale = 0.5 } } }` →
//!   `background.image=app.Safari background.image.scale=0.5`;
//! * booleans become `on`/`off`;
//! * numbers keep their integer form when integral (`14.0` → `14`);
//! * numbers under a color key (`color`, `*_color`) become `0x%08x`;
//! * keys are emitted in sorted order so output is deterministic.
//!
//! `mbar.borders` uses [`flatten_borders`] instead: its values follow the
//! JankyBorders grammar (design doc `2026-10-09-borders-design.md` §4,
//! `docs/spec/borders.md` §2.3), so color tables compile to `glow(…)` /
//! `gradient(…)` strings and lists become comma-joined values.

use mlua::{Function, Result, Table, Value};

/// Maximum nesting depth of property tables (guards against cycles).
const MAX_DEPTH: usize = 32;

/// Resolves a function value found in a property table (only `script` and
/// `click_script` accept functions) into the value string to send.
pub(crate) type FnResolver<'a> = dyn FnMut(&str, Function) -> Result<String> + 'a;

/// `true` for keys whose numeric values are colors.
pub(crate) fn is_color_key(key: &str) -> bool {
    let last = key.rsplit('.').next().unwrap_or(key);
    last == "color" || last.ends_with("_color")
}

/// Formats a Lua number the way the command language expects it.
pub(crate) fn format_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

fn format_color(v: i64) -> String {
    format!("0x{:08x}", (v as u64) & 0xffff_ffff)
}

/// Formats a scalar value for `key`. Returns an error for values that have no
/// command-language representation (tables, functions, userdata, nil).
pub(crate) fn format_scalar(key: &str, v: &Value) -> Result<String> {
    Ok(match v {
        Value::Boolean(b) => (if *b { "on" } else { "off" }).to_string(),
        Value::Integer(i) if is_color_key(key) => format_color(*i),
        Value::Integer(i) => i.to_string(),
        Value::Number(f) if is_color_key(key) && f.is_finite() => format_color(*f as i64),
        Value::Number(f) => format_float(*f),
        Value::String(s) => s.to_string_lossy(),
        other => {
            return Err(mlua::Error::runtime(format!(
                "mbar: unsupported value of type '{}' for property '{key}'",
                other.type_name()
            )))
        }
    })
}

/// Flattens a property table into `key=value` tokens.
pub(crate) fn flatten(props: &Table, resolve: &mut FnResolver<'_>) -> Result<Vec<String>> {
    let mut out = Vec::new();
    flatten_into("", props, &mut out, resolve, 0)?;
    Ok(out)
}

/// A table split into its positional part (sorted by index) and its named part
/// (sorted by key).
pub(crate) type SplitTable = (Vec<Value>, Vec<(String, Value)>);

/// Splits a table into its positional and named parts.
pub(crate) fn split_table(t: &Table) -> Result<SplitTable> {
    let mut positional: Vec<(i64, Value)> = Vec::new();
    let mut named: Vec<(String, Value)> = Vec::new();
    for pair in t.pairs::<Value, Value>() {
        let (k, v) = pair?;
        match k {
            Value::Integer(i) => positional.push((i, v)),
            Value::Number(f) if f.fract() == 0.0 => positional.push((f as i64, v)),
            Value::String(s) => named.push((s.to_string_lossy(), v)),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "mbar: unsupported key of type '{}' in property table",
                    other.type_name()
                )))
            }
        }
    }
    positional.sort_by_key(|(i, _)| *i);
    named.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((positional.into_iter().map(|(_, v)| v).collect(), named))
}

fn flatten_into(
    prefix: &str,
    t: &Table,
    out: &mut Vec<String>,
    resolve: &mut FnResolver<'_>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(mlua::Error::runtime(
            "mbar: property table nested too deeply (cycle?)",
        ));
    }
    let (positional, named) = split_table(t)?;
    if !positional.is_empty() {
        if prefix.is_empty() {
            return Err(mlua::Error::runtime(
                "mbar: property tables need named keys (got a list at the top level)",
            ));
        }
        let parts = positional
            .iter()
            .map(|v| format_scalar(prefix, v))
            .collect::<Result<Vec<_>>>()?;
        out.push(format!("{prefix}={}", parts.join(",")));
    }
    for (k, v) in named {
        let key = if prefix.is_empty() {
            k
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Value::Table(sub) => flatten_into(&key, &sub, out, resolve, depth + 1)?,
            Value::Function(f) => {
                let value = resolve(&key, f)?;
                out.push(format!("{key}={value}"));
            }
            other => {
                let value = format_scalar(&key, &other)?;
                out.push(format!("{key}={value}"));
            }
        }
    }
    Ok(())
}

/// Keys of `mbar.borders` whose list value is joined with `,` (spec BR-PARSE-08).
const BORDER_LIST_KEYS: [&str; 2] = ["blacklist", "whitelist"];

/// A border color component: a number (`0x%08x`) or a string, verbatim.
fn border_color_part(what: &str, v: &Value) -> Result<String> {
    match v {
        Value::Integer(_) | Value::Number(_) | Value::String(_) => format_scalar("color", v),
        other => Err(mlua::Error::runtime(format!(
            "mbar.borders: expected a color for '{what}', got {}",
            other.type_name()
        ))),
    }
}

/// Compiles a color table to the JankyBorders color syntax (spec BR-PARSE-05):
/// `{ glow = c }` → `glow(c)`,
/// `{ gradient = { top_left = a, bottom_right = b } }` →
/// `gradient(top_left=a,bottom_right=b)`, and the `top_right`/`bottom_left` form.
fn border_color_table(key: &str, t: &Table) -> Result<String> {
    let bad = || {
        mlua::Error::runtime(format!(
            "mbar.borders: '{key}' must be a color, {{ glow = <color> }} or \
             {{ gradient = {{ top_left = <color>, bottom_right = <color> }} }} \
             (or top_right/bottom_left)"
        ))
    };
    let (positional, named) = split_table(t)?;
    if !positional.is_empty() || named.len() != 1 {
        return Err(bad());
    }
    let (kind, value) = &named[0];
    match (kind.as_str(), value) {
        ("glow", v) => Ok(format!(
            "glow({})",
            border_color_part(&format!("{key}.glow"), v)?
        )),
        ("gradient", Value::Table(g)) => {
            let (positional, named) = split_table(g)?;
            if !positional.is_empty() || named.len() != 2 {
                return Err(bad());
            }
            let get = |name: &str| named.iter().find(|(k, _)| k == name).map(|(_, v)| v);
            for (first, second) in [("top_left", "bottom_right"), ("top_right", "bottom_left")] {
                if let (Some(a), Some(b)) = (get(first), get(second)) {
                    let a = border_color_part(&format!("{key}.gradient.{first}"), a)?;
                    let b = border_color_part(&format!("{key}.gradient.{second}"), b)?;
                    return Ok(format!("gradient({first}={a},{second}={b})"));
                }
            }
            Err(bad())
        }
        _ => Err(bad()),
    }
}

/// Flattens the property table of `mbar.borders` into `key=value` tokens
/// (design doc §4). Scalars follow [`format_scalar`] (booleans `on`/`off`,
/// numbers under `*_color` as `0x%08x`, integral widths without a decimal
/// point); color keys also take glow/gradient tables; `blacklist`/`whitelist`
/// take lists (joined with `,`; an empty list clears the filter); `apply_to`
/// is sent as JankyBorders' `apply-to`. Other tables and functions are errors.
pub(crate) fn flatten_borders(props: &Table) -> Result<Vec<String>> {
    let (positional, named) = split_table(props)?;
    if !positional.is_empty() {
        return Err(mlua::Error::runtime(
            "mbar.borders: property tables need named keys (got a list at the top level)",
        ));
    }
    let mut out = Vec::with_capacity(named.len());
    for (k, v) in named {
        let key = if k == "apply_to" {
            "apply-to".to_string()
        } else {
            k
        };
        let value = match &v {
            Value::Table(t) if is_color_key(&key) => border_color_table(&key, t)?,
            Value::Table(t) if BORDER_LIST_KEYS.contains(&key.as_str()) => {
                let (positional, named) = split_table(t)?;
                if !named.is_empty() {
                    return Err(mlua::Error::runtime(format!(
                        "mbar.borders: '{key}' must be a list of process names"
                    )));
                }
                positional
                    .iter()
                    .map(|v| format_scalar(&key, v))
                    .collect::<Result<Vec<_>>>()?
                    .join(",")
            }
            Value::Table(_) => {
                return Err(mlua::Error::runtime(format!(
                    "mbar.borders: property '{key}' does not take a table"
                )))
            }
            other => format_scalar(&key, other)?,
        };
        out.push(format!("{key}={value}"));
    }
    Ok(out)
}
