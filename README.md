# mbar

mbar is a status bar and menu-bar replacement for macOS, written in Rust. It
reimplements [SketchyBar](https://github.com/FelixKratz/SketchyBar)'s command
language and behaviour, so existing `sketchybarrc` files and plugin scripts keep
working, and adds a Metal renderer, an in-process Lua 5.4 config, native data
providers and a native application menu.

> **Status: early development (0.1.0).** The platform-independent core is unit
> and integration tested on Linux (headless mode). The macOS layer builds and its
> tests pass on GitHub's macOS runners, but mbar has **not been tested on real
> hardware beyond CI**. Expect bugs and missing polish. Please report what you
> find.

## Features

- **SketchyBar-compatible command language.** `--add`, `--set`, `--bar`,
  `--default`, `--subscribe`, `--trigger`, `--push`, `--query`, `--animate`,
  `--clone`, `--move`, `--reorder`, `--rename`, `--remove`, `--hotload`,
  `--reload`, `--update`, `--exit`. Same property names, events, script
  environment and `--query` JSON. Plugins that call `sketchybar` reach mbar
  through a `sketchybar -> mbar` symlink. The behavioural reference is
  [`docs/spec/`](docs/spec/), extracted from the SketchyBar C source.
- **Lua 5.4 config, in-process.** `init.lua` with an API modelled on
  [SbarLua](https://github.com/FelixKratz/SbarLua). Event handlers are Lua
  functions that run inside the daemon, so no shell is forked per event, click or
  `update_freq` tick. LuaLS type definitions ship in
  [`lua/mbar.d.lua`](lua/mbar.d.lua). See [`docs/LUA.md`](docs/LUA.md).
- **Native providers.** `provider=clock|cpu|memory|battery|volume|wifi|network|disk|front_app|media`
  updates an item's label (and optionally icon) from in-process sampling or system
  notifications. These replace the most common polling scripts.
- **`app_menu` item.** Draws the front application's menu titles natively.
  Clicking a title opens the real menu through the Accessibility API.
- **Menu-extra aliases.** `--add alias "<owner>,<name>"` mirrors native menu-bar
  extras into the bar, as in SketchyBar.
- **Menu-bar replacement.** `mbar --menubar hide|show|toggle` (or the bar property
  `hide_menubar=on`) turns on macOS's "automatically hide the menu bar" setting.
  `--menu <index|title>` opens a menu of the front app. The bar is notch-aware.
- **GPU (Metal) renderer.** One `CAMetalLayer` per bar and popup window.
  Instanced shape and text quads. Animations are paced to the display's refresh
  rate.
- **Management UI (`mbar-ui`, optional).** A separate app built with
  [GPUI](https://www.gpui.rs) and [gpui-kit](https://gpui-kit.com). It has a live
  item inspector with property editing, an event log, performance statistics
  (frame times, script spawns, slowest handlers), and a system page for
  permissions, menu-bar auto-hide, launch at login and config reload.
- **Introspection.** `mbar --query stats` returns runtime statistics.
  `mbar --monitor events|stats|all` streams events and statistics as JSON lines.

The full list of additions is in [`docs/EXTENSIONS.md`](docs/EXTENSIONS.md).
Places where mbar deliberately differs from SketchyBar (crashes, memory
corruption and undefined behaviour replaced by defined behaviour) are in
[`docs/DEVIATIONS.md`](docs/DEVIATIONS.md).

## Quick start

Requirements: macOS, the Xcode Command Line Tools, and a stable Rust toolchain.

```sh
git clone https://github.com/rubenvitt/mbar.git
cd mbar
make release                      # cargo build --release -p mbar
make install PREFIX=$HOME/.local  # installs mbar plus a `sketchybar` symlink
```

Then write a config and start the daemon:

```lua
-- ~/.config/mbar/init.lua
local mbar = require("mbar")

mbar.bar({ height = 32, color = 0xcc1e1e2e, blur_radius = 20 })
mbar.default({ label = { font = "SF Pro:Semibold:13.0", color = 0xffcdd6f4 } })

mbar.add("app_menu", "menus", "left")
mbar.add("item", "clock", { position = "right", provider = { "clock", args = "%a %d %b %H:%M" } })
mbar.add("item", "cpu", { position = "right", provider = "cpu" })
mbar.add("item", "battery", { position = "right", provider = "battery" })
```

```sh
mbar                    # start the daemon in the foreground
mbar --menubar hide     # from another shell: auto-hide the native menu bar
```

A shell `sketchybarrc` works too. If no mbar config exists, mbar reads
`~/.config/sketchybar/sketchybarrc`. To start mbar at login, run
`make install-agent PREFIX=$HOME/.local`. See [`docs/INSTALL.md`](docs/INSTALL.md) for the details:
permissions, the LaunchAgent and uninstalling.

## Coming from SketchyBar

Most configs run unchanged. mbar uses its own IPC name (`dev.rubeen.mbar`), so
the original `sketchybar` binary, SbarLua and compiled helpers that talk to
SketchyBar's mach port will not reach it. See
[`docs/MIGRATING.md`](docs/MIGRATING.md) for the differences, the SbarLua-to-mbar
mapping, and before/after examples of replacing polling scripts with native
providers.

## Performance design

- **No work when idle.** A single deadline timer drives `update_freq`,
  animations, scrolling text and alias refresh. Nothing polls, and only dirty
  windows are redrawn.
- **GPU rendering.** Each frame is one render pass: one instanced draw for shapes
  (SDF rounded rectangles, borders, shadows) and one for textured quads (text and
  images).
- **Cached text.** CoreText layout is cached per `(font, string)`. Rasterized runs
  live in an atlas, and colour changes tint the mask instead of re-rasterizing.
- **Batched commands.** One IPC message is one layout pass, no matter how many
  `--set`s it carries. Lua configs queue commands and send them as one message.
- **No fork/exec per event.** Lua handlers and native providers run inside the
  daemon. Shell scripts still work exactly as in SketchyBar.

The architecture is described in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Screenshots

There are no screenshots yet. They will be added once mbar has been run on real
hardware.

## Documentation

| Document | Contents |
|---|---|
| [`docs/INSTALL.md`](docs/INSTALL.md) | Building, installing, LaunchAgent, permissions, uninstalling |
| [`docs/MIGRATING.md`](docs/MIGRATING.md) | Switching from SketchyBar / SbarLua |
| [`docs/LUA.md`](docs/LUA.md) | Lua configuration API |
| [`docs/EXTENSIONS.md`](docs/EXTENSIONS.md) | Commands, properties, providers and the `app_menu` item that are new in mbar |
| [`docs/DEVIATIONS.md`](docs/DEVIATIONS.md) | Deliberate differences from SketchyBar |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Crates, data flow, IPC, renderer |
| [`docs/spec/`](docs/spec/) | Behavioural specification of SketchyBar that mbar implements |

## Building and testing

```sh
make build      # debug build of the workspace
make test       # cargo test --workspace (also runs on Linux, headless)
make lint       # rustfmt check + clippy -D warnings
make ui         # build the management app (crates/mbar-ui, its own workspace)
```

On Linux, `mbar` runs with a headless platform: no windows, deterministic text
metrics, but scripts, timers, events and queries all work. The integration tests
use it.

## License

mbar is licensed under **GPL-3.0-only** (see [`LICENSE`](LICENSE)). It is an independent reimplementation, but its
behaviour is derived from SketchyBar, which is GPL-3.0 licensed, by Felix Kratz.
mbar is not affiliated with the SketchyBar project.
