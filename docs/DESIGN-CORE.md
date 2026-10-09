# mbar-core design brief

This document fixes the interfaces of `mbar-core` so that modules can be implemented in
parallel. Behaviour comes from `docs/spec/*.md`; this file decides *structure*.

## Principles

* Single-threaded state machine. No globals. Everything lives in `Runtime`.
* Platform-facing surface is small and explicit: `Resources` (metrics in), `Input`
  (events in), `Effect` + `FrameOutput` (actions out).
* Rendering is **one window per bar per display** (not one window per item like
  SketchyBar) plus one window per visible popup. Items are drawn into their bar's
  scene (parts of an item window outside its bar/popup window are clipped, deviation
  D21). Items with `blur_radius > 0` produce a `BlurRegion` in the scene; the platform
  realises it with a child window behind the bar window.
* Every SketchyBar quirk documented in the spec is reproduced unless
  `docs/EXTENSIONS.md` lists a deliberate deviation.

## Module map and key types

```
color.rs       Color                                   (done)
value.rs       parse_int/parse_u32/parse_float/parse_bool (done)
geometry.rs    Point/Size/Rect                         (done)
props.rs       PropCx, PropError, AnimTarget, AnimSlot
components/
  font.rs      FontSpec { family, style, size, features } + parse "Family:Style:Size"
  shadow.rs    Shadow
  image.rs     Image, ImageSource
  background.rs Background
  text.rs      Text (icon/label/knob)
  graph.rs     Graph
  slider.rs    Slider
  alias.rs     Alias
  app_menu.rs  AppMenu (extension)
item.rs        BarItem, ItemType, Position, Align
bar.rs         BarProps (`--bar` properties)
popup.rs       Popup
group.rs       Bracket membership helpers
layout.rs      layout(&Runtime state, displays, resources) -> per-window geometry
scene.rs       Scene, Primitive
animation.rs   Curve, Animation, Animator
command.rs     parse(argv) -> Vec<Command>
query.rs       JSON for --query
borders/       window borders (JankyBorders take-over, extension): BorderSettings,
               BordersState (`--borders`, `--query borders`, held in Model), parse.rs
               (JankyBorders argument grammar), pure helpers the platform uses
event.rs       EventKind, EventMask, CustomEvents, EventInfo
script.rs      ScriptEnv building
provider.rs    native providers: names, templates
platform.rs    Resources, Input, Effect, DisplayInfo, SpaceInfo, WindowKey, FrameOutput
runtime.rs     Runtime
```

## Property setting & animation

```rust
pub struct PropCx<'a> {
    pub animation: Option<AnimSpec>,          // set while inside --animate
    pub target: AnimTarget,                   // Bar | Item(ItemId) | Default
    pub prefix: String,                       // e.g. "icon.background."
    pub anims: &'a mut Vec<PendingAnim>,      // collected, applied by Runtime
    pub changed: bool,                        // needs relayout/redraw
}
pub enum AnimValue { Int(i32), Float(f32), Color(u32) }
pub enum AnimSlot<'a> { Int(&'a mut i32), Float(&'a mut f32), Color(&'a mut Color), UInt(&'a mut u32) }
```

Each component implements:

```rust
fn set_prop(&mut self, key: &str, value: &str, cx: &mut PropCx) -> Result<(), PropError>;
fn anim_slot(&mut self, key: &str) -> Option<AnimSlot<'_>>;   // for animatable keys
fn to_json(&self) -> serde_json::Value;                       // --query fragment
```

Animatable setters call `cx.set_int(&mut self.x, v, "x")`, `cx.set_float`,
`cx.set_color`. Without animation these write directly; with animation they push a
`PendingAnim { target, path: prefix+key, from, to }` and leave the field unchanged.
Every frame the `Animator` resolves `target + path` through `anim_slot` and writes the
interpolated value. Cancellation/chaining rules follow `docs/spec/events.md`.

## Runtime

```rust
impl Runtime {
    pub fn new(config: RuntimeConfig) -> Self;
    pub fn handle(&mut self, input: Input, res: &mut dyn Resources) -> Vec<Effect>;
    pub fn frame(&mut self, now: Instant, res: &mut dyn Resources) -> FrameOutput;
    pub fn next_deadline(&self) -> Option<Instant>;
}
```

`handle` never draws. It marks dirty state. The platform calls `frame` when
`needs_frame()` is true or at `next_deadline()`.

## Platform surface

```rust
pub trait Resources {
    fn text_metrics(&mut self, font: &FontSpec, text: &str) -> TextMetrics; // width, ascent, descent, key
    fn image_info(&mut self, source: &ImageSource) -> Option<ImageInfo>;    // key, size
    fn displays(&self) -> &[DisplayInfo];
    fn spaces(&self) -> &[SpaceInfo];
}

pub enum Input {
    Message { args: Vec<String>, reply: ReplyToken },
    Event(OsEvent),                    // front app, space change, volume, ...
    Mouse(MouseInput),
    Timer,                             // deadline reached
    ScriptFinished { pid: u32, item: Option<String>, output: Option<String> },
    ProviderSample { provider: String, values: Vec<(String, String)> },
    AliasImage { owner: String, name: String, image: Option<ImageInfo> },
    DisplaysChanged,
    Lua(LuaRequest),                   // from mbar-lua
}

pub enum Effect {
    Reply { reply: ReplyToken, text: String },
    RunScript { script: String, env: Vec<(String, String)>, item: Option<String> },
    Exit,
    Reload,
    Platform(PlatformRequest),         // hide menubar, open app menu, set provider freq, ...
    LuaCallback { handler: u64, env: Vec<(String, String)> },
}
```

`PlatformRequest::SetBorders(Box<BordersUpdate>)` carries the complete borders
configuration (drawing, global settings, `apply-to` overrides) plus an update mask. The
runtime emits it once per `--borders` message that changed something. The borders
configuration survives `--reload` and hotload (carried over into the new model, no
`SetBorders`; the re-run config applies its keys on top). The core never sees windows:
the platform tracks them and draws the border windows itself
(`docs/superpowers/specs/2026-10-09-borders-design.md`).

`FrameOutput { windows: Vec<WindowUpdate>, closed: Vec<WindowKey> }` where
`WindowUpdate { key, frame: Rect (screen points), level: WindowLevel, scene: Scene,
blur_radius, corner_radius, topmost, sticky, shadow }`.

## Scene

```rust
pub enum Primitive {
    Rect { rect, color, corner_radius, border_width, border_color },   // SDF quad
    Shadow { rect, color, corner_radius },                             // offset copy
    Text { origin, key: TextKey, color, clip: Option<Rect> },
    Image { rect, key: ImageKey, corner_radius, border_width, border_color },
    Graph { rect, points: Vec<f32>, line_color, fill_color, line_width, fill: bool },
    PushClip { rect, corner_radius }, PopClip,
    BlurRegion { rect, corner_radius, radius },
}
```

Coordinates: points, origin top-left of the window. The renderer multiplies by the
backing scale.

## Testing

* Unit tests per module.
* `tests/` integration: drive `Runtime` with argv sequences from `docs/spec/examples.md`
  and assert `--query` JSON.
* `HeadlessResources`: monospace metrics (`width = 0.6 * size * chars`,
  ascent `0.8*size`, descent `0.2*size`), fixed display 1920×1080, one space.
