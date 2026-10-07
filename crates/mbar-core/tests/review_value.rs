//! Review regressions for the shared value parsers (`cli.md` §4.1).
mod wpc_common;
use mbar_core::value::parse_float;
use wpc_common::H;

// --- CLI-5: float values use strtof, which accepts hex floats ---------------------------

#[test]
fn parse_float_accepts_hex_floats() {
    assert_eq!(parse_float("0x1p-2"), 0.25);
    assert_eq!(parse_float("0x10"), 16.0);
    assert_eq!(parse_float("-0x1.8p1"), -3.0);
    // No hex digit after `0x`: strtof converts only the leading `0`.
    assert_eq!(parse_float("0xz"), 0.0);
}

#[test]
fn graph_push_and_float_properties_take_hex_floats() {
    let mut h = H::new();
    h.msg(&["--add", "graph", "g", "right", "3"]);
    h.msg(&["--push", "g", "0x1p-2", "0x10"]);
    let q = h.query(&["g"]);
    let data: Vec<&str> = q["graph"]["data"]
        .as_array()
        .expect("graph.data array")
        .iter()
        .map(|v| v.as_str().expect("string sample"))
        .collect();
    assert!(data.contains(&"0.250000"), "{data:?}");
    assert!(data.contains(&"16.000000"), "{data:?}");

    h.msg(&["--set", "g", "graph.line_width=0x1.8p1"]);
    let q = h.query(&["g"]);
    assert_eq!(q["graph"]["line_width"].as_str(), Some("3.000000"), "{q}");
}
