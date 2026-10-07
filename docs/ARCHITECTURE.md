# mbar — Architecture

mbar is a Rust reimplementation of [SketchyBar](https://github.com/FelixKratz/SketchyBar)
for macOS. Goals, in priority order:

1. **Drop-in compatible** with SketchyBar configs: same command language
   (`--add/--set/--bar/--default/--subscribe/--trigger/--push/--query/--animate/...`),
   same property names, same events, same script environment, same query JSON.
   Existing `sketchybarrc` files, plugins, the original `sketchybar` client binary and
   SbarLua must keep working (mach IPC under the bootstrap name `git.felix.<bar_name>`).
2. **Faster**: GPU (Metal) rendering, damage-driven redraw, frame-paced animations,
   cached text runs, coalesced updates, native data providers instead of
   fork/exec'ing shell scripts every few seconds.
3. **Full menu-bar replacement**: native `app_menu` item (front app menus via the
   Accessibility API, opening the real menus), menu-extra aliases, optional hiding of
   the native menu bar, notch awareness.
4. **Highly customizable**: every SketchyBar property plus the extensions documented in
   `docs/EXTENSIONS.md`.

The behavioural reference is `docs/spec/*.md` (extracted from the SketchyBar C source).

## Crates

```
crates/
  mbar-core/            platform-independent, fully unit tested on Linux
  mbar-ipc/             wire protocol: Unix socket (all platforms) + mach (macOS)
  mbar-macos/           everything that touches Apple frameworks (cfg(target_os="macos"))
  mbar/                 the binary: client mode, daemon mode, headless platform
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
   Runtime::frame(now, &mut dyn Resources) ──► FrameOutput { windows: Vec<WindowUpdate> }
                                               (only dirty windows; each carries a Scene)
```

* `Resources` (implemented by the platform) gives the core **text metrics** and
  **image sizes**; renderers later resolve the same `TextKey`/`ImageKey` to GPU textures.
* The platform owns a single timer: `Runtime::next_deadline()` tells it when to call
  `frame()`/`tick()` next (update_freq timers, animations, scroll texts, alias refresh).
  No polling when idle.
* Everything is single-threaded on the main thread; background threads (IPC server,
  script reaper, providers) only post `Input`s through a `Waker`.

### mbar-ipc

* Unix domain socket `$TMPDIR/mbar_<user>_<bar_name>.socket`, frames:
  `u32 LE length` + payload. Request payload = argv joined by `\0`, terminated `\0\0`
  (same as SketchyBar's mach payload). Response = UTF-8 bytes (may be empty).
* On macOS additionally a mach server registered as `git.felix.<bar_name>` that speaks
  SketchyBar's exact OOL-descriptor message format, so the original `sketchybar` CLI
  and SbarLua talk to mbar unchanged.

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
| `events` | NSWorkspace/Distributed notifications, CoreAudio volume, IOKit power + brightness, CoreWLAN wifi, MediaRemote, sleep/wake |
| `mouse` | tracking areas + global monitors → `Input::Mouse` |
| `alias` | menu-extra discovery (CGWindowList) and capture |
| `menus` | Accessibility: front app menu titles, open a menu (AXPress), hide native menu bar |
| `providers` | native samples: cpu, memory, battery, network, disk, volume, wifi |
| `mach` | SketchyBar-compatible mach server |

### mbar (binary)

* `mbar [--config <file>]` → daemon. `mbar --set ...` → client (sends argv, prints response).
* When invoked as `sketchybar` (symlink) it behaves identically, so plugins calling
  `sketchybar --set` keep working.
* Config lookup: `--config`, `$XDG_CONFIG_HOME/mbar/mbarrc`, `~/.config/mbar/mbarrc`,
  then SketchyBar locations `$XDG_CONFIG_HOME/sketchybar/sketchybarrc`, `~/.config/sketchybar/sketchybarrc`.
* On Linux the daemon runs with the **headless platform** (no windows, monospace text
  metrics). It still runs scripts, timers, events and answers queries — used for
  integration tests of full configs.

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
