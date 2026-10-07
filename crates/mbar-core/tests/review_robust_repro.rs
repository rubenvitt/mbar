//! Minimal reproducers for robustness findings (review scratch).
mod wpc_common;
use std::time::Instant;
use wpc_common::H;

#[test]
fn bg_x_offset_min_negate() {
    let mut h = H::new();
    h.msg(&["--add", "item", "a", "left"]);
    h.msg(&[
        "--set",
        "a",
        "background.drawing=on",
        "background.x_offset=-2147483648",
    ]);
}

#[test]
fn vertical_y_offset_min_negate() {
    let mut h = H::new();
    h.msg(&["--bar", "position=left"]);
    h.msg(&["--add", "item", "a", "left"]);
    h.msg(&["--set", "a", "y_offset=-2147483648"]);
}

#[test]
fn bre_quadratic_stars() {
    let ns: Vec<usize> = std::env::var("STARS")
        .ok()
        .map(|v| v.split(',').map(|x| x.parse().unwrap()).collect())
        .unwrap_or(vec![5_000, 40_000]);
    for n in ns {
        let mut h = H::new();
        let tok = format!("/a{}/", "*".repeat(n));
        let t = Instant::now();
        let r = h.msg(&["--set", &tok, "label=x"]);
        eprintln!(
            "stars n={n}: {:?} rsp={:?}",
            t.elapsed(),
            &r[..r.len().min(60)]
        );
    }
}

#[test]
fn regex_nested_interval() {
    let mut h = H::new();
    h.msg(&["--add", "item", "aaa", "left"]);
    for tok in [
        "/\\(\\(a\\{255\\}\\)\\{255\\}\\)\\{255\\}/",
        "/\\(.\\{255\\}\\)\\{255\\}/",
        "/[[:alpha:]]\\{255\\}\\{255\\}/",
    ] {
        let t = Instant::now();
        let r = h.msg(&["--set", tok, "label=x"]);
        eprintln!("{tok}: {:?} {:?}", t.elapsed(), r);
    }
}

#[test]
fn graph_large_width_alloc() {
    let mut h = H::new();
    let t = Instant::now();
    h.msg(&["--add", "graph", "g", "left", "50000000"]);
    eprintln!("graph 50M: {:?}", t.elapsed());
    let t = Instant::now();
    let q = h.msg(&["--query", "g"]);
    eprintln!("query len {} in {:?}", q.len(), t.elapsed());
}

#[test]
fn odd_argv() {
    let cases: &[&[&str]] = &[
        &[],
        &[""],
        &["", "--update"],
        &["-"],
        &["--"],
        &["--set"],
        &["--set", ""],
        &["--set", "/"],
        &["--set", "//"],
        &["--set", "/", "x"],
        &["--add"],
        &["--add", "item"],
        &["--add", "item", ""],
        &["--add", "item", "", ""],
        &["--add", "graph", "g"],
        &["--add", "bracket"],
        &["--add", "bracket", "b"],
        &["--add", "bracket", "b", "//"],
        &["--add", "alias"],
        &["--add", "alias", "", "left"],
        &["--animate"],
        &["--animate", "l"],
        &["--clone"],
        &["--clone", "x"],
        &["--clone", "", ""],
        &["--subscribe"],
        &["--push"],
        &["--trigger"],
        &["--trigger", ""],
        &["--trigger", "x", "="],
        &["--query"],
        &["--query", "item"],
        &["--query", ""],
        &["--reorder"],
        &["--move"],
        &["--remove"],
        &["--remove", "/"],
        &["--rename"],
        &["--rename", "", ""],
        &["--hotload"],
        &["--load-font"],
        &["--reload", "/nonexistent"],
        &["--menu"],
        &["--menu", "99999999999999999999"],
        &["--menu", "-1"],
        &["--menubar"],
        &["--bar"],
        &["--bar", "="],
        &["--default", "="],
        &["--set", "a", "="],
        &["--set", "a", ".=1"],
        &["--set", "a", "label.=1"],
        &["--set", "a", "..=1"],
        &["--set", "a", "popup.=1"],
        &["--set", "a", "popup..=1"],
        &["--set", "a", "popup.background.=1"],
        &["--bar", "image.=1"],
        &["--set", "a", "label.font=:::"],
        &["--set", "a", "label.font=🙂:🙂:🙂"],
    ];
    let mut h = H::new();
    h.msg(&[
        "--add", "item", "a", "left", "--add", "graph", "g", "left", "3",
    ]);
    for c in cases {
        h.msg(c);
    }
    for c in cases {
        let mut v = vec!["--add", "item", "a", "left", "--set", "a"];
        v.extend_from_slice(c);
        h.msg(&v);
    }
}

#[test]
fn odd_displays() {
    use mbar_core::geometry::Rect;
    use mbar_core::platform::{DisplayInfo, HeadlessResources, Input, OsEvent, SpaceInfo};
    for variant in 0..4 {
        let mut res = HeadlessResources::default();
        match variant {
            0 => {
                res.displays.clear();
                res.spaces.clear();
            }
            1 => {
                res.displays[0].adid = 0;
                res.active_adid = 0;
            }
            2 => {
                res.displays.push(DisplayInfo {
                    id: 9,
                    adid: 40,
                    uuid: None,
                    frame: Rect::new(0.0, 0.0, 0.0, 0.0),
                    builtin: true,
                    menu_bar_height: 0.0,
                    current_space: 99,
                });
                res.active_adid = 40;
                for i in 0..70u64 {
                    res.spaces.push(SpaceInfo {
                        id: i + 2,
                        display: 40,
                        fullscreen: i % 2 == 0,
                    });
                }
            }
            _ => {
                res.displays[0].frame = Rect::new(f32::NAN, f32::INFINITY, -5.0, f32::MAX);
                res.displays[0].current_space = u64::MAX;
            }
        }
        let mut h = H::with_res(res);
        h.msg(&[
            "--add",
            "space",
            "sp",
            "left",
            "--set",
            "sp",
            "space=31",
            "display=32",
            "--add",
            "item",
            "a",
            "left",
            "--set",
            "a",
            "popup.drawing=on",
            "--add",
            "item",
            "p",
            "popup.a",
        ]);
        h.msg(&[
            "--bar",
            "display=all",
            "notch_width=200",
            "--update",
            "--query",
            "displays",
        ]);
        h.input(Input::Event(OsEvent::SpaceChanged));
        h.input(Input::DisplaysChanged);
        h.frame();
        h.click(10.0, 10.0);
        h.msg(&[
            "--trigger",
            "space_change",
            "--trigger",
            "display_change",
            "--query",
            "bar",
        ]);
        h.advance(std::time::Duration::from_secs(3));
    }
}

#[test]
fn anim_overshoot_x_offset() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "a",
        "left",
        "--set",
        "a",
        "background.drawing=on",
    ]);
    h.msg(&[
        "--animate",
        "overshoot",
        "30",
        "--set",
        "a",
        "background.x_offset=-2147483000",
    ]);
    h.advance_by(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_millis(16),
    );
}
