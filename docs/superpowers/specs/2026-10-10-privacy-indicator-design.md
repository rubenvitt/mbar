# Privacy indicator awareness: design

Date: 2026-10-10. Status: draft, under review.

macOS shows a coloured dot while an app uses the microphone (orange), the camera
(green) or records the screen or system audio (purple). With the native menu bar
hidden (`hide_menubar=on`, or the system setting), WindowServer draws that dot in
the top-right corner of the display, on top of mbar's bar: it covers the right-most
item (usually the clock). mbar learns when the dot is shown, where it is and which
apps caused it. A config can then move the bar's right-hand items out of the way
and react to the event (an icon per sensor, a coloured clock, …).

## Findings (macOS 27.0.1, 2026-10-10)

Measured with throwaway probes on the built-in display (2056 pt wide):

- **The dot is a window.** `CGWindowListCopyWindowInfo` lists it while it is
  visible: owner `Window Server`, name `StatusIndicator`, layer `2147483630`,
  bounds `{x: 2025, y: 3, w: 28, h: 28}`. Two windows with identical bounds were
  listed at once. The bounds move briefly (`{x: 2029, y: 1}` for about 10 s) when a
  new source appears.
- **No window without a source.** With no source active the window is gone
  (`optionAll` too). It appears together with the first source and disappears
  about 0.8 s after the last one ends (fade-out).
- **SkyLight does not report it.** `SLSRegisterNotifyProc` 1325/1326 (window
  created/destroyed) fire for every app window, but never for the indicator
  windows. Window events alone cannot drive the detection.
- **Control Center logs every change, publicly.** Subsystem
  `com.apple.controlcenter`, category `sensor-indicators`, format string
  `Active activity attributions changed to %{public}s`:

  ```
  Active activity attributions changed to ["aud:com.rogueamoeba.arkaudiod", "cam:com.apple.PhotoBooth", "mic:com.goodsnooze.MacWhisper"]
  Active activity attributions changed to []
  ```

  Kinds seen: `mic`, `cam`, `scr` (screen capture), `aud` (system audio
  capture), `loc` (location). It also logs, on every SystemStatus update (several
  times a minute while anything is active), the full current list:

  ```
  Sorted active attributions from SystemStatus update: [[aud] Audio Routing Kit (ARK) (com.rogueamoeba.arkaudiod)]
  ```

  The line arrives before the window appears. `log stream` with a predicate on
  these lines works without special rights and uses no measurable CPU.
  MacWhisper ships exactly this technique (a long-running `log stream --predicate
  'process == "ControlCenter" AND eventMessage CONTAINS "Active activity
  attributions changed to " …'` child).
