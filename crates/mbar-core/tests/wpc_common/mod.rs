//! Shared harness for the WP-C runtime integration tests: drives `Runtime` with argv
//! messages through `HeadlessResources`.
#![allow(dead_code)]

use mbar_core::geometry::Point;
use mbar_core::platform::{
    Effect, FrameOutput, HeadlessResources, Input, MouseButton, MouseInput, MouseKind,
    PlatformRequest, ReplyToken, WindowKey,
};
use mbar_core::{Runtime, RuntimeConfig};
use std::collections::HashMap;
use std::time::Duration;

/// One `Effect::RunScript` / `Effect::LuaCallback`, flattened.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub script: String,
    pub item: Option<String>,
    pub env: Vec<(String, String)>,
}

impl Run {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    }
    pub fn sender(&self) -> Option<&str> {
        self.get("SENDER")
    }
}

pub struct H {
    pub rt: Runtime,
    pub res: HeadlessResources,
    next_reply: u64,
    /// Every effect produced so far (drained by `take`).
    pub effects: Vec<Effect>,
    pub last_frame: FrameOutput,
}

impl Default for H {
    fn default() -> Self {
        Self::new()
    }
}

impl H {
    pub fn new() -> H {
        Self::with_res(HeadlessResources::default())
    }

    pub fn with_res(mut res: HeadlessResources) -> H {
        let mut rt = Runtime::new(RuntimeConfig {
            bar_name: "mbar".into(),
            home: "/home/u".into(),
            config_path: None,
        });
        let fx = rt.begin(&mut res);
        let mut h = H {
            rt,
            res,
            next_reply: 1,
            effects: fx,
            last_frame: FrameOutput::default(),
        };
        h.frame();
        h
    }

    /// Runs one `Runtime::frame` at the current clock.
    pub fn frame(&mut self) -> FrameOutput {
        let now = self.res.now;
        let out = self.rt.frame(now, &mut self.res);
        self.last_frame = out.clone();
        out
    }

    /// Feeds an input (no frame).
    pub fn input(&mut self, input: Input) -> Vec<Effect> {
        let fx = self.rt.handle(input, &mut self.res);
        self.effects.extend(fx.clone());
        fx
    }

    /// Sends one IPC message, runs a frame, returns the reply (or `None` without reply)
    /// and the effects of the message.
    pub fn msg_fx(&mut self, args: &[&str]) -> (Option<String>, Vec<Effect>) {
        let reply = ReplyToken(self.next_reply);
        self.next_reply += 1;
        let fx = self.input(Input::Message {
            args: args.iter().map(|s| s.to_string()).collect(),
            reply,
        });
        let text = fx.iter().find_map(|e| match e {
            Effect::Reply { reply: r, text } if *r == reply => Some(text.clone()),
            _ => None,
        });
        self.frame();
        (text, fx)
    }

    /// Sends one message and returns its reply text.
    pub fn msg(&mut self, args: &[&str]) -> String {
        self.msg_fx(args).0.unwrap_or_default()
    }

    /// `--query <what...>` parsed as JSON.
    pub fn query(&mut self, what: &[&str]) -> serde_json::Value {
        let mut args = vec!["--query"];
        args.extend_from_slice(what);
        let text = self.msg(&args);
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad JSON ({e}): {text}"))
    }

    /// Advances the clock by `d` in steps of at most `step`, delivering `Input::Timer`
    /// whenever `next_deadline` is due, and a frame after each step.
    pub fn advance_by(&mut self, d: Duration, step: Duration) -> Vec<Effect> {
        let mut out = Vec::new();
        let end = self.res.now + d;
        while self.res.now < end {
            let next = (self.res.now + step).min(end);
            self.res.now = next;
            if self.rt.next_deadline().is_some_and(|t| t <= next) {
                out.extend(self.input(Input::Timer));
            }
            self.frame();
        }
        out
    }

    pub fn advance(&mut self, d: Duration) -> Vec<Effect> {
        self.advance_by(d, Duration::from_millis(100))
    }

    pub fn mouse(&mut self, kind: MouseKind, x: f32, y: f32, window: Option<WindowKey>) -> Vec<Effect> {
        let fx = self.input(Input::Mouse(MouseInput {
            kind,
            point: Point::new(x, y),
            window,
            modifiers: 256,
        }));
        self.frame();
        fx
    }

    pub fn click(&mut self, x: f32, y: f32) -> Vec<Effect> {
        self.mouse(
            MouseKind::Up {
                button: MouseButton::Left,
                button_code: 0,
            },
            x,
            y,
            Some(WindowKey::Bar(1)),
        )
    }

    /// Center of an item's virtual window on display 1.
    pub fn center(&mut self, name: &str) -> (f32, f32) {
        let q = self.query(&["item", name]);
        let r = &q["bounding_rects"]["display-1"];
        let x = r["origin"][0].as_f64().unwrap() + r["size"][0].as_f64().unwrap() / 2.0;
        let y = r["origin"][1].as_f64().unwrap() + r["size"][1].as_f64().unwrap() / 2.0;
        (x as f32, y as f32)
    }
}

/// Script runs (shell and Lua) in the effects, in order.
pub fn runs(fx: &[Effect]) -> Vec<Run> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::RunScript { script, env, item } => Some(Run {
                script: script.clone(),
                item: item.clone(),
                env: env.clone(),
            }),
            Effect::LuaCallback { handler, env } => Some(Run {
                script: format!("lua:{handler}"),
                item: None,
                env: env.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Script runs of one item.
pub fn runs_of(fx: &[Effect], item: &str) -> Vec<Run> {
    runs(fx)
        .into_iter()
        .filter(|r| r.item.as_deref() == Some(item))
        .collect()
}

pub fn platform(fx: &[Effect]) -> Vec<PlatformRequest> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Platform(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

pub fn env_map(r: &Run) -> HashMap<String, String> {
    r.env.iter().cloned().collect()
}
