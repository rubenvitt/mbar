//! The stock `sketchybarrc` (`docs/spec/examples.md` §1.1) as argv messages.

mod wpc_common;
use mbar_core::platform::{Effect, PlatformRequest, PowerSource, SystemValue};
use wpc_common::*;

const P: &str = "/cfg/plugins";

pub fn stock(h: &mut H) {
    assert_eq!(
        h.msg(&[
            "--bar",
            "position=top",
            "height=40",
            "blur_radius=30",
            "color=0x40000000"
        ]),
        ""
    );
    assert_eq!(
        h.msg(&[
            "--default",
            "padding_left=5",
            "padding_right=5",
            "icon.font=Hack Nerd Font:Bold:17.0",
            "label.font=Hack Nerd Font:Bold:14.0",
            "icon.color=0xffffffff",
            "label.color=0xffffffff",
            "icon.padding_left=4",
            "icon.padding_right=4",
            "label.padding_left=4",
            "label.padding_right=4",
        ]),
        ""
    );
    for sid in 1..=10 {
        let name = format!("space.{sid}");
        let space = format!("space={sid}");
        let icon = format!("icon={sid}");
        let script = format!("script={P}/space.sh");
        let click = format!("click_script=yabai -m space --focus {sid}");
        assert_eq!(
            h.msg(&[
                "--add",
                "space",
                &name,
                "left",
                "--set",
                &name,
                &space,
                &icon,
                "icon.padding_left=7",
                "icon.padding_right=7",
                "background.color=0x40ffffff",
                "background.corner_radius=5",
                "background.height=25",
                "label.drawing=off",
                &script,
                &click,
            ]),
            ""
        );
    }
    assert_eq!(
        h.msg(&[
            "--add",
            "item",
            "chevron",
            "left",
            "--set",
            "chevron",
            "icon=\u{f054}",
            "label.drawing=off",
            "--add",
            "item",
            "front_app",
            "left",
            "--set",
            "front_app",
            "icon.drawing=off",
            &format!("script={P}/front_app.sh"),
            "--subscribe",
            "front_app",
            "front_app_switched",
        ]),
        ""
    );
    assert_eq!(
        h.msg(&[
            "--add",
            "item",
            "clock",
            "right",
            "--set",
            "clock",
            "update_freq=10",
            "icon=\u{f43a}",
            &format!("script={P}/clock.sh"),
            "--add",
            "item",
            "volume",
            "right",
            "--set",
            "volume",
            &format!("script={P}/volume.sh"),
            "--subscribe",
            "volume",
            "volume_change",
            "--add",
            "item",
            "battery",
            "right",
            "--set",
            "battery",
            "update_freq=120",
            &format!("script={P}/battery.sh"),
            "--subscribe",
            "battery",
            "system_woke",
            "power_source_change",
        ]),
        ""
    );
}

