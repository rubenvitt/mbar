# mbar extensions

Everything here is new in mbar. A plain SketchyBar config never triggers any of it, so
existing configs behave exactly as documented in `docs/spec/`.

## Command line

| Command | Description |
|---|---|
| `--query stats` | JSON with runtime statistics (see below). |
| `--query menus` | JSON array of the front application's top-level menu titles. |
| `--monitor [events\|stats\|all]` | Streaming: the connection stays open and the daemon writes one JSON object per line for every event dispatched (`{"type":"event","name":…,"sender":…,"info":…,"items":[…],"ts_ms":…}`) and/or a stats snapshot every second (`{"type":"stats",…}`). Unix-socket transport only. |
| `--menu <index\|title>` | Opens that top-level menu of the front app (index 0 = Apple menu). |
| `--menubar hide\|show\|toggle` | Sets macOS "Automatically hide and show the menu bar" (`_HIHideMenuBar`) and notifies the system. |
| `--reload` | Reloads the config (SketchyBar has this as `--hotload`-adjacent behaviour; mbar exposes it explicitly). |

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
  "ipc_messages": 950
}
```

## Item properties

| Property | Description |
|---|---|
| `provider=<name>` | Native data source updating the item without spawning a script. Names: `clock`, `cpu`, `memory`, `battery`, `volume`, `wifi`, `network`, `disk`, `front_app`, `media`. `none` disables. |
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

## Lua

See `docs/LUA.md`. Event handlers registered from Lua run in-process; their item `script`
shows as `lua:<id>` in `--query`.
