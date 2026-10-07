//! Item commands: `--add`, `--set` (regex), `--remove`, `--move`, `--clone`, `--rename`,
//! `--reorder`, `--push`, popups, brackets, queries (`docs/spec/cli.md` §6, `item.md` §10).

mod wpc_common;
use mbar_core::platform::{Effect, PlatformRequest, WindowKey};
use wpc_common::*;

fn names(h: &mut H) -> Vec<String> {
    h.query(&["bar"])["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn add_errors_and_types() {
    let mut h = H::new();
    assert_eq!(
        h.msg(&[
            "--add", "item", "a", "left", "--add", "item", "a", "left", "--add", "foo", "b",
            "right", "--add", "item", "c", "xyz", "--add", "graph", "g", "right", "50",
            "--add", "slider", "s", "right", "--add", "item", "p", "popup.nothere",
        ]),
        "[?] Add: Item 'a' already exists\n[?] Add b: Invalid type 'foo', assuming 'item'\n\
         [!] Add c: Illegal position 'xyz'\n[!] Add (Popup) p: Item 'nothere' is not a valid popup host\n"
    );
    assert_eq!(names(&mut h), vec!["a", "b", "g", "s"]);
    let g = h.query(&["g"]);
    assert_eq!(g["type"], "graph");
    assert_eq!(g["graph"]["data"].as_array().map(|a| a.len()), Some(50));
    assert_eq!(
        h.query(&["s"])["slider"]["width"]
            .to_string()
            .trim_matches('"'),
        "0"
    );
    assert_eq!(
        h.msg(&[
            "--set",
            "a",
            "graph.color=0xff00ff00",
            "lazy=1",
            "--set",
            "nope",
            "label=x"
        ]),
        "[!] Item (a): Trying to set a graph property on a non-graph item\n\
         [!] Item (a): Invalid property 'lazy' \n[!] Set: Item not found 'nope'\n"
    );
    assert_eq!(
        h.msg(&["--set", "a", "label", "label=hi"]),
        "[!] Set (a): Expected <key>=<value> pair, but got: 'label'\n"
    );
    assert_eq!(h.query(&["a"])["label"]["value"], "hi");
    assert_eq!(
        h.msg(&["--bar", "nope", "height=30"]),
        "[!] Bar: Expected <key>=<value> pair, but got: 'nope'\n[!] Unknown domain 'height=30'\n"
    );
    assert_eq!(
        h.msg(&["--default", "x"]),
        "[!] Set (default): Expected <key>=<value> pair, but got: 'x'\n"
    );
    assert_eq!(h.msg(&["--blah"]), "[!] Unknown domain '--blah'\n");
}

#[test]
fn regex_set_and_remove() {
    let mut h = H::new();
    h.msg(&[
        "--add", "item", "space.1", "left", "--add", "item", "space.2", "left", "--add", "item",
        "clock", "right",
    ]);
    assert_eq!(h.msg(&["--set", "/space\\.[0-9]/", "label=S"]), "");
    assert_eq!(h.query(&["space.2"])["label"]["value"], "S");
    assert_eq!(h.query(&["clock"])["label"]["value"], "");
    assert_eq!(
        h.msg(&["--set", "/zzz/", "label=S"]),
        "[?] Regex: No match found for regex '/zzz/'\n"
    );
    assert_eq!(
        h.msg(&["--set", "//", "label=S"]),
        "[!] Regex: Could not compile regex '//'\n"
    );
    assert_eq!(
        h.msg(&["--remove", "nope"]),
        "[!] Remove: Item 'nope' not found\n"
    );
    assert_eq!(h.msg(&["--remove", "/space/"]), "");
    assert_eq!(names(&mut h), vec!["clock"]);
}

#[test]
fn move_reorder_rename_clone() {
    let mut h = H::new();
    h.msg(&[
        "--add", "item", "a", "left", "--add", "item", "b", "left", "--add", "item", "c", "left",
        "--add", "item", "d", "left",
    ]);
    assert_eq!(h.msg(&["--move", "d", "before", "a"]), "");
    assert_eq!(names(&mut h), vec!["d", "a", "b", "c"]);
    assert_eq!(h.msg(&["--move", "d", "whatever", "c"]), "");
    assert_eq!(names(&mut h), vec!["a", "b", "c", "d"]);
    assert_eq!(h.msg(&["--move", "a", "before", "a"]), "");
    assert_eq!(names(&mut h), vec!["a", "b", "c", "d"]);
    assert_eq!(
        h.msg(&["--move", "a", "before", "x"]),
        "[!] Move: Item 'a' or 'x' not found\n"
    );
    assert_eq!(
        h.msg(&["--reorder", "d", "x", "b", "d"]),
        "[!] Order: Item 'x' not found\n"
    );
    assert_eq!(names(&mut h), vec!["a", "d", "c", "b"]);
    assert_eq!(
        h.msg(&["--rename", "a", "b"]),
        "[!] Rename: Failed to rename item: a -> b\n"
    );
    assert!(h
        .msg(&["--rename", "a", "z", "--query", "z"])
        .contains("\"name\": \"z\""));
    h.msg(&[
        "--set",
        "z",
        "label=hello",
        "script=echo $NAME",
        "--subscribe",
        "z",
        "mouse.clicked",
    ]);
    assert_eq!(h.msg(&["--clone", "z2", "z", "after"]), "");
    assert_eq!(names(&mut h), vec!["z", "z2", "d", "c", "b"]);
    let c = h.query(&["z2"]);
    assert_eq!(c["label"]["value"], "hello");
    assert_eq!(c["scripting"]["update_mask"], 64);
    assert_eq!(
        h.msg(&["--clone", "z3", "nope"]),
        "[!] Clone: Parent Item 'nope' not found\n"
    );
    assert_eq!(
        h.msg(&["--clone", "z2", "z"]),
        "[?] Clone: Item 'z2' already exists\n"
    );
    h.msg(&["--clone", "z4", "z", "before"]);
    assert_eq!(names(&mut h)[..3], ["z4", "z", "z2"]);
    // D6: graph clones do not share samples.
    h.msg(&["--add", "graph", "g", "right", "3", "--push", "g", "0.5"]);
    h.msg(&["--clone", "g2", "g", "--push", "g2", "1"]);
    assert_eq!(
        h.query(&["g"])["graph"]["data"][2],
        h.query(&["g"])["graph"]["data"][2]
    );
    let a = h.query(&["g"])["graph"]["data"].clone();
    let b = h.query(&["g2"])["graph"]["data"].clone();
    assert_ne!(a, b);
    assert_eq!(
        h.msg(&["--push", "z", "1"]),
        "[!] Push: Item 'z' not a graph\n"
    );
    assert_eq!(
        h.msg(&["--push", "q", "1"]),
        "[!] Push: Item 'q' not found\n"
    );
}

#[test]
fn brackets() {
    let mut h = H::new();
    h.msg(&[
        "--add", "item", "a", "left", "--add", "item", "b", "left", "--add", "item", "c", "right",
    ]);
    assert_eq!(
        h.msg(&[
            "--add",
            "bracket",
            "br",
            "a",
            "missing",
            "/^[bc]$/",
            "--set",
            "br",
            "background.color=0xff00ff00",
            "background.height=20"
        ]),
        "[?] Add (Group) br: Failed to add member 'missing', item not found\n"
    );
    let br = h.query(&["br"]);
    assert_eq!(br["type"], "bracket");
    let m: Vec<&str> = br["bracket"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(m, vec!["a", "b", "c"]);
    // nested bracket flattens
    h.msg(&["--add", "bracket", "outer", "br"]);
    let m = h.query(&["outer"])["bracket"].as_array().unwrap().len();
    assert_eq!(m, 3);
    // D8: first token missing keeps the bracket
    assert_eq!(
        h.msg(&["--add", "bracket", "b2", "zz", "a"]),
        "[?] Add (Group) b2: Failed to add member 'zz', item not found\n"
    );
    assert_eq!(h.query(&["b2"])["bracket"][0], "a");
    // Bracket frame spans its drawn members.
    let r = &h.query(&["br"])["bounding_rects"]["display-1"];
    assert!(r["size"][0].as_f64().unwrap() > 0.0, "{r}");
    // remove a member and the bracket
    h.msg(&["--remove", "b"]);
    let m = h.query(&["br"])["bracket"].as_array().unwrap().len();
    assert_eq!(m, 2);
    let outer = h.rt.model.find("outer");
    assert!(h.rt.model.items.iter().any(|i| i.group == outer));
    h.msg(&["--remove", "outer"]);
    assert!(h.rt.model.items.iter().all(|i| i.group != outer));
    // D7: clone of a bracket copies the members.
    h.msg(&["--clone", "br2", "br"]);
    assert_eq!(h.query(&["br2"])["bracket"].as_array().unwrap().len(), 2);
}

#[test]
fn popups() {
    let mut h = H::new();
    h.msg(&[
        "--add",
        "item",
        "host",
        "left",
        "--set",
        "host",
        "label=Host",
        "--add",
        "item",
        "p1",
        "popup.host",
        "--set",
        "p1",
        "label=one",
        "--add",
        "item",
        "p2",
        "popup.host",
        "--set",
        "p2",
        "label=two",
    ]);
    let q = h.query(&["host"]);
    let items: Vec<&str> = q["popup"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(items, vec!["p1", "p2"]);
    assert_eq!(h.query(&["p1"])["geometry"]["position"], "popup");
    // closed popup: no popup window
    assert!(h
        .last_frame
        .windows
        .iter()
        .all(|w| !matches!(w.key, WindowKey::Popup(_))));
    h.msg(&["--set", "host", "popup.drawing=toggle"]);
    let host_id = h.rt.model.find("host").unwrap();
    assert!(
        h.last_frame
            .windows
            .iter()
            .any(|w| w.key == WindowKey::Popup(host_id)),
        "{:?}",
        h.last_frame
            .windows
            .iter()
            .map(|w| w.key)
            .collect::<Vec<_>>()
    );
    assert!(h
        .rt
        .model
        .find("p1")
        .map(|id| h.rt.model.item(id).unwrap().is_shown())
        .unwrap());
    // re-setting popup position moves it to the end
    h.msg(&["--set", "p1", "position=popup.host"]);
    let q = h.query(&["host"]);
    assert_eq!(q["popup"]["items"][0], "p2");
    // invalid host: message, position stays popup
    assert_eq!(
        h.msg(&["--set", "p2", "position=popup.zzz"]),
        "[!] Item Position (p2): Item 'zzz' is not a valid popup host\n"
    );
    // close
    h.msg(&["--set", "host", "popup.drawing=toggle"]);
    assert!(h.last_frame.closed.contains(&WindowKey::Popup(host_id)));
    // hidden=on closes all popups
    h.msg(&["--set", "host", "popup.drawing=on"]);
    h.msg(&["--bar", "hidden=on"]);
    assert_eq!(h.query(&["bar"])["hidden"], "on");
    assert_eq!(h.query(&["host"])["popup"]["drawing"], "off");
    h.msg(&["--bar", "hidden=off"]);
    // removing the host detaches its children
    h.msg(&["--remove", "host"]);
    let p1 = h.rt.model.find("p1").unwrap();
    assert_eq!(h.rt.model.item(p1).unwrap().parent, None);
}

#[test]
fn queries() {
    let mut h = H::new();
    h.msg(&["--add", "item", "bar", "left"]);
    // keyword shadows the item name
    assert!(h.msg(&["--query", "bar"]).starts_with("{\n\t\"position\""));
    assert!(h
        .msg(&["--query", "item", "bar"])
        .starts_with("{\n\t\"name\": \"bar\""));
    assert_eq!(
        h.msg(&["--query", "item", "x"]),
        "[!] Query: Item 'x' not found\n"
    );
    assert_eq!(
        h.msg(&["--query", "x"]),
        "[!] Query: Invalid query, or item 'x' not found \n"
    );
    let ev = h.query(&["events"]);
    assert_eq!(ev["space_windows_change"]["bit"], 131072);
    let d = h.query(&["displays"]);
    let _ = d;
    let st = h.query(&["stats"]);
    assert_eq!(st["items"], 1);
    assert!(st["frames"].as_u64().unwrap() >= 1);
    assert!(st["ipc_messages"].as_u64().unwrap() >= 5);
    // bounding rects are written by layout before a query
    let q = h.query(&["item", "bar"]);
    assert!(
        q["bounding_rects"]["display-1"]["size"][1]
            .as_f64()
            .unwrap()
            > 0.0
    );
    // defaults reset
    h.msg(&[
        "--default",
        "label.color=0xff00ff00",
        "--set",
        "bar",
        "reset=1",
    ]);
    let d = h.query(&["defaults"]);
    assert_eq!(d["name"], "(null)");
    assert_eq!(d["label"]["color"], "0xffffffff");
}

#[test]
fn alias_and_providers_and_app_menu() {
    let mut h = H::new();
    let (_, fx) = h.msg_fx(&["--add", "alias", "Control Center,Battery", "right"]);
    let p = platform(&fx);
    assert!(p.contains(&PlatformRequest::RequestScreenCapture));
    let id = h.rt.model.find("Control Center,Battery").unwrap();
    assert!(p.contains(&PlatformRequest::CaptureAlias {
        item: id,
        owner: "Control Center".into(),
        name: Some("Battery".into()),
        forced: true
    }));
    // Shown alias items are recaptured on the routine tick.
    let fx = h.advance(std::time::Duration::from_millis(1100));
    assert!(platform(&fx)
        .iter()
        .any(|p| matches!(p, PlatformRequest::CaptureAlias { forced: false, .. })));

    let (_, fx) = h.msg_fx(&[
        "--add",
        "item",
        "cpu",
        "right",
        "--set",
        "cpu",
        "provider=cpu",
        "provider.format={percent}%",
        "script=cpu.sh",
    ]);
    let cpu = h.rt.model.find("cpu").unwrap();
    assert!(platform(&fx).iter().any(|p| matches!(p,
        PlatformRequest::StartProvider { item, provider, freq: Some(f), .. } if *item == cpu && provider == "cpu" && *f == 2.0)));
    let fx = h.input(mbar_core::platform::Input::ProviderSample {
        item: cpu,
        values: vec![("percent".into(), "42".into())],
    });
    h.frame();
    assert_eq!(h.query(&["cpu"])["label"]["value"], "42%");
    let r = runs(&fx);
    assert_eq!(r[0].sender(), Some("provider"));
    let info: serde_json::Value = serde_json::from_str(r[0].get("INFO").unwrap()).unwrap();
    assert_eq!(info["percent"], "42");
    let (_, fx) = h.msg_fx(&["--remove", "cpu"]);
    assert!(platform(&fx).contains(&PlatformRequest::StopProvider { item: cpu }));

    h.msg(&[
        "--add",
        "app_menu",
        "appmenu",
        "left",
        "--add",
        "item",
        "w",
        "left",
        "--set",
        "w",
        "script=w.sh",
        "--subscribe",
        "w",
        "menus_change",
    ]);
    let fx = h.input(mbar_core::platform::Input::MenuTitles {
        app: "Finder".into(),
        titles: vec!["Apple".into(), "Finder".into(), "File".into()],
    });
    assert_eq!(
        runs(&fx)[0].get("INFO"),
        Some("[\"Apple\",\"Finder\",\"File\"]")
    );
    assert_eq!(
        h.msg(&["--query", "menus"])
            .trim_end()
            .replace([' ', '\n', '\t'], ""),
        "[\"Apple\",\"Finder\",\"File\"]"
    );
    let (_, fx) = h.msg_fx(&["--menu", "File"]);
    assert!(platform(&fx).contains(&PlatformRequest::OpenMenu { index: 2 }));
    assert!(fx.iter().any(|e| matches!(e, Effect::Reply { .. })));
}
