# Migrating from SketchyBar

mbar implements SketchyBar's command language, properties, events, script
environment and `--query` output (reference: SketchyBar v2.24.0, see
[`docs/spec/`](spec/)). A shell config usually runs unchanged. A SbarLua config
needs a few small edits.

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
