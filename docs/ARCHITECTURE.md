# mbar — Architecture

mbar is a Rust reimplementation of [SketchyBar](https://github.com/FelixKratz/SketchyBar)
for macOS. Goals, in priority order:

1. **Drop-in compatible** with SketchyBar configs: same command language
   (`--add/--set/--bar/--default/--subscribe/--trigger/--push/--query/--animate/...`),
   same property names, same events, same script environment, same query JSON.
   Existing `sketchybarrc` files and plugins must keep working (plugins calling
   `sketchybar` reach mbar through a `sketchybar -> mbar` symlink).
   mbar has its **own** IPC identity: mach bootstrap name `dev.rubeen.<bar_name>`
   (default bar name `mbar` → `dev.rubeen.mbar`); it does not register SketchyBar's
   `git.felix.*` name.
   The same holds for [JankyBorders](https://github.com/FelixKratz/JankyBorders)
   (`borders`): mbar draws the window borders itself, and `borders …` calls reach it
   through a `borders -> mbar` link (`docs/spec/borders.md`). `git.felix.borders` is not
   registered either.
2. **Faster**: GPU (Metal) rendering, damage-driven redraw, frame-paced animations,
   cached text runs, coalesced updates, native data providers instead of
   fork/exec'ing shell scripts every few seconds.
3. **Full menu-bar replacement**: native `app_menu` item (front app menus via the
   Accessibility API, opening the real menus), menu-extra aliases, optional hiding of
   the native menu bar, notch awareness.
4. **Highly customizable**: every SketchyBar property plus the extensions documented in
   `docs/EXTENSIONS.md`.

The behavioural reference is `docs/spec/*.md` (extracted from the SketchyBar C source;
`docs/spec/borders.md` from the JankyBorders C source).

## Crates

```
crates/
  mbar-core/            platform-independent, fully unit tested on Linux
  mbar-ipc/             wire protocol: Unix socket (all platforms) + mach (macOS)
  mbar-macos/           everything that touches Apple frameworks (cfg(target_os="macos"))
  mbar-lua/             embedded Lua 5.4 config/scripting (mlua), in-process callbacks
  mbar/                 the binary: client mode, daemon mode, headless platform
  mbar-ui/              separate management app (GPUI + gpui-kit), talks to the daemon over IPC
```

### mbar-core

Pure logic. No Apple types, no threads, no I/O except what is injected.

| module | responsibility |
|---|---|
| `color` | `Color` (ARGB u32 + f32 channels), `0xAARRGGBB` parsing |
| `value` | shared value parsers: bool/toggle, ints, floats, strings |
| `geometry` | `Point`, `Size`, `Rect` (f32, points, origin top-left) |
| `props` | `PropCx` — property setter context (animation hook, change tracking) |
| `components/*` | `Text`, `Font`, `Background`, `Image`, `Shadow`, `Graph`, `Slider`, `Alias`, `AppMenu` |
| `item` | `BarItem`, item types, item-level props, per-item width calc |
| `bar` | bar properties (`--bar`) |
| `popup` | popup properties and layout |
| `group` | brackets |
| `layout` | computes geometry for every window → `scene::Scene` |
| `scene` | display list consumed by renderers |
| `animation` | curves, animator, per-frame stepping |
| `command` | argv → `Vec<Command>` parser (exact SketchyBar grammar) |
| `query` | `--query` JSON output |
| `event` | built-in + custom events, subscriptions, `EventInfo` |
| `script` | env var construction for scripts |
| `provider` | native providers (clock, cpu, memory, battery, ...) formatting |
| `borders` | window-border configuration (`--borders`, `--query borders`): JankyBorders' argument parser, settings, `apply-to` overrides, update masks |
| `platform` | the traits/enums the core uses to talk to a platform |
| `runtime` | `Runtime`: owns all state, consumes `Input`, emits `Effect`s |

#### Data flow

```
          Input (IPC request, OS event, mouse, timer, provider sample, alias image)
            │
            ▼
   Runtime::handle(input, &mut dyn Resources) ──► Vec<Effect>
            │                                      (RunScript, Respond, Exit, Reload,
            │                                       PlatformRequest, ...)
            ▼
   Runtime::frame(now, &mut dyn Resources) ──► FrameOutput { windows, closed, space_moves }
                                               (only dirty windows, each carrying a Scene;
                                                non-sticky windows to send to a new space)
```

* `Resources` (implemented by the platform) gives the core **text metrics** and
  **image sizes**; renderers later resolve the same `TextKey`/`ImageKey` to GPU textures.
* The platform owns a single timer: `Runtime::next_deadline()` tells it when to call
  `frame()`/`tick()` next (update_freq timers, animations, scroll texts, alias refresh).
  No polling when idle. While animations run the deadline is "now" and a display link
  paces the frames; the headless platform has none and uses
  `Runtime::next_deadline_paced(FRAME_INTERVAL)` (60 Hz) so it does not spin.
* Everything is single-threaded on the main thread; background threads (IPC server,
  script reaper, providers) only post `Input`s through a `Waker`.

#### Window borders

The core owns only the borders **configuration**. The platform owns the tracked windows
and their border windows:

```
 client (`mbar --borders …` / `borders …`) ──IPC──▶ Runtime (mbar-core)
                                                    │ borders::BordersState in the model
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

* One `SetBorders` per message that changed something. It carries the complete settings,
  the `apply-to` overrides, `drawing` and an update mask (redraw focused / unfocused /
  all, recreate all).
* Window move, resize and order events (hundreds per second while dragging) are handled
  by `sys::borders` in the notify proc on the main thread. They never reach the Runtime.
* `sys::notify` registers **one** SkyLight notify proc per event id and fans out to
  `spaces` and `borders`. It also keeps one shared `SLSRequestNotificationsForWindows`
  set (the union of all owners), because that call replaces the set instead of adding
  to it.
* Display reconfiguration and wake recreate all borders; shutdown destroys them.

### mbar-ipc

* Unix domain socket `<dir>/mbar_<user>_<bar_name>.socket` (mode 0600), where `<dir>` is
  `$TMPDIR` (else `/tmp`) if that directory is owned by the user and not group/world
  writable, otherwise a private `mbar-<uid>` subdirectory (0700) of it, e.g.
  `/tmp/mbar-1000/` on Linux. The single-instance lock file does not follow `$TMPDIR`
  (SketchyBar: fixed `/tmp/<g_name>_<USER>.lock`, `cli.md` §1.1): it is always
  `/tmp/mbar-<uid>/mbar_<user>_<bar_name>.lock`, so daemons started with different
  `$TMPDIR`s (launchd/systemd vs. a shell) exclude each other. `MBAR_LOCK_DIR`
  replaces `/tmp` as its base (test hook for running independent daemons side by
  side). Both ends check
  the peer's uid (`SO_PEERCRED`/`getpeereid`; same user or root), so a socket squatted
  by another local user is never used. Frames:
  `u32 LE length` + payload. Request payload = argv joined by `\0`, terminated `\0\0`
  (same as SketchyBar's mach payload). Response = UTF-8 bytes (may be empty).
* On macOS additionally a mach server registered as `dev.rubeen.<bar_name>`
  (default `dev.rubeen.mbar`). Payload layout follows SketchyBar's OOL-descriptor
  message format (argv joined by `\0`), so porting a SketchyBar client such as
  SbarLua only requires changing the bootstrap name.

### mbar-macos

| module | responsibility |
|---|---|
| `app` | NSApplication (accessory policy), main run loop integration, Waker via GCD main queue |
| `window` | one borderless non-activating `NSPanel` per bar/popup window, levels, all-spaces, CAMetalLayer-backed view, private SkyLight blur |
| `render` | Metal renderer: instanced SDF quads (rounded rect + border + shadow), textured quads (text runs, images), clipping, graph strips |
| `text` | CoreText: font resolution (`Family:Style:Size`), measurement, run rasterization into an atlas (A8 mask or BGRA for color glyphs) |
| `image` | ImageIO/NSWorkspace image loading: files, `app.<bundle>`, app icons, media artwork |
| `displays` | NSScreen/CGDisplay enumeration, notch detection, display change callbacks |
| `spaces` | SkyLight private API: spaces per display, active space, windows per space |
| `notify` | the shared SkyLight notify proc and window-notification set, dispatching to `spaces` and `borders` |
| `borders` | JankyBorders-style window borders: SkyLight/CoreGraphics FFI (missing symbols disable borders with one warning), one border window per tracked window, drawing (solid, glow, gradient, background, styles), focus detection (SkyLight and AX paths) |
| `events` | NSWorkspace/Distributed notifications, CoreAudio volume, IOKit power + brightness, CoreWLAN wifi, MediaRemote, sleep/wake |
| `mouse` | tracking areas + global monitors → `Input::Mouse` |
| `alias` | menu-extra discovery (CGWindowList) and capture |
| `menus` | Accessibility: front app menu titles, open a menu (AXPress), hide native menu bar |
| `providers` | native samples: cpu, memory, battery, network, disk, volume, wifi |
| `mach` | mach server `dev.rubeen.<bar_name>` |

### mbar (binary)

* `mbar [--config <file>]` → daemon. `mbar --set ...` → client (sends argv, prints response).
* When invoked as `sketchybar` (symlink) it behaves identically, so plugins calling
  `sketchybar --set` keep working.
* When invoked as `borders` (symlink) it is a JankyBorders-compatible client: `-v` prints
  `borders-v1.9.0`; the arguments are checked with the core parser, invalid ones print
  JankyBorders' `[?]` lines, and the valid ones go to the bar `mbar` as `--borders …`.
  It retries for up to 5 s while mbar is not reachable and never starts a daemon.
* After its config, the default bar `mbar` runs `~/.config/borders/bordersrc` (else
  `~/.bordersrc`), on start and on every `--reload`.
* Config lookup: `--config`, `$XDG_CONFIG_HOME/mbar/mbarrc`, `~/.config/mbar/mbarrc`,
  then SketchyBar locations `$XDG_CONFIG_HOME/sketchybar/sketchybarrc`, `~/.config/sketchybar/sketchybarrc`.
* `SIGTERM`/`SIGINT`/`SIGHUP` end the daemon like `--exit` (self-pipe → `Event::Terminate`
  into the platform loop): menu-bar auto-hide restored, socket removed, the process groups
  of scripts still running terminated (D24).
* On Linux the daemon runs with the **headless platform** (no windows, monospace text
  metrics). It still runs scripts, timers, events and answers queries — used for
  integration tests of full configs.

## Configuration & scripting

Two equivalent ways to configure, both first-class:

1. **Shell** (`mbarrc` / `sketchybarrc`): unchanged SketchyBar workflow, scripts are
   spawned per event/update exactly like SketchyBar.
2. **Lua 5.4** (`init.lua`), embedded via `mlua` in the daemon. API modelled on SbarLua
   (`mbar.add`, `mbar.set`, `mbar.bar`, `mbar.default`, `mbar.subscribe`,
   `mbar.animate`, `mbar.trigger`, `mbar.query`, `mbar.exec`, `mbar.delay`) with
   additions for native providers and menus. Event handlers are **Lua functions running
   in-process** — no fork/exec per event, which is the single biggest speed-up over
   shell-script configs. `mbar.exec` runs shell commands asynchronously and calls back
   with their output. A LuaLS type-definition file (`lua/mbar.d.lua`) ships for
   autocompletion and type checking in editors.

Config lookup prefers `init.lua` over `mbarrc` in the same directory.

## Management UI (`mbar-ui`)

A separate, optional app so the bar process stays small. Built with
[GPUI](https://www.gpui.rs) (Zed's GPU UI framework, Metal on macOS) and
[gpui-kit](https://gpui-kit.com) components (tables, tree, inputs, dock layout). It is an IPC
client of the daemon and uses only public commands plus a few query extensions
(`--query stats`, `--monitor`):

* **Inspector**: live tree of bars, items, brackets, popups; edit any property live,
  copy the resulting `mbar --set ...` / Lua line.
* **Events**: live event log (event name, sender, INFO, which items/handlers ran).
* **Performance**: frame times, redraws per window, script spawns and durations,
  slowest handlers.
* **System**: permission status (Accessibility for `app_menu`, Screen Recording for
  aliases), native menu-bar auto-hide toggle, launch at login, reload config.

The bar itself does **not** use GPUI: it needs exact control over window levels,
all-spaces behaviour, private blur and a minimal memory footprint, so it keeps its own
small Metal renderer.

## Performance design

* **No work when idle.** One deadline timer; redraw only dirty windows.
* **GPU**: every window is a CAMetalLayer; a frame is a single render pass with one
  instanced draw for shapes and one for textured quads. Display-synced pacing while
  animating, nothing otherwise.
* **Text**: CoreText line layout is cached per `(font, string)`; rasterized runs live in
  an atlas keyed by `(TextKey, scale)`. Color changes don't re-rasterize (mask tinting).
* **Batched commands**: one IPC message = one layout pass, regardless of how many
  `--set`s it carries.
* **Native providers** (`provider=cpu|memory|battery|clock|...`) replace the most common
  polling scripts with in-process sampling.
