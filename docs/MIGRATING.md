# Migrating from SketchyBar and JankyBorders

mbar implements SketchyBar's command language, properties, events, script
environment and `--query` output (reference: SketchyBar v2.24.0, see
[`docs/spec/`](spec/)). A shell config usually runs unchanged. A SbarLua config
needs a few small edits.

mbar also draws JankyBorders' window borders. If you use JankyBorders
(`borders`), see [Migrating from JankyBorders](#migrating-from-jankyborders).

## Quick checklist

1. Install `mbar.app` ([`INSTALL.md`](INSTALL.md)); its setup stops and
   removes SketchyBar for you. It also puts `mbar` and `sketchybar` on the
   `PATH`, starts mbar at login and asks for Accessibility / Screen Recording.
   The setup handles Homebrew SketchyBar; if you start SketchyBar some other
   way (your own LaunchAgent), stop that yourself.
2. Leave the config where it is (`~/.config/sketchybar/sketchybarrc`) or copy it
   to `~/.config/mbar/`. mbar finds both. A SbarLua `sketchybarrc` gets an
   `init.lua` from the setup (see [below](#from-sbarlua-to-mbars-lua) for what it
   changes).

Alternatively, build from source:

1. Install mbar with the `sketchybar -> mbar` symlink on the `PATH`
   ([`INSTALL.md`](INSTALL.md#build-from-source)). Make sure no real
   `sketchybar` binary comes before it.
2. Stop SketchyBar: `brew services stop sketchybar`, or unload your own
   LaunchAgent.
3. Leave the config where it is (`~/.config/sketchybar/sketchybarrc`) or copy it
   to `~/.config/mbar/`. mbar finds both.
4. Start mbar (`mbar`, or `make install-agent`) and grant Accessibility / Screen
   Recording to the `mbar` binary if you use `app_menu` or aliases.

## What works unchanged

- `sketchybarrc` shell configs and `plugins/*.sh`, including `$NAME`, `$SENDER`,
  `$INFO`, `$CONFIG_DIR`, `$BAR_NAME`, `$BUTTON`, `$MODIFIER`, `$SELECTED`,
  `$SID`, `$DID` and the other script variables.
- All domains and commands: `--bar`, `--add item|space|graph|slider|alias|bracket|event`,
  `--set`, `--default`, `--subscribe`, `--trigger`, `--push`, `--query`,
  `--animate`, `--clone`, `--move`, `--reorder`, `--rename`, `--remove`,
  `--hotload`, `--reload`, `--update`, `--exit`, `--load-font`.
- All built-in events (`front_app_switched`, `space_change`,
  `space_windows_change`, `display_change`, `volume_change`,
  `brightness_change`, `power_source_change`, `wifi_change`, `media_change`,
  `system_will_sleep`, `system_woke`, `mouse.*`, ...) and custom events backed by
  distributed notifications.
- `--query` JSON. It has the same keys and order, with properly escaped strings
  (D13 below).
- Item names, regex selectors (`/regex/`), popups, brackets, animations and their
  curves.
- Multiple bars: run the binary under another name (`ln -s mbar bottom`), exactly
  as with SketchyBar.

## Differences

### IPC name

mbar registers the mach service **`dev.rubeen.<bar_name>`** (default
`dev.rubeen.mbar`), not SketchyBar's `git.felix.<bar_name>`. It also listens on a
per-user Unix socket. Because of this:

- the original SketchyBar binary cannot send commands to mbar. Use the `mbar`
  binary or the `sketchybar -> mbar` symlink.
- **SbarLua** (`sketchybar.so`) cannot connect. Use mbar's built-in Lua instead
  (below).
- compiled helpers that send messages to SketchyBar's mach port themselves, for
  example event providers that embed SketchyBar's `sketchybar.h`, cannot connect
  either. Change the bootstrap name in the helper to `dev.rubeen.mbar`, call the
  `sketchybar` symlink, or replace the helper with a native provider.
- SketchyBar and mbar can run at the same time without interfering. Each one
  has its own lock file, socket and service name. Plugins reach whichever
  `sketchybar` comes first on the daemon's `PATH`.

### Config lookup

The first regular file found wins:

1. `--config <file>`
2. `$XDG_CONFIG_HOME/mbar/{init.lua,mbarrc,sketchybarrc}`
3. `~/.config/mbar/{init.lua,mbarrc,sketchybarrc}`
4. `$XDG_CONFIG_HOME/sketchybar/{init.lua,sketchybarrc}`
5. `~/.config/sketchybar/{init.lua,sketchybarrc}`
6. `~/.sketchybarrc`

`init.lua` is a Lua config that runs inside the daemon. Every other file is run
as a program, as in SketchyBar. A SbarLua `sketchybarrc` (a `#!/usr/bin/env lua`
script) would therefore be executed and would try to load SbarLua. Convert it to
`init.lua` as described below.

### Deliberate deviations

mbar copies SketchyBar's quirks, except where SketchyBar crashes, corrupts memory
or depends on undefined behaviour. The full table is in
[`DEVIATIONS.md`](DEVIATIONS.md). The ones you are most likely to notice:

| | SketchyBar | mbar |
|---|---|---|
| Script environment (D1, D16) | Variables can leak between items that share an event | Every script run gets a fresh environment |
| Animation duration (D12) | Counted in 60 Hz display frames | `n` means `n/60` seconds of wall-clock time, rendered at the display's refresh rate |
| `bounce` / `overshoot` curves (D15) | Behave as linear | Real ease-out-bounce / ease-out-back |
| `--query` (D13) | Strings are not JSON-escaped | Strings are escaped, so the output always parses |
| Client timeout (D17) | 100 ms; a slow daemon gives an empty reply | Up to 5 s |
| Drawing (D21) | Each item has its own window | Items are drawn into their bar or popup window; parts outside it are clipped |
| Graph width (D22) | Unbounded | Clamped to 4096 points |
| Lock file (D23) | `/tmp/<bar>_<USER>.lock` | `/tmp/mbar-<uid>/mbar_<USER>_<bar>.lock` |

### Other differences

- `mbar -v` prints `mbar-v<version>`. Invoked as `sketchybar`, it prints
  `sketchybar-v2.24.0` and SketchyBar's help text, so version checks in scripts
  keep passing.
- mbar refuses to run as root, like SketchyBar. Set `MBAR_ALLOW_ROOT=1` to allow
  it anyway (containers, CI).
- `MBAR_LOG=error|warn|info|debug|trace` controls mbar's own diagnostics on
  stderr. The SketchyBar-style daemon log still goes to stdout.
- New commands, properties and events (`--menubar`, `--menu`, `--query stats`,
  `--monitor`, `provider=`, `app_menu`, `menus_change`, `hide_menubar`) are listed
  in [`EXTENSIONS.md`](EXTENSIONS.md). A plain SketchyBar config never triggers
  any of them.

## From SbarLua to mbar's Lua

mbar embeds Lua 5.4 with an SbarLua-style API ([`LUA.md`](LUA.md)). Nothing
needs to be compiled or installed.

1. Move the entry point to `~/.config/mbar/init.lua` (or
   `~/.config/sketchybar/init.lua`). Take the body of your SbarLua
   `sketchybarrc` and drop the shebang and the `package.cpath` line.
2. `require("sketchybar")` keeps working and returns the `mbar` table, so
   `sbar = require("sketchybar")` needs no change. `require("items")` and other
   modules load from the config directory (`package.path` starts with
   `<CONFIG_DIR>/?.lua;<CONFIG_DIR>/?/init.lua`).
3. Check the points in the table below, especially `env.INFO`, the `exec`
   callback and `io.popen`.

Before (SbarLua `~/.config/sketchybar/sketchybarrc`):

```lua
#!/usr/bin/env lua
package.cpath = package.cpath .. ";/Users/" .. os.getenv("USER") .. "/.local/share/sketchybar_lua/?.so"
sbar = require("sketchybar")
sbar.begin_config()
require("bar")
require("items")
sbar.end_config()
sbar.event_loop()
```

After (`~/.config/mbar/init.lua`, with `bar.lua` and `items/` next to it):

```lua
sbar = require("sketchybar")   -- or: local mbar = require("mbar")
sbar.begin_config()
require("bar")
require("items")
sbar.end_config()
sbar.event_loop()              -- no-op, can stay
```

### Equivalents

| SbarLua | mbar | Notes |
|---|---|---|
| `package.cpath = ...sketchybar_lua/?.so` | (remove) | No C module. Loading C modules is disabled. |
| `require("sketchybar")` | `require("sketchybar")` or `require("mbar")` | Both return the same table. |
| `sbar.add(type, name, props)` | `mbar.add(type, name, props)` | Also accepts position and width as positional arguments. Name is optional. |
| `sbar.add("bracket", name, members, props)` | same | Members may be item objects. |
| `sbar.add("event", name, notification)` | same | |
| `sbar.set`, `sbar.bar`, `sbar.default`, `sbar.remove` | same names | |
| `item:set(props)`, `item:subscribe(events, fn)`, `item:query()` | same | Plus `item:remove()`, `item:push(...)`, `item:provider(spec)`. |
| `sbar.animate(curve, duration, fn)` | same | Duration in 60 Hz frames (D12). |
| `sbar.trigger(event, env)` | same | Table values are JSON encoded. |
| `sbar.query(what)` | same | Returns decoded JSON, or `nil, response`. |
| `sbar.exec(cmd, function(result, exit_code) ...)` | `mbar.exec(cmd, function(result, output) ...)` | The second argument is the raw output, not the exit code. |
| `sbar.delay(seconds, fn)` | same | |
| `sbar.hotload(bool)` | same | |
| `sbar.begin_config()` / `sbar.end_config()` | same | `end_config` sends everything queued. Commands are batched either way (one message per config, handler or callback). |
| `sbar.event_loop()` | same (no-op) | Handlers run inside the daemon, so no event loop is needed. |
| `env.INFO` as a table | `env.info` (decoded) / `env.INFO` (raw string) | Use `env.info or env.INFO` in code that should run on both. |
| `io.popen("sketchybar --query ...")` | `mbar.query(...)` | Calling the bar's own client from a blocking `io.popen`/`os.execute` fails immediately. See [`LUA.md`](LUA.md#differences-from-sbarlua). |
| `io.popen("slow command")` in a handler | `mbar.exec(cmd, fn)` | Handlers run on the daemon's main thread. A blocking call freezes the bar. |
| `sbar.exec("sketchybar --set ...")` | `mbar.set(...)` | Works, but the direct call skips a process spawn. |
| (none) | `mbar.provider`, `mbar.menu`, `mbar.command`, `mbar.flush`, `mbar.json` | New in mbar. |
| `script = "..."` (shell) | `script = function(env) ... end` | Function handlers run in-process. Shell strings still work. |

## Replacing polling scripts with native providers

Many SketchyBar plugins exist only to poll a system value every few seconds and
write it into a label. Each poll forks a shell, the plugin and its tools (`date`,
`top`, `pmset`, ...). A native provider samples the value inside the daemon and
sets the label directly from a `{key}` template. The provider list and keys are
in [`EXTENSIONS.md`](EXTENSIONS.md#item-properties).

The item's script, if it has one, still runs with `SENDER=provider` and the
sample as JSON in `$INFO`, so a script can post-process the value without
measuring anything itself.

### Clock

Before:

```sh
sketchybar --add item clock right \
           --set clock update_freq=10 script="$PLUGIN_DIR/clock.sh"
# plugins/clock.sh
sketchybar --set "$NAME" label="$(date '+%d/%m %H:%M')"
```

After (no script):

```sh
sketchybar --add item clock right \
           --set clock provider=clock provider.args="%d/%m %H:%M" provider.freq=10
```

### CPU load

Before:

```sh
sketchybar --add item cpu right \
           --set cpu update_freq=2 script="$PLUGIN_DIR/cpu.sh"
# plugins/cpu.sh
CPU=$(top -l 2 | grep -E "^CPU" | tail -1 | awk '{ print $3 + $5 }')
sketchybar --set "$NAME" label="${CPU%.*}%"
```

After:

```sh
sketchybar --add item cpu right \
           --set cpu provider=cpu provider.format="{percent}%" provider.freq=2
```

The same as a graph, fed by the provider sample (Lua, no shell at all):

```lua
local cpu = mbar.add("graph", "cpu.graph", 60, {
  position = "right",
  graph = { color = 0xff9dd274, fill_color = 0x409dd274 },
  provider = { "cpu", format = "{percent}%", freq = 2 },
})
cpu:subscribe("*", function(env)
  if env.SENDER == "provider" then cpu:push(tonumber(env.info.percent) / 100) end
end)
```

### Battery

Before (the stock plugin: `pmset` on every tick and power event):

```sh
sketchybar --add item battery right \
           --set battery update_freq=120 script="$PLUGIN_DIR/battery.sh" \
           --subscribe battery system_woke power_source_change
```

After: the label comes from the provider. The script only picks the icon from
the sample and no longer runs `pmset`:

```sh
sketchybar --add item battery right \
           --set battery provider=battery provider.format="{percent}%" \
                         script="$PLUGIN_DIR/battery_icon.sh"
```

```sh
# plugins/battery_icon.sh: INFO = {"percent": "87", "charging": "false", "remaining": "5:12"}
[ "$SENDER" = provider ] || exit 0
PCT=$(printf '%s\n' "$INFO" | sed -n 's/.*"percent": "\([0-9]*\)".*/\1/p')
case "$INFO" in
  *'"charging": "true"'*) ICON="⚡" ;;
  *) if   [ "$PCT" -ge 60 ]; then ICON="▰▰▰"
     elif [ "$PCT" -ge 30 ]; then ICON="▰▰▱"
     else ICON="▰▱▱"; fi ;;
esac
sketchybar --set "$NAME" icon="$ICON"
```

In Lua the whole item is in-process:

```lua
local battery = mbar.add("item", "battery", {
  position = "right",
  provider = { "battery", format = "{percent}%" },
})
battery:subscribe("*", function(env)
  if env.SENDER ~= "provider" then return end
  local pct = tonumber(env.info.percent) or 0
  local icon = env.info.charging == "true" and "⚡"
    or pct >= 60 and "▰▰▰" or pct >= 30 and "▰▰▱" or "▰▱▱"
  battery:set({ icon = icon })
end)
```

### Front app, volume, Wi-Fi, media

These are already event-driven in SketchyBar, but each event still spawns the
plugin. A provider sets the label directly:

```sh
# Before: --subscribe front_app front_app_switched + plugins/front_app.sh
#         (sketchybar --set "$NAME" label="$INFO")
sketchybar --add item front_app left  --set front_app provider=front_app

sketchybar --add item volume    right --set volume    provider=volume provider.format="{percent}%"
sketchybar --add item wifi      right --set wifi      provider=wifi   provider.format="{ssid}"
sketchybar --add item media     right --set media     provider=media  provider.format="{artist} – {title}"
```

### Memory, disk, network

These had no built-in event in SketchyBar and were always polled:

```sh
sketchybar --add item mem  right --set mem  provider=memory  provider.format="{used_gb}G"
sketchybar --add item disk right --set disk provider=disk    provider.args=/ provider.freq=300
sketchybar --add item net  right --set net  provider=network provider.format="↓{down} ↑{up}"
```

`mbar --query stats` shows `scripts.spawned` and per-item script times, so you
can measure what the switch saves.

## Migrating from JankyBorders

mbar draws colored borders around windows and highlights the focused one, like
[JankyBorders](https://github.com/FelixKratz/JankyBorders) (reference:
JankyBorders v1.9.0, see [`docs/spec/borders.md`](spec/borders.md)). The
separate `borders` process is no longer needed. Your `bordersrc` and your
`borders …` lines keep working. Borders stay off until something configures
them, so nothing changes if you never used JankyBorders.

### Checklist

1. Install `mbar.app` ([`INSTALL.md`](INSTALL.md)). Its setup finds Homebrew
   `borders`, runs `brew services stop borders` and, with the switch "Also
   uninstall Homebrew borders" (on by default), `brew uninstall borders`.
   Leave the switch on: a Homebrew `borders` that comes earlier on the `PATH`
   hides mbar's `borders` link. If you start `borders` some other way (your
   own LaunchAgent, a login script), stop that yourself.
   - While JankyBorders still runs (its mach service `git.felix.borders` is
     registered), mbar does **not** run `bordersrc`, so the two do not both
     draw borders. The log says `bordersrc not run: JankyBorders is running
     (git.felix.borders); stop it with brew services stop borders or finish
     the borders step in mbar.app setup`, and the System page of `mbar.app`
     shows a warning with a button that stops it. After that, the next start
     or `mbar --reload` runs `bordersrc`.
2. Leave `~/.config/borders/bordersrc` (or `~/.bordersrc`) where it is. mbar
   runs it together with its own config, on every start and every `--reload`.
   The `borders …` lines in it reach mbar through the `borders` link. A shell
   config (`mbarrc`, `sketchybarrc`) and `bordersrc` run at the same time, so
   if both set the same key either may win; a Lua config runs first. Keep each
   borders setting in one file.
3. Check the launch lines in your window manager's config, for example
   `borders active_color=0xffe1e3e4 width=5.0 &` in `yabairc` or
   `exec-and-forget borders …` in `aerospace.toml`. The setup lists them.
   - They keep working through the `borders` link when the window manager's
     `PATH` contains it. The app adds it through `/etc/paths.d/mbar`, which
     login shells read. A window manager started as a login item usually does
     not, so use the full path
     `/Applications/mbar.app/Contents/Resources/bin/borders` there.
   - Or remove them. mbar runs `bordersrc` itself, so a line that only starts
     `borders` is not needed. Without arguments it now prints JankyBorders'
     "already running" error and exits 1.
   - JankyBorders skipped `bordersrc` when its launch line had valid arguments.
     mbar runs both, so keep each setting in one place.
4. Or move the settings into mbar's config and delete `bordersrc`:
   `mbar.borders({ … })` in `init.lua`, or `mbar --borders …` in a shell
   config.
5. With `ax_focus` unset or `on`, mbar finds the focused window through
   Accessibility. Grant Accessibility to mbar (the setup asks for it anyway).
   The grant JankyBorders had does not carry over.

From source: `make install` also creates the `borders -> mbar` link
(`BORDERS_LINK=1`, see [`INSTALL.md`](INSTALL.md#the-borders-symlink)). Stop
JankyBorders with `brew services stop borders && brew uninstall borders`.

Before (`~/.config/borders/bordersrc`, the example from JankyBorders' README):

```sh
#!/bin/bash

options=(
	style=round
	width=6.0
	hidpi=off
	active_color=0xffe2e2e3
	inactive_color=0xff414550
)

borders "${options[@]}"
```

This keeps working as it is. After moving it into `~/.config/mbar/init.lua`:

```lua
mbar.borders({
  style = "round",
  width = 6.0,
  hidpi = false,
  active_color = 0xffe2e2e3,
  inactive_color = 0xff414550,
})
```

or in a shell config:

```sh
mbar --borders style=round width=6.0 hidpi=off \
               active_color=0xffe2e2e3 inactive_color=0xff414550
```

### What works unchanged

Every documented option, with the same syntax, the same defaults and the same
result on screen:

| Option | Notes |
|---|---|
| `active_color`, `inactive_color` | `0xAARRGGBB`, `glow(0x…)`, `gradient(top_left=0x…,bottom_right=0x…)`, `gradient(top_right=0x…,bottom_left=0x…)` |
| `background_color` | `0xAARRGGBB`. As in JankyBorders, a gradient is accepted but not drawn and a glow draws as a solid fill |
| `width` | float, default `4.0` (`inf` and `nan` are rejected, see below) |
| `style` | `round`, `square`, `uniform` |
| `order` | `above`, `below` (undocumented in JankyBorders) |
| `hidpi` | `on`, `off` |
| `ax_focus` | `on`, `off`; default on when mbar has Accessibility permission |
| `blacklist`, `whitelist` | comma-separated process names, exact and case-sensitive |
| `apply-to` | window id: the other keys of that call apply to this window only (undocumented in JankyBorders). Each call sets that window's override to the current global settings plus its own keys, replacing an earlier one, as in JankyBorders |

Also unchanged:

- the parser's leniency: `width=5px` is 5, `style=` and `order=` look at the
  first character only, a longer key that starts with a color key
  (`active_colorX=…`) is a color error.
- the error lines the `borders` command prints on stdout:
  `[?] Borders: Invalid argument '<arg>'` and
  `[?] Borders: Invalid color argument color<rest>`.
- `borders -v` prints `borders-v1.9.0`, so version checks keep passing.
- `bordersrc` lookup (`~/.config/borders/bordersrc`, then `~/.bordersrc`,
  `$XDG_CONFIG_HOME` is not consulted) and the 60 s limit for it.
- `hidpi=`, `blacklist=` and `whitelist=` recreate all borders, and that drops
  the `apply-to` overrides, as in JankyBorders.

### Differences

The full list is in [`DEVIATIONS.md`](DEVIATIONS.md#jankyborders-borders) (B1…).
The ones you are most likely to notice:

- **IPC name.** mbar does not register JankyBorders' mach service
  `git.felix.borders`. The original `borders` binary and tools that send to
  that service cannot reach mbar. Use the `borders -> mbar` link.
- **Errors.** The `borders` command prints invalid arguments on stdout as
  before, but the bar does not print them a second time in its log.
  `mbar --borders` reports them as `[!] Borders: …` on stderr and exits 1
  (mbar's error convention). Valid keys in the same call still apply.
- **No daemon from `borders`.** `borders` never starts a daemon. If mbar is not
  running, it waits up to 5 s (a window manager can run its startup lines
  before mbar is up), then prints
  `borders: mbar is not running. mbar draws the window borders; start mbar.app.`
  and exits 1.
- **`drawing=on|off`.** A new key. `mbar --borders drawing=off` hides all
  borders and keeps the settings. The System page of `mbar.app` has a switch
  for it. The first `--borders` call without `drawing=` turns borders on.
- **`mbar --query borders`** prints the current configuration as JSON
  ([`EXTENSIONS.md`](EXTENSIONS.md#window-borders)).
- **`bordersrc` always runs**, also when a launch line passed arguments (see
  the checklist), and again on `--reload`. Its path is quoted, so a home
  directory with spaces works.
- **`apply-to` is not sticky.** In JankyBorders, a daemon started with
  `apply-to=N` sent every later call to window N. In mbar `apply-to=` only
  affects the call it is in.
- **`apply-to` overrides are capped.** mbar keeps at most 64 of them and drops
  the oldest. The override of a closed window stays until a `hidpi=`,
  `blacklist=` or `whitelist=` change or the cap drops it; JankyBorders lost it
  with the window.
- **Borders survive `--reload`.** Like the separate JankyBorders process, the
  borders configuration is not reset when mbar reloads its config (or
  hotloads it), so borders set by a window manager's launch line stay. The
  config and `bordersrc` run again and apply their keys on top. Deleting the
  borders lines does not turn borders off on the next reload: run
  `mbar --borders drawing=off` (or restart mbar).
- **`width=inf` / `width=nan`** are rejected as invalid arguments.
  JankyBorders accepted them.
- **`ax_focus=on` without Accessibility** logs a warning and falls back to the
  SkyLight focus path. JankyBorders exited.
- **Focus at startup.** mbar highlights the focused window right after borders
  are configured. JankyBorders waited for the first focus change.
- **No yabai proxy windows.** JankyBorders draws borders on yabai's animation
  proxies (`git.felix.jbevent`). mbar does not, so with yabai's window
  animations the border catches up when the animation ends.
- **Only the default bar.** `bordersrc` and the `borders` command go to the
  bar `mbar`. Other bar names (`bottom -> mbar`) do not run `bordersrc`.
- **No man page.** `borders -h` still says "Refer to the man page", but mbar
  does not install `man borders`. The options are in
  [`EXTENSIONS.md`](EXTENSIONS.md#window-borders).

### Known unknowns

mbar's borders are built from the JankyBorders source and its spec, but have
not run on a real Mac yet. These questions from the spec
([`docs/spec/borders.md`](spec/borders.md) §14) need real hardware. If you see
one of them, please report it.

1. **SkyLight events.** What exactly triggers events 723, 808, 1322, 1401,
   1508 and 815/816 on macOS 14 to 26. If they differ from JankyBorders'
   reading, a border can lag behind a focus change, a move or a resize.
2. **Minimize and hide.** Which events fire when a window is minimized or its
   app hidden. If none reaches mbar, a border could stay visible until the next
   update.
3. **Moving a window to another space** (Mission Control drag, yabai
   `--space`). mbar sends the border window to the new space when the target's
   space changes (B13). It is not known whether WindowServer would have moved
   it anyway, or whether a stale border showed on the old space.
4. **`SLSCopyWindowsWithOptionsAndTags`** may write back into its tag
   arguments. mbar passes fresh values on every call, as JankyBorders does.
5. **Corner radius on macOS 26.** mbar picks the macOS 26 value at runtime
   (B16). Whether it matches the system's window corners on every macOS 26.x
   needs a look.
6. **Sharing SkyLight notifications with spaces.** mbar uses one notification
   handler for the space tracking and the borders. Event 1322 means "focus" to
   the borders and "capture gating" to the space tracking; both run. mbar's own
   border windows must not show up in `space_windows_change`.
7. **macOS 13.** JankyBorders supports macOS 14 and later. `mbar.app` runs on
   macOS 13, where borders are untested.

Questions 6 and 7 of the spec (the raw MIG sub-level reply and the symtab scan
for `CGSGetConnectionPortById`) do not apply: mbar does not use either (B15).
