---@meta mbar

-- LuaLS type definitions for the mbar Lua API (docs/LUA.md).
--
-- Usage: add this directory to your LuaLS workspace library, e.g. in
-- ~/.config/mbar/.luarc.json:
--   { "workspace.library": ["/path/to/mbar/lua"], "runtime.version": "Lua 5.4" }
--
-- Every function translates into the regular mbar/SketchyBar command language.
-- Property tables are flattened: nested tables become dot keys, booleans become
-- on/off, numbers under color keys become 0xAARRGGBB, and the list part of a
-- nested table is the value of the key itself (`image = { "app.Safari", scale = 0.5 }`).

------------------------------------------------------------------------------
-- Scalars
------------------------------------------------------------------------------

---A color: `0xAARRGGBB` as an integer, or a string such as `"0xff00ff00"`.
---@alias mbar.Color integer|string

---A boolean property. `"toggle"` flips the current value.
---@alias mbar.Bool boolean|"on"|"off"|"toggle"|"yes"|"no"|"true"|"false"|"1"|"0"

---Item position. `"popup.<item>"` places the item in the popup of `<item>`;
---`"q"`/`"e"` are left/right of the notch.
---@alias mbar.Position "left"|"right"|"center"|"q"|"e"|string

---Item types for `mbar.add`.
---@alias mbar.ItemType "item"|"space"|"alias"|"bracket"|"graph"|"slider"|"app_menu"|"event"

---Animation curves (only the first letter counts).
---@alias mbar.Curve "linear"|"quadratic"|"tanh"|"sin"|"exp"|"circ"

---Built-in events plus pseudo senders. Custom events (`mbar.add("event", ...)`) are plain strings.
---@alias mbar.Event
---| "front_app_switched"
---| "space_change"
---| "display_change"
---| "system_woke"
---| "system_will_sleep"
---| "mouse.entered"
---| "mouse.exited"
---| "mouse.clicked"
---| "mouse.scrolled"
---| "mouse.entered.global"
---| "mouse.exited.global"
---| "mouse.scrolled.global"
---| "volume_change"
---| "brightness_change"
---| "power_source_change"
---| "wifi_change"
---| "media_change"
---| "space_windows_change"
---| "menus_change"
---| "aerospace_workspace_change"
---| "aerospace_focus_change"
---| "aerospace_monitor_change"
---| "aerospace_mode_change"
---| "aerospace_window_detected"
---| "aerospace_binding_triggered"
---| "routine"  # update_freq tick (handler-side only, never sent to --subscribe)
---| "forced"   # --update (handler-side only)
---| "*"        # catch-all
---| string

---Native data providers (docs/EXTENSIONS.md).
---@alias mbar.ProviderName "clock"|"cpu"|"memory"|"battery"|"volume"|"wifi"|"network"|"disk"|"front_app"|"media"|"aerospace"|"none"

------------------------------------------------------------------------------
-- Property tables
------------------------------------------------------------------------------

---@class mbar.ColorProps
---@field [1]? integer The color itself
---@field alpha? number 0..1
---@field red? number
---@field green? number
---@field blue? number

---@class mbar.FontProps
---@field family? string e.g. "Hack Nerd Font"
---@field style? string e.g. "Bold"
---@field size? number
---@field features? string OpenType features, e.g. "+ss01"
---@field typographical_width? mbar.Bool

---@class mbar.ShadowProps
---@field drawing? mbar.Bool
---@field color? mbar.Color|mbar.ColorProps
---@field angle? integer Degrees
---@field distance? integer

---@class mbar.ImageProps
---@field [1]? string Image source (same as `string`)
---@field string? string Path, `app.<bundle id>`, `media.artwork`, ...
---@field drawing? mbar.Bool
---@field scale? number
---@field corner_radius? integer
---@field border_width? number
---@field border_color? mbar.Color|mbar.ColorProps
---@field padding_left? integer
---@field padding_right? integer
---@field y_offset? integer
---@field shadow? mbar.ShadowProps

