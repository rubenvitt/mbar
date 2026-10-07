//! WP-B: tokenizer, `parse` and BRE translation (`docs/spec/cli.md` §3–§9, §11, extensions).

use mbar_core::animation::Curve;
use mbar_core::command::{
    bre_to_regex, parse, AddCommand, Command, MenuBarAction, MonitorMode, Placement, QueryTarget,
    Selector, SetToken, Tokens,
};

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn p(a: &[&str]) -> Vec<Command> {
    parse(&argv(a))
}

fn pair(k: &str, v: &str) -> SetToken {
    SetToken::Pair {
        key: k.into(),
        value: v.into(),
    }
}

fn bad(t: &str) -> SetToken {
    SetToken::Malformed(t.into())
}

fn set(target: &str, tokens: Vec<SetToken>) -> Command {
    Command::Set {
        target: Selector::parse(target),
        tokens,
    }
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

// ---------------------------------------------------------------------------
// Tokens (get_token, Mode A stop test, get_batch_line)
// ---------------------------------------------------------------------------

#[test]
fn get_token_returns_tokens_then_empty_forever() {
    let a = argv(&["a", "b"]);
    let mut t = Tokens::new(&a);
    assert_eq!(t.next_token(), "a");
    assert_eq!(t.next_token(), "b");
    assert_eq!(t.next_token(), "");
    assert_eq!(t.next_token(), "");
}

#[test]
fn get_token_stops_at_empty_element() {
    let a = argv(&["a", "", "b"]);
    let mut t = Tokens::new(&a);
    assert_eq!(t.next_token(), "a");
    assert_eq!(t.next_token(), "");
    assert_eq!(t.next_token(), "");
    assert!(!t.next_starts_with_dash());
    assert!(t.batch_line().is_empty());

    let a = argv(&["", "b"]);
    let mut t = Tokens::new(&a);
    assert_eq!(t.next_token(), "");
    assert_eq!(t.next_token(), "");

    let a: Vec<String> = Vec::new();
    let mut t = Tokens::new(&a);
    assert_eq!(t.next_token(), "");
    assert!(t.batch_line().is_empty());
}

#[test]
fn dash_check_looks_at_the_next_token() {
    let a = argv(&["x=1", "-y", "z"]);
    let mut t = Tokens::new(&a);
    assert!(!t.next_starts_with_dash());
    assert_eq!(t.next_token(), "x=1");
    assert!(t.next_starts_with_dash());
    assert_eq!(t.next_token(), "-y");
    assert!(!t.next_starts_with_dash());
    assert_eq!(t.next_token(), "z");
    assert!(!t.next_starts_with_dash());
}

#[test]
fn batch_line_takes_first_token_unconditionally() {
    let a = argv(&["--set", "x", "y", "--bar", "q"]);
    let mut t = Tokens::new(&a);
    assert_eq!(t.batch_line(), vec!["--set", "x", "y"]);
    assert_eq!(t.next_token(), "--bar");
    assert_eq!(t.batch_line(), vec!["q"]);
    assert_eq!(t.next_token(), "");
}

#[test]
fn batch_line_stops_at_empty_element() {
    let a = argv(&["a", "b", "", "c"]);
    let mut t = Tokens::new(&a);
    assert_eq!(t.batch_line(), vec!["a", "b"]);
    assert_eq!(t.next_token(), "");
    assert!(t.batch_line().is_empty());
}

// ---------------------------------------------------------------------------
// Dispatch loop and Mode A quirks
// ---------------------------------------------------------------------------

#[test]
fn empty_message() {
    assert!(p(&[]).is_empty());
    assert!(p(&[""]).is_empty());
    assert!(p(&["", "--update"]).is_empty());
}

#[test]
fn set_chaining() {
    assert_eq!(
        p(&["--set", "a", "k=v", "--set", "b", "k=w", "l=x"]),
        vec![
            set("a", vec![pair("k", "v")]),
            set("b", vec![pair("k", "w"), pair("l", "x")]),
        ]
    );
}

#[test]
fn set_value_rules() {
    // split at the first '=', empty value, '=' in value, empty key, value starting with '-'.
    assert_eq!(
        p(&[
            "--set",
            "a",
            "label=",
            "x=a=b",
            "=v",
            "y_offset=-5",
            "label=a b"
        ]),
        vec![set(
            "a",
            vec![
                pair("label", ""),
                pair("x", "a=b"),
                pair("", "v"),
                pair("y_offset", "-5"),
                pair("label", "a b"),
            ]
        )]
    );
}

#[test]
fn set_without_pairs_swallows_next_command() {
    // cli.md §3.2: `--set foo --set bar label=x` applies label=x to foo.
    assert_eq!(
        p(&["--set", "foo", "--set", "bar", "label=x"]),
        vec![set(
            "foo",
            vec![bad("--set"), bad("bar"), pair("label", "x")]
        )]
    );
}

#[test]
fn set_malformed_continues() {
    assert_eq!(
        p(&["--set", "a", "x=1", "oops", "y=2", "--update"]),
        vec![
            set("a", vec![pair("x", "1"), bad("oops"), pair("y", "2")]),
            Command::Update
        ]
    );
}

#[test]
fn set_missing_target_range_matches_batch_line() {
    // `--set missing --bar height=10`: the runtime discards exactly these tokens.
    assert_eq!(
        p(&["--set", "missing", "--bar", "height=10", "--update"]),
        vec![
            set("missing", vec![bad("--bar"), pair("height", "10")]),
            Command::Update
        ]
    );
}

#[test]
fn set_without_target() {
    assert_eq!(p(&["--set"]), vec![set("", vec![])]);
    assert_eq!(p(&["--set", "a"]), vec![set("a", vec![])]);
}

#[test]
fn set_regex_target() {
    assert_eq!(
        p(&["--set", "/space\\..*/", "icon=x"]),
        vec![Command::Set {
            target: Selector::Regex("/space\\..*/".into()),
            tokens: vec![pair("icon", "x")]
        }]
    );
    assert_eq!(Selector::parse("/").pattern(), None);
    assert_eq!(Selector::parse("//"), Selector::Regex("//".into()));
    assert_eq!(Selector::parse("//").pattern(), Some(""));
    assert_eq!(Selector::parse("/a"), Selector::Name("/a".into()));
}

#[test]
fn bare_dash_token_after_first_pair_ends_list() {
    assert_eq!(
        p(&["--set", "a", "x=1", "-5", "y=2"]),
        vec![
            set("a", vec![pair("x", "1")]),
            Command::UnknownDomain("-5".into())
        ]
    );
}

#[test]
fn empty_argv_element_ends_message() {
    assert_eq!(
        p(&["--set", "a", "label=", "", "--set", "b", "label=x"]),
        vec![set("a", vec![pair("label", "")])]
    );
    assert_eq!(
        p(&["--set", "a", "", "--set", "b", "label=x"]),
        vec![set("a", vec![])]
    );
    assert_eq!(p(&["--update", "", "--exit"]), vec![Command::Update]);
}

#[test]
fn bar_pairs_and_malformed_break() {
    assert_eq!(
        p(&["--bar", "height=30", "color=0xff000000", "--update"]),
        vec![
            Command::Bar {
                pairs: pairs(&[("height", "30"), ("color", "0xff000000")]),
                malformed: None
            },
            Command::Update
        ]
    );
    // cli.md §5: `--bar foo height=30` → malformed, then `height=30` is a command.
    assert_eq!(
        p(&["--bar", "foo", "height=30", "x=1", "--update"]),
        vec![
            Command::Bar {
                pairs: vec![],
                malformed: Some("foo".into())
            },
            Command::UnknownDomain("height=30".into()),
            Command::Update
        ]
    );
}

#[test]
fn bar_without_pairs_swallows_next_command() {
    // cli.md §3.2: `--bar --set foo x=1`.
    assert_eq!(
        p(&["--bar", "--set", "foo", "x=1", "--update"]),
        vec![
            Command::Bar {
                pairs: vec![],
                malformed: Some("--set".into())
            },
            Command::UnknownDomain("foo".into()),
            Command::Update
        ]
    );
    assert_eq!(
        p(&["--bar"]),
        vec![Command::Bar {
            pairs: vec![],
            malformed: None
        }]
    );
}

#[test]
fn default_pairs_and_malformed_break() {
    assert_eq!(
        p(&["--default", "icon.color=0xff00ff00", "label.font=A:B:12"]),
        vec![Command::Default {
            pairs: pairs(&[("icon.color", "0xff00ff00"), ("label.font", "A:B:12")]),
            malformed: None
        }]
    );
    assert_eq!(
        p(&["--default", "a=1", "reset", "b=2"]),
        vec![
            Command::Default {
                pairs: pairs(&[("a", "1")]),
                malformed: Some("reset".into())
            },
            Command::UnknownDomain("b=2".into())
        ]
    );
    assert_eq!(
        p(&["--default", "--set", "a"]),
        vec![
            Command::Default {
                pairs: vec![],
                malformed: Some("--set".into())
            },
            Command::UnknownDomain("a".into())
        ]
    );
}

// ---------------------------------------------------------------------------
// Mode C
// ---------------------------------------------------------------------------

#[test]
fn animate_reads_two_tokens_without_dash_check() {
    assert_eq!(
        p(&["--animate", "sin", "30", "--set", "a", "y=1"]),
        vec![
            Command::Animate {
                curve: Curve::Sin,
                duration: 30
            },
            set("a", vec![pair("y", "1")])
        ]
    );
    assert_eq!(
        p(&["--animate", "--set", "x"]),
        vec![Command::Animate {
            curve: Curve::Linear,
            duration: 0
        }]
    );
    // First-char matching and strtoul(…, 0) durations.
    let cases = [
        ("linear", "0x10", Curve::Linear, 16),
        ("quadratic", "010", Curve::Quadratic, 8),
        ("tanh", "5frames", Curve::Tanh, 5),
        ("smooth", "1", Curve::Sin, 1),
        ("exp", "2", Curve::Exp, 2),
        ("circ", "3", Curve::Circ, 3),
        ("bounce", "4", Curve::Bounce, 4),
        ("overshoot", "6", Curve::Overshoot, 6),
        ("xyz", "-1", Curve::Linear, u32::MAX),
    ];
    for (c, d, curve, duration) in cases {
        assert_eq!(
            p(&["--animate", c, d]),
            vec![Command::Animate { curve, duration }],
            "{c} {d}"
        );
    }
    assert_eq!(
        p(&["--animate"]),
        vec![Command::Animate {
            curve: Curve::Linear,
            duration: 0
        }]
    );
}

#[test]
fn update_hotload_load_font() {
    assert_eq!(
        p(&[
            "--update",
            "--hotload",
            "on",
            "--load-font",
            "/f.ttf",
            "--update"
        ]),
        vec![
            Command::Update,
            Command::Hotload("on".into()),
            Command::LoadFont("/f.ttf".into()),
            Command::Update
        ]
    );
    // Mode C (1) takes a `-` token as its argument.
    assert_eq!(
        p(&["--hotload", "--update"]),
        vec![Command::Hotload("--update".into())]
    );
    assert_eq!(p(&["--hotload"]), vec![Command::Hotload("".into())]);
    // `--update` takes no argument: the next token is a command.
    assert_eq!(
        p(&["--update", "foo"]),
        vec![Command::Update, Command::UnknownDomain("foo".into())]
    );
}

#[test]
fn exit_ends_parsing() {
    assert_eq!(
        p(&["--update", "--exit", "--update"]),
        vec![Command::Update, Command::Exit]
    );
}

// ---------------------------------------------------------------------------
// Mode B domains
// ---------------------------------------------------------------------------

fn add(t: &str, name: &str, position: &str, args: &[&str]) -> Command {
    Command::Add(AddCommand {
        item_type: t.into(),
        name: name.into(),
        position: position.into(),
        args: s(args),
    })
}

#[test]
fn add_items() {
    assert_eq!(
        p(&["--add", "item", "foo", "left", "--set", "foo", "a=b"]),
        vec![
            add("item", "foo", "left", &[]),
            set("foo", vec![pair("a", "b")])
        ]
    );
    assert_eq!(
        p(&["--add", "graph", "g", "right", "100", "extra"]),
        vec![add("graph", "g", "right", &["100", "extra"])]
    );
    assert_eq!(
        p(&["--add", "slider", "s", "popup.host"]),
        vec![add("slider", "s", "popup.host", &[])]
    );
    assert_eq!(
        p(&["--add", "alias", "Control Center,Battery", "right"]),
        vec![add("alias", "Control Center,Battery", "right", &[])]
    );
    assert_eq!(
        p(&["--add", "bracket", "b", "a", "/space\\..*/", "c"]),
        vec![add("bracket", "b", "a", &["/space\\..*/", "c"])]
    );
    assert_eq!(p(&["--add"]), vec![add("", "", "", &[])]);
    assert_eq!(p(&["--add", "item"]), vec![add("item", "", "", &[])]);
    // The first argument is swallowed even when it starts with '-'.
    assert_eq!(
        p(&["--add", "--set", "x", "--update"]),
        vec![add("--set", "x", "", &[]), Command::Update]
    );
    // Arguments cannot start with '-'.
    assert_eq!(
        p(&["--add", "item", "foo", "-left"]),
        vec![
            add("item", "foo", "", &[]),
            Command::UnknownDomain("-left".into())
        ]
    );
}

#[test]
fn add_event() {
    assert_eq!(
        p(&["--add", "event", "my_event"]),
        vec![Command::AddEvent {
            name: "my_event".into(),
            notification: None
        }]
    );
    assert_eq!(
        p(&["--add", "event", "e", "com.apple.note", "ignored"]),
        vec![Command::AddEvent {
            name: "e".into(),
            notification: Some("com.apple.note".into())
        }]
    );
    assert_eq!(
        p(&["--add", "event"]),
        vec![Command::AddEvent {
            name: "".into(),
            notification: None
        }]
    );
    assert_eq!(
        p(&["--add", "event", "e", "--update"]),
        vec![
            Command::AddEvent {
                name: "e".into(),
                notification: None
            },
            Command::Update
        ]
    );
}

#[test]
fn clone_placement() {
    let clone = |pl: Option<Placement>| Command::Clone {
        name: "new".into(),
        parent: "parent".into(),
        placement: pl,
    };
    assert_eq!(p(&["--clone", "new", "parent"]), vec![clone(None)]);
    assert_eq!(
        p(&["--clone", "new", "parent", "before"]),
        vec![clone(Some(Placement::Before))]
    );
    assert_eq!(
        p(&["--clone", "new", "parent", "after"]),
        vec![clone(Some(Placement::After))]
    );
    assert_eq!(
        p(&["--clone", "new", "parent", "Before"]),
        vec![clone(None)]
    );
    assert_eq!(
        p(&["--clone"]),
        vec![Command::Clone {
            name: "".into(),
            parent: "".into(),
            placement: None
        }]
    );
}

#[test]
fn subscribe_push_trigger() {
    assert_eq!(
        p(&[
            "--subscribe",
            "a",
            "front_app_switched",
            "mouse.clicked",
            "--update"
        ]),
        vec![
            Command::Subscribe {
                item: "a".into(),
                events: s(&["front_app_switched", "mouse.clicked"])
            },
            Command::Update
        ]
    );
    assert_eq!(
        p(&["--subscribe"]),
        vec![Command::Subscribe {
            item: "".into(),
            events: vec![]
        }]
    );
    assert_eq!(
        p(&["--push", "g", "0.5", "1", "1e-1", "abc", ".25x"]),
        vec![Command::Push {
            item: "g".into(),
            values: vec![0.5, 1.0, 0.1, 0.0, 0.25]
        }]
    );
    // cli.md §3.2: `--push g -0.5`.
    assert_eq!(
        p(&["--push", "g", "-0.5", "0.3"]),
        vec![
            Command::Push {
                item: "g".into(),
                values: vec![]
            },
            Command::UnknownDomain("-0.5".into())
        ]
    );
    assert_eq!(
        p(&["--trigger", "my_event", "FOO=bar", "x", "K="]),
        vec![Command::Trigger {
            event: "my_event".into(),
            args: s(&["FOO=bar", "x", "K="])
        }]
    );
    assert_eq!(
        p(&["--trigger"]),
        vec![Command::Trigger {
            event: "".into(),
            args: vec![]
        }]
    );
}

#[test]
fn query_targets() {
    let q = |a: &[&str]| {
        let mut v = vec!["--query"];
        v.extend_from_slice(a);
        p(&v)
    };
    let one = |t: QueryTarget| vec![Command::Query(t)];
    assert_eq!(q(&["bar"]), one(QueryTarget::Bar));
    assert_eq!(q(&["defaults"]), one(QueryTarget::Defaults));
    assert_eq!(q(&["events"]), one(QueryTarget::Events));
    assert_eq!(q(&["displays"]), one(QueryTarget::Displays));
    assert_eq!(
        q(&["default_menu_items"]),
        one(QueryTarget::DefaultMenuItems)
    );
    assert_eq!(q(&["item", "bar"]), one(QueryTarget::Item("bar".into())));
    assert_eq!(q(&["item"]), one(QueryTarget::Item("".into())));
    assert_eq!(q(&["foo"]), one(QueryTarget::Name("foo".into())));
    assert_eq!(q(&["foo", "extra"]), one(QueryTarget::Name("foo".into())));
    assert_eq!(q(&[]), one(QueryTarget::Name("".into())));
    assert_eq!(q(&["Bar"]), one(QueryTarget::Name("Bar".into())));
    assert_eq!(q(&["stats"]), one(QueryTarget::Stats));
    assert_eq!(q(&["menus"]), one(QueryTarget::Menus));
    // Several queries in one message.
    assert_eq!(
        p(&["--query", "bar", "--query", "item", "x"]),
        vec![
            Command::Query(QueryTarget::Bar),
            Command::Query(QueryTarget::Item("x".into()))
        ]
    );
}

#[test]
fn reorder_move_remove_rename() {
    assert_eq!(
        p(&["--reorder", "a", "b", "c"]),
        vec![Command::Reorder(s(&["a", "b", "c"]))]
    );
    assert_eq!(p(&["--reorder"]), vec![Command::Reorder(vec![])]);
    let mv = |before: bool| Command::Move {
        item: "a".into(),
        before,
        reference: "b".into(),
    };
    assert_eq!(p(&["--move", "a", "before", "b"]), vec![mv(true)]);
    assert_eq!(p(&["--move", "a", "after", "b"]), vec![mv(false)]);
    assert_eq!(p(&["--move", "a", "behind", "b"]), vec![mv(false)]);
    assert_eq!(p(&["--move", "a", "BEFORE", "b"]), vec![mv(false)]);
    assert_eq!(
        p(&["--remove", "/space\\..*/"]),
        vec![Command::Remove(Selector::Regex("/space\\..*/".into()))]
    );
    assert_eq!(
        p(&["--remove", "foo"]),
        vec![Command::Remove(Selector::Name("foo".into()))]
    );
    assert_eq!(
        p(&["--remove"]),
        vec![Command::Remove(Selector::Name("".into()))]
    );
    assert_eq!(
        p(&["--rename", "old", "new"]),
        vec![Command::Rename {
            old: "old".into(),
            new: "new".into()
        }]
    );
    assert_eq!(
        p(&["--rename", "old"]),
        vec![Command::Rename {
            old: "old".into(),
            new: "".into()
        }]
    );
}

#[test]
fn reload() {
    assert_eq!(p(&["--reload"]), vec![Command::Reload(None)]);
    assert_eq!(
        p(&["--reload", "/tmp/rc", "x"]),
        vec![Command::Reload(Some("/tmp/rc".into()))]
    );
    assert_eq!(
        p(&["--reload", "--update"]),
        vec![Command::Reload(Some("--update".into()))]
    );
}

#[test]
fn unknown_domain_skips_its_batch_line() {
    assert_eq!(
        p(&["--foo", "a", "b", "--update"]),
        vec![Command::UnknownDomain("--foo".into()), Command::Update]
    );
    // Matching is exact and case-sensitive; no short aliases.
    assert_eq!(
        p(&["--SET", "a", "x=1"]),
        vec![Command::UnknownDomain("--SET".into())]
    );
    assert_eq!(p(&["-s"]), vec![Command::UnknownDomain("-s".into())]);
    // `--add component` is just an invalid --add type.
    assert_eq!(
        p(&["--add", "component", "x", "left"]),
        vec![add("component", "x", "left", &[])]
    );
}

#[test]
fn extensions() {
    assert_eq!(p(&["--monitor"]), vec![Command::Monitor(MonitorMode::All)]);
    assert_eq!(
        p(&["--monitor", "events"]),
        vec![Command::Monitor(MonitorMode::Events)]
    );
    assert_eq!(
        p(&["--monitor", "stats"]),
        vec![Command::Monitor(MonitorMode::Stats)]
    );
    assert_eq!(
        p(&["--monitor", "all"]),
        vec![Command::Monitor(MonitorMode::All)]
    );
    assert_eq!(p(&["--menu", "0"]), vec![Command::Menu("0".into())]);
    assert_eq!(p(&["--menu", "File"]), vec![Command::Menu("File".into())]);
    assert_eq!(p(&["--menu"]), vec![Command::Menu("".into())]);
    assert_eq!(
        p(&["--menubar", "hide"]),
        vec![Command::MenuBar(Some(MenuBarAction::Hide))]
    );
    assert_eq!(
        p(&["--menubar", "show"]),
        vec![Command::MenuBar(Some(MenuBarAction::Show))]
    );
    assert_eq!(
        p(&["--menubar", "toggle"]),
        vec![Command::MenuBar(Some(MenuBarAction::Toggle))]
    );
    assert_eq!(p(&["--menubar", "on"]), vec![Command::MenuBar(None)]);
    assert_eq!(p(&["--menubar"]), vec![Command::MenuBar(None)]);
}

#[test]
fn realistic_config_line() {
    let cmds = p(&[
        "--animate",
        "tanh",
        "20",
        "--bar",
        "y_offset=-32",
        "--set",
        "/space\\.[0-9]\\{1,2\\}/",
        "icon.highlight=on",
        "background.drawing=toggle",
        "--add",
        "event",
        "aerospace_workspace_change",
        "--subscribe",
        "clock",
        "system_woke",
        "routine",
        "--query",
        "clock",
    ]);
    assert_eq!(
        cmds,
        vec![
            Command::Animate {
                curve: Curve::Tanh,
                duration: 20
            },
            Command::Bar {
                pairs: pairs(&[("y_offset", "-32")]),
                malformed: None
            },
            set(
                "/space\\.[0-9]\\{1,2\\}/",
                vec![
                    pair("icon.highlight", "on"),
                    pair("background.drawing", "toggle")
                ]
            ),
            Command::AddEvent {
                name: "aerospace_workspace_change".into(),
                notification: None
            },
            Command::Subscribe {
                item: "clock".into(),
                events: s(&["system_woke", "routine"])
            },
            Command::Query(QueryTarget::Name("clock".into())),
        ]
    );
}

// ---------------------------------------------------------------------------
// BRE translation
// ---------------------------------------------------------------------------

fn re(pattern: &str) -> regex::Regex {
    let translated =
        bre_to_regex(pattern).unwrap_or_else(|_| panic!("{pattern:?} should translate"));
    regex::Regex::new(&translated)
        .unwrap_or_else(|e| panic!("{pattern:?} → {translated:?} does not compile: {e}"))
}

fn matches(pattern: &str, yes: &[&str], no: &[&str]) {
    let r = re(pattern);
    for y in yes {
        assert!(r.is_match(y), "{pattern:?} should match {y:?} ({r})");
    }
    for n in no {
        assert!(!r.is_match(n), "{pattern:?} should not match {n:?} ({r})");
    }
}

#[test]
fn bre_ordinary_and_unanchored() {
    matches("foo", &["foo", "xfoox"], &["fo", "FOO"]);
    matches("a.c", &["abc", "a.c", "a\nc"], &["ac"]);
    matches("a\\.c", &["a.c"], &["abc"]);
    matches("é.", &["éa"], &["e"]);
}

#[test]
fn bre_ere_operators_are_literal() {
    matches("a|b", &["a|b"], &["a", "b"]);
    matches("a+", &["a+"], &["a", "aa"]);
    matches("a?", &["a?"], &["a", ""]);
    matches("(a)", &["(a)"], &["a"]);
    matches("a{2}", &["a{2}"], &["aa"]);
    matches("x}", &["x}"], &["x"]);
}

#[test]
fn bre_escaped_operators() {
    matches("a\\|b", &["a", "b", "xbx"], &["c"]);
    matches("^a\\|b$", &["ax", "xb"], &["xa", "bx"]);
    matches("a\\+", &["a", "aaa"], &["b"]);
    matches("^ab\\+c$", &["abc", "abbbc"], &["ac"]);
    matches("^ab\\?c$", &["abc", "ac"], &["abbc"]);
    matches("^a\\{2\\}$", &["aa"], &["a", "aaa"]);
    matches("^a\\{2,\\}$", &["aa", "aaaa"], &["a"]);
    matches("^a\\{1,3\\}$", &["a", "aaa"], &["", "aaaa"]);
    matches("^\\(ab\\)\\{2\\}$", &["abab"], &["ab"]);
    matches("^\\(a\\|b\\)c$", &["ac", "bc"], &["cc"]);
    matches("\\(x\\)*y", &["y", "xxy"], &[]);
}

#[test]
fn bre_star() {
    matches("^ab*c$", &["ac", "abc", "abbbc"], &["adc"]);
    // `*` at the start of an RE, after `\(`, `\|` or `^` is literal.
    matches("*a", &["*a", "x*a"], &["a"]);
    matches("^*a", &["*a"], &["a", "x*a"]);
    matches("\\(*a\\)", &["*a"], &["a"]);
    matches("x\\|*a", &["x", "*a"], &["a"]);
    // Stacked quantifiers compile and apply to the quantified atom.
    matches("^a**$", &["", "aaa"], &["b"]);
    matches("^a*\\?$", &["", "aa"], &["b"]);
    matches("^a\\+*$", &["", "aa"], &["b"]);
    // `\+`/`\?` at the start of an RE are literal like `*`.
    matches("\\+a", &["+a"], &["a"]);
}

#[test]
fn bre_anchors() {
    matches("^a", &["ab"], &["ba"]);
    matches("a$", &["ba"], &["ab"]);
    matches("^a$", &["a"], &["aa", ""]);
    // Elsewhere `^`/`$` are literal.
    matches("a^b", &["a^b"], &["ab"]);
    matches("a$b", &["a$b"], &["ab"]);
    matches("^^", &["^x"], &["x"]);
    matches("$$", &["x$"], &["x"]);
    matches("x\\(^a$\\)", &[], &["xa", "x^a$"]);
    matches("\\(^a$\\)", &["a"], &["ba"]);
}

#[test]
fn bre_bracket_expressions() {
    matches("[[:digit:]]", &["x1"], &["xy"]);
    matches("^[[:alpha:]]*$", &["abc", ""], &["a1"]);
    matches("^[^[:digit:]]$", &["a", "-"], &["1"]);
    matches("[[:space:][:punct:]]", &[" ", "."], &["a"]);
    matches("^[]a]$", &["]", "a"], &["b"]);
    matches("^[^]a]$", &["b"], &["]", "a"]);
    matches("^[a-]$", &["a", "-"], &["b"]);
    matches("^[-a]$", &["a", "-"], &["b"]);
    matches("^[a-c]$", &["a", "b", "c"], &["d", "-"]);
    matches("^[\\]$", &["\\"], &["a"]);
    matches("^[\\n]$", &["\\", "n"], &["\n"]);
    matches("^[[]$", &["["], &["a"]);
    matches("^[&~^]$", &["&", "~", "^"], &["a"]);
    matches("^[a&&b]$", &["a", "&", "b"], &["c"]);
    matches("^[x[.-.]]$", &["x", "-"], &["a"]);
    matches("^[[=a=]]$", &["a"], &["b"]);
    matches(
        "[.*+?(){}|]",
        &[".", "*", "+", "?", "(", ")", "{", "}", "|"],
        &["a"],
    );
    matches("space\\.[0-9]", &["space.1"], &["space.x", "spacex1"]);
}

#[test]
fn bre_escaped_literals() {
    matches("a\\*", &["a*"], &["a", "aa"]);
    matches("\\[x\\]", &["[x]"], &["x"]);
    matches("\\^a", &["b^a"], &[]);
    matches("a\\$b", &["a$b"], &[]);
    matches("a\\\\b", &["a\\b"], &["ab"]);
    matches("\\/", &["/"], &[]);
    matches("\\a", &["a"], &["b"]);
}

#[test]
fn bre_errors() {
    for bad in [
        "",           // REG_EMPTY (`//`)
        "\\(a",       // unbalanced
        "a\\)",       // unbalanced
        "[abc",       // REG_EBRACK
        "[",          // REG_EBRACK
        "[]",         // `]` first is literal → unterminated
        "\\(a\\)\\1", // back-references
        "a\\",        // trailing backslash
        "[[:foo:]]",  // REG_ECTYPE
        "[[:alpha:]", // unterminated
        "[[:alpha",   // unterminated class name
        "[z-a]",      // REG_ERANGE
        "[[.ab.]]",   // multi-char collating element
        "\\{2\\}",    // nothing to repeat
        "a\\{3,1\\}", // REG_BADBR
        "a\\{x\\}",   // REG_BADBR
        "a\\{2",      // REG_EBRACE
        "a\\{256\\}", // > RE_DUP_MAX
    ] {
        assert!(bre_to_regex(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn bre_selector_pattern_roundtrip() {
    let sel = Selector::parse("/^space\\.\\(1\\|2\\)$/");
    let r = re(sel.pattern().unwrap());
    assert!(r.is_match("space.1"));
    assert!(r.is_match("space.2"));
    assert!(!r.is_match("space.3"));
    assert!(bre_to_regex(Selector::parse("//").pattern().unwrap()).is_err());
}