- **The proper API is closed.** `SystemStatus.framework`
  (`STDynamicActivityAttributionMonitor`, `STMediaStatusDomain`) rejects clients
  without an Apple entitlement ("attempting to access dynamic attribution without
  entitlement").
- **Public APIs fall short.** CoreAudio (`kAudioDevicePropertyDeviceIsRunningSomewhere`,
  process objects since macOS 14) covers the microphone per app, CoreMediaIO
  the camera per device only; there is nothing for screen or system-audio capture.
  The always-on purple dot on the author's machine is `aud` (Rogue Amoeba ARK),
  which no public API reports.

## What changes for users

Nothing, until a config asks for it. Then:

```sh
mbar --bar privacy_indicator_inset=on            # right-hand items avoid the dot
mbar --add item mic right \
     --subscribe mic privacy_indicator_change \
     --set mic script='[ -n "$MIC" ] && mbar --set $NAME drawing=on || mbar --set $NAME drawing=off'
```

```lua
mbar.bar({ privacy_indicator_inset = true })
local clock = mbar.add("item", "clock", "right", { provider = "clock" })
clock:subscribe("privacy_indicator_change", function(env)
  local i = env.info
  clock:set({ label = { color = #i.mic > 0 and 0xffff9f0a or 0xffffffff } })
end)
```

## Event `privacy_indicator_change`

Built in, like the `aerospace_*` events: `--subscribe` works without `--add event`.
A config that runs `--add event privacy_indicator_change` is accepted (it registers
the name like any custom event; a notification name given with it is ignored), and a
manual `--trigger privacy_indicator_change` still reaches subscribers but does not
change the stored state.

It fires whenever the stored state changes (see [State](#state)); identical samples
are dropped.

| Variable | Content |
|---|---|
| `VISIBLE` | `on` while the dot is on screen, else `off` |
| `MIC`, `CAMERA`, `SCREEN`, `AUDIO`, `LOCATION` | comma-separated bundle ids of the apps using that sensor, sorted; empty when none |

`INFO` (Lua: decoded in `env.info`):

```json
{
	"visible": "on",
	"frame": { "x": 2025, "y": 3, "w": 28, "h": 28 },
	"mic": ["com.goodsnooze.MacWhisper"],
	"camera": [],
	"screen": [],
	"audio": ["com.rogueamoeba.arkaudiod"],
	"location": [],
	"attribution": "on"
}
```

- `frame` is the bounding box of all indicator windows, in global points with a
  top-left origin (the space of `--query displays` and the bar frames). It is
  omitted while `visible` is `off`.
- `attribution` is `on` while the log stream runs. It is `off` while the stream
  is not running (not available, restarting), and also once the dot has been
  visible for 10 s without a single line having parsed since the stream started
  (the line format changed). The five lists are then empty; `visible` and
  `frame` are still correct.
- `LOCATION`/`location` is reported because Control Center reports it; whether it
  makes the dot visible is up to macOS (`visible` comes from the window, not from
  the lists).
- An item or handler that subscribes after the state is known gets the current
  state once, as an event of this name delivered to that subscriber only (as for
  AeroSpace). The state rarely changes (an always-on source keeps the dot for
  hours), so without this a subscriber added at startup would wait for the next
  change.

## Bar property `privacy_indicator_inset=on|off`

Default `off`. While `on`, a horizontal bar whose window frame intersects an
indicator window treats the indicator's left edge as its own right edge:

```
cur_r = min(w, indicator.x - bar.frame.x) - padding_right
```

instead of `w - padding_right` (`layout.rs`, `horizontal_pass`). The bar's existing
`padding_right` stays the gap, so no new number is needed. Only `right` items move;
`left`, `center`, `q` and `e` items and the bar background do not. Bars on other
displays, bottom bars and vertical bars are not affected (no intersection). The
change is applied on the next frame, without animation.

`--query bar` reports the property. Setting it to `on` starts the detection (see
[Lifecycle](#lifecycle)).

## `--query privacy_indicator`

The `INFO` object plus `"active": "on"|"off"` (whether the detection was started).
The query never starts it. An item named `privacy_indicator` wins over this query, as
with `--query borders`.

## Lua

No new namespace. `item:subscribe("privacy_indicator_change", fn)`,
`mbar.query("privacy_indicator")` and `mbar.bar({ privacy_indicator_inset = … })`
use the existing API. `lua/mbar.d.lua` gets the property, the event name and a
`PrivacyIndicatorInfo` class for `env.info`.

## Architecture

### mbar-core: `privacy` (new module)

- `PrivacyState { active, visible, frames: Vec<Rect>, attributions: Attributions,
  attribution: bool }` in the model. `Attributions` holds the five sorted,
  de-duplicated lists.
- `parse_log_message(&str) -> Option<Attributions>`, pure: accepts both message
  forms above. Change lines: a JSON array of `"<kind>:<bundle id>"` strings. Sorted
  lines: entries `[<kind>] <display name> (<bundle id>)`, the bundle id being the
  last parenthesised group of each entry (display names may contain parentheses,
  e.g. `Audio Routing Kit (ARK)`). Unknown kinds are ignored; a line that does not
  match returns `None`.
- `PrivacySample { visible, frames, attributions: Option<Attributions> }`: what the
  platform reports. `attributions: None` means "unknown" (stream down).
- Event variables, `INFO` and the query JSON are built here.

### mbar-core: runtime and layout

- `PlatformRequest::StartPrivacyIndicator`, emitted once per runtime lifetime: on
  the first `--subscribe` to `privacy_indicator_change` or the first
  `privacy_indicator_inset=on`.
- `Input::PrivacyIndicator(PrivacySample)`: merged into the stored state. If
  anything changed, `privacy_indicator_change` is triggered with the variables and
  `INFO` above; if `visible` or `frames` changed and the inset is on, the bars
  are laid out again.
- Late subscribers get the stored state as a synthetic event (same mechanism as
  `aerospace_initial`).
- `--reload` keeps the state and the running detection (like AeroSpace); the
  inset property comes back with the re-run config.
- `layout::horizontal_pass` applies the inset described above.

### mbar-macos: `sys::privacy` (new module)

One worker thread owns the detection and posts `Input::PrivacyIndicator` through
the `Waker`; nothing runs on the main thread.

- **Log reader.** A child `/usr/bin/log stream --style ndjson --predicate
  '<predicate>'` (own process group), with the predicate
  `subsystem == "com.apple.controlcenter" AND category == "sensor-indicators" AND
  (eventMessage BEGINSWITH "Active activity attributions changed to " OR
  eventMessage BEGINSWITH "Sorted active attributions from SystemStatus update: ")`.
  A reader thread parses each line's `eventMessage` with
  `privacy::parse_log_message` and forwards changes to the worker.
- **Initial state.** At start, one `log show --last 1h --style ndjson` with the
  same predicate; the newest parsed line is the starting list. If nothing is found
  the lists start empty and fill with the next line (the sorted line repeats
  several times a minute while a source is active).
- **Window check.** `CGWindowListCopyWindowInfo(optionOnScreenOnly)`, keeping
  windows with owner `Window Server`, layer `2147483630` and at most 64×64 pt.
  Owner, layer and bounds are readable without the Screen Recording permission;
  the name `StatusIndicator` is not used for matching. The filter is a pure
  function over a list of window records, so it is unit tested.
- **When the window is checked.** After every attribution change, at 0, 0.3, 1
  and 2 s (fade-in and fade-out); immediately on display reconfiguration and wake
  (the platform nudges the worker). While the log stream is not running, every
  2 s.
- **Format check.** The worker tracks whether any line parsed since the stream
  started. When the window has been visible for 10 s and none has, it reports
  `attributions: None` (with one warning) until a line parses.
- **Restart.** If `log stream` exits or fails to start, the worker reports
  `attributions: None`, falls back to the 2 s window check and restarts the child
  with a backoff (1 s, doubling, at most 30 s).
- **Shutdown.** The child's process group is terminated when mbar exits, like
  running scripts.

The headless platform logs `StartPrivacyIndicator` and ignores it; tests inject
`Input::PrivacyIndicator` directly.

## Risks

- **The log line is not an API.** Apple can change or drop it with any macOS
  update. The stream then keeps running but nothing parses: the 10-second rule
  in [the event](#event-privacy_indicator_change) turns `attribution` `off` and
  logs one warning. The inset and `visible` keep working from the window.
- **The window may change.** A different layer or owner name would make
  `visible` stay `off` (the bar then behaves as without the feature). The layer is
  a named constant next to the filter, with the measured values in a comment.
- **More than one display.** Only the built-in display was measured. The design
  does not assume a display: every indicator window counts for every bar it
  intersects.

## Testing

- Core unit tests: both message forms (fixtures copied from the real log lines
  above, including `[]` and a display name with parentheses), unknown kinds,
  garbage; state merge and de-duplication; event variables and `INFO`; frame
  omitted while hidden; query JSON with `active`.
- Layout tests: inset applied when the indicator intersects the bar; not applied
  for another display, a bottom bar, a vertical bar or with the property `off`;
  `padding_right` kept as the gap; non-`right` items unchanged.
- Integration test `crates/mbar-core/tests/wpb_privacy.rs` (like `wpb_aerospace`):
  lazy start from `--subscribe` and from the property, no start from `--query`,
  late subscribers get the state once, `--reload` keeps the state, manual
  `--trigger` does not change it.
- macOS: the window filter over recorded window lists (two identical windows,
  other Window Server windows, an oversized window). Manual check with Photo
  Booth (camera), Voice Memos (microphone) and a screen recording, with the menu
  bar hidden and shown.

## Docs

- `docs/EXTENSIONS.md`: new section "Privacy indicator" (event, variables,
  `INFO`, property, query, the log-line caveat); event and query added to the
  tables at the top.
- `docs/LUA.md` and `lua/mbar.d.lua`: property, event, `PrivacyIndicatorInfo`.
- `docs/ARCHITECTURE.md`: `privacy` in the core module table, `privacy` in the
  mbar-macos table, a short data-flow paragraph.

## Out of scope

- Animating the inset.
- Per-app attribution through CoreAudio process objects (microphone only) as a
  second source.
- Drawing mbar's own indicator or hiding the system one.
- A System-page entry in `mbar.app`.