---@class mbar.BackgroundProps
---@field drawing? mbar.Bool
---@field color? mbar.Color|mbar.ColorProps Also enables drawing
---@field border_color? mbar.Color|mbar.ColorProps
---@field border_width? integer
---@field height? integer 0 = auto
---@field corner_radius? integer
---@field padding_left? integer
---@field padding_right? integer
---@field x_offset? integer
---@field y_offset? integer
---@field clip? number 0..1
---@field image? string|mbar.ImageProps
---@field shadow? mbar.ShadowProps

---Icon, label and slider knob.
---@class mbar.TextProps
---@field [1]? string The text (same as `string`)
---@field string? string
---@field drawing? mbar.Bool
---@field color? mbar.Color|mbar.ColorProps
---@field highlight? mbar.Bool
---@field highlight_color? mbar.Color|mbar.ColorProps
---@field font? string|mbar.FontProps "Family:Style:Size" or a table
---@field padding_left? integer
---@field padding_right? integer
---@field y_offset? integer
---@field width? integer|"dynamic"
---@field align? "left"|"center"|"right"
---@field max_chars? integer
---@field scroll_duration? integer Frames at 60 Hz
---@field background? mbar.BackgroundProps
---@field shadow? mbar.ShadowProps

---@class mbar.PopupProps
---@field drawing? mbar.Bool
---@field horizontal? mbar.Bool
---@field align? "left"|"center"|"right"
---@field height? integer
---@field y_offset? integer
---@field blur_radius? integer
---@field topmost? mbar.Bool
---@field background? mbar.BackgroundProps

---@class mbar.GraphProps
---@field color? mbar.Color|mbar.ColorProps Line color
---@field fill_color? mbar.Color|mbar.ColorProps
---@field line_width? number

---@class mbar.SliderProps
---@field percentage? integer 0..100
---@field width? integer Track length
---@field highlight_color? mbar.Color|mbar.ColorProps
---@field knob? string|mbar.TextProps
---@field background? mbar.BackgroundProps

---@class mbar.AliasProps
---@field color? mbar.Color|mbar.ColorProps
---@field scale? number
---@field update_freq? integer
---@field shadow? mbar.ShadowProps

---Native provider (docs/EXTENSIONS.md). `{ "cpu", format = "{percent}%" }`.
---@class mbar.ProviderProps
---@field [1]? mbar.ProviderName
---@field name? mbar.ProviderName
---@field format? string Label template with `{key}` placeholders
---@field icon_format? string Icon template
---@field freq? number Sampling interval in seconds
---@field args? string Provider specific (strftime format, interface, mount point, aerospace "workspace"/"mode"/"monitor")

---@class mbar.AppMenuProps
---@field font? string|mbar.FontProps
---@field app_font? string|mbar.FontProps
---@field color? mbar.Color
---@field highlight_color? mbar.Color
---@field spacing? integer
---@field apple? mbar.Bool
---@field max_titles? integer
---@field corner_radius? integer

