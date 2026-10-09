# Window borders in mbar (JankyBorders take-over): design

Date: 2026-10-09. Status: accepted, being implemented.

mbar takes over [JankyBorders](https://github.com/FelixKratz/JankyBorders)
(`borders`, v1.9.0) the same way it took over SketchyBar: a behavioural spec
extracted from the C source ([`docs/spec/borders.md`](../../spec/borders.md),
FFI table in [`docs/spec/borders-skylight.md`](../../spec/borders-skylight.md)),
a compatible command line through a `borders -> mbar` link, the old config read
in place, documented deviations, and a setup step in `mbar.app` that stops and
removes the Homebrew service. Users drop the separate `borders` process.

## Goals

- Every documented JankyBorders option works with the same syntax and the same
  visual result: `active_color`, `inactive_color`, `background_color` (solid,
  `glow(…)`, both `gradient(…)` forms), `width`, `style` (round / square /
  uniform), `order` (above / below), `hidpi`, `ax_focus`, `blacklist`,
  `whitelist`, `apply-to`.
- Existing launch lines keep working: `borders active_color=… width=5.0 &` in a
  `yabairc`, `exec-and-forget borders …` in `aerospace.toml`, a `bordersrc`.
- No extra process, no extra IPC name, no extra login item.
- Borders are **off** until something configures them, so users who never used
  JankyBorders see no change.

## Non-goals (documented as deviations)

- The yabai proxy-window integration (`git.felix.jbevent`, spec §10). It only
  matters with yabai's window animations; mbar's users run AeroSpace. It can be
  added later without changing anything here.
- The `git.felix.borders` mach service. As with SketchyBar, compatibility goes
  through the `borders` link, not the foreign IPC name.
- JankyBorders' raw-MIG sub-level fast path (spec §7.6) and its
  `CGSGetConnectionPortById` symtab scan; mbar uses `SLSGetWindowSubLevel` /
  sub-level 0.
- Quirks marked "no" in spec §13 (memory-safety bugs, asserts, unbounded
  buffers, the sticky `apply-to` of a primary started with `apply-to=N`).

## User-facing surface

### 1. `--borders` domain (mbar command language, extension)

```
mbar --borders active_color=0xffe1e3e4 inactive_color=0xff494d64 width=5.0
mbar --borders drawing=off
mbar --query borders
```

- Key/value list like `--bar` (Mode A `pair_list`: stops at the next `-` token
  or a token without `=`; the malformed token is reported as
  `[!] Borders: Expected <key>=<value> pair, but got: '<tok>'`).
- Every JankyBorders key with the JankyBorders grammar (spec §2.3 incl. the
  sscanf leniency BR-PARSE-02, the prefix rules BR-PARSE-01, list rules
  BR-PARSE-08).
- Extension key `drawing=on|off` (mbar boolean parsing). The **first**
  `--borders` message that does not contain `drawing=` turns borders on
  (`drawing=on`); later messages only change `drawing` when they contain it.
- Invalid keys answer `[!] Borders: Invalid argument '<tok>'\n` and invalid
  colors `[!] Borders: Invalid color argument color<rest>\n` (mbar's `[!]`
  error convention: the client prints them on stderr and exits 1). Valid keys
  in the same message still apply.
- `--query borders` prints the configuration as JSON (an item named `borders`
  wins, like `--query stats`):

```json
{
	"drawing": "on",
	"active_color": "0xffe1e3e4",
	"inactive_color": "0x00000000",
	"background_color": "0x00000000",
	"width": 4.000000,
	"style": "round",
	"order": "below",
	"hidpi": "off",
	"ax_focus": "auto",
	"blacklist": [],
	"whitelist": [],
	"overrides": []
}
```

  Colors print as `0xAARRGGBB`, `glow(0xAARRGGBB)` or
  `gradient(top_left=0x…,bottom_right=0x…)` /
  `gradient(top_right=0x…,bottom_left=0x…)` (the input syntax). `ax_focus` is
  `auto` until set (auto = on when mbar has Accessibility permission, which is
  JankyBorders' default). `overrides` lists `{ "window": <wid>, …same keys }`.
- The borders configuration **survives** `--reload` and hotload (JankyBorders
  was a separate process that bar reloads never touched, so borders set by a
  window manager's launch line such as `exec-and-forget borders …` in
  `aerospace.toml` stay). The reload carries `model.borders` over into the new
  model and sends no `SetBorders`; the config and `bordersrc` (below) run again
  and apply their keys on top, which sends an update only when something
  changed. To turn borders off, send `--borders drawing=off`.

### 2. `borders` argv0 mode (compatibility)

`mbar` invoked as `borders` (symlink in `mbar.app/Contents/Resources/bin`, or
`make install` with `BORDERS_LINK=1`):

| Invocation | Behaviour |
|---|---|
| `borders -v` / `--version` (argv[1]) | stdout `borders-v1.9.0\n`, exit 0 |
| `borders -h` / `--help` (argv[1]) | stdout `Refer to the man page for help: man borders\n` plus one line `borders is provided by mbar: see docs/MIGRATING.md`, exit 0 |
| `borders k=v …` | every argument is validated locally with the core parser; invalid ones print `[?] Borders: Invalid argument '<arg>'\n` (or the color error) on **stdout** exactly like JankyBorders' client. The valid ones are sent to the default bar `mbar` as `--borders <valid…>`. Exit 0. |
| `borders` with no valid argument | stderr `A borders instance is already running and no valid arguments where provided. To modify properties of the running instance provide them as arguments.\n`, exit 1, when mbar runs; otherwise stderr `borders: mbar is not running. mbar draws the window borders; start mbar.app.\n`, exit 1. Never starts a daemon. |
| mbar not reachable | retry for up to 5 s (window-manager startup commands can run before the login item is up), then the "not running" message above, exit 1 |

- `bar_name_from_argv0("borders") == "mbar"`; `BAR_NAME` is not involved
  because `borders` never runs as a daemon.

### 3. Config file `bordersrc`

Together with the main config (daemon start and every `--reload`), the default
bar `mbar` runs `~/.config/borders/bordersrc`, else `~/.bordersrc`, if one is a
regular file (spec §4 lookup; `$XDG_CONFIG_HOME` is not consulted, as in
JankyBorders). It runs like any shell config of mbar: made executable if
needed, `sh -c` with the path **quoted**, the bundle's `Resources/bin` first on
`PATH` (so its `borders …` lines reach mbar through the link), killed after
60 s. It is started right after the main config is started: a shell config
(`mbarrc`, `sketchybarrc`) and `bordersrc` run **concurrently**, so their
`--borders` messages can interleave in any order; a Lua config runs
synchronously, so there `bordersrc` runs after it. Keep each borders setting in
one file. Users who move their settings into `init.lua` delete the file.

On macOS, when a foreign JankyBorders is running (its mach service
`git.felix.borders` is registered), mbar does **not** run `bordersrc` and logs
`bordersrc not run: JankyBorders is running (git.felix.borders); stop it with
brew services stop borders or finish the borders step in mbar.app setup`. The
`mbar.app` System page shows a warning with a button that stops it.

### 4. Lua

`mbar.borders({ active_color = 0xffe1e3e4, inactive_color = 0xff494d64, width = 5.0, style = "round", blacklist = { "Safari", "kitty" } })`
emits one `--borders` command. Number colors are formatted as `0x%08x`.
Arrays join with `,`. Table colors:
`{ glow = 0xff… }` → `glow(0x…)`,
`{ gradient = { top_left = 0x…, bottom_right = 0x… } }` and
`{ gradient = { top_right = 0x…, bottom_left = 0x… } }` → the gradient strings.
Booleans become `on`/`off`. Types in `lua/mbar.d.lua` (`mbar.BordersProps`).

### 5. `mbar.app`

- **Setup page:** detection gets `borders` (Homebrew formula and service, both
  named `borders`; a `borders` binary/link in `~/.local/bin` or `/usr/local/bin`;
  `~/.config/borders/bordersrc` / `~/.bordersrc`; launch lines in
  `~/.aerospace.toml`, `~/.config/aerospace/aerospace.toml`, `~/.yabairc`,
  `~/.config/yabai/yabairc`). Cleanup runs `brew services stop borders` and,
  with a switch (default on, because a Homebrew `borders` earlier on the `PATH`
  shadows mbar's link), `brew uninstall borders`. The take-over step explains
  that `bordersrc` is used in place, and lists window-manager launch lines that
  start `borders` (they keep working through the link when the window manager's
  `PATH` contains `/etc/paths.d` entries; otherwise remove them, mbar already
  runs `bordersrc`). The command-line step also checks
  `command -v borders` resolves into the bundle.
- **System page:** a "Window borders" section: status (on/off, active and
  inactive color, width, style) from `--query borders`, a switch that sends
  `--borders drawing=on|off`, and a warning with a button that stops a running
  JankyBorders (`git.felix.borders` registered).

## Architecture

```
 client (`mbar --borders …` / `borders …`) ──IPC──▶ Runtime (mbar-core)
                                                    │ borders::BordersState in Model
                                                    ▼
                         Effect::Platform(PlatformRequest::SetBorders(Box<BordersUpdate>))
                                                    │
                    headless: log + ignore          ▼
                    macOS: Services::execute ──▶ sys::borders::configure()
                                                    │ main thread
                    SkyLight notify procs ───▶ sys::notify (shared dispatcher)
                                                    ├──▶ sys::spaces
                                                    └──▶ sys::borders (create/destroy/move/
                                                         resize/order/focus/space change)
```

The core owns the **configuration** only. The platform owns the live set of
tracked windows and their border windows; window move/resize/order events
(hundreds per second while dragging) are handled in the notify proc on the main
thread and never reach the Runtime.

### Core contract (`crates/mbar-core/src/borders.rs`)

These names are the interface between work packages; keep them.

```rust
pub const BORDERS_VERSION: &str = "borders-v1.9.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientDirection { TopLeftToBottomRight, TopRightToBottomLeft }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderColor {
    Solid(u32),                       // 0xAARRGGBB
    Glow(u32),
    Gradient { direction: GradientDirection, color1: u32, color2: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle { Round, Square, Uniform }   // any other style char = Round

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderOrder { Above, Below }

#[derive(Debug, Clone, PartialEq)]
pub struct BorderSettings {
    pub active: BorderColor,          // default Solid(0xffe1e3e4)
    pub inactive: BorderColor,        // default Solid(0x00000000)
    pub background: BorderColor,      // default Solid(0x00000000)
    pub show_background: bool,        // spec BR-PARSE-07
    pub width: f32,                   // default 4.0
    pub style: BorderStyle,           // default Round
    pub order: BorderOrder,           // default Below
    pub hidpi: bool,                  // default false
    pub ax_focus: Option<bool>,       // None = auto (AX trusted)
    pub blacklist: Vec<String>,       // empty = disabled
    pub whitelist: Vec<String>,
}

/// Update mask (spec §2.3 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdateMask(pub u8);
impl UpdateMask {
    pub const ACTIVE: u8 = 1 << 0;
    pub const INACTIVE: u8 = 1 << 1;
    pub const ALL: u8 = Self::ACTIVE | Self::INACTIVE;
    pub const RECREATE_ALL: u8 = 1 << 2;
    pub const SETTING: u8 = 1 << 3;
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BordersState {
    pub drawing: bool,                // false until configured
    pub configured: bool,             // a --borders message was applied
    pub settings: BorderSettings,
    pub overrides: Vec<(u32, BorderSettings)>, // apply-to=<wid>, oldest first, at most 64
}

/// What the platform receives.
#[derive(Debug, Clone, PartialEq)]
pub struct BordersUpdate {
    pub drawing: bool,
    pub settings: BorderSettings,
    pub overrides: Vec<(u32, BorderSettings)>,
    pub mask: UpdateMask,             // what changed: platform redraws/recreates
}

/// Applies one argument to `settings` (spec §2.3 order and semantics).
pub fn parse_arg(settings: &mut BorderSettings, arg: &str) -> Result<UpdateMask, ArgError>;
pub enum ArgError { InvalidArgument, InvalidColor(String /* rest after key */) }
impl ArgError { pub fn jankyborders_text(&self, arg: &str) -> String; // "[?] Borders: …\n"
                pub fn mbar_text(&self, arg: &str) -> String; }      // "[!] Borders: …\n"

/// The `borders` client: (valid args, JankyBorders-style error lines).
pub fn validate_args(args: &[String]) -> (Vec<String>, Vec<String>);

impl BordersState {
    /// Applies one `--borders` message (spec BR-IPC-07 without the quirks marked
    /// "no"); returns the update to send, or None when nothing changed.
    pub fn apply(&mut self, pairs: &[(String, String)], rsp: &mut String) -> Option<BordersUpdate>;
    pub fn to_json(&self) -> String;
}
```

- `PlatformRequest::SetBorders(Box<BordersUpdate>)` is emitted once per message
  that changed something; `--reload` keeps `model.borders` and emits none.
- `apply-to=<wid>` (wid > 0): the override of that window is rebuilt as the
  current global settings plus this message's keys, replacing any earlier
  override for it (BR-IPC-07), and moves to the end of the list; the global
  settings are untouched. At most `MAX_OVERRIDES` (64) overrides are kept, the
  oldest is dropped (mbar: the core does not see windows close, so overrides of
  closed windows stay until `RECREATE_ALL` or the cap). `apply-to=0` takes the global path. A global message also
  applies its keys to every override (BR-IPC-08). `RECREATE_ALL` clears all
  overrides (JankyBorders loses them on recreate).
- Pure helpers the platform uses (Linux-tested): `BorderSettings::color_for(focused) -> BorderColor`,
  `BorderSettings::admits(process_name) -> bool` (blacklist/whitelist,
  exact, case-sensitive), `border_geometry(window: Rect, settings, scale) -> BorderGeometry`
  (frame of the border window, inset rect, corner radius per style; spec §7.2,
  §7.3, §7.5 with `BORDER_TSMW` chosen at runtime from a `macos26: bool` arg).

### Platform (`crates/mbar-macos/src/sys/borders/`)

- `ffi.rs`: the SkyLight/CoreGraphics symbols from `borders-skylight.md` via
  the existing `private_fns!` dlsym macro (missing symbols → borders disabled
  with one warning, never a crash).
- `mod.rs`: `configure(update)`, `shutdown()`, `on_displays_changed()`,
  `on_system_woke()`, `on_front_app_switched()`. Main thread only. Table
  `target wid → Border` (own `SLSNewConnection` per border window, as in
  JankyBorders, released on destroy).
- `border.rs`: create / update / move / hide / destroy (spec §7.1–§7.8), one
  SLS transaction per update.
- `draw.rs`: CoreGraphics drawing (`SLWindowContextCreate` + `objc2-core-graphics`):
  stroke, glow, gradient, background, styles (spec §7.4).
- `focus.rs`: focused-window detection (spec §6), SkyLight path and AX path,
  with JankyBorders' delayed re-checks (10/20/50 ms) coalesced (BQ25).
- `sys/notify.rs` (new, shared): **one** SkyLight notify proc for every id,
  fanning out to `spaces` and `borders`; and one shared
  `SLSRequestNotificationsForWindows` set (`request_windows(owner, wids)`
  unions the sets of all owners). `spaces.rs` moves onto it. This avoids
  double registration of 815/816/1325/1326/1401/1508 and the replace-the-set
  semantics of `SLSRequestNotificationsForWindows`. Note: JankyBorders reads
  1322 as a focus trigger; spaces reads it as capture gating. Both handlers run.
- Border windows are owned by mbar's pid: `spaces` already skips or must skip
  windows of its own pid so they never show up in `space_windows_change`.
- `Services::execute(SetBorders)` → `borders::configure`; `DisplaysReconfigured`
  and `SystemWoke` → recreate; `Services::shutdown` destroys all border windows.

### Headless

`PlatformRequest::SetBorders` is logged at debug level and ignored.

## Work packages

| WP | Scope | Files | Host |
|---|---|---|---|
| A | core: `borders.rs`, `--borders` parser, Runtime, `--query borders`, `PlatformRequest::SetBorders`, reload | `crates/mbar-core/**` | Linux |
| B | binary: argv0 `borders`, client retry, `bordersrc` lookup and run, headless | `crates/mbar/**`, `crates/mbar-ipc/**`, `crates/mbar-app/**` | Linux |
| C | macOS: `sys/notify.rs`, `sys/borders/`, services wiring | `crates/mbar-macos/**` | Linux cross-check (`cargo clippy -p mbar-macos --target aarch64-apple-darwin`), macOS CI |
| D | Lua: `mbar.borders`, `mbar.d.lua`, `LUA.md` | `crates/mbar-lua/**`, `lua/**`, `docs/LUA.md` | Linux |
| E | mbar.app: setup page and System page | `crates/mbar-ui/**` | Linux (`--no-default-features`), macOS CI |
| F | packaging and docs | `packaging/**`, `Makefile`, `README.md`, `docs/*.md` | Linux; macOS CI `make verify-app` |

## Verification

Linux: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, `cd crates/mbar-ui && cargo test --no-default-features`,
`cargo clippy -p mbar-macos --target aarch64-apple-darwin --all-targets -- -D warnings`.
macOS CI builds and tests everything and assembles `mbar.app`.
Real-hardware checks (spec §14 open questions) are listed in `docs/MIGRATING.md`
as known unknowns until someone runs it on a Mac.