#[test]
fn stock_config_state() {
    let mut h = H::new();
    stock(&mut h);
    let bar = h.query(&["bar"]);
    assert_eq!(bar["position"], "top");
    assert_eq!(bar["height"], 40);
    assert_eq!(bar["blur_radius"], 30);
    assert_eq!(bar["color"], "0x40000000");
    assert_eq!(bar["drawing"], "on");
    let names: Vec<&str> = bar["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let mut expect: Vec<String> = (1..=10).map(|i| format!("space.{i}")).collect();
    expect.extend(["chevron", "front_app", "clock", "volume", "battery"].map(String::from));
    assert_eq!(names, expect);

    let d = h.query(&["defaults"]);
    assert_eq!(d["geometry"]["padding_left"], 5);
    assert_eq!(d["icon"]["font"], "Hack Nerd Font:Bold:17.00");
    assert_eq!(d["label"]["font"], "Hack Nerd Font:Bold:14.00");

    let s3 = h.query(&["space.3"]);
    assert_eq!(s3["type"], "space");
    assert_eq!(s3["geometry"]["associated_space_mask"], 8);
    assert_eq!(s3["geometry"]["background"]["drawing"], "on");
    assert_eq!(s3["icon"]["value"], "3");
    assert_eq!(s3["icon"]["padding_left"], 7);
    assert_eq!(s3["label"]["drawing"], "off");
    assert_eq!(s3["scripting"]["script"], format!("{P}/space.sh"));
    assert_eq!(s3["scripting"]["click_script"], "yabai -m space --focus 3");
    assert_eq!(s3["scripting"]["update_mask"].as_u64().unwrap() & 2, 2);

    let fa = h.query(&["front_app"]);
    assert_eq!(fa["scripting"]["update_mask"], 1);
    assert_eq!(fa["icon"]["drawing"], "off");
    let clock = h.query(&["clock"]);
    assert_eq!(clock["geometry"]["position"], "right");
    assert_eq!(clock["scripting"]["update_freq"], 10);
    assert_eq!(clock["scripting"]["update_mask"], 0);
    assert_eq!(h.query(&["volume"])["scripting"]["update_mask"], 4096);
    assert_eq!(h.query(&["battery"])["scripting"]["update_mask"], 16392);
    assert_eq!(h.query(&["chevron"])["label"]["drawing"], "off");
    assert!(h.rt.listeners().volume);
}

#[test]
fn stock_update() {
    let mut h = H::new();
    h.res.spaces = (1..=10)
        .map(|i| mbar_core::platform::SpaceInfo {
            id: i,
            display: 1,
            fullscreen: false,
        })
        .collect();
    stock(&mut h);
    h.res
        .system
        .insert("FrontApp".into(), SystemValue::Text("Safari".into()));
    h.res
        .system
        .insert("Volume".into(), SystemValue::Level(0.73));
    h.res
        .system
        .insert("PowerSource".into(), SystemValue::Power(PowerSource::Ac));
    let (rsp, fx) = h.msg_fx(&["--update"]);
    assert_eq!(rsp.as_deref(), Some(""));
    let all = runs(&fx);
    // space.1 is the selected space (headless: one display, space 1): space_change run.
    let s1 = runs_of(&fx, "space.1");
    assert_eq!(s1.len(), 2, "{s1:?}");
    assert_eq!(s1[0].sender(), Some("space_change"));
    assert_eq!(s1[0].get("SELECTED"), Some("true"));
    assert_eq!(s1[0].get("INFO"), Some("{\n\t\"display-1\": 1\n}"));
    assert_eq!(s1[0].get("NAME"), Some("space.1"));
    assert_eq!(s1[0].get("SID"), Some("1"));
    assert_eq!(s1[1].sender(), Some("forced"));
    // forced: all other spaces run on space_change too (forced handling), SELECTED=false.
    let s2 = runs_of(&fx, "space.2");
    assert_eq!(s2[0].sender(), Some("space_change"));
    assert_eq!(s2[0].get("SELECTED"), Some("false"));
    // D1: no variables leak from space.1 into later items.
    assert_eq!(runs_of(&fx, "front_app")[0].get("SID"), None);
    let front = runs_of(&fx, "front_app");
    assert_eq!(front[0].sender(), Some("front_app_switched"));
    assert_eq!(front[0].get("INFO"), Some("Safari"));
    assert_eq!(front[1].sender(), Some("forced"));
    let vol = runs_of(&fx, "volume");
    assert_eq!(vol[0].sender(), Some("volume_change"));
    assert_eq!(vol[0].get("INFO"), Some("73"));
    let bat = runs_of(&fx, "battery");
    assert_eq!(bat[0].sender(), Some("power_source_change"));
    assert_eq!(bat[0].get("INFO"), Some("AC"));
    assert_eq!(bat[1].sender(), Some("forced"));
    assert_eq!(runs_of(&fx, "clock").len(), 1);
    assert!(runs_of(&fx, "chevron").is_empty());
    assert!(all.iter().all(|r| r.script.starts_with(P)));
    // A real bar window is produced.
    assert!(h
        .rt
        .model
        .items
        .iter()
        .find(|i| i.name.as_deref() == Some("clock"))
        .unwrap()
        .is_shown());
}

#[test]
fn stock_routine_clock() {
    let mut h = H::new();
    stock(&mut h);
    h.msg(&["--update"]);
    // clock: update_freq=10 -> SENDER=routine every 10 s; battery every 120 s.
    let fx = h.advance(std::time::Duration::from_millis(30_500));
    let clock = runs_of(&fx, "clock");
    assert_eq!(clock.len(), 3, "{clock:?}");
    assert!(clock.iter().all(|r| r.sender() == Some("routine")));
    assert!(runs_of(&fx, "battery").is_empty());
    let fx = h.advance(std::time::Duration::from_secs(90));
    assert_eq!(runs_of(&fx, "battery").len(), 1);
    assert_eq!(runs_of(&fx, "clock").len(), 9);
    // No platform requests for idle ticks besides scripts.
    assert!(fx.iter().all(|e| matches!(
        e,
        Effect::RunScript { .. } | Effect::Platform(PlatformRequest::CaptureAlias { .. })
    )));
}