---Handler environment. All fields are strings, as for shell scripts.
---@class mbar.Env
---@field NAME string Item name
---@field SENDER? mbar.Event|string Event name, "routine" or "forced" (absent for click_script)
---@field INFO? string Event payload
---@field info? table|any[] `INFO` decoded, when it is a JSON object or array
---@field BUTTON? "left"|"right"|"other"
---@field MODIFIER? string Comma list of shift,ctrl,alt,cmd,fn or "none"
---@field SCROLL_DELTA? string
---@field SELECTED? "true"|"false" Space items
---@field SID? string Space items: Mission Control index
---@field DID? string Display index
---@field PERCENTAGE? string Slider items
---@field FOCUSED_WORKSPACE? string AeroSpace workspace, focus and monitor events
---@field PREV_WORKSPACE? string aerospace_workspace_change
---@field AEROSPACE_FOCUSED_WORKSPACE? string Alias of FOCUSED_WORKSPACE (AeroSpace's own name)
---@field AEROSPACE_PREV_WORKSPACE? string Alias of PREV_WORKSPACE
---@field WINDOW_ID? string aerospace_focus_change (empty on an empty workspace), aerospace_window_detected
---@field MONITOR_ID? string aerospace_monitor_change (1-based)
---@field MODE? string aerospace_mode_change, aerospace_binding_triggered
---@field WORKSPACE? string aerospace_window_detected
---@field APP_BUNDLE_ID? string aerospace_window_detected
---@field APP_NAME? string aerospace_window_detected
---@field BINDING? string aerospace_binding_triggered
---@field [string] string Custom `mbar.trigger` variables

---@alias mbar.Handler fun(env: mbar.Env)

---@class mbar.ItemProps
---@field position? mbar.Position Used as the `--add` position by mbar.add
---@field drawing? mbar.Bool
---@field updates? mbar.Bool|"when_shown"
---@field scroll_texts? mbar.Bool
---@field width? integer|"dynamic"
---@field script? string|mbar.Handler Shell command, or a Lua function (in-process)
---@field click_script? string|mbar.Handler
---@field update_freq? integer Seconds between "routine" updates
---@field align? "left"|"center"|"right"
---@field space? integer|integer[]|string
---@field associated_space? integer|integer[]|string
---@field display? integer|integer[]|"active"
---@field associated_display? integer|integer[]|"active"
---@field y_offset? integer
---@field padding_left? integer
---@field padding_right? integer
---@field blur_radius? integer
---@field shadow? mbar.Bool
---@field ignore_association? mbar.Bool
---@field mach_helper? string
---@field icon? string|mbar.TextProps
---@field label? string|mbar.TextProps
---@field background? mbar.BackgroundProps
---@field popup? mbar.PopupProps
---@field graph? mbar.GraphProps
---@field slider? mbar.SliderProps
---@field alias? mbar.AliasProps
---@field provider? mbar.ProviderName|mbar.ProviderProps
---@field app_menu? mbar.AppMenuProps

---@class mbar.BarProps
---@field position? "top"|"bottom"
---@field height? integer
---@field margin? integer
---@field y_offset? integer
---@field blur_radius? integer
---@field font_smoothing? mbar.Bool
---@field shadow? mbar.Bool
---@field notch_width? integer
---@field notch_offset? integer
---@field notch_display_height? integer
---@field hidden? mbar.Bool|"current"
---@field topmost? mbar.Bool|"window"
---@field sticky? mbar.Bool
---@field display? "main"|"all"|integer|integer[]
---@field show_in_fullscreen? mbar.Bool
---@field hide_menubar? mbar.Bool
---@field color? mbar.Color|mbar.ColorProps
---@field border_color? mbar.Color|mbar.ColorProps
---@field border_width? integer
---@field corner_radius? integer
---@field padding_left? integer
---@field padding_right? integer
---@field image? string|mbar.ImageProps
---@field shadow? mbar.Bool

---A window border color (JankyBorders syntax): a solid `0xAARRGGBB`, a glow,
---or a two-color gradient. Strings are sent verbatim (`"glow(0xffe1e3e4)"`).
---@alias mbar.BorderColor mbar.Color|{ glow: mbar.Color }|{ gradient: { top_left: mbar.Color, bottom_right: mbar.Color } }|{ gradient: { top_right: mbar.Color, bottom_left: mbar.Color } }

---Window border settings (`--borders`, the JankyBorders options).
---@class mbar.BordersProps
---@field drawing? mbar.Bool Borders are off until the first `mbar.borders` call
---@field active_color? mbar.BorderColor Focused window (default 0xffe1e3e4)
---@field inactive_color? mbar.BorderColor Other windows (default 0x00000000)
---@field background_color? mbar.BorderColor Fill behind windows (default 0x00000000: off)
---@field width? number Border width in points (default 4.0; must be finite)
---@field style? "round"|"square"|"uniform"
---@field order? "above"|"below"
---@field hidpi? boolean
---@field ax_focus? boolean Track focus through Accessibility (default: on when trusted)
---@field blacklist? string[]|string Process names that get no border
---@field whitelist? string[]|string Only these process names get a border
---@field apply_to? integer Window id: the other keys apply to this window only, on top of the global settings, replacing its earlier override (sent as `apply-to`)

------------------------------------------------------------------------------
-- Item objects
------------------------------------------------------------------------------

---@class mbar.Item
---@field name string
local Item = {}

---Sets properties (`--set <name> ...`).
---@param props mbar.ItemProps
---@return mbar.Item self
function Item:set(props) end

---Subscribes to events and registers an in-process handler. Calling it again
---adds handlers for more events; `fn` alone (or `"*"`) is the catch-all.
---@param events mbar.Event|mbar.Event[]|mbar.Handler
---@param fn? mbar.Handler
---@return mbar.Item self
function Item:subscribe(events, fn) end

---Returns the decoded `--query item <name>` JSON, or `nil, response`.
---@return table|nil
---@return string? error
function Item:query() end

---Removes the item.
function Item:remove() end

---Pushes values (0..1) to a graph.
---@param ... number|number[]
function Item:push(...) end

---Sets the native provider of this item.
---@param spec mbar.ProviderName|mbar.ProviderProps|false
---@return mbar.Item self
function Item:provider(spec) end

------------------------------------------------------------------------------
-- Module
------------------------------------------------------------------------------

---@class mbar.api
---@field config_dir string Directory of the config file (same as CONFIG_DIR)
mbar = {}

---Directory of the config file.
---@type string
CONFIG_DIR = ""

---Adds an item. Forms:
--- * `mbar.add("item", name, props?)`, `mbar.add("item", name, position, props?)`
--- * `mbar.add("graph", name, width, props?)`, `mbar.add("slider", name, position?, width, props?)`
--- * `mbar.add("bracket", name, { members... }, props?)`
--- * `mbar.add("event", name, notification?)` (returns nil)
--- * the name may be omitted: a unique name is generated (`item.name` has it)
---
---The position defaults to `props.position`, then "left".
---@param type mbar.ItemType
---@param name? string
---@param ... string|number|table|mbar.ItemProps
---@return mbar.Item
function mbar.add(type, name, ...) end

---Sets item properties. `name` may be a `/regex/`.
---@param name string|mbar.Item
---@param props mbar.ItemProps
function mbar.set(name, props) end

---Sets bar properties.
---@param props mbar.BarProps
function mbar.bar(props) end

---Configures the window borders (`--borders ...`, the JankyBorders options).
---Color tables become `glow(...)` / `gradient(...)`, lists join with `,`.
---@param props mbar.BordersProps
function mbar.borders(props) end

---Sets defaults for items added afterwards.
---@param props mbar.ItemProps
function mbar.default(props) end

---Removes an item (or all items matching a `/regex/`).
---@param name string|mbar.Item
function mbar.remove(name) end

---Subscribes an item to events with an in-process Lua handler. The item's
---`script` becomes `lua:<id>`; handlers are chosen by `env.SENDER`, with the
---catch-all (`fn` only, or event `"*"`) as fallback. `"routine"` and `"forced"`
---can be used as events for update_freq ticks and `--update`.
---@param name string|mbar.Item
---@param events mbar.Event|mbar.Event[]|mbar.Handler
---@param fn? mbar.Handler
function mbar.subscribe(name, events, fn) end

---Animates every property change made inside `fn` (sent as one message).
---@param curve mbar.Curve
---@param duration integer Frames at 60 Hz
---@param fn fun()
function mbar.animate(curve, duration, fn) end

---Triggers an event. Table values in `env` are JSON encoded.
---@param event string
---@param env? table<string, string|number|boolean|table>
function mbar.trigger(event, env) end

---Queries the daemon (`--query ...`) and returns decoded JSON, or `nil, response`.
---@param what "bar"|"defaults"|"events"|"displays"|"default_menu_items"|"stats"|"menus"|"item"|string
---@param ... string
---@return any|nil
---@return string? error
function mbar.query(what, ...) end

---Runs a shell command asynchronously. `fn(result, output)` gets the decoded
---JSON object/array (or the plain output) and the raw output.
---@param cmd string
---@param fn? fun(result: table|string, output: string)
function mbar.exec(cmd, fn) end

---Calls `fn` once after `seconds`.
---@param seconds number
---@param fn fun()
function mbar.delay(seconds, fn) end

---Pushes values (0..1) to a graph item.
---@param name string|mbar.Item
---@param ... number|number[]
function mbar.push(name, ...) end

---Sets the native provider of an item: `mbar.provider("cpu", { "cpu", format = "{percent}%" })`.
---@param name string|mbar.Item
---@param spec mbar.ProviderName|mbar.ProviderProps|false
function mbar.provider(name, spec) end

---Starts a batch (commands are always batched; kept for SbarLua compatibility).
function mbar.begin_config() end

---Ends a batch and sends everything queued so far.
function mbar.end_config() end

---Sends everything queued so far; returns the daemon's response.
---@return string
function mbar.flush() end

---Enables or disables reloading on config changes.
---@param enabled? mbar.Bool
function mbar.hotload(enabled) end

---Forces an update of all items (`--update`).
function mbar.update() end

---Opens a top-level menu of the front application (index 0 = Apple menu).
---@param index integer|string
function mbar.menu(index) end

---Reloads the config (optionally from another path).
---@param path? string
function mbar.reload(path) end

---Sends raw argv immediately and returns the response, e.g.
---`mbar.command("--move", "a", "before", "b")`.
---@param ... string|number
---@return string
function mbar.command(...) end

---No-op (SbarLua compatibility: the daemon owns the event loop).
function mbar.event_loop() end

mbar.json = {}

---Decodes JSON into Lua values (`null` becomes nil). Returns `nil, error` on failure.
---@param s string
---@return any
---@return string? error
function mbar.json.decode(s) end

---Encodes a Lua value as JSON (sequences become arrays).
---@param v any
---@return string
function mbar.json.encode(v) end

------------------------------------------------------------------------------
-- AeroSpace (docs/LUA.md "AeroSpace", docs/EXTENSIONS.md)
------------------------------------------------------------------------------

---AeroSpace events for `mbar.aerospace.on`, with or without the `aerospace_` prefix.
---@alias mbar.AerospaceEvent
---| "workspace_change"
---| "focus_change"
---| "monitor_change"
---| "mode_change"
---| "window_detected"
---| "binding_triggered"
---| "aerospace_workspace_change"
---| "aerospace_focus_change"
---| "aerospace_monitor_change"
---| "aerospace_mode_change"
---| "aerospace_window_detected"
---| "aerospace_binding_triggered"

---Result of `mbar.aerospace.run`.
---@class mbar.AerospaceResult
---@field exit_code integer The command's exit code; -1 when it could not run (AeroSpace not running, …)
---@field stdout string
---@field stderr string The command's error output, or why it could not run

---Talks to a running AeroSpace directly (its socket; the `aerospace` CLI as fallback).
---Commands run on a worker thread, callbacks on the daemon's Lua thread. The first
---call connects mbar to AeroSpace.
mbar.aerospace = {}

---Runs one AeroSpace command, e.g. `mbar.aerospace.run({ "workspace", "3" })`.
---Without `fn` it is fire and forget.
---@param args (string|number)[] The `aerospace` arguments
---@param fn? fun(result: mbar.AerospaceResult)
function mbar.aerospace.run(args, fn) end

---Runs one AeroSpace command and decodes its stdout as JSON, e.g.
---`mbar.aerospace.query({ "list-windows", "--all", "--json" }, fn)`. `value` is nil and
---`err` set when the command failed or its output is not JSON.
---@param args (string|number)[]
---@param fn fun(value: any, err: string?)
function mbar.aerospace.query(args, fn) end

---Registers an item-less, in-process handler for an AeroSpace event: item settings
---(`updates`, `drawing`, the default item) do not affect it, `env.SENDER` is the event
---and there is no `env.NAME`. Several handlers per event all run, in registration
---order. When mbar already knows the state for the event, the handler is called once
---right away with it.
---@param event mbar.AerospaceEvent
---@param fn mbar.Handler
function mbar.aerospace.on(event, fn) end

return mbar
