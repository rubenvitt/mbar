//! JSON <-> Lua conversion via `serde_json`.

use mlua::{Lua, Result, Table, Value};
use serde_json::Value as Json;

const MAX_DEPTH: usize = 64;

/// Converts a JSON value into a Lua value. Objects become tables, arrays become
/// sequences (1-based), `null` becomes `nil`.
pub(crate) fn to_lua(lua: &Lua, v: &Json) -> Result<Value> {
    Ok(match v {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
        },
        Json::String(s) => Value::String(lua.create_string(s)?),
        Json::Array(a) => {
            let t = lua.create_table_with_capacity(a.len(), 0)?;
            for (i, item) in a.iter().enumerate() {
                t.raw_set(i + 1, to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        Json::Object(o) => {
            let t = lua.create_table_with_capacity(0, o.len())?;
            for (k, item) in o {
                t.raw_set(k.as_str(), to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
    })
}

/// Parses `s` as JSON. Returns `None` if it is not valid JSON.
pub(crate) fn decode(lua: &Lua, s: &str) -> Result<Option<Value>> {
    match serde_json::from_str::<Json>(s) {
        Ok(j) => Ok(Some(to_lua(lua, &j)?)),
        Err(_) => Ok(None),
    }
}

/// Parses `s` as JSON only if it is an object or an array.
pub(crate) fn decode_structured(lua: &Lua, s: &str) -> Result<Option<Value>> {
    let t = s.trim_start();
    if !(t.starts_with('{') || t.starts_with('[')) {
        return Ok(None);
    }
    decode(lua, s)
}

/// Converts a Lua value into JSON. Tables whose keys are exactly `1..n` become
/// arrays, other tables objects (an empty table is `{}`).
pub(crate) fn from_lua(v: &Value) -> Result<Json> {
    from_lua_depth(v, 0)
}

fn from_lua_depth(v: &Value, depth: usize) -> Result<Json> {
    if depth > MAX_DEPTH {
        return Err(mlua::Error::runtime(
            "mbar.json.encode: table nested too deeply (cycle?)",
        ));
    }
    Ok(match v {
        Value::Nil => Json::Null,
        Value::Boolean(b) => Json::Bool(*b),
        Value::Integer(i) => Json::from(*i),
        Value::Number(f) => serde_json::Number::from_f64(*f)
            .map(Json::Number)
            .unwrap_or(Json::Null),
        Value::String(s) => Json::String(s.to_string_lossy()),
        Value::Table(t) => table_to_json(t, depth)?,
        other => {
            return Err(mlua::Error::runtime(format!(
                "mbar.json.encode: cannot encode a value of type '{}'",
                other.type_name()
            )))
        }
    })
}

fn table_to_json(t: &Table, depth: usize) -> Result<Json> {
    let len = t.raw_len();
    let mut count = 0usize;
    for pair in t.pairs::<Value, Value>() {
        pair?;
        count += 1;
    }
    if len > 0 && count == len {
        let mut arr = Vec::with_capacity(len);
        for i in 1..=len {
            let v: Value = t.raw_get(i)?;
            arr.push(from_lua_depth(&v, depth + 1)?);
        }
        return Ok(Json::Array(arr));
    }
    let mut map = serde_json::Map::new();
    for pair in t.pairs::<Value, Value>() {
        let (k, v) = pair?;
        let key = match k {
            Value::String(s) => s.to_string_lossy(),
            Value::Integer(i) => i.to_string(),
            Value::Number(f) => crate::props::format_float(f),
            Value::Boolean(b) => b.to_string(),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "mbar.json.encode: unsupported key type '{}'",
                    other.type_name()
                )))
            }
        };
        map.insert(key, from_lua_depth(&v, depth + 1)?);
    }
    Ok(Json::Object(map))
}
