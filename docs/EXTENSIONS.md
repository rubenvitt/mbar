# mbar extensions

Everything here is new in mbar. A plain SketchyBar config never triggers any of it, so
existing configs behave exactly as documented in `docs/spec/`. Window borders take over
JankyBorders; its own command line (`borders …`) is covered in
[Window borders](#window-borders). [AeroSpace](#aerospace) events are built in; mbar
connects to AeroSpace only when a config uses them.

## Command line

| Command | Description |
|---|---|
| `--query stats` | JSON with runtime statistics (see below). |
| `--query menus` | JSON array of the front application's top-level menu titles. |
| `--monitor [events\|stats\|all]` | Streaming: the connection stays open and the daemon writes one JSON object per line for every event dispatched (`{"type":"event","name":…,"sender":…,"info":…,"items":[…],"ts_ms":…}`) and/or a stats snapshot every second (`{"type":"stats",…}`). Unix-socket transport only. The first frame is the reply of the message (empty unless earlier commands in it printed something, e.g. `--query bar --monitor`); a `--monitor` the daemon does not execute (for example after an empty argument, which ends the message) gets a normal reply and the connection closes. Subscribers that stop reading never stall the daemon: they are dropped once 1024 lines behind or after 1 s without progress. |
| `--menu <index\|title>` | Opens that top-level menu of the front app (index 0 = Apple menu). |
| `--borders <key>=<value> ...` | Window borders (JankyBorders options plus `drawing=`). See [Window borders](#window-borders). |
| `--query borders` | JSON of the borders configuration. See [Window borders](#window-borders). |
| `--query aerospace` | JSON of the AeroSpace connection and state. See [AeroSpace](#aerospace). |
| `--menubar hide\|show\|toggle` | Sets macOS "Automatically hide and show the menu bar" (`_HIHideMenuBar`) and notifies the system. |
| `--reload` | Reloads the config (SketchyBar has this as `--hotload`-adjacent behaviour; mbar exposes it explicitly). |
| `-h`, `--help` (as `mbar`) | Invoked under any name other than `sketchybar`, the help lists mbar's config locations, `--headless` and the extensions above (and `--clone` in the order the code reads it). Invoked as `sketchybar` (e.g. a `sketchybar -> mbar` symlink), it prints SketchyBar's `misc/help.h` verbatim (`cli.md` §1.3), just as `-v` prints `sketchybar-v2.24.0`. |

### `--query stats`

```json
{
  "uptime_s": 1234.5,
  "items": 42,
  "windows": 3,
  "frames": 1200,
  "frame_time_us": { "avg": 310, "p95": 640, "max": 2100 },
  "layout_time_us": { "avg": 45, "p95": 90, "max": 400 },
  "redraws_by_window": { "bar:1": 300, "popup:apple:1": 4 },
  "scripts": { "spawned": 812, "running": 0, "avg_ms": 6.1, "max_ms": 80.0,
               "by_item": { "clock": { "runs": 120, "avg_ms": 3.2, "max_ms": 9.0 } } },
  "lua": { "callbacks": 400, "avg_us": 35, "max_us": 900 },
  "events": { "front_app_switched": 31, "routine": 600 },
  "ipc_messages": 950,
  "version": "0.1.0"
}
```

`version` is the daemon version (semver), used by mbar.app to detect a stale daemon after an update.

## Item properties

| Property | Description |
|---|---|
| `provider=<name>` | Native data source updating the item without spawning a script. Names: `clock`, `cpu`, `memory`, `battery`, `volume`, `wifi`, `network`, `disk`, `front_app`, `media`, `aerospace` (see [AeroSpace](#aerospace)). `none` disables. |
| `provider.format=<template>` | Template for `label` with `{key}` placeholders, e.g. `"{percent}%"`. Default per provider. |
| `provider.icon_format=<template>` | Optional template for `icon`. |
| `provider.freq=<seconds>` | Sampling interval (float). Event-driven providers (`volume`, `wifi`, `front_app`, `media`, `battery`) ignore it. |
| `provider.args=<string>` | Provider specific (e.g. strftime format for `clock`, interface for `network`, mount point for `disk`). |

The provider also fires the item's `script` (if any) with `SENDER=provider` and the
sample in `INFO` as JSON, so scripts can post-process cheaply.

### Provider keys

| Provider | Keys |
|---|---|
| `clock` | `time` (formatted with `provider.args`, strftime, default `%H:%M`) |
| `cpu` | `percent`, `user`, `sys` |
| `memory` | `percent`, `used_gb`, `total_gb` |
| `battery` | `percent`, `charging` (`true`/`false`), `remaining` (`h:mm` or empty) |
| `volume` | `percent`, `muted` |
| `wifi` | `ssid`, `rssi` |
| `network` | `down`, `up` (human-readable per second), `down_bytes`, `up_bytes` |
| `disk` | `percent`, `free_gb`, `total_gb` |
| `front_app` | `name`, `bundle_id` |
| `media` | `title`, `artist`, `album`, `app`, `state` |

### Provider defaults

| Provider | default `provider.format` | default `provider.freq` (s) | default `provider.args` |
|---|---|---|---|
| `clock` | `{time}` | 1 | `%H:%M` |
| `cpu` | `{percent}%` | 2 | |
| `memory` | `{percent}%` | 5 | |
| `network` | `↓{down} ↑{up}` | 2 | default interface |
| `disk` | `{percent}%` | 60 | `/` |
| `battery` | `{percent}%` | event-driven | |
| `volume` | `{percent}%` | event-driven | |
| `wifi` | `{ssid}` | event-driven | |
| `front_app` | `{name}` | event-driven | |
| `media` | `{title}` | event-driven | |

Template rules: unknown keys expand to an empty string; `{{` and `}}` are literal braces;
malformed braces are copied verbatim. An empty `provider.format=""` leaves the label
untouched (the sample then only reaches the script via `INFO`). `INFO` uses SketchyBar's
payload layout (`{\n\t"k": "v",\n…\n}`, all values strings).

The `clock` format is a pure-Rust `strftime` (C locale): `%a %A %b %B %c %C %d %D %e %F %H
%I %j %k %l %m %M %n %p %R %S %t %T %u %w %x %X %y %Y %z %Z %%`, with the padding flags
`-`, `_` and `0`. Network rates use base 1024 (`512 B/s`, `12.3 KB/s`).

## Item type `app_menu`

`--add app_menu <name> <position>` — draws the front application's menu bar (app name in
bold, then each top-level menu title) natively, without scripts. Clicking a title opens
the real menu (Accessibility API). Requires the Accessibility permission.

| Property | Default | Description |
|---|---|---|
| `app_menu.font` | label font | Font for menu titles |
| `app_menu.app_font` | `app_menu.font` with style `Bold` | Font for the app name |
| `app_menu.color` | `0xffffffff` | Title color |
| `app_menu.highlight_color` | `0x33ffffff` | Background of the title under the mouse / open menu |
| `app_menu.spacing` | `14` | Gap between titles |
| `app_menu.apple` | `off` | Draw the Apple logo menu first |
| `app_menu.max_titles` | `0` (all) | Limit of menu titles |
| `app_menu.corner_radius` | `5` | Highlight corner radius |

Event `menus_change` fires when the front app's menu titles change; `INFO` is the JSON
array of titles.

## Bar properties

| Property | Description |
|---|---|
| `hide_menubar=on\|off` | Same as `--menubar hide/show`, persisted while mbar runs and restored on exit. |

## Window borders

mbar draws colored borders around windows and highlights the focused one, like
[JankyBorders](https://github.com/FelixKratz/JankyBorders) v1.9.0. The behavioural
reference is [`docs/spec/borders.md`](spec/borders.md); the differences are the B rows in
[`DEVIATIONS.md`](DEVIATIONS.md#jankyborders-borders). Borders are **off** until something
configures them, so a config without borders sees no change. Only the default bar `mbar`
runs `bordersrc` and receives `borders` commands.

### `--borders`

```sh
mbar --borders active_color=0xffe1e3e4 inactive_color=0xff494d64 width=5.0
mbar --borders style=square blacklist="Safari,kitty"
mbar --borders drawing=off
```

A key/value list like `--bar`: it ends at the next `-` token or at a token without `=`.
That token is reported as `[!] Borders: Expected <key>=<value> pair, but got: '<tok>'`.

| Key | Values | Default |
|---|---|---|
| `active_color` | `0xAARRGGBB`, `glow(0xAARRGGBB)`, `gradient(top_left=0x…,bottom_right=0x…)`, `gradient(top_right=0x…,bottom_left=0x…)` | `0xffe1e3e4` |
| `inactive_color` | same | `0x00000000` |
| `background_color` | `0xAARRGGBB` (a gradient is accepted but not drawn, a glow draws as a solid fill) | `0x00000000` |
| `width` | float (finite: `inf` and `nan` are rejected) | `4.0` |
| `style` | `round`, `square`, `uniform` (first character counts; anything else is round) | `round` |
| `order` | `above`, `below` (first character `a` is above, anything else below) | `below` |
| `hidpi` | `on`, `off` | `off` |
| `ax_focus` | `on`, `off`: find the focused window through Accessibility | on when mbar has Accessibility permission |
| `blacklist` | comma-separated process names (exact, case-sensitive, not trimmed) | empty |
| `whitelist` | same | empty |
| `apply-to` | window id: the message's other keys apply to this window only (`0` = all). Each message rebuilds the window's override as the current global settings plus its own keys, replacing an earlier override, as in JankyBorders. At most 64 overrides are kept (the oldest is dropped); overrides of closed windows stay until a `hidpi`/`blacklist`/`whitelist` change or the cap drops them | `0` |
| `drawing` | `on`, `off` (mbar boolean values) | off until configured |

All keys except `drawing` use JankyBorders' grammar, including its leniency (`width=5px`
is 5, `order=above` is above). `drawing=` is the extension. The first `--borders` message
that does not contain `drawing=` turns borders on; later messages change `drawing` only
when they contain it. `drawing=off` hides every border and keeps the settings.

Errors use mbar's `[!]` convention (the client prints them on stderr and exits 1):
`[!] Borders: Invalid argument '<tok>'` and `[!] Borders: Invalid color argument
color<rest>`. Valid keys in the same message still apply.

The borders configuration **survives** `--reload` and hotload: JankyBorders was a
separate process that bar reloads never touched, so borders set by a window manager's
launch line (`exec-and-forget borders …` in `aerospace.toml`) stay. The config and
`bordersrc` run again and apply their keys on top; nothing is redrawn when they change
nothing. Removing the borders lines from a config therefore does not turn borders off on
the next reload: send `mbar --borders drawing=off` (or restart mbar).

### `--query borders`

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

Colors print in their input syntax: `0xAARRGGBB`, `glow(0x…)` or `gradient(…)`.
`ax_focus` is `auto` until it is set. `overrides` lists the `apply-to` windows as
`{ "window": <wid>, …same keys }`. An item named `borders` wins over this query, as with
`--query stats`.

### `bordersrc`

Together with its main config, the default bar `mbar` runs `~/.config/borders/bordersrc`,
else `~/.bordersrc` (`$XDG_CONFIG_HOME` is not consulted, as in JankyBorders). It runs like
mbar's shell configs: made executable if needed, `sh -c` with the path quoted, killed
after 60 s. In `mbar.app` its `PATH` starts with the bundle's `Resources/bin`, so the
`borders …` lines in it reach mbar through the `borders` link. This happens on every start
and every `--reload`.

`bordersrc` is started right after the main config is started. A shell config (`mbarrc`,
`sketchybarrc`) and `bordersrc` run **concurrently**, so when both set the same borders key
either may win; a Lua config (`init.lua`) runs synchronously, so `bordersrc` runs after it.
Keep each borders setting in one file.

On macOS, when JankyBorders itself is still running (its mach service `git.felix.borders`
is registered), mbar does **not** run `bordersrc`, so the two do not both draw borders. It
logs `bordersrc not run: JankyBorders is running (git.felix.borders); stop it with brew
services stop borders or finish the borders step in mbar.app setup`, and the `mbar.app`
System page shows a warning with a button that stops it. The next start or `--reload`
after JankyBorders is stopped runs `bordersrc`.

### The `borders` command

Invoked as `borders` (the `borders -> mbar` link in `mbar.app/Contents/Resources/bin`, or
from `make install`), the binary is a JankyBorders-compatible client:

| Invocation | Behaviour |
|---|---|
| `borders -v`, `borders --version` | prints `borders-v1.9.0`, exit 0 |
| `borders -h`, `borders --help` | prints `Refer to the man page for help: man borders` and a line saying that mbar provides `borders`, exit 0 |
| `borders <key>=<value> ...` | checks every argument with the same parser. Invalid ones print `[?] Borders: Invalid argument '<arg>'` (or the color error) on stdout, like JankyBorders. The valid ones go to the bar `mbar` as `--borders ...`. Exit 0 |
| `borders` with no valid argument | stderr `A borders instance is already running and no valid arguments where provided. ...`, exit 1 |
| mbar not running | retries for up to 5 s, then stderr `borders: mbar is not running. mbar draws the window borders; start mbar.app.`, exit 1 |

`borders` never starts a daemon. `BAR_NAME` is not involved.

### Lua

```lua
mbar.borders({
  active_color = 0xffe1e3e4,
  inactive_color = 0xff494d64,
  width = 5.0,
  style = "round",
  blacklist = { "Safari", "kitty" },
})
```

`mbar.borders(props)` sends one `--borders` command. Number colors become `0x%08x`,
arrays are joined with `,`, booleans become `on`/`off`. `{ glow = 0xff… }` and
`{ gradient = { top_left = 0x…, bottom_right = 0x… } }` (or `top_right`/`bottom_left`)
become the JankyBorders color strings. Details in [`LUA.md`](LUA.md); the type is
`mbar.BordersProps` in `lua/mbar.d.lua`.

## AeroSpace

mbar talks to a running [AeroSpace](https://github.com/nikitabobko/AeroSpace)
directly. It subscribes to AeroSpace's event stream and sends commands over its
socket, so no `exec-on-workspace-change` shell chain is needed. AeroSpace stays a
separate program that manages the windows. The design is in
[`superpowers/specs/2026-10-09-aerospace-design.md`](superpowers/specs/2026-10-09-aerospace-design.md);
moving an existing setup over is in [`MIGRATING.md`](MIGRATING.md#using-aerospace).

### Events

| mbar event | AeroSpace event | Variables (besides `NAME`, `SENDER`, `INFO`) |
|---|---|---|
| `aerospace_workspace_change` | `focused-workspace-changed` | `FOCUSED_WORKSPACE`, `PREV_WORKSPACE` |
| `aerospace_focus_change` | `focus-changed` | `FOCUSED_WORKSPACE`, `WINDOW_ID` (empty on an empty workspace) |
| `aerospace_monitor_change` | `focused-monitor-changed` | `FOCUSED_WORKSPACE`, `MONITOR_ID` (1-based) |
| `aerospace_mode_change` | `mode-changed` | `MODE` |
| `aerospace_window_detected` | `window-detected` | `WINDOW_ID`, `WORKSPACE`, `APP_BUNDLE_ID`, `APP_NAME` |
| `aerospace_binding_triggered` | `binding-triggered` | `MODE`, `BINDING` |

`INFO` holds the same data as a JSON object with lower-case keys, for example
`{"focused_workspace":"2","prev_workspace":"1"}`.

```sh
mbar --add item workspace left \
     --subscribe workspace aerospace_workspace_change \
     --set workspace script='mbar --set $NAME label="$FOCUSED_WORKSPACE"'
```

- The events are built in: `--subscribe` works without `--add event`. A config that
  still runs `--add event aerospace_workspace_change` (the SketchyBar recipe) is
  accepted and changes nothing.
- A manual `--trigger aerospace_workspace_change FOCUSED_WORKSPACE=…` keeps working.
  If AeroSpace's `exec-on-workspace-change` still sends it, items get the event twice
  (harmless). Remove that line from `aerospace.toml`.
- On connect, AeroSpace sends the current workspace, focus, monitor and mode right
  away, so items are correct at startup without a separate query.

### Provider `provider=aerospace`

Sets the item's label from AeroSpace's events, without a script:

| `provider.args` | label |
|---|---|
| (none) or `workspace` | focused workspace |
| `mode` | current binding mode (`main`, …) |
| `monitor` | focused monitor id |

```sh
mbar --add item aerospace.mode right --set aerospace.mode provider=aerospace provider.args=mode
```

Highlighting the focused workspace item is a handler on `aerospace_workspace_change`
(a script, or Lua as in [`LUA.md`](LUA.md#aerospace)).

### `--query aerospace`

```json
{
	"connected": "on",
	"transport": "socket",
	"server_version": "0.20.0-Beta 33fa0643",
	"error": "",
	"focused_workspace": "2",
	"prev_workspace": "1",
	"mode": "main",
	"monitor": 1
}
```

`transport` is `socket`, `cli` or `none`. `error` says why the last connection attempt
failed while `connected` is `off`. An item named `aerospace` wins over this query, as
with `--query borders`. The System page of `mbar.app` shows the same status.

### Lua

```lua
mbar.aerospace.run({ "workspace", "3" })                        -- fire and forget
mbar.aerospace.run({ "list-workspaces", "--all" }, function(r)  -- r.exit_code, r.stdout, r.stderr
end)
mbar.aerospace.query({ "list-windows", "--all", "--json" }, function(list, err)
  -- stdout parsed as JSON; err is set when the command or the parse failed
end)
mbar.aerospace.on("workspace_change", function(env) end)         -- = aerospace_workspace_change
```

Commands run one after another on a worker thread. Callbacks run on the daemon's Lua
thread, like `mbar.exec` callbacks. A hanging AeroSpace never blocks the bar. A command
that cannot run (AeroSpace not running) reaches `run` callbacks as `exit_code = -1` with
the reason in `stderr`. `on` takes the event name with or without the `aerospace_`
prefix and registers an in-process handler that needs no item of yours (mbar adds one
hidden carrier item, `__mbar_aerospace`, with `drawing=off`). Details in
[`LUA.md`](LUA.md#aerospace).

### Connection

The connection starts lazily, on the first of:

- a `--subscribe` to any `aerospace_*` event,
- an item with `provider=aerospace`,
- `--query aerospace`,
- a Lua `mbar.aerospace` call.

Without any of these mbar never connects, so nothing changes for users without
AeroSpace. The connection and the stored state (focused and previous workspace, mode,
monitor) survive `--reload`.

**Socket.** mbar uses AeroSpace's socket `/tmp/bobko.aerospace-$USER.sock` and its
documented protocol (AeroSpace `docs/guide.adoc`, "Socket protocol"): a version
handshake, then length-prefixed JSON frames. Commands are `ClientRequest`s; events come
from one connection running `subscribe --all`.

**CLI fallback.** For AeroSpace versions whose server predates the socket protocol (the
handshake gets another version, the socket closes, or there is no answer within 1 s),
mbar runs one long-lived `aerospace subscribe --all` child for the events and
`aerospace <args>` for commands. The `aerospace` binary is looked up on `PATH`, then in
`/opt/homebrew/bin`, `/usr/local/bin` and
`/Applications/AeroSpace.app/Contents/Resources/bin`. If the CLI has no `subscribe`
either, `--query aerospace` says so in `error`, and only manual `--trigger`s work.

**Reconnect.** When AeroSpace quits or restarts, mbar retries with a backoff (1 s,
doubling, at most 30 s) for as long as it runs. Every change shows in
`--query aerospace`.

**Overrides.** `MBAR_AEROSPACE_SOCKET` replaces the socket path and
`MBAR_AEROSPACE_CLI` the `aerospace` binary. Both are meant for tests.

## Lua

See `docs/LUA.md`. Event handlers registered from Lua run in-process; their item `script`
shows as `lua:<id>` in `--query`.
