//! Review regression CLI-4: `--bar display=` entries `0` or non-numeric are ignored without
//! an error reply (`cli.md` §5 `display`: "mbar should ignore such entries"), so the client
//! exits 0 like SketchyBar. Entries > 32 keep the D9 error response.
mod wpc_common;
use mbar_core::bar::{DISPLAY_ALL, DISPLAY_MAIN};
use wpc_common::H;

#[test]
fn display_zero_is_ignored_silently() {
    let mut h = H::new();
    assert_eq!(h.msg(&["--bar", "display=0"]), "");
    // Only an ignored entry: the pattern starts at 0, which means "main".
    assert_eq!(h.rt.model.bar.displays, DISPLAY_MAIN);
}

#[test]
fn non_numeric_display_entry_is_ignored_silently() {
    let mut h = H::new();
    assert_eq!(h.msg(&["--bar", "display=main,abc"]), "");
    assert_eq!(h.rt.model.bar.displays, DISPLAY_MAIN);

    assert_eq!(h.msg(&["--bar", "display=all"]), "");
    assert_eq!(h.rt.model.bar.displays, DISPLAY_ALL);
    assert_eq!(h.msg(&["--bar", "display=abc,2,0"]), "");
    assert_eq!(h.rt.model.bar.displays, 0b10);
}

#[test]
fn display_above_32_still_errors_per_d9() {
    let mut h = H::new();
    let rsp = h.msg(&["--bar", "display=1,40"]);
    assert_eq!(rsp, "[!] Bar: Invalid display '40'\n");
    // The valid entry is still applied.
    assert_eq!(h.rt.model.bar.displays, 0b1);
}

#[test]
fn ignored_entries_do_not_abort_the_rest_of_the_message() {
    let mut h = H::new();
    let rsp = h.msg(&["--bar", "display=0", "height=33"]);
    assert_eq!(rsp, "");
    assert_eq!(h.query(&["bar"])["height"], 33);
}
