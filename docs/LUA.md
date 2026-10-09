# Configuring mbar with Lua

mbar embeds Lua 5.4. A Lua config (`init.lua`) does everything a shell
`sketchybarrc` does, but event handlers are **Lua functions that run inside the
daemon**: no `fork`/`exec` of a shell per event, per click or per `update_freq`
tick. The API is modelled on [SbarLua](https://github.com/FelixKratz/SbarLua), so
most SbarLua configs run after changing `require("sketchybar")` to
`require("mbar")` (and even that is optional, see
[Differences from SbarLua](#differences-from-sbarlua)).

Every Lua call is translated into the regular command language
(`docs/spec/cli.md`): `mbar.set("clock", { label = "hi" })` is exactly
`mbar --set clock label=hi`. Anything the CLI can do, Lua can do, and the
`--query` output, events and environment variables are the same.

## Where the config lives

The daemon looks for `init.lua` before `mbarrc` / `sketchybarrc` in the same
directory: `$XDG_CONFIG_HOME/mbar/init.lua`, `~/.config/mbar/init.lua`, and the
`sketchybar` directories as fallback (`--config <file>` overrides the lookup).

When the config runs:

* the global `CONFIG_DIR` (and `mbar.config_dir`) is the directory of `init.lua`;
* `package.path` starts with `<CONFIG_DIR>/?.lua;<CONFIG_DIR>/?/init.lua`, so a
  config can be split into modules: `require("items.clock")` loads
  `<CONFIG_DIR>/items/clock.lua`;
* `require("mbar")` and `require("sketchybar")` return the global `mbar` table;
* the safe Lua standard libraries are available (`string`, `table`, `math`,
  `utf8`, `os`, `io`, `coroutine`, `package`); loading C modules is disabled.

`mbar --reload` (or a change in the config directory with `mbar.hotload(true)`)
throws the Lua state away and runs `init.lua` again in a fresh one.

## Editor support

`lua/mbar.d.lua` in the mbar repository contains [LuaLS](https://luals.github.io)
annotations for the whole API and every property table (`mbar.ItemProps`,
`mbar.TextProps`, `mbar.BackgroundProps`, ...). Point LuaLS at it, e.g. with
`~/.config/mbar/.luarc.json`:

```json
{
  "runtime.version": "Lua 5.4",
  "workspace.library": ["/path/to/mbar/lua"]
}
```

## Complete example: the stock SketchyBar config in Lua

This is the default `sketchybarrc` with its `plugins/*.sh` scripts, written as one
`init.lua`. The plugin scripts become Lua functions.

<!-- example: stock -->
```lua
-- ~/.config/mbar/init.lua
local mbar = require("mbar")

-- Everything up to end_config() goes to the daemon as one message.
mbar.begin_config()

---------------------------------------------------------------- bar / defaults
mbar.bar({
  position = "top",
  height = 40,
  blur_radius = 30,
  color = 0x40000000,
})

mbar.default({
  padding_left = 5,
  padding_right = 5,
  icon = {
    font = "Hack Nerd Font:Bold:17.0",
    color = 0xffffffff,
    padding_left = 4,
    padding_right = 4,
  },
  label = {
    font = { family = "Hack Nerd Font", style = "Bold", size = 14.0 },
    color = 0xffffffff,
    padding_left = 4,
    padding_right = 4,
  },
})

---------------------------------------------------------------- left: spaces
-- plugins/space.sh: highlight the selected space.
local function space_changed(env)
  mbar.set(env.NAME, { background = { drawing = env.SELECTED == "true" } })
end

for sid = 1, 10 do
  mbar.add("space", "space." .. sid, {
    position = "left",
    space = sid,
    icon = { string = tostring(sid), padding_left = 7, padding_right = 7 },
    background = { color = 0x40ffffff, corner_radius = 5, height = 25 },
    label = { drawing = false },
    script = space_changed,                       -- runs in-process
    click_script = "yabai -m space --focus " .. sid, -- still a shell command
  })
end

---------------------------------------------------------------- left: chevron, front app
mbar.add("item", "chevron", {
  icon = "\u{f054}", -- nf-fa-chevron_right
  label = { drawing = false },
})

-- plugins/front_app.sh
local front_app = mbar.add("item", "front_app", { icon = { drawing = false } })
front_app:subscribe("front_app_switched", function(env)
  front_app:set({ label = env.INFO })
end)

---------------------------------------------------------------- right: clock, volume, battery
-- plugins/clock.sh (update_freq = 10 -> SENDER "routine"; --update -> "forced")
mbar.add("item", "clock", {
  position = "right",
  update_freq = 10,
  icon = "\u{f43a}", -- nf-oct-clock
  script = function(env)
    mbar.set(env.NAME, { label = os.date("%d/%m %H:%M") })
  end,
})

-- plugins/volume.sh
local volume = mbar.add("item", "volume", { position = "right" })
volume:subscribe("volume_change", function(env)
  local v = tonumber(env.INFO) or 0
  local icon = v >= 60 and "\u{f057e}" -- volume_high
    or v >= 30 and "\u{f0580}"         -- volume_medium
    or v >= 1 and "\u{f057f}"          -- volume_low
    or "\u{f0581}"                     -- volume_off
  volume:set({ icon = icon, label = v .. "%" })
end)

-- plugins/battery.sh
local battery = mbar.add("item", "battery", { position = "right", update_freq = 120 })
local function battery_update()
  mbar.exec("pmset -g batt", function(out)
    local pct = tonumber(out:match("(%d+)%%"))
    if not pct then return end -- no battery (desktop Mac)
    local icon
    if out:find("AC Power") then icon = "\u{f0e7}" -- bolt
    elseif pct >= 90 then icon = "\u{f240}"
    elseif pct >= 60 then icon = "\u{f241}"
    elseif pct >= 30 then icon = "\u{f242}"
    elseif pct >= 10 then icon = "\u{f243}"
    else icon = "\u{f244}" end
    battery:set({ icon = icon, label = pct .. "%" })
  end)
end
battery:subscribe({ "routine", "forced", "system_woke", "power_source_change" }, battery_update)

mbar.end_config()

-- Run every item's handler once (SENDER=forced), like `sketchybar --update`.
mbar.update()
```

The same items with mbar's native providers instead of handlers (no shell
commands at all):

```lua
mbar.add("item", "clock", {
  position = "right",
  icon = "\u{f43a}",
  provider = { "clock", args = "%d/%m %H:%M", freq = 10 },
})
mbar.add("item", "battery", {
  position = "right",
  provider = { "battery", format = "{percent}%" },
})
```

## API reference

Names in the `name` parameter may also be item objects (anything with a `.name`).

| Function | Command(s) sent | Notes |
|---|---|---|
| `mbar.add(type, name?, ..., props?)` | `--add <type> <name> <position> [<width>]` `--set <name> ...` | Returns an item object. See below. |
| `mbar.add("bracket", name?, { members }, props?)` | `--add bracket <name> <members...>` `--set ...` | Members are names (or `/regex/`) or item objects. |
| `mbar.add("event", name, notification?)` | `--add event <name> [<notification>]` | Returns nil. |
| `mbar.set(name, props)` | `--set <name> ...` | `name` may be `/regex/`. |
| `mbar.bar(props)` | `--bar ...` | |
| `mbar.borders(props)` | `--borders ...` | Window borders (JankyBorders options). See [Window borders](#window-borders). |
| `mbar.default(props)` | `--default ...` | |
| `mbar.remove(name)` | `--remove <name>` | |
| `mbar.subscribe(name, events, fn)` | `--set <name> script=lua:<id>` `--subscribe <name> <events...>` | `events`: a string (space separated) or a list. `mbar.subscribe(name, fn)` registers a catch-all. |
| `mbar.animate(curve, duration, fn)` | `--animate <curve> <duration> <everything fn sent>` | One message. Duration in 60 Hz frames. Nests. |
| `mbar.trigger(event, env?)` | `--trigger <event> K=V ...` | Table values are JSON encoded. |
| `mbar.query(what, ...)` | `--query <what> ...` | Returns the decoded JSON, or `nil, response`. |
| `mbar.exec(cmd, fn?)` | – | Runs `sh -c cmd` asynchronously; `fn(result, output)`. |
| `mbar.delay(seconds, fn)` | – | Calls `fn()` once after `seconds`. |
| `mbar.push(name, values...)` | `--push <name> <values...>` | Numbers or lists of numbers. |
| `mbar.provider(name, spec)` | `--set <name> provider=<p> provider.<k>=<v> ...` | `spec`: `"cpu"`, `{ "cpu", format = "{percent}%", freq = 2 }`, or `false` (= `none`). |
| `mbar.begin_config()` / `mbar.end_config()` | – | `end_config` sends everything queued. |
| `mbar.flush()` | – | Sends everything queued, returns the response. |
| `mbar.hotload(bool)` | `--hotload on\|off` | |
| `mbar.update()` | `--update` | |
| `mbar.menu(index)` | `--menu <index\|title>` | |
| `mbar.reload(path?)` | `--reload [<path>]` | |
| `mbar.command(...)` | the arguments, verbatim | Escape hatch (`--move`, `--rename`, `--clone`, ...). Sent immediately, returns the response. |
| `mbar.json.decode(s)` / `mbar.json.encode(v)` | – | `null` decodes to nil; sequences encode as arrays. |
| `mbar.event_loop()` | – | No-op (SbarLua compatibility). |

Item objects returned by `mbar.add` have the field `name` and the methods
`item:set(props)`, `item:subscribe(events, fn)`, `item:query()`
(`--query item <name>`), `item:remove()`, `item:push(values...)` and
`item:provider(spec)`. `set`, `subscribe` and `provider` return the item.

### `mbar.add`

```lua
mbar.add("item", "clock", { position = "right", update_freq = 10 })
mbar.add("item", "clock", "right", { update_freq = 10 })   -- position as argument
mbar.add("graph", "cpu.graph", 80, { position = "right" })  -- 80 samples
mbar.add("slider", "volume.slider", "popup.volume", 100)    -- track width 100
mbar.add("space", "space.1", { space = 1, icon = "1" })
mbar.add("alias", "Control Center,Battery", { position = "right" })
mbar.add("item", { width = 8 })                             -- name generated
mbar.add("bracket", "status", { "clock", "volume", battery }, { background = { color = 0x40000000 } })
mbar.add("event", "theme_changed", "AppleInterfaceThemeChangedNotification")
mbar.add("app_menu", "menus", "left")
```

After the type, an optional name (a string) is followed by any mix of: a
string (the position), a number (the width of graphs and sliders) and a table
(the properties). The position is taken from the argument, else from
`props.position`, else `"left"`; `position` is never repeated in the `--set`.
Without a name, mbar generates a unique one (`item.name` holds it).

### Property tables

Property tables are flattened into `key=value` pairs:

| Lua | Pair(s) |
|---|---|
| `{ update_freq = 10 }` | `update_freq=10` |
| `{ icon = { font = { family = "Hack", size = 14.0 } } }` | `icon.font.family=Hack icon.font.size=14` |
| `{ label = { drawing = false } }` | `label.drawing=off` |
| `{ background = { color = 0x40ffffff } }` | `background.color=0x40ffffff` |
| `{ icon = "1" }` (a string where a sub-domain also exists) | `icon=1` |
| `{ space = { 1, 2, 3 } }` (list) | `space=1,2,3` |
| `{ background = { image = { "app.Safari", scale = 0.5 } } }` | `background.image=app.Safari background.image.scale=0.5` |
| `{ provider = { "cpu", format = "{percent}%" } }` | `provider=cpu provider.format={percent}%` |
| `{ y_offset = 1.0 }`, `{ width = 1.5 }` | `y_offset=1`, `width=1.5` |
| `{ script = function(env) ... end }` | `script=lua:<id>` |

Rules: nested tables become dot keys; the list part of a nested table is the
value of the key itself (joined with `,`); booleans become `on`/`off`; integral
numbers are written without a decimal point; numbers under a color key
(`color` or `*_color`, at any depth) are written as `0xAARRGGBB`; strings pass
through unchanged. Keys are sent in sorted order. An empty table sends nothing.
Functions are accepted only for `script` and `click_script`.

### Handlers

```lua
local wifi = mbar.add("item", "wifi", { position = "right" })

wifi:subscribe("wifi_change", function(env)
  wifi:set({ label = env.INFO ~= "" and env.INFO or "offline" })
end)

wifi:subscribe({ "mouse.entered", "mouse.exited" }, function(env)
  mbar.animate("tanh", 15, function()
    wifi:set({ background = { color = env.SENDER == "mouse.entered" and 0x40ffffff or 0 } })
  end)
end)

wifi:subscribe("mouse.clicked", function(env)
  if env.BUTTON == "right" then mbar.exec("open 'x-apple.systempreferences:com.apple.Network-Settings.extension'") end
  wifi:set({ popup = { drawing = "toggle" } })
end)
```

* The first `subscribe` (or a `script = function` property) sets the item's
  `script` to `lua:<id>`. `--query` shows that value. Further `subscribe` calls on
  the same item reuse the id and add handlers for more events; each one sends
  `--set <name> script=lua:<id>` again (a no-op when unchanged), so an item that
  was removed and re-added elsewhere, or whose script a shell replaced, still
  gets its handler.
* `mbar.add` and `mbar.remove(name)` / `item:remove()` discard the Lua handlers
  registered under that name, so a re-created item starts from a fresh handler.
  `--remove` and `--rename` sent through `mbar.command` are tracked too (a
  rename keeps the handlers with the renamed item). An item renamed by a shell
  keeps working until Lua `mbar.add`s an item with its old name.
* `script = function` and `click_script = function` reuse the item's (or the
  `/regex/` selector's) handler id, so replacing them from a handler does not
  grow memory. In `mbar.default` / `mbar.bar` every function gets a new id
  (items created earlier keep their function).
* When the item's script fires, the handler registered for `env.SENDER` runs;
  if there is none, the catch-all runs (`mbar.subscribe(name, fn)`,
  `item:subscribe("*", fn)` or `script = fn`). With neither, nothing happens.
* `"routine"` (the `update_freq` tick) and `"forced"` (`mbar --update`) are
  senders, not events: subscribing to them only registers the handler, they
  are not sent to `--subscribe`.
* `env` holds the script environment as **strings**: `NAME`, `SENDER`, `INFO`,
  `BUTTON`, `MODIFIER`, `SCROLL_DELTA`, `SELECTED`, `SID`, `DID`, `PERCENTAGE`,
  and any `K=V` passed to `--trigger`. When `INFO` is a JSON object or array
  (`mouse.clicked`, `space_change`, `space_windows_change`, `media_change`,
  providers, ...), `env.info` holds it decoded:

  ```lua
  space:subscribe("space_windows_change", function(env)
    local apps = {}
    for app in pairs(env.info.apps) do apps[#apps + 1] = app end
    space:set({ label = table.concat(apps, " ") })
  end)
  ```
* `click_script = function(env) ... end` runs on click, with `BUTTON`,
  `MODIFIER` and `INFO`, but no `SENDER` (as for shell click scripts).
* An error in a handler is logged by the daemon with a Lua traceback; commands
  the handler sent before the error are still applied.

### Batching and flush points

Commands are queued and sent to the daemon as **one message** (one layout pass)
when the config, handler, `exec` callback or `delay` callback returns. The queue
is sent earlier when Lua needs the daemon to be current:

* before `mbar.query(...)` and `item:query()`,
* before `mbar.exec(...)` starts the shell command (the command may call `mbar`),
* before an `mbar.animate` block is sent,
* on `mbar.end_config()`, `mbar.flush()` and `mbar.command(...)`.

So the whole initial config costs a single message unless it queries the daemon
in between. `mbar.begin_config()` exists for SbarLua compatibility.

### Animations

```lua
mbar.animate("sin", 30, function()
  mbar.set("clock", { y_offset = 10 })
  mbar.set("clock", { y_offset = 0 })   -- chained: bounces
  mbar.bar({ color = 0x80000000 })
end)
```

Everything sent inside `fn` goes out as one message `--animate sin 30 --set ...`
after the commands queued before it. Nested blocks switch the curve for their
part and restore the outer one afterwards. The duration is in frames at 60 Hz.

### `mbar.exec` and `mbar.delay`

```lua
mbar.exec("curl -s 'wttr.in/?format=j1'", function(weather, raw)
  if type(weather) == "table" then
    mbar.set("weather", { label = weather.current_condition[1].temp_C .. "°" })
  end
end)

mbar.delay(0.5, function() mbar.set("toast", { drawing = false }) end)
```

`exec` runs `sh -c <cmd>` without blocking the bar. The callback gets
`(result, output)`: `result` is the decoded JSON when the output is a JSON
object or array, otherwise the output string itself; `output` is always the raw
string. The callback runs as soon as the shell exits: output that background
processes started by the command (`cmd &`) write after that is not captured
(their stdout is closed). At most 4 MiB of output are kept; a command that
writes more has its stdout closed at that point, like `cmd | head -c 4194304`.
Like every child, the shell gets `SIGALRM` after 60 s.

### Graphs and providers

```lua
local cpu = mbar.add("graph", "cpu", 60, {
  position = "right",
  graph = { color = 0xff9dd274, fill_color = 0x409dd274 },
  provider = { "cpu", format = "{percent}%", freq = 2 },
})

cpu:subscribe("*", function(env)
  if env.SENDER == "provider" then cpu:push(tonumber(env.info.percent) / 100) end
end)
```

A provider fires the item's script with `SENDER=provider` and the sample in
`INFO` (so `env.info`) — see `docs/EXTENSIONS.md` for the providers and their keys.

### Window borders

mbar draws colored borders around windows, highlighting the focused one (it
replaces [JankyBorders](https://github.com/FelixKratz/JankyBorders)).
Borders are off until the config turns them on; the first `mbar.borders` call
does that (unless it sets `drawing = false`).

<!-- example: borders -->
```lua
mbar.borders({
  active_color = 0xffe1e3e4,
  inactive_color = 0xff494d64,
  width = 5.0,
  style = "round",
  hidpi = false,
  blacklist = { "Safari", "kitty" },
})

-- Glow and gradients:
mbar.borders({ active_color = { glow = 0xd2e1e3e4 } })
mbar.borders({
  active_color = { gradient = { top_left = 0xffa6da95, bottom_right = 0xff8aadf4 } },
})

mbar.borders({ drawing = false })  -- hide all borders
```

One call sends one `--borders` message. The keys are the JankyBorders options
with the same meaning (`active_color`, `inactive_color`, `background_color`,
`width`, `style`, `order`, `hidpi`, `ax_focus`, `blacklist`, `whitelist`), plus
mbar's `drawing`. The values are converted to the JankyBorders syntax:

| Lua | Pair |
|---|---|
| `active_color = 0xffe1e3e4` | `active_color=0xffe1e3e4` |
| `active_color = { glow = 0xffe1e3e4 }` | `active_color=glow(0xffe1e3e4)` |
| `active_color = { gradient = { top_left = 0xffff0000, bottom_right = 0xff0000ff } }` | `active_color=gradient(top_left=0xffff0000,bottom_right=0xff0000ff)` |
| `active_color = { gradient = { top_right = 0xffff0000, bottom_left = 0xff0000ff } }` | `active_color=gradient(top_right=0xffff0000,bottom_left=0xff0000ff)` |
| `blacklist = { "Safari", "kitty" }` | `blacklist=Safari,kitty` |
| `whitelist = {}` | `whitelist=` (no filter) |
| `width = 5.0`, `width = 4.5` | `width=5`, `width=4.5` |
| `hidpi = true`, `drawing = false` | `hidpi=on`, `drawing=off` |
| `apply_to = 4242` | `apply-to=4242` (window 4242 gets the global settings plus the other keys, replacing its earlier override) |

Strings are sent verbatim, so `active_color = "glow(0xffe1e3e4)"` works too.
A table that has no JankyBorders form (`active_color = { shimmer = 1 }`,
`style = { "round" }`) is a Lua error. Unknown keys and values the daemon
rejects are not: its `[!] Borders: ...` answer is logged as a warning, like
for every queued command, and the valid keys of the same call still apply. A `~/.config/borders/bordersrc` is still run
after `init.lua`; once its settings are in `init.lua`, delete it. The current
settings are in `mbar.query("borders")`. The borders settings survive
`mbar --reload` and hotload, so deleting an `mbar.borders` call does not turn
borders off on the next reload; `mbar.borders({ drawing = false })` does.

## Differences from SbarLua

* `require("sketchybar")` works and returns `mbar`; `sbar.event_loop()` is a
  no-op. No compiled module (`sketchybar.so`) and no `package.cpath` setup is
  needed.
* `env.INFO` is always the raw string, like for shell scripts. The decoded
  JSON is in `env.info` (SbarLua replaces `env.INFO` with the table). Use
  `env.info or env.INFO` in code that should run with both.
* The `mbar.exec` callback gets `(result, raw_output)`; SbarLua passes the exit
  code as second argument.
* Handlers run in-process on the daemon's main thread. A handler that blocks
  (e.g. `io.popen` of a slow command) blocks the bar: use `mbar.exec`.
* Do not call the `mbar` / `sketchybar` client of the same bar through
  `io.popen` or `os.execute` (a common SbarLua pattern, where Lua runs in a
  separate process). The daemon answers messages on the thread that is
  running the Lua code, so the client could only wait for its 5 s timeout and
  print nothing, freezing the bar meanwhile. Instead, these two functions
  export `MBAR_LUA_SYNC=<bar name>` into the shell they start, and a client
  that sees its own bar name there fails at once (stderr message, exit code
  1). Use `mbar.query(...)` for queries and `mbar.exec(cmd, fn)` (asynchronous,
  not marked) for commands that talk to the bar. A process started in the
  background from such a shell inherits the marker; it can `unset
  MBAR_LUA_SYNC` before messaging the bar.
  Handlers queued by other handlers (`mbar.trigger`, a `set` that fires a
  subscribed event) run in bounded batches between IPC requests, timers and
  frames, so a handler that re-triggers its own event burns CPU like the
  equivalent shell script would, but the bar stays responsive.
* New: `mbar.provider`, `mbar.borders`, `mbar.menu`, `mbar.command`, `mbar.flush`,
  `mbar.json`, `script = function` / `click_script = function` properties,
  list values (`space = { 1, 2 }`) and the positional value convention
  (`image = { "app.Safari", scale = 0.5 }`).

## For daemon integrators

The `mbar-lua` crate does not depend on `mbar-core`. The daemon implements
`mbar_lua::Host` (`command(argv) -> response`, `spawn_shell(cmd, callback)`,
`schedule(delay, callback)`) and owns one `LuaEngine` per config load:

* `LuaEngine::load_file(path, host)` runs `init.lua`.
* When an item's `script` or `click_script` is `lua:<id>`
  (`mbar_lua::parse_script`), call `LuaEngine::run_handler(id, env, host)`
  with the same environment a shell script would get, instead of spawning.
* When a `spawn_shell` with `Some(id)` exits, call `exec_finished(id, stdout, host)`;
  when a `schedule` deadline passes, call `timer_fired(id, host)`.
* `Host::command` runs while the engine is borrowed: events that target Lua
  handlers, and `--reload`, produced by those commands must be queued and run
  after the engine call returns.
* On reload, drop the engine (pending exec/timer ids become unknown and are
  ignored) and create a new one.
