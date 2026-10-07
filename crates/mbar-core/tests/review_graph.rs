//! Review regressions for graphs (`components.md` §8.1, D22).
mod wpc_common;
use mbar_core::components::graph::MAX_GRAPH_WIDTH;
use wpc_common::H;

fn data_len(h: &mut H, name: &str) -> usize {
    let q = h.query(&[name]);
    q["graph"]["data"]
        .as_array()
        .expect("graph.data array")
        .len()
}

// --- R1: `--add graph` width was unbounded; u32::MAX aborted the daemon -----------------

#[test]
fn add_graph_huge_width_is_clamped() {
    let mut h = H::new();
    h.msg(&["--add", "graph", "g", "left", "4294967295"]);
    assert_eq!(data_len(&mut h, "g"), MAX_GRAPH_WIDTH as usize);
    h.msg(&["--add", "graph", "m", "left", "50000000"]);
    assert_eq!(data_len(&mut h, "m"), MAX_GRAPH_WIDTH as usize);
    // Clones copy the clamped buffer; pushes still work.
    h.msg(&["--clone", "c", "g"]);
    assert_eq!(data_len(&mut h, "c"), MAX_GRAPH_WIDTH as usize);
    let r = h.msg(&["--push", "g", "0.5"]);
    assert!(r.is_empty(), "{r}");
}

#[test]
fn add_graph_width_up_to_limit_is_kept() {
    let mut h = H::new();
    h.msg(&["--add", "graph", "g", "left", "4096"]);
    assert_eq!(data_len(&mut h, "g"), 4096);
    h.msg(&["--add", "graph", "s", "left", "7"]);
    assert_eq!(data_len(&mut h, "s"), 7);
}
