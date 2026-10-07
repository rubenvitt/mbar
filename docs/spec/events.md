# SketchyBar Event Model, Timers and Animation — Behavioral Spec

Source of truth: SketchyBar checkout at commit `5f358ec` ("use opentype directly (#828)"), version 2.24.0 (`src/sketchybar.c` MAJOR/MINOR/PATCH).
All `file:function` citations refer to `src/` of that checkout. "Quirk" marks behavior that is surprising but real; "Bug" marks behavior
that is almost certainly unintended — each such item states whether mbar should replicate it.

Contents

1. Event loop architecture (threads, run loop, dispatch)
2. Internal event types (`enum event_type`) and their OS sources
3. Subscribable event names, bit flags, `--add event`, `--query events`
4. Item update pipeline (`bar_item_update`): SENDER, env-var assembly, gating, script/mach delivery
5. Per-event specification (source, dedup, INFO payload, extra env vars, recipients)
6. Mouse events in detail (entered/exited/clicked/scrolled + global variants, slider drag)
7. `--trigger` semantics
8. Routine timer (`update_freq`), `--update` (forced), startup order
9. Sleep / wake / display reconfiguration / hotload
10. Animation system (complete)
11. Quirk & bug index
12. Open questions

---

## 1. Event loop architecture

### 1.1 Single-threaded core

All state mutation happens on the **main thread**. The process runs the Carbon event loop: `main()` ends with
`RunApplicationEventLoop()` (`sketchybar.c:main`), which spins the main `CFRunLoop`. Every event source either
already delivers on the main thread or is marshalled onto it.

`event_post(struct event*)` (`event.c:event_post`) is the single funnel:

```
event_post(e):
  if e.type == EVENT_TYPE_UNKNOWN: return
  if not on main thread (pthread_main_np()==0):
      dispatch_sync(main_queue, { event_execute(e) })   // caller thread BLOCKS until handled
  else:
      event_execute(e)                                  // synchronous, re-entrant
```

`event_execute(e)` (`event.c:event_execute`):

```
event_execute(e):
  if g_space_management_mode != 1:          // "Displays have separate Spaces" is OFF
      bar_manager_poll_active_display()     // may emit display_change (see §5.3)
  event_handler[e.type](e.context)
  windows_unfreeze()                         // commits the pending SkyLight transaction (window.c)
```

Consequences that mbar must preserve:

- Events are handled strictly one at a time, in arrival order, to completion. There is no event queue inside
  SketchyBar besides the main run loop / main dispatch queue.
- `event_post` called *from the main thread inside a handler* executes the nested event **immediately and
  re-entrantly** (e.g. `--reload` posts `HOTLOAD` from inside the mach-message handler, `message.c:handle_message_mach`;
  `bar_manager_handle_system_woke` is invoked synchronously, etc.).
- Off-main producers (CoreAudio listener thread, CVDisplayLink thread) block until the main thread has processed the
  event (`dispatch_sync`) — except the display link, which uses `dispatch_async` itself (see §10.6).
- The event `context` pointer frequently points to a **stack buffer of the producer** (e.g. power `"AC"`, wifi SSID,
  media JSON, volume float). This is only valid because handling is synchronous. In Rust, pass owned values.
- `g_space_management_mode = SLSGetSpaceManagementMode(SLSMainConnectionID())` is read once at startup
  (`sketchybar.c:init_misc_settings`). Value 1 means "Displays have separate Spaces" is enabled; in that case the
  active display is the one owning the active menu bar (`SLSCopyActiveMenuBarDisplayIdentifier`), otherwise it is the
  display containing the mouse cursor (`display.c:display_active_display_uuid`).

### 1.2 Thread / delivery map

| Producer | Registration API | Delivery thread | Marshalling |
|---|---|---|---|
| NSWorkspace notifications (space, app, sleep, wake, active display) | `[[NSWorkspace sharedWorkspace] notificationCenter] addObserver:` (`workspace.m:-init`) | main | direct `event_post` |
| NSDistributedNotificationCenter (menu bar hiding, screen unlocked, custom events) | `[NSDistributedNotificationCenter defaultCenter] addObserver:` | main | direct |
| Carbon mouse events | `InstallEventHandler(GetEventDispatcherTarget(), …)` (`mouse.c:mouse_begin`) | main | direct |
| CG display reconfiguration | `CGDisplayRegisterReconfigurationCallback` (`display.c:display_begin`) | main | direct |
| SkyLight notify procs (space 1327/1328, window 815/816/1325/1326, 1401, capture gating 904/905/1401/1508/1322) | `SLSRegisterNotifyProc` | main (connection port on main run loop) | direct |
| Mach IPC (client commands) | `CFMachPortCreateWithPort` + `CFRunLoopAddSource(CFRunLoopGetMain(), …, kCFRunLoopDefaultMode)` (`mach.c:mach_server_begin`) | main | `MACH_MESSAGE` event |
| Routine timer (1 s) | `CFRunLoopTimerCreate` + `CFRunLoopAddTimer(main, kCFRunLoopCommonModes)` (`bar_manager.c:bar_manager_init`) | main | `SHELL_REFRESH` event |
| Power source | `IOPSNotificationCreateRunLoopSource` → current (main) run loop, default mode (`power.c`) | main | direct |
| Wi-Fi | `SCDynamicStoreCreateRunLoopSource` → current (main) run loop, default mode (`wifi.m`) | main | direct |
| FSEvents (hotload) | `FSEventStreamScheduleWithRunLoop(current, default mode)` (`hotload.c`) | main | direct |
| MediaRemote | `MRMediaRemoteRegisterForNowPlayingNotifications(main_queue)` + `NSNotificationCenter.defaultCenter`; all MR getters reply on `dispatch_get_main_queue()` (`media.m`) | main | direct |
| DisplayServices brightness | `DisplayServicesRegisterForBrightnessChangeNotifications(did, did, cb)` (`display.c`) | (main, unverified) | `event_post` handles either |
| CoreAudio volume | `AudioObjectAddPropertyListener` (`volume.c`) | CoreAudio HAL thread | `dispatch_sync` main |
| CVDisplayLink frames | `CVDisplayLinkSetOutputCallback` (`animation.c`) | display-link thread | `dispatch_async` main, then `event_post` |
| Second wake event | `dispatch_async(global LOW)` + `usleep(500000)` + `dispatch_async(main)` (`bar_manager.c:bar_manager_handle_system_woke`) | main | `event_post` |

### 1.3 Freeze / refresh discipline

- `bar_manager.frozen` (`bar_manager_freeze/unfreeze`) suppresses `bar_manager_refresh` (returns immediately when
  frozen) and the non-forced `bar_manager_update`.
- Mach command batches run fully frozen: `handle_message_mach` sets `frozen=true` before parsing, then at the end
  `animator_lock` → `unfreeze` → `bar_manager_refresh(false)` → reply.
- `bar_manager_handle_space_change` freezes during its work and refreshes at the end.
- `bar_manager_animator_refresh` freezes while stepping animations.
- Quirk Q7: `frozen` is a plain boolean, not a counter. Any nested `freeze()/unfreeze()` pair (e.g.
  `bar_manager_handle_space_change`, `bar_manager_display_changed`, `bar_manager_animator_refresh` reached
  synchronously from inside a mach batch via `--trigger space_change`, `--update`, `--reload`) **clears the batch's
  freeze**, so later commands in that batch refresh immediately. mbar may implement a proper scoped freeze; the only
  observable difference is intermediate redraws.
- `windows_freeze()/windows_unfreeze()` (`window.c`) is the separate SkyLight transaction batching layer
  (`SLSDisableUpdate` + `SLSTransactionCreate` … `SLSTransactionCommit` + `SLSReenableUpdate`; on macOS 26 no
  disable/reenable). Every handled event ends with `windows_unfreeze()`.

---

## 2. Internal event types and OS sources

`enum event_type` (`event.h`) is the internal dispatch type. The subscribable names of §3 are produced by handlers of these.

| Internal type | Producer (file:function) | Exact OS source | Context payload | Handler → effect |
|---|---|---|---|---|
| `APPLICATION_FRONT_SWITCHED` | `workspace.m:-appSwitched:`; `workspace.m:forced_front_app_event` | `NSWorkspaceDidActivateApplicationNotification` (`userInfo[NSWorkspaceApplicationKey].localizedName`); forced: `[[NSWorkspace sharedWorkspace] frontmostApplication].localizedName` | heap `char*` app name or NULL | `bar_manager_handle_front_app_switch` → `front_app_switched` |
| `SPACE_CHANGED` | `workspace.m:-activeSpaceDidChange:`; `sketchybar.c:space_events` | `NSWorkspaceActiveSpaceDidChangeNotification`; SkyLight notify **1327** and **1328** (registered only on macOS ≥ 13) | none | `bar_manager_handle_space_change(forced=false)` → `space_change` |
| `DISPLAY_CHANGED` | `workspace.m:-activeDisplayDidChange:` | NSWorkspace notification named literally `@"NSWorkspaceActiveDisplayDidChangeNotification"` (private) | none | `bar_manager_handle_display_change` → `display_change` |
| `DISPLAY_ADDED` / `DISPLAY_REMOVED` / `DISPLAY_MOVED` / `DISPLAY_RESIZED` | `display.c:display_handler` | `CGDisplayRegisterReconfigurationCallback`; flags tested in order `kCGDisplayAddFlag`, `kCGDisplayRemoveFlag`, `kCGDisplayMovedFlag`, `kCGDisplayDesktopShapeChangedFlag` (first match wins, else nothing) | display id | all four → `bar_manager_display_changed` (full rebuild, §9.3) |
| `MENU_BAR_HIDDEN_CHANGED` | `workspace.m:-didChangeMenuBarHiding:` | distributed `"AppleInterfaceMenuBarHidingChangedNotification"` | none | `bar_manager_resize`; `bar_needs_update=true`; `bar_manager_refresh(false)`. **No script event.** |
| `SYSTEM_WILL_SLEEP` | `workspace.m:-willSleep:` | `NSWorkspaceWillSleepNotification` | none | `bar_manager_handle_system_will_sleep` → `system_will_sleep` |
| `SYSTEM_WOKE` | `workspace.m:-didWake:`; delayed re-post in `bar_manager_handle_system_woke` | `NSWorkspaceDidWakeNotification` **and** distributed `"com.apple.screenIsUnlocked"` (same selector) | none | `bar_manager_handle_system_woke` → `system_woke` (+ display rebuild) |
| `SHELL_REFRESH` | `bar_manager.c:clock_handler` | 1 s `CFRunLoopTimer` | none | `bar_manager_update(forced=false)` (routine updates, §8) |
| `ANIMATOR_REFRESH` | `animation.c:animation_frame_callback` | `CVDisplayLink` output callback | `output_time->hostTime` (u64) | `bar_manager_animator_refresh` (§10) |
| `MACH_MESSAGE` | `message.c:mach_message_handler` | Mach port `git.felix.<bar_name>` | mach buffer | `handle_message_mach` (command batch) |
| `MOUSE_UP` | `mouse.c:mouse_handler` | Carbon `kEventClassMouse/kEventMouseUp` → `CopyEventCGEvent` | `CGEventRef` | `event_mouse_up` → `mouse.clicked` |
| `MOUSE_DRAGGED` | same | `kEventMouseDragged` | `CGEventRef` | `event_mouse_dragged` (slider only) |
| `MOUSE_ENTERED` | same | `kEventMouseEntered` (SkyLight tracking rects, §6.1) | `CGEventRef` | `event_mouse_entered` |
| `MOUSE_EXITED` | same | `kEventMouseExited` | `CGEventRef` | `event_mouse_exited` |
| `MOUSE_SCROLLED` | same | **both** `kEventMouseWheelMoved` and `kEventMouseScroll` | `CGEventRef` | `event_mouse_scrolled` |
| `VOLUME_CHANGED` | `volume.c:handler` | CoreAudio property listeners (§5.6) | `float*` 0..1 | `bar_manager_handle_volume_change` |
| `WIFI_CHANGED` | `wifi.m:update_ssid` | `SCDynamicStore` pattern `".*/Network/Global/IPv4"` | `char*` SSID | `bar_manager_handle_wifi_change` |
| `BRIGHTNESS_CHANGED` | `display.c:brightness_handler` | private `DisplayServicesRegisterForBrightnessChangeNotifications` | `float*` 0..1 | `bar_manager_handle_brightness_change` |
| `POWER_SOURCE_CHANGED` | `power.c:power_handler` | `IOPSNotificationCreateRunLoopSource` + `IOPSGetProvidingPowerSourceType` | `char*` `"AC"`/`"BATTERY"` | `bar_manager_handle_power_source_change` |
| `MEDIA_CHANGED` | `media.m:-update` | private MediaRemote (§5.9) | `char*` JSON | `bar_manager_handle_media_change` |
| `COVER_CHANGED` | `media.m:-update` | same | `CGImageRef` artwork | `bar_manager_handle_media_cover_change` (no script event; updates `media.artwork` images) |
| `SPACE_WINDOWS_CHANGED` | `app_windows.c:app_windows_post_event_for_space` | SkyLight notify 1325/1326/815/816/1327/1328 (§5.10) | `char*` JSON | `bar_manager_handle_space_windows_change` |
| `DISTRIBUTED_NOTIFICATION` | `workspace.m:-allDistributedNotifications:` | `NSDistributedNotificationCenter` observer per custom notification name | `struct notification{name, info}` | `bar_manager_handle_notification` → custom event |
| `HOTLOAD` | `hotload.c:handler`; `message.c` (`--reload`) | FSEvents on config dir; `--reload` | none | destroy + re-init bar manager, re-exec config (§9.4) |
| `INIT_MUTEX`, `EVENT_TYPE_UNKNOWN` | — | — | — | no handler; `UNKNOWN` is dropped by `event_post` |

SkyLight notify procs registered at startup that do **not** produce script events (`sketchybar.c:system_events`):
these only gate window capture for alias items (`window.c:window_capture`):

| SLS event | Effect on `g_disable_capture` |
|---|---|
| 1322 | set to now (monotonic ns) → captures disabled until `now - t > 2^30 ns` (~1.074 s) |
| 905 | set to -1 → captures disabled indefinitely |
| 904, 1401, 1508 | set to 0 → captures enabled |

---

## 3. Subscribable event names, bits, custom events

### 3.1 Registry

`struct custom_events` (`custom_events.c`) is an ordered list of `{name, notification}`. An event's **bit** is
`1ULL << index`. `custom_events_init` (called from `bar_manager_init`, i.e. at startup *and on every hotload*) appends the
18 built-ins in this exact order:

| Bit index | Mask const (`custom_events.h`) | Name | Notes |
|---|---|---|---|
| 0 | `UPDATE_FRONT_APP_SWITCHED` | `front_app_switched` | |
| 1 | `UPDATE_SPACE_CHANGE` | `space_change` | auto-subscribed by `space` items |
| 2 | `UPDATE_DISPLAY_CHANGE` | `display_change` | |
| 3 | `UPDATE_SYSTEM_WOKE` | `system_woke` | |
| 4 | `UPDATE_MOUSE_ENTERED` | `mouse.entered` | also enables item tracking rect |
| 5 | `UPDATE_MOUSE_EXITED` | `mouse.exited` | also enables item tracking rect |
| 6 | `UPDATE_MOUSE_CLICKED` | `mouse.clicked` | |
| 7 | `UPDATE_MOUSE_SCROLLED` | `mouse.scrolled` | |
| 8 | `UPDATE_SYSTEM_WILL_SLEEP` | `system_will_sleep` | |
| 9 | `UPDATE_ENTERED_GLOBAL` | `mouse.entered.global` | |
| 10 | `UPDATE_EXITED_GLOBAL` | `mouse.exited.global` | |
| 11 | `UPDATE_SCROLLED_GLOBAL` | `mouse.scrolled.global` | |
| 12 | `UPDATE_VOLUME_CHANGE` | `volume_change` | subscribing starts CoreAudio listeners |
| 13 | `UPDATE_BRIGHTNESS_CHANGE` | `brightness_change` | subscribing starts DisplayServices listeners |
| 14 | `UPDATE_POWER_SOURCE_CHANGE` | `power_source_change` | |
| 15 | `UPDATE_WIFI_CHANGE` | `wifi_change` | |
| 16 | `UPDATE_MEDIA_CHANGE` | `media_change` | subscribing enables MediaRemote processing |
| 17 | `UPDATE_SPACE_WINDOWS_CHANGE` | `space_windows_change` | subscribing starts window tracking |
| 18.. | — | user events (`--add event`) | in insertion order |

`routine` and `forced` are **not** events; they are only values of `$SENDER` (§4).

Item subscription state is the 64-bit `bar_item.update_mask`. Bits ≥ 64 are undefined behavior in C (`1ULL << 64`);
mbar should reject or cap at 64 events total (46 user events). Lookups are linear `strcmp` (case-sensitive, exact).

### 3.2 `--add event <name> [<notification>]`

`message.c:handle_domain_add`:

```
event = next token
if remaining batch-line text is non-empty:   // strlen(message) > 0 after consuming <name>
    notification = next token
else notification = NULL
custom_events_append(name, notification)
```

`custom_events_append`:
- If `name` already exists (any event, built-in or user): **silently ignored** — the notification of the existing entry
  is *not* updated, no response text.
- Otherwise appended (next bit). If `notification != NULL`, an `NSDistributedNotificationCenter` observer is added for
  that notification name, `object:nil` (`workspace.m:-addCustomObserver:`).
- No response is written on success.

There is no `--remove event`. Events survive until hotload/reload (which rebuilds the list from built-ins; the
distributed observers registered on the long-lived workspace context are **not** removed — see Bug B6).

### 3.3 `--subscribe <item> <event> [<event> …]`

`message.c:handle_domain_subscribe` + `bar_item.c:bar_item_parse_subscribe_message`:

- `<item>` is an exact name (no regex). Unknown → response `[!] Subscribe: Item not found '<name>'\n`.
- For each token: `flag = bit(name)`; unknown → response `[?] Event: '<name>' not found\n` (flag 0, nothing set).
- Side effects when the flag includes: `volume_change` → `begin_receiving_volume_events()`;
  `brightness_change` → `begin_receiving_brightness_events()`; `media_change` → `begin_receiving_media_events()`;
  `space_windows_change` → `begin_receiving_space_window_events()`. Each is idempotent (global flag) and is never undone
  (not even on hotload — globals `g_volume_events`, `g_brightness_events`, `g_media_events`, `g_space_window_events`
  persist).
- `update_mask |= flag`. There is no unsubscribe.
- `update_mask` is copied by `--clone` and by default-item inheritance (`bar_item_inherit_from_item` memcpy),
  but `--subscribe` cannot target `defaults`.
- Additionally `image=media.artwork` on any image (`image.c`) calls `begin_receiving_media_events()`.

### 3.4 `--query events`

`custom_events.c:custom_events_serialize`, exact byte format (`\t` = TAB):

```
{\n
\t"<name0>": {\n
\t\t"bit": <decimal 1<<0>,\n
\t\t"notification": "<notification or (null)>"\n
\t},\n
\t"<name1>": {\n
...
\t\t"notification": "(null)"\n
\t}\n
}\n
```

- `bit` is printed `%llu` (decimal value of the mask, e.g. `1`, `2`, `4`, …, `131072` for `space_windows_change`).
- A NULL notification is printed through `%s` → macOS libc prints `(null)`; so built-ins show `"notification": "(null)"`.
  Strings are not escaped.
- Edge: with zero events the output would be `{\n\t}\n}\n` (unreachable; built-ins always exist).

### 3.5 Distributed-notification custom events at runtime

`workspace.m:-allDistributedNotifications:` → `DISTRIBUTED_NOTIFICATION` with
`name = note.name (UTF-8)`, and `info` = `NSJSONSerialization dataWithJSONObject:note.userInfo options:NSJSONWritingPrettyPrinted`
**only if** `userInfo != nil` and `[NSJSONSerialization isValidJSONObject:userInfo]` and the data is non-empty.

`bar_manager.c:bar_manager_handle_notification`:
- `name = first custom event (in list order) whose notification string equals note.name` (built-ins have none).
  None → drop.
- env: `INFO = info` only if info present (pretty-printed JSON: Apple format, 2-space indentation, `"key" : value`,
  dictionary key order unspecified, no trailing newline).
- `bar_manager_custom_events_trigger(name, env)`.
- `note.object` is ignored.

---

## 4. Item update pipeline

### 4.1 Dispatch to subscribers

`bar_manager.c:bar_manager_custom_events_trigger(name, env)`:

```
flag = custom_events_get_flag_for_name(name)       // 0 if unknown → nobody matches
for item in bar_items (array order = bar ordering):
    if item.update_mask & flag:
        bar_item_update(item, sender=name, forced=false, env)
```

The same `env` object is passed to each item in turn and **mutated** by each (`env_vars_set` of the item's own vars,
NAME, SENDER). Item N+1 therefore sees leftovers of item N's persistent vars (e.g. `SELECTED`, `SID`, `DID`,
`PERCENTAGE` keys from a previous space/slider item) unless it defines the same keys itself (Quirk Q1). Exact
replication = one mutable env map threaded through all recipients in order; a per-recipient clone of the event env is
a (documented) deviation that only removes the leftovers.

### 4.2 `bar_item_update(item, sender, forced, env)` — exact algorithm (`bar_item.c`)

```
is_shown = (item.associated_bar != 0)                     // drawn on at least one bar right now
if is_shown and item.scroll_texts and item.counter % 15 == 0:
    text_animate_scroll(icon); text_animate_scroll(label)
    if item.type == slider: text_animate_scroll(slider.knob)
item.counter += 1

if (!item.updates or (item.update_frequency == 0 and sender == NULL)) and !forced:
    return false

scheduled = item.update_frequency <= item.counter
should    = item.updates_only_when_shown ? is_shown : true

if ((scheduled or sender != NULL) and should) or forced:
    item.counter = 0
    if (script non-empty) or item.event_port:
        if env == NULL:
            env = &item.signal_args.env_vars          // the item's PERSISTENT env (mutated!)
        else:
            for (k, v) in item.signal_args.env_vars (in order): env.set(k, v)
            env.set("NAME", item.name)
        env.set("SENDER", sender ?? (forced ? "forced" : "routine"))
    if script non-empty: fork_exec(script, env)
    if event_port:       mach_send(event_port, serialize(env), no reply)
return false                                          // always false
```

Notes:

- `counter` is incremented on **every** call, including event deliveries, and reset whenever a script actually fires
  (including event-triggered runs). So an event-triggered run restarts the `update_freq` countdown (Quirk Q2).
- When `env == NULL` (routine, forced, and the internally generated `mouse.entered`, `mouse.exited`,
  `mouse.entered.global`, `mouse.exited.global`, `system_woke`, `system_will_sleep`; `--trigger` always passes a
  non-NULL env) the
  `SENDER` key is written **into the item's persistent env**, so it stays there and is later copied into other events'
  env and into the `click_script` env (Quirk Q3: a click_script sees a stale `$SENDER`).
- `sender != NULL` with `forced=false` (normal event delivery) respects `updates` (off → no run) and
  `updates_only_when_shown` (hidden → no run, counter still incremented, not reset).
- Mouse deliveries call with `forced=true` (bypass `updates`/`when_shown`) but a non-NULL sender, so SENDER is the event
  name.
- Nothing happens (other than counter/scroll) if the item has neither a script nor a mach helper port.

### 4.3 Item persistent env (`signal_args.env_vars`)

Keys that SketchyBar itself writes into an item's persistent env:

| Key | Written by | Value |
|---|---|---|
| `NAME` | `bar_item_set_name` (add, rename, clone) | item name |
| `SENDER` | `bar_item_update` when called with `env == NULL` | last such sender |
| `SELECTED` | space items: `bar_item_set_type`, `bar_manager_update_space_components` | `"true"`/`"false"` (initial `"false"`) |
| `SID` | space items: `bar_item_set_type` (`"0"`), `bar_item_append_associated_space` | bit position of `associated_space` (decimal) |
| `DID` | space items: `bar_item_set_type` (`"0"`), `bar_item_append_associated_display` | bit position of `associated_display` |
| `PERCENTAGE` | slider items: `bar_item_cancel_drag` (after a click on the slider) | `"%d"` percentage |

Inheritance from `defaults` / `--clone` does **not** copy the env (pointers cleared), except for space items where
`SELECTED="false"`, `SID = ancestor's DID` (Bug B1: SID copied from DID) and `DID = ancestor's DID`
(`bar_item.c:bar_item_inherit_from_item`).

### 4.4 Env-var key order (only relevant for mach helpers)

`env_vars_set(k, v)` = unset k if present, then append at the end. For an event env the resulting order is:
event-specific keys (in the order given in §5), then the item's persistent keys (in their order), then `NAME`, then
`SENDER`.

### 4.5 Script execution (`misc/helpers.h:fork_exec` / `sync_exec`)

```
pid = vfork()
child:  alarm(60)                                   // FORK_TIMEOUT; survives exec → SIGALRM kills script after 60 s
        for (k,v) in env: setenv(k, v, overwrite=1)
        execvp("/usr/bin/env", ["/usr/bin/env", "sh", "-c", <script>, NULL])
        exit(<execvp result>)                       // only on exec failure
parent: return immediately (no wait; SIGCHLD is SIG_IGN → auto-reaped)
```

- Script string: `script` / `click_script` property, with a leading `~` replaced by `$HOME` (`resolve_path`).
  It is a shell command line, not a path.
- Inherited process environment: the daemon's environment, which includes `BAR_NAME=<basename(argv[0])>` (set at
  startup) and `CONFIG_DIR=<dirname(config file)>` (set in `exec_config_file`). Working directory: the config
  directory (the daemon `chdir`s there in `exec_config_file`).
- Quirk Q4 (vfork env leak): `setenv` runs in the vfork child, which shares the parent's address space on macOS,
  so variables set for one script are likely to **persist in the daemon's environ** and be inherited by all later
  children (e.g. `$INFO` of a previous event visible in a later event that does not set INFO). See Open Question 1.
- No concurrency limit; scripts run fully asynchronously; stdout/stderr are inherited from the daemon.

### 4.6 Mach helper delivery (`mach_helper=<bootstrap name>`)

- `bar_item_set_event_port`: `bootstrap_look_up(bootstrap_port, name)`; failure → port 0 (no delivery).
- Payload (`env_vars_copy_serialized_representation`): for each var in order `key\0value\0`, then one extra `\0`.
  Sent as a single out-of-line descriptor (`MACH_MSG_VIRTUAL_COPY`), header complex bit, no reply port, no reply awaited.
- On `bar_manager_destroy` (`--exit`, hotload/reload) every item with a port receives the 2-byte message `"k\0"`.

---

## 5. Per-event specification

Summary table (details below). "Env" lists event-specific variables in insertion order; every recipient additionally
gets its persistent vars, `NAME`, `SENDER=<event name>` (§4.2). "Lazy" = OS listener only installed on first
subscription.

| Event | OS source(s) | Dedup | INFO | Other env | Lazy |
|---|---|---|---|---|---|
| `front_app_switched` | NSWorkspaceDidActivateApplicationNotification | none | app localizedName (absent if nil) | — | no |
| `space_change` | NSWorkspaceActiveSpaceDidChangeNotification; SLS 1327, 1328 (macOS ≥ 13) | none | JSON `{"display-<adid>": <mc index>}` | — | no |
| `display_change` | private NSWorkspaceActiveDisplayDidChangeNotification; per-event active-display poll; CG reconfiguration; wake | poll path only | active arrangement id `"%d"` | — | no |
| `system_will_sleep` | NSWorkspaceWillSleepNotification | none | — (env NULL) | — | no |
| `system_woke` | NSWorkspaceDidWakeNotification; distributed `com.apple.screenIsUnlocked`; +500 ms self re-post | none | — (env NULL) | — | no |
| `volume_change` | CoreAudio default-output-device volume/mute listeners | Δ > 0.01 | `"%d"` round(v·100) | — | yes |
| `brightness_change` | DisplayServices brightness notifications | Δ > 0.01 (global) | `"%d"` round(b·100) | — | yes |
| `power_source_change` | IOPS power source notification | state change | `AC` / `BATTERY` | — | no |
| `wifi_change` | SCDynamicStore `.*/Network/Global/IPv4` | none | SSID (raw bytes) or empty | — | no |
| `media_change` | MediaRemote now-playing notifications | full JSON string compare | JSON (§5.10) | — | gate only |
| `space_windows_change` | SLS window/space notify procs | none | JSON (§5.11) | — | yes |
| `mouse.clicked` | Carbon mouse up | — | JSON | `BUTTON`, `MODIFIER` | n/a |
| `mouse.scrolled` | Carbon wheel/scroll | 150 ms throttle | JSON | `SCROLL_DELTA`, `MODIFIER` | n/a |
| `mouse.scrolled.global` | same | same | JSON | `SCROLL_DELTA`, `DID`, `MODIFIER` | n/a |
| `mouse.entered` / `mouse.exited` | Carbon enter/exit of tracking rects | per-item `mouse_over` (entered only) | — (env NULL) | — | n/a |
| `mouse.entered.global` / `mouse.exited.global` | Carbon enter/exit of bar/popup windows | bar/popup `mouse_over` | — (env NULL) | — | n/a |
| custom (`--add event n notif`) | NSDistributedNotificationCenter | none | pretty JSON userInfo (if valid) | — | — |
| custom via `--trigger` | client | none | only if passed | all `KEY=VALUE` passed | — |

### 5.1 `front_app_switched`

- `workspace.m:-appSwitched:`: `name = string_copy(userInfo[NSWorkspaceApplicationKey].localizedName.UTF8String)`;
  NULL if missing. Every activation notification produces an event (re-activating the same app also fires).
- `bar_manager_handle_front_app_switch`: `INFO = name` (only if non-NULL; ownership moved into env).
- Forced variant `forced_front_app_event` (on `--update`, §8.2): uses `frontmostApplication.localizedName` via
  `cStringUsingEncoding:NSUTF8StringEncoding`; posts only if non-nil.

### 5.2 `space_change`

Sources (each produces one event → one full handling; a single Space switch typically yields **several**
`space_change` deliveries): `NSWorkspaceActiveSpaceDidChangeNotification`, SkyLight notify 1327 and 1328 (macOS ≥ 13,
`sketchybar.c:space_events`). Also `--trigger space_change` and `--update` (both `forced=true`), and every display
rebuild (`forced=true`).

`bar_manager.c:bar_manager_handle_space_change(forced)`:

```
freeze()
force_refresh = false
info = "{\n"
for i, bar in bars (bar order = display arrangement order filtered by --bar display):
    dsid = SLSManagedDisplayGetCurrentSpace(cid, uuid(bar.did))          // 0 if uuid unavailable
    bar.sid = mission_control_index(dsid)
    was_shown = bar.shown
    bar.shown = SLSSpaceGetType(cid, dsid) != 4 || show_in_fullscreen   // 4 = fullscreen space
    needs_ordering |= !was_shown && bar.shown
    force_refresh  |= was_shown != bar.shown
    if bar.dsid != dsid:
        bar.dsid = dsid
        if !sticky && bar.shown: bar_change_space(bar, dsid)            // move bar windows to the space
    info += "\t\"display-" + bar.adid + "\": " + bar.sid + ("," unless last) + "\n"
info += "}"
bar_manager_update_space_components(forced)                              // §5.2.1
custom_events_trigger("space_change", {INFO: info})
unfreeze(); bar_manager_refresh(force_refresh)
```

INFO example (two bars), exact bytes, **no trailing newline**:

```
{
	"display-1": 2,
	"display-2": 5
}
```

- Only displays that have a bar appear (respects `--bar display=`). With zero bars: `"{\n}"`.
- `mission_control_index(dsid)` (`misc/helpers.h`): iterate `SLSCopyManagedDisplaySpaces` displays in order and
  their `"Spaces"` arrays in order, counting from 1; return the 1-based position of the space whose `id64 == dsid`,
  or 0 if not found. Fullscreen spaces are counted.
- Bug B2: the buffer is `19*bar_count + 4` bytes; `snprintf` truncates if a line exceeds 18 chars (adid ≥ 10 or
  sid ≥ 100 etc.). mbar: generate the full string (do not replicate truncation).

#### 5.2.1 Space components (`bar_manager_update_space_components(forced)`)

For every item of type `space`:

```
if !item.overrides_association:                    // no explicit display=… was set on the space item
    space = bit_position(item.associated_space)     // UINT32_MAX if none
    did = display_id_for_space(space)               // mission-control index → dsid → SLSCopyManagedDisplayForSpace → CGDirectDisplayID
    item.associated_display = did ? 1 << display_arrangement(did) : 1 << 30
for bar in bars:
    if (1 << bar.adid) & item.associated_display:
        sid = bar.sid; if sid == 0: continue
        if (!item.selected || forced) && (item.associated_space & (1 << sid)):
            item.selected = true;  item.updates = true;  env SELECTED="true"
        elif (item.selected || forced) && !(item.associated_space & (1 << sid)):
            item.selected = false; item.updates = true;  env SELECTED="false"
        else:
            item.updates = false
```

Then the generic dispatch runs: space items are auto-subscribed to `space_change` (`bar_item_set_type` sets
`update_mask |= UPDATE_SPACE_CHANGE`, `updates=false`, default script
`sketchybar -m --set $NAME icon.highlight=$SELECTED`). Because `bar_item_update` is called with `forced=false`,
`updates==false` suppresses the run. Net effect: **a space item's script runs on `space_change` only when its
selection state flipped, or on forced handling** (then for all space items). `SENDER=space_change`, `INFO`=JSON,
`SELECTED`, `SID`, `DID`, `NAME` available. If an item is associated with several displays, the last matching bar's
evaluation decides `updates`. The user cannot make a space item run on every space change (`updates=on` is
overwritten on the next space change).

### 5.3 `display_change`

`bar_manager.c:bar_manager_handle_display_change`:

```
active_adid = display_active_display_adid()
INFO = snprintf(buf[3], "%d", active_adid)        // Bug B3: ≥ 100 truncated to 2 chars (irrelevant in practice)
custom_events_trigger("display_change", {INFO})
bar_manager_refresh(false)
```

`display.c:display_active_display_adid()`: 1 if exactly one active display; otherwise the 1-based index of the
active display's UUID in `SLSCopyManagedDisplays` (0 if not found). "Active display" = menu-bar display when
`g_space_management_mode == 1`, else the display whose `CGDisplayBounds` contains the cursor (`CGEventCreate(NULL)`
location; first match in `CGGetActiveDisplayList` order).

Producers:

| Path | Dedup |
|---|---|
| `NSWorkspaceActiveDisplayDidChangeNotification` (private name string) | none (fires even if adid unchanged) |
| `bar_manager_poll_active_display()` at the start of **every** handled event when `g_space_management_mode != 1` | only if adid ≠ stored `active_adid` |
| `bar_manager_display_changed` (CG reconfiguration, wake) | none |
| `--trigger display_change` | none |

Quirk Q5: with separate Spaces disabled, moving the cursor to another display produces `display_change` lazily at the
next event of any kind (timer tick ≤ 1 s, mouse event, animation frame, …), not immediately.

### 5.4 `system_will_sleep` / 5.5 `system_woke`

See §9.1 / §9.2. Both deliver with `env == NULL` (no INFO).

### 5.6 `volume_change` (`volume.c`)

Listeners (installed by `begin_receiving_volume_events`, first subscription only):
- On the default output device (`kAudioHardwarePropertyDefaultOutputDevice` of `kAudioObjectSystemObject`):
  `kAudioDevicePropertyMute` and `kAudioDevicePropertyVolumeScalar`, scope Output, elements **Main (0)** and **1**
  (left channel) — 4 listeners, all → `handler`.
- On `kAudioObjectSystemObject`: `kAudioHardwarePropertyDefaultOutputDevice` → `device_changed`: remove the 4
  listeners from the old device, add them to the new one, reset `g_last_volume = -1`, call `handler`.

`handler(device)`:

```
muted_main  = Mute[element 0]    (0 on error)
muted_left  = Mute[element 1]    (0 on error)
volume_main = VolumeScalar[element 0]   (0.0 on error)
volume_left = VolumeScalar[element 1]   (0.0 on error)
if volume_left > 0: v = (muted_left || muted_main) ? 0 : volume_left
else:               v = muted_main ? 0 : volume_main
if v > last + 0.01 || v < last - 0.01:      // strict; initial last = -1
    last = v; post VOLUME_CHANGED(v)
```

INFO = `"%d"` of `(int)(v*100.0 + 0.5)` (0–100). Forced (`forced_volume_event`, on `--update` and
`--trigger volume_change`): `last = -1` then `handler(g_audio_id)`; if listeners were never installed
`g_audio_id == 0`, all reads fail → posts `INFO=0` (only subscribers receive it, and there are none in that case).

### 5.7 `brightness_change` (`display.c`)

- `begin_receiving_brightness_events` (first subscription): for each active display with
  `DisplayServicesCanChangeBrightness(did)`: `DisplayServicesRegisterForBrightnessChangeNotifications(did, did, cb)`.
  Displays added later are registered from the reconfiguration callback (`kCGDisplayAddFlag`); removed ones
  unregistered.
- `brightness_handler(_, did, …)`: `b = DisplayServicesGetBrightness(did)` (0 on failure);
  post if `|b - last| > 0.01` (strict, single global `last` shared by all displays, initial -1).
- INFO = `"%d"` of `(int)(b*100.0 + 0.5)`.
- Forced (`forced_brightness_event`, `--update` only — **not** reachable via `--trigger brightness_change`, which is a
  plain custom trigger): `last=-1`, read the *active* display (`display_active_display_id()`), always posts.

### 5.8 `power_source_change` (`power.c`)

- Installed unconditionally at startup (`begin_receiving_power_events`).
- `power_handler`: `type = IOPSGetProvidingPowerSourceType(IOPSCopyPowerSourcesInfo())`;
  `"AC Power"` (`kIOPMACPowerKey`) → state AC; `"Battery Power"` (`kIOPMBatteryPowerKey`) → state BATTERY;
  anything else (e.g. `"UPS Power"`) → no event and state unchanged. Post only if state differs from `g_power_source`
  (initial 0 = unknown).
- INFO = `AC` or `BATTERY` (exact uppercase strings).
- Forced (`--update`, `--trigger power_source_change`): reset state to 0, re-run handler → always posts (if AC/BATTERY).
- No event at startup until the first IOPS notification or a forced update.

### 5.9 `wifi_change` (`wifi.m`)

- Installed unconditionally at startup: `SCDynamicStoreCreate(NULL, "network", update_ssid, …)`,
  `SCDynamicStoreSetNotificationKeys(store, keys=NULL, patterns=[".*/Network/Global/IPv4"])` (regex pattern; matches
  `State:/Network/Global/IPv4` and `Setup:/Network/Global/IPv4`).
- Fires on any change of the global IPv4 state (primary service/router changes), **not** on SSID changes per se;
  no dedup — the same SSID may be reported repeatedly.
- INFO = `[[[CWWiFiClient sharedWiFiClient] interface] ssidData]` bytes, NUL-terminated (embedded NUL truncates; not
  validated as UTF-8). nil data (no Wi-Fi, disconnected, or no Location Services authorization on recent macOS) →
  INFO = `""` (empty string, still set).
- Forced: `--update`, `--trigger wifi_change`.

### 5.10 `media_change` (`media.m`)

Private MediaRemote API (SketchyBar's own comment: the framework is locked for third-party use since macOS 15.3/15.4,
so on current macOS this event is effectively dead).

- Startup (`initialize_media_events`, unconditional): `MRMediaRemoteRegisterForNowPlayingNotifications(main_queue)`;
  observe (local `NSNotificationCenter.defaultCenter`):
  `kMRMediaRemoteNowPlayingInfoDidChangeNotification` → `media_change:`,
  `kMRMediaRemoteNowPlayingApplicationDidChangeNotification` → `media_change:`,
  `kMRMediaRemoteNowPlayingApplicationIsPlayingDidChangeNotification` → `playing_change:`.
- Both selectors return immediately unless `g_media_events` (set by first `media_change` subscription or any
  `image=media.artwork`).
- `playing_change:` → `MRMediaRemoteGetNowPlayingApplicationIsPlaying(main)` → `self.playing = playing` →
  `media_change:`.
- `media_change:` → `MRMediaRemoteGetNowPlayingApplicationDisplayName(0, main)` (nil → stop) →
  `MRMediaRemoteGetNowPlayingInfo(main)` → require dict non-nil and **artist, title, album all non-nil**
  (`kMRMediaRemoteNowPlayingInfoArtist/Title/Album`; empty strings allowed) → store; artwork:
  if `kMRMediaRemoteNowPlayingInfoArtworkMIMEType` and `…ArtworkData` present, decode via
  `CGImageSourceCreateWithDataProvider` index 0, else NULL → `-update`.
- `-update` (only if app, artist, title, album all set):

```
info = "{\n\t\"state\": \"<playing|paused>\",\n\t\"title\": \"<t>\",\n\t\"album\": \"<al>\",\n\t\"artist\": \"<ar>\",\n\t\"app\": \"<app>\"\n}"
```

  (key order state, title, album, artist, app; **no trailing newline**; title/album/artist escaped with
  `escape_string`: only `"`→`\"` and LF→`\n`; backslashes and other control chars are NOT escaped; `app` is not
  escaped at all.) `state` reflects the last `IsPlaying` answer (initially "paused" until a `playing_change`).
  If artwork != NULL → post `COVER_CHANGED` (every time, before the dedup check). If `info != last_info` → store and
  post `MEDIA_CHANGED`; INFO = info.
- `COVER_CHANGED` → `bar_manager_handle_media_cover_change`: set the shared `current_artwork` image; every item whose
  `background.image`, `icon.background.image` or `label.background.image` links to it gets `needs_update`; refresh if
  any such item is shown. No script event.
- Forced (`--update`, `--trigger media_change`): clear `last_info`, call `playing_change:` (still gated by
  `g_media_events`).

### 5.11 `space_windows_change` (`app_windows.c`)

`begin_receiving_space_window_events` (first subscription): register SLS notify procs
`1325` (window created/ordered in → `window_spawn_handler`), `1326` (window destroyed), `815`/`816`
(window un-hidden / hidden → `window_hide_handler`), `1401` (→ `space_handler`, silent), and on macOS ≥ 13 `1327`,
`1328` (→ `space_handler`, with events); then silently scan all spaces.

Window bookkeeping: `g_windows` (list of `{wid, sid, pid}`) and `g_hidden_windows`.

- `app_windows_update_space(sid, silent)`: clear entries of `sid`; enumerate
  `SLSCopyWindowsWithOptionsAndTags(cid, owner 0, [sid], options 0x2, set_tags=1, clear_tags=0)` →
  `SLSWindowQueryWindows` iterator; keep windows where `iterator_window_suitable`:
  `parent_wid == 0 && ((attributes & 0x2) || (tags & 0x400000000000000)) && ((tags & 0x1) || ((tags & 0x2) && (tags & 0x80000000)))`;
  pid via `SLSGetWindowOwner` + `SLSConnectionGetPID`. If not silent → post event for that space. Then
  `SLSRequestNotificationsForWindows` for all known (visible + hidden) wids.
- 1325 with `{sid, wid}` payload: if the window is suitable → update that space (event).
- 1326: if `{wid, sid}` is known → update that space (event) and clear it from hidden list.
- 816 (hide): if wid known → add to hidden list (if absent) and update its space (event).
- 815 (unhide): if wid is in hidden list → update its space (event), remove from hidden, re-register notifications.
- 1327/1328: update **every** space of every active display (`display_space_list`), each posting its own event →
  N events per Space switch (N = number of spaces). 1401: same but silent.
- Forced (`--update`, `--trigger space_windows_change`): all spaces with events, only if already started.

Payload (`app_windows_post_event_for_space(sid)`), exact bytes:

```
{\n
\t"space": <mission_control_index(sid)>,\n
\t"apps": {\n
\t\t"<App A>": <window count>,\n
\t\t"<App B>": <window count>\n
\t}\n
}\n
```

- Apps are grouped by pid in first-seen order of `g_windows` (array order), name = `NSRunningApplication
  localizedName` (pids without a name are skipped); later pids with an identical name are merged into the first
  (counts summed). Names are **not** JSON-escaped.
- No apps: `"{\n\t\"space\": 3,\n\t\"apps\": {\n\n\t}\n}\n"` (note the empty line).
- Trailing newline present.

---

## 6. Mouse events

### 6.1 Sources and window identification

- Carbon handler on `GetEventDispatcherTarget()` for `kEventClassMouse` kinds `MouseUp`, `MouseDragged`,
  `MouseEntered`, `MouseExited`, `MouseWheelMoved`, `MouseScroll` (`mouse.c`). Each Carbon event is converted with
  `CopyEventCGEvent` and posted; then `CallNextEventHandler`. Clicks fire on **button release** (MouseUp), any button.
- The window under the event: `wid = CGEventGetIntegerValueField(ev, 0x33)` (undocumented field 51)
  (`helpers.h:get_wid_from_cg_event`). Location: `CGEventGetLocation` (global, top-left origin).
- Enter/exit come from SkyLight tracking rects (`window.c:window_assign_mouse_tracking_area` =
  `SLSRemoveAllTrackingAreas` + `SLSAddTrackingRect(cid, wid, frame)`):
  - bar windows: always (on create and on every resize);
  - popup windows: always (on every popup draw);
  - item windows: only if the item's `update_mask` has `mouse.entered` or `mouse.exited`, (re)assigned when the item
    window is redrawn (`bar.c:bar_draw`).
- Lookup helpers (`bar_manager.c`): `get_item_by_wid` / `get_item_by_point` consider only items with
  `drawing == true` and iterate all their per-display windows; point containment is inclusive on all edges
  (`cgrect_contains_point`). `get_popup_by_wid/point` consider only items with `drawing && popup.drawing`
  (`CGRectContainsPoint`: half-open). `get_bar_by_point` uses `CGRectContainsPoint`.

### 6.2 `mouse.clicked` (`event.c:event_mouse_up` → `bar_item.c:bar_item_on_click`)

```
item = get_item_by_wid(wid, &window)
if !item or item.type == bracket: item = get_item_by_point(point, &window)
bar = get_bar_by_wid(wid); popup = get_popup_by_wid(wid)
if !item and !popup and !bar: return
p = item && window ? point - window.origin : (0,0)          // item-window-local
bar_item_on_click(item, CGEventGetType, kCGMouseEventButtonNumber, CGEventGetFlags (low 32 bits), p)
if item && item.needs_update: bar_manager_refresh(false)
```

`bar_item_on_click(item, type, button, flags, p)` (returns immediately if item NULL — clicks on empty bar/popup
area do nothing):

```
env = {}
env.INFO     = "{\n\t\"button\": \"<b>\",\n\t\"button_code\": <button>,\n\t\"modifier\": \"<m>\",\n\t\"modfier_code\": <flags>\n}\n"
env.BUTTON   = <b>
env.MODIFIER = <m>
if item.has_slider:
    if slider.is_dragged or CGRectContainsPoint(slider.background.bounds, p):
        if slider_handle_drag(p): item.needs_update = true
        bar_item_cancel_drag(item)        // is_dragged=false; persistent env PERCENTAGE="%d"
    else: return                          // click on slider item outside the track: NO click_script, NO event
if click_script non-empty:
    copy item persistent env into env;  fork_exec(click_script, env)     // no NAME/SENDER added explicitly
if item.update_mask & mouse.clicked:
    bar_item_update(item, "mouse.clicked", forced=true, env)            // adds persistent vars, NAME, SENDER
```

- `<b>` (`helpers.h:get_type_description`): `kCGEventLeftMouseUp` → `left`, `kCGEventRightMouseUp` → `right`,
  else `other`. `button_code` = `kCGMouseEventButtonNumber` (0 left, 1 right, 2 middle, …).
- `<m>` (`get_modifier_description`): concatenation in this order of `shift`, `ctrl`, `alt`, `cmd`, `fn` for flags
  `kCGEventFlagMaskShift`, `…Control`, `…Alternate`, `…Command`, `…SecondaryFn`, joined by `,`; `none` if empty.
- `modfier_code` (sic, typo is part of the format) = raw `CGEventFlags` truncated to u32 printed `%u`
  (e.g. `256` = `kCGEventFlagMaskNonCoalesced` with no modifiers).
- INFO ends with `}\n` (trailing newline).
- The click_script runs **before** the `mouse.clicked` script; both are independent processes.

### 6.3 `mouse.scrolled` / `mouse.scrolled.global` (`event.c:event_mouse_scrolled`)

Throttle state (global): `ts` (u64 ns, initial 0), `acc` (int, initial 0). `TIMEOUT = 150 ms`.

```
delta = CGEventGetIntegerValueField(ev, kCGScrollWheelEventDeltaAxis1)    // vertical, integer lines
now = clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW_APPROX)
if ts + 150ms > now: acc += delta; return                                 // swallowed
if ts + 300ms < now: acc = 0                                              // stale accumulation dropped
ts = now
total = delta + acc
item = get_item_by_wid(wid); if !item or bracket: item = get_item_by_point(point)
if !item:
    if bar = get_bar_by_wid(wid):
        if bar.mouse_over and !mouse_over_any_popup(): scrolled_global(total, bar.adid, flags)
        acc = 0; return
    if popup = get_popup_by_wid(wid):
        if popup.mouse_over and !mouse_over_any_bar(): scrolled_global(total, popup.adid, flags)
        acc = 0; return
bar_item_on_scroll(item, total, flags)        // no-op if item NULL
if item && item.needs_update: refresh(false)
acc = 0
```

i.e. a leading-edge throttle; deltas arriving within 150 ms after an emitted event are summed and added to the next
emitted event only if that occurs ≤ 300 ms after the previous emission. Note both `kEventMouseWheelMoved` and
`kEventMouseScroll` are subscribed, so one physical scroll may produce two Carbon events (the second is absorbed by
the throttle).

Item scroll (`bar_item_on_scroll`), only if subscribed to `mouse.scrolled`; `bar_item_update(item, "mouse.scrolled",
forced=true, env)` with env (in order):
- `INFO` = `"{\n\t\"delta\": <total>,\n\t\"modifier\": \"<m>\",\n\t\"modfier_code\": <flags>\n}\n"`
- `SCROLL_DELTA` = `"%d"` total
- `MODIFIER` = `<m>`

Global scroll (`bar_manager_handle_mouse_scrolled_global`) → `custom_events_trigger("mouse.scrolled.global", env)` with
env (in order): `SCROLL_DELTA`, `INFO` (same format as item scroll), `DID` = `"%u"` arrangement id of the bar/popup
(`adid`, despite the name), `MODIFIER`.

`mouse.scrolled.global` fires **only** when scrolling over empty bar/popup area (not over an item window).

### 6.4 `mouse.entered` / `mouse.entered.global` (`event.c:event_mouse_entered`)

```
if bar = get_bar_by_wid(wid):
    if !bar.mouse_over and !mouse_over_any_popup():
        bar.mouse_over = true; custom_events_trigger("mouse.entered.global", NULL)
    return
if popup = get_popup_by_wid(wid):
    if !popup.mouse_over and !mouse_over_any_bar():
        popup.mouse_over = true; custom_events_trigger("mouse.entered.global", NULL)
    return
item = get_item_by_wid(wid)
if item:                                                  // bar_item_mouse_entered
    if (item.update_mask & mouse.entered) and !item.mouse_over:
        bar_item_update(item, "mouse.entered", forced=true, NULL)
    item.mouse_over = true
```

Global enter/exit state is driven solely by enter/exit events whose `wid` is the bar or popup window itself;
item windows sit above the bar window, so whether entering the bar directly onto an item window also produces a
bar-window enter depends on SkyLight tracking-rect semantics (unverified, Open Question 9). The `mouse_over` flags
of bars are reset to false whenever bars are recreated (display rebuild).

### 6.5 `mouse.exited` / `mouse.exited.global` (`event.c:event_mouse_exited`)

```
point = event location
if bar = get_bar_by_wid(wid):
    origin = bar.window; popup_target = get_popup_by_point(point); over_target = popup_target != NULL
elif popup = get_popup_by_wid(wid):
    origin = popup.window; bar_target = get_bar_by_point(point); over_target = bar_target != NULL
if bar or popup:
    frame = origin.frame at origin.origin, inset by 1 px on every side
    over_origin = CGRectContainsPoint(frame, point)
    if !over_origin and !over_target:
        (bar or popup).mouse_over = false
        custom_events_trigger("mouse.exited.global", NULL)
        for every item: bar_item_mouse_exited(item)        // ALL items subscribed to mouse.exited get it (Quirk Q6)
    elif !over_origin and over_target:
        if bar: bar.mouse_over = false; popup_target.mouse_over = true
        else:   bar_target.mouse_over = true; popup.mouse_over = false
                host = popup.host
                if host.update_mask & (mouse.exited | mouse.exited.global)
                   and get_item_by_point(point) != host:
                    bar_item_mouse_exited(host)
    // over_origin (e.g. cursor moved onto an item window inside the bar): nothing
    return
item = get_item_by_wid(wid)
if item and (item.update_mask & mouse.exited.global) and get_popup_by_point(point) == &item.popup:
    return                                                  // moving from an item into its own popup
if item: bar_item_mouse_exited(item)
```

`bar_item_mouse_exited(item)`: if subscribed to `mouse.exited` → `bar_item_update(item, "mouse.exited", forced=true,
NULL)` (**no** `mouse_over` check, unlike entered); then `item.mouse_over = false`.

### 6.6 Drag (slider)

`event.c:event_mouse_dragged`: only `get_item_by_wid` (no point fallback); only items with `has_slider`. Point →
item-window-local; `slider_handle_drag`: `pct = clamp(round((p.x - track.origin.x) / track.width * 100), 0..100)`
(computed `uint32(delta/width*100 + 0.5)`, `delta = max(0, …)`, then `min(…,100)`), `is_dragged = true`, set
percentage (no animation) → `needs_update` → refresh. No script event during drag; on release `mouse.clicked`
delivers with `PERCENTAGE` in the item env. While `is_dragged`, `slider.percentage=` and `slider.width=` set via
`--set` are ignored.

---

## 7. `--trigger <event> [KEY=VALUE …]`

`message.c:handle_domain_trigger` (inside a frozen mach batch; the batch ends with a refresh):

```
event = next token
env = {}
for each remaining token on the batch line:
    split at the FIRST '='; key = before, value = after
    if no '=' or value empty ("KEY="): skip silently
    env.set(key, value)                     // later duplicates replace earlier ones
switch event:
  "space_change":          bar_manager_handle_space_change(forced=true)   // env ignored
  "display_change":        bar_manager_handle_display_change()            // env ignored
  "space_windows_change":  forced_space_windows_event()                   // env ignored; no-op unless started
  "volume_change":         forced_volume_event()                          // env ignored
  "media_change":          forced_media_change_event()                    // env ignored; gated by g_media_events
  "wifi_change":           forced_network_event()                         // env ignored
  "power_source_change":   forced_power_event()                           // env ignored
  default:                 custom_events_trigger(event, env)
```

- All other names — user events *and* built-ins like `front_app_switched`, `brightness_change`, `system_woke`,
  `system_will_sleep`, `mouse.*` — go through `custom_events_trigger`: subscribers run with
  `SENDER=<event>`, the passed vars (e.g. `INFO` only if the user passed `INFO=…`), persistent vars and `NAME`.
- Unknown event names: no error, no response, nothing happens.
- Values cannot contain spaces unless quoted by the shell (each argv element is one token). Keys are not validated.
- Delivery is synchronous within the batch; scripts are spawned before the client receives its reply.

---

## 8. Timers, routine and forced updates, startup

### 8.1 Routine clock

- One `CFRunLoopTimer` per bar manager: first fire `CFAbsoluteTimeGetCurrent() + 1`, interval 1.0 s, tolerance
  default, mode `kCFRunLoopCommonModes`, on the main run loop (`bar_manager.c:bar_manager_init`). Missed fires are
  skipped (CFRunLoopTimer semantics), not caught up. Re-created on hotload.
- Each fire → `SHELL_REFRESH` → `bar_manager_update(forced=false)`:

```
if (frozen && !forced) || sleeps: return
if forced: (forced OS events, §8.2)
needs_refresh = false
for item in bar_items:
    bar_item_update(item, NULL, forced, NULL)              // always returns false
    if item.has_alias && is_shown(item) && alias_update(item.alias, false):
        item.needs_update = true; needs_refresh = true
if needs_refresh || forced: bar_manager_refresh(forced)
```

- `update_freq=N` (u32 via `strtoul(…, 0)`, so `0x`/octal prefixes work; default 0 = never): with the counter
  semantics of §4.2 the script fires on every N-th tick since the last run of that item, i.e. every N seconds when no
  events intervene. `N=1` fires every tick. Any event-triggered run resets the countdown.
- `updates=off` disables routine runs and event runs (but not forced/mouse runs). `updates=when_shown` runs only while
  the item is drawn on some bar (routine and events; forced and mouse bypass). `updates=on|toggle|…` uses
  `evaluate_boolean_state` and clears `when_shown`.
- `scroll_texts=on` items start a marquee every 15th call while shown (§10.9).

### 8.2 Forced update (`--update`)

`message.c` `--update` → `bar_manager_update(forced=true)` and `bar_needs_refresh = true` for the batch. Order:

1. `bar_manager_handle_space_change(forced=true)` → `space_change` (all space items re-evaluated and run)
2. `forced_network_event()` → `wifi_change`
3. `forced_volume_event()` → `volume_change`
4. `forced_brightness_event()` → `brightness_change` (only path that forces brightness)
5. `forced_power_event()` → `power_source_change`
6. `forced_front_app_event()` → `front_app_switched`
7. `forced_media_change_event()` → async `media_change` (if enabled; arrives later on main queue)
8. `forced_space_windows_event()` → `space_windows_change` per space (if started)
9. every item: `bar_item_update(item, NULL, forced=true, NULL)` → `SENDER=forced`, ignoring `updates`,
   `update_freq`, `when_shown` (still requires script or mach port)
10. `bar_manager_refresh(forced=true)` (resets bar associations, all items `needs_update`, resize). This
    only executes because step 1 cleared the batch freeze (Quirk Q7).

Blocked entirely while `sleeps` (§9.1). Not blocked by the batch freeze. Nothing runs item scripts automatically at
startup: configs are expected to end with `sketchybar --update`.

### 8.3 Startup sequence (`sketchybar.c:main`)

1. `g_name = basename(argv[0])` (also the mach service suffix and `BAR_NAME`); refuse root; `setenv("BAR_NAME")`.
2. CLI parsing (client mode exits here).
3. `init_misc_settings`: lock file `/tmp/<name>_<USER>.lock`; `SIGCHLD`, `SIGPIPE` ignored;
   `CGSetLocalEventsSuppressionInterval(0)`; `CGEnableEventStateCombining(false)`; SLS connection; space management mode.
4. Load `SLSTransactionAddPostDecodeAction` (macOS 26); acquire lock (`F_SETLK` write lock; failure → exit).
5. SLS notify procs 904, 905, 1401, 1508, 1322 (capture gating); 1327, 1328 → `space_change` (macOS ≥ 13).
6. Workspace context alloc; `bar_manager_init` (custom events, animator + display link start, routine timer).
7. `mouse_begin`, `display_begin` (CG reconfig callback), workspace observers (`-init`).
8. `windows_freeze`; `bar_manager_begin` (create bars); `windows_unfreeze`.
9. Mach server `git.felix.<name>` (failure → exit).
10. Power, Wi-Fi listeners (always); MediaRemote registration (gated).
11. `exec_config_file()` (async), FSEvents watcher on the config directory.
12. macOS ≥ 14: `SLSWindowManagementBridgeSetDelegate(NULL)`.
13. `RunApplicationEventLoop()`.

---

## 9. Sleep, wake, display reconfiguration, hotload

### 9.1 `system_will_sleep` (`bar_manager_handle_system_will_sleep`)

```
custom_events_trigger("system_will_sleep", NULL)
animator_destroy_display_link()          // animations freeze where they are (not cancelled)
sleeps = true                            // routine ticks and --update become no-ops
```

### 9.2 `system_woke` (`bar_manager_handle_system_woke`)

```
if sleeps:
    sleeps = false
    after ~500 ms (global LOW queue usleep(500000) → main queue): post SYSTEM_WOKE again
bar_manager_display_changed()            // §9.3: full rebuild + display_change + forced space_change + new display link
custom_events_trigger("system_woke", NULL)
```

- Triggered by `NSWorkspaceDidWakeNotification` **and** by the distributed notification `com.apple.screenIsUnlocked`.
- After real sleep: `system_woke` subscribers run **twice** (~0.5 s apart), each preceded by `display_change` and a
  forced `space_change`. A screen unlock without a prior sleep notification: once (no re-post).
- Only the first wake after sleep schedules the re-post (`sleeps` is false for the second).

### 9.3 Display reconfiguration (`bar_manager_display_changed`)

Called for `DISPLAY_ADDED/REMOVED/MOVED/RESIZED` (one call per CG callback with a matching flag; the
`kCGDisplayBeginConfigurationFlag` pre-notification matches none) and from wake:

```
active_adid = display_active_display_adid()
freeze(); bar_manager_reset()            // destroy all bars & item windows, recreate bars per --bar display
unfreeze(); bar_manager_refresh(forced=true)
bar_manager_handle_display_change()      // display_change event
bar_manager_handle_space_change(true)    // forced space_change event
animator_renew_display_link()            // new CVDisplayLink for the current display set (started even if idle)
```

On add/remove with brightness events active, DisplayServices registration is added/removed for that display.

### 9.4 Hotload / reload

- `--hotload on|off|toggle|…` sets `g_hotload` (default **false**). The FSEvents stream is always created at startup
  on `dirname(config)`, `kFSEventStreamEventIdSinceNow`, latency 0.5 s,
  flags `kFSEventStreamCreateFlagNoDefer | kFSEventStreamCreateFlagFileEvents` (recursive: any file change under the
  config directory, including plugin scripts).
- FSEvents callback: if `g_hotload && count > 0` and `now - last_hotload > 2^30 ns` (≈1.074 s, monotonic raw) →
  `last_hotload = now`, post `HOTLOAD`.
- `--reload [path]`: optional new config path (`realpath`; invalid → response `[?] Reload: Invalid config path '<p>'\n`
  and no reload); otherwise posts `HOTLOAD` immediately and synchronously **inside** the current mach batch
  (remaining batch commands then apply to the new state).
- `HOTLOAD` handler (`event.c:event_hotload`):
  `bar_manager_destroy` (mach helpers get `"k"`, animations dropped, all items removed, bars destroyed, events list
  freed, timer invalidated) → `bar_manager_init` (fresh built-in event list, new animator + display link, new timer)
  → `bar_manager_begin` → `exec_config_file()` (async fork/exec).
- Not reset by hotload: OS listener globals (volume/brightness/media/space-window events stay installed and enabled),
  distributed-notification observers of earlier custom events (Bug B6), power/media/volume dedup state, scroll
  throttle, `g_hotload` itself.

---

## 10. Animation system

### 10.1 `--animate <curve> <duration>` (`message.c:handle_message_mach`)

- At the **start of every mach batch** (one client invocation): `animator.interp_function = '\0'`,
  `animator.duration = 0`.
- `--animate c d`: `interp_function = first byte of token c` (empty/missing token → `'\0'`);
  `duration = strtoul(d, NULL, 0)` as u32 (`0x..` hex and leading-`0` octal accepted; garbage → 0; negative →
  wraps to a huge value, Quirk Q9).
- The setting applies to every animatable property assignment that follows **in the same batch** (`--set`,
  `--default`, `--bar`, including regex `--set`), until another `--animate` in the same batch overrides it.
  `--animate <c> 0` turns animation off for the following commands.
- Quirk Q8: `text_animate_scroll` (marquee, §10.9) writes `animator.duration`/`interp_function` and resets them to
  `0`/`'\0'` when it runs; if it runs synchronously inside a batch (e.g. via `--trigger` → item update), it cancels the
  batch's `--animate` for all following commands.

### 10.2 Curves (`animation.c:animation_setup`, formulas in `misc/helpers.h`)

Selection is by the **first character only**:

| First char | Documented name | Function | Formula `f(x)`, x ∈ [0,1] |
|---|---|---|---|
| `l` | linear | `function_linear` | `x` |
| `q` | quadratic | `function_square` | `x*x` |
| `t` | tanh | `function_tanh` | `a = 0.52; a*tanh(2*atanh(1/(2a))*(x-0.5)) + 0.5` (f(0)=0, f(1)=1 exactly in exact arithmetic) |
| `s` | sin | `function_sin` | `sin(π/2 * x)` |
| `e` | exp | `function_exp` | `x * exp(x - 1)` |
| `c` | circ | `function_circ` | `sqrt(1 - powf(x - 1, 2))` — the square is computed in **f32** (`powf`, `1.f`), the sqrt in f64 |
| `b` | bounce | — | **falls back to linear** (`INTERP_FUNCTION_BOUNCE` is defined but not mapped) |
| `o` | overshoot | — | **falls back to linear** (`INTERP_FUNCTION_OVERSHOOT` defined but not mapped) |
| anything else, `'\0'` | — | `function_linear` | `x` |

So `--animate smooth 20` = sin, `--animate elastic 20` = exp, `--animate bounce 20` = linear. Computation is f64.
On the final frame the curve is bypassed (`slider = 1.0`), so every animation lands exactly on its target.

### 10.3 Duration units and time base

- Duration unit: **frames at 60 Hz**. `animation.duration_seconds = duration / 60.0` (`animation_setup`). E.g. 30 →
  0.5 s. Wall-clock based, independent of the actual display refresh rate.
- Time base: `CVDisplayLink` output callback's `output_time->hostTime` (mach absolute ticks of the *upcoming* vsync),
  scaled by `animator.clock = CVGetHostClockFrequency()` (ticks/s; 24 MHz on Apple Silicon, 1e9 on Intel).
  mbar may use any monotonic ns clock with `clock = 1e9`.
- Progress of one animation at frame time `T`:

```
if initial_time == 0: initial_time = T                  // first frame it is actually stepped
t = duration_seconds > 0 ? (T - initial_time) / (duration_seconds * clock) : 1.0
final = t >= 1.0
t = clamp(t, 0, 1)
s = final ? 1.0 : curve(t)
```

  The first stepped frame has `t = 0` (value = initial value, usually no visible change); a 0-duration animation
  completes on its first stepped frame.

### 10.4 The three value kinds (`ANIMATE`, `ANIMATE_FLOAT`, `ANIMATE_BYTES` in `animation.h`; `animation_update`)

Initial/final values are stored as 32-bit `int` (floats bit-cast; u32 colors reinterpreted).

| Kind | Interpolation per frame | Final frame |
|---|---|---|
| int (`ANIMATE`) | `value = (int)((1-s)*i + s*f + 0.5)` — C double→int **truncates toward zero**, so for negative intermediates this is not round-half-up (e.g. −2.7 → −2). In Rust `((1.0-s)*i as f64 + s*f as f64 + 0.5) as i32` matches. | `value = f` exactly |
| float (`ANIMATE_FLOAT`, `as_float`) | `value = (float)((1-s)*i + s*f)` computed in f64 | same formula with s=1 (= f) |
| bytes (`ANIMATE_BYTES`, `separate_bytes`) | for each of the 4 bytes in **memory (little-endian) order** of the 32-bit value (byte0 = bits 0–7 = blue of `0xAARRGGBB`, …, byte3 = alpha): `b = (uint8)((1-s)*bi + s*bf)` (truncation, no +0.5) | same formula with s=1 (= f) |

The setter is called every frame with the computed value; its boolean return ("changed") drives redraw (§10.7).

### 10.5 Creating, queueing, locking, cancelling

Animation identity key = **(target object, setter function)**. Two different syntaxes that reach different
(target, setter) pairs for the same visual field are independent animations and can fight (Quirk Q10), e.g.
`icon.color` (target `text`, `text_set_color`) vs `icon.color.hex` (target `text.color`, `color_set_hex`);
`background.color` vs `background.color.hex` vs `background.color.alpha`.

Macro behavior for a property assignment `prop = new` whose current value is `cur`:

```
if animator.duration > 0:                                     // animated path
    cancel_locked(key)                     // remove all LOCKED animations with this key; final values NOT applied
    a = new Animation(key, initial=cur, final=new, duration, curve, kind)
    // animator_add → calculate_offset:
    prev = last animation in the list (searching from the end) with the same key
    if prev:                               // necessarily unlocked = created earlier in this same batch
        a.initial = prev.final; prev.next = a; a.previous = prev; a.waiting = true
    append a; ensure display link running
    // the assignment itself reports "no refresh needed"; frames will mark the owner dirty
else:                                                         // immediate path
    needs_refresh = cancel(key)            // remove ALL animations with this key (locked or not), calling
                                           // setter(final) for each in list order (snap to their targets)
    needs_refresh |= setter(new)
```

At the very end of each batch (`handle_message_mach`): `animator_lock()` marks **every** existing animation
`locked = true` (before the final refresh).

Resulting semantics:

| Situation | Behavior |
|---|---|
| Same key assigned twice in one batch with animation (`--animate l 30 --set a y_offset=10 y_offset=0`) | sequential chain: 0→10 then 10→0 (second starts when first finishes) |
| Same key assigned animated in a later batch while an animation (or chain) is in flight | in-flight chain removed without snapping; new animation starts from the **current intermediate value** |
| Same key assigned without animation while animations are in flight | in-flight snaps to its final value(s), then the new value is set immediately |
| Different keys | run concurrently |
| Animation with `cur == new` | still created; runs its duration doing nothing |

Bug B4 (do not replicate the mechanism, replicate the effect): for float animations the immediate-path snap calls
the float setter through an int-typed pointer (garbage float argument); the effect is overwritten by the following
`setter(new)`, so the observable result is "snap then set".

### 10.6 Display link and frame stepping

- `animator_init` creates and starts a `CVDisplayLinkCreateWithActiveCGDisplays` link (even with no animations).
  `animator_add` re-creates it if it was destroyed. `animator_renew_display_link` (display rebuild, wake) stops,
  releases and re-creates it. `bar_manager_handle_system_will_sleep` destroys it.
- Output callback (display-link thread): `dispatch_async(main, post ANIMATOR_REFRESH(hostTime))`. Frames are never
  dropped/coalesced by SketchyBar; each queued frame is processed in order.
- `ANIMATOR_REFRESH` → `bar_manager_animator_refresh(T)`:

```
freeze()
if animator_update(T):                    // any setter reported a change
    unfreeze()
    if bar_needs_resize: bar_manager_resize()
    bar_manager_refresh(false)
unfreeze()
```

- `animator_update(T)`:

```
changed = false; finished = []
for a in animations (array = insertion order):
    if a.waiting or !a.target or !a.setter: continue (contributes false)
    step a as in §10.3/10.4; call setter; mark owner dirty if setter returned true (§10.7)
    a.finished = final
    if final and a.next:                  // release the successor
        a.next.previous = NULL; a.next.waiting = false; a.next = NULL
    if a.finished: finished.push(a)
for a in finished: remove(a)
if animations empty: destroy display link
return changed
```

  Because a successor is always later in the array, it is stepped **in the same frame** its predecessor finished
  (with `initial_time = T`, i.e. t=0; a 0-duration successor completes in that same frame).
- Event processing of every frame also runs the per-event active-display poll (§1.1) and `windows_unfreeze`.
- Sleep: link destroyed, animations kept; on wake the link is renewed and because progress is wall-clock based, all
  animations jump to (or near) completion.

### 10.7 Redraw propagation (`animation_update`)

If the setter returns true: if the target address lies inside any `struct bar_item` in `bar_items`
(`[item, item + sizeof(bar_item))`, which covers embedded text/background/image/shadow/font/color/slider/popup
structs) → that item `needs_update = true`; otherwise (bar-level targets, `defaults` item) →
`bar_needs_update = true` (full bar redraw). mbar: tag each animation with its owner (item id or Bar).

Bug B7: removing an item (`--remove`) does not cancel its animations (dangling target → use-after-free). mbar must
cancel all animations owned by a removed item (and on hotload, drop all animations — SketchyBar does that).

### 10.8 Animatable properties

"Initial" = value read at parse time as `cur`. Setters return "changed"; side effects listed. All others
properties are never animated (applied immediately even inside `--animate`).

**Item (`--set <item>` / `--default`)** — `bar_item.c:bar_item_parse_set_message`

| Property | Kind | Key (target, setter) | Initial `cur` | Notes |
|---|---|---|---|---|
| `y_offset` | int | (item, `bar_item_set_yoffset`) | `y_offset` | |
| `padding_left` | int | (item.background, `background_set_padding_left`) | `background.padding_left` | same key as `background.padding_left` |
| `padding_right` | int | (item.background, `background_set_padding_right`) | `background.padding_right` | |
| `blur_radius` | int | (item, `bar_item_set_blur_radius`) | `blur_radius` | applies to item windows |
| `width=<n>` | int | (item, `bar_item_set_width`) | current effective width: `bar_item_get_length(item,false) + (has_const_width ? 0 : pad_l + pad_r)` | setter: `n<0` → `has_const_width=false`; else const width n |
| `width=dynamic` | int ×2 | same | `custom_width` (stale if no const width!) | §10.9 |

**Text** (`icon.*`, `label.*`, `slider.knob.*` via `text_parse_sub_domain`; `icon=`/`label=` = `.string`)

| Property | Kind | Key | Notes |
|---|---|---|---|
| `color` | bytes | (text, `text_set_color`) | |
| `highlight_color` | bytes | (text, `text_set_highlight_color`) | |
| `highlight` | bytes (composite) | see §10.9 | boolean toggle animated as a color cross-fade |
| `padding_left`, `padding_right`, `y_offset` | int | (text, `text_set_*`) | |
| `width=<n>` | int | (text, `text_set_width`) | initial `text_get_length(text,false)`; `n<0` clears const width |
| `width=dynamic` | int ×2 | (text, `text_set_width`) | §10.9 |
| `string` | int (width) | (text, `text_set_width`) | §10.9 auto width animation |
| `font.size` | float | (text.font, `font_set_size`) | marks font changed (re-layout) |
| `color.hex` / `.alpha` / `.red` / `.green` / `.blue` | bytes / float ×4 | (text.color, `color_set_hex` / `color_set_alpha|r|g|b`) | float channels clamped 0..1; hex recomputed as `(u32)(a*255)<<24 | …` (truncation) |
| `highlight_color.*` | as above | (text.highlight_color, …) | |
| `background.*`, `shadow.*` | see below | | |

**Background** (`background.*`, `icon.background.*`, `label.background.*`, `popup.background.*`, `slider.background.*`,
and `--bar <prop>` for the bar background) — `background.c:background_parse_sub_domain`

| Property | Kind | Setter side effects |
|---|---|---|
| `clip` | float | sets `bar_needs_update`, `might_need_clipping`; `clip > 0` enables background |
| `height` | int | `overrides_height = (h != 0)` |
| `corner_radius`, `border_width` | int | |
| `color` | bytes | also enables the background (`drawing=on`) |
| `border_color` | bytes | |
| `padding_left`, `padding_right`, `x_offset`, `y_offset` | int | |
| `color.*`, `border_color.*` | color sub-domain (bytes/float) | |
| `image.scale` | float | recomputes image bounds |
| `image.corner_radius` | int (u32 parse) | |
| `image.padding_left`, `image.padding_right`, `image.y_offset` | int | |
| `image.border_width` | float | |
| `image.border_color` | bytes | (`image.border_color.*` color sub-domain) |
| `shadow.distance`, `shadow.angle` | int | recompute offset `(d·cos(angle°), −d·sin(angle°))` |
| `shadow.color` | bytes | also enables shadow |
| `shadow.color.*` | color sub-domain | |

`slider.background.<prop>` is parsed **twice**: first into the slider *foreground* background, then
`background_set_color(foreground, foreground_color)` is applied immediately (cancels nothing, just restores the fill
color), then into the slider track background — so an animated `slider.background.height=…` creates two independent
animations (keys `(slider.foreground, …)` and `(slider.background, …)`). `slider.knob=<s>` is the knob string,
`slider.knob.<prop>` the knob text sub-domain (all text animations apply).

**Slider** (`slider.*`, `slider.c:slider_parse_sub_domain`)

| Property | Kind | Notes |
|---|---|---|
| `slider.percentage` | int (u32 parse) | clamped 0..100; ignored entirely while dragging |
| `slider.width` | int (u32 parse) | ignored while dragging |
| `slider.highlight_color` | bytes | key (slider, `slider_set_foreground_color`); also sets foreground background color |

**Popup** (`popup.*`, `popup.c:popup_parse_sub_domain`)

| Property | Kind | Notes |
|---|---|---|
| `popup.y_offset` | int | |
| `popup.height` | int | key (popup, `popup_set_cell_size`); sets `overrides_cell_size` |
| `popup.blur_radius` | int | setter applies blur directly and returns false (never triggers redraw) |

**Graph**: only `graph.color.*` / `graph.fill_color.*` color sub-domains are animatable; `graph.color=`,
`graph.fill_color=`, `graph.line_width=` are immediate.

**Bar** (`--bar`, `message.c:handle_domain_bar`; owner = bar → `bar_needs_update`)

| Property | Kind | Key | Notes |
|---|---|---|---|
| `margin` | int | (bar_manager, `bar_manager_set_margin`) | sets `bar_needs_resize` |
| `y_offset` | int | (bar_manager, `bar_manager_set_y_offset`) | bar background y_offset; `bar_needs_resize` |
| `blur_radius` | int | (bar_manager, `bar_manager_set_background_blur`) | applies to bar windows; returns false |
| `notch_width` | int | (bar_manager, `bar_manager_set_notch_width`) | no resize flag |
| `notch_offset`, `notch_display_height` | int | (bar_manager, …) | `bar_needs_resize` |
| `height` | int | (bar_manager, `bar_manager_set_bar_height`) | sets background height; `bar_needs_resize` |
| any other key | → bar background table above | (bar_manager.background, …) | e.g. `color`, `border_color`, `corner_radius`, `x_offset`, `padding_*`, `clip` |

Non-animatable (always immediate): `drawing`, `position`, `align`, `font=` (full font string), `font.family/style/
features/typographical_width`, `max_chars`, `scroll_texts`, `scroll_duration`, `updates`, `update_freq`, scripts,
`associated_*`, `shadow` (item), `ignore_association`, `image=`/`image.string`, `popup.drawing/horizontal/align/
topmost`, all non-numeric bar settings.

### 10.9 Composite / implicit animations

**Text `string` change** (`text.c:text_parse_sub_domain`, PROPERTY_STRING), only when `animator.duration > 0`:

```
pre = text_get_length(text, false)       // == custom_width if text has a const width
changed = text_set_string(new)           // false if identical string
if changed:
    post = text_get_length(text, false)
    if post != pre:                       // never true for const-width texts
        text_set_width(pre)              // immediately pin current width (const)
        ANIMATE int (text, text_set_width): pre → post
        add chained 0-duration animation (text, text_set_width): → -1   // unpin at the end
```

i.e. labels/icons animate their width to the new natural width, then return to dynamic width.

**`width=dynamic`** (item and text): `ANIMATE(width: custom_width → natural)` followed by a chained 0-duration
`→ -1` animation (always created, also when not animating: then the width is set immediately to the natural length
and the `-1` animation runs on the next display-link frame).
Natural length: item `bar_item_get_length(item, true) + pad_l + pad_r`; text `text_get_length(text, true)`.

**`highlight=<bool>`** (text), only when `animator.duration > 0` (otherwise just a flag flip):
- on → off: `cancel(text, text_set_color)` (snap), `target = color`; `color := highlight_color` (immediate);
  animate `color` bytes from highlight_color → target. (Drawing uses `color` when not highlighted.)
- off → on: `cancel(text, text_set_highlight_color)`; `target = highlight_color`;
  `highlight_color := color`; animate `highlight_color` bytes color → target.
- The flag changes immediately; returns "changed" iff the flag changed.

**Marquee scroll** (`text.c:text_animate_scroll`, called from `bar_item_update`, §4.2, for icon/label/knob of items
with `scroll_texts=on` while shown and `counter % 15 == 0`):

```
preconditions: max_chars > 0; scroll == 0 (not already scrolling);
               not (has_const_width && custom_width < width);
               width != 0 and width != bounds.width            // text is actually truncated
// width = truncated (visible) width; bounds.width = full text width; scroll is subtracted from the draw x
animator.curve = linear
animator.duration = (u32)(scroll_duration * (bounds.width / width))      // frames
ANIMATE_FLOAT scroll: 0 → bounds.width                                   // scroll fully out to the left
add chained 0-duration float animation scroll → -width                   // jump to the right edge
animator.duration = scroll_duration
ANIMATE_FLOAT scroll: (chained, from -width) → 0                          // scroll back in
animator.duration = 0; animator.curve = '\0'                              // Quirk Q8
```

Speed is constant: `width` px per `scroll_duration` frames (default `scroll_duration = 100` → 100/60 s).
These animations are created outside a batch and stay unlocked until the next batch ends.

### 10.10 Lifecycle summary

| Trigger | Effect on animations |
|---|---|
| end of each mach batch | all animations locked |
| `--remove` item | not cancelled (Bug B7; mbar: cancel) |
| hotload / `--reload` / `--exit` | `animator_destroy`: all dropped without applying finals |
| system_will_sleep | display link destroyed; animations retained |
| wake / display reconfiguration | display link renewed; animations resume (wall-clock → usually complete at once) |
| no animations left after a frame | display link destroyed |

---

## 11. Quirk & bug index

"Replicate?" is a recommendation for drop-in compatibility (configs/plugins depending on it).

| ID | Behavior | Where | Replicate? |
|---|---|---|---|
| Q1 | One env map is threaded through all recipients of an event; item N+1 inherits item N's persistent keys it does not override | `bar_manager_custom_events_trigger` | optional (low value); at minimum each recipient must get its own persistent vars, NAME, SENDER |
| Q2 | `counter` increments on every update call and resets on every actual run → events postpone the next routine run | `bar_item_update` | yes |
| Q3 | `SENDER` from env-less deliveries is stored in the item's persistent env and leaks into click_script env and later events | `bar_item_update` | yes (cheap; plugins may read `$SENDER` in click scripts) |
| Q4 | `setenv` in a vfork child likely mutates the daemon environ → vars of earlier scripts leak into later ones | `helpers.h:fork_exec` | see Open Question 1 |
| Q5 | With separate Spaces off, active display is cursor-based and only re-evaluated at the start of the next event | `event_execute` | yes (or poll on cursor move; event still only on change) |
| Q6 | Leaving the bar/popup to nowhere sends `mouse.exited` to **every** subscribed item (no `mouse_over` check) | `event_mouse_exited` | yes |
| Q7 | `frozen` is a boolean; nested freeze/unfreeze clears the batch freeze | `bar_manager_freeze` | no (only intermediate redraws differ) |
| Q8 | Marquee start inside a batch resets `--animate` for the rest of the batch | `text_animate_scroll` | optional |
| Q9 | `--animate` duration via `strtoul(…,0)`: hex/octal accepted, negative wraps to ~4.29e9 frames | `handle_message_mach` | parse identically |
| Q10 | Animation key is (target, setter): `color` vs `color.hex` vs `color.alpha` are independent animations | `animation.h` | yes |
| Q11 | One Space switch → several `space_change` events (NSWorkspace + SLS 1327 + 1328) | §5.2 | recommended to coalesce *only if* script-visible results are identical; SketchyBar users rely on idempotent scripts — keep ≥1 |
| Q12 | `system_woke` fires twice after real sleep (+500 ms), each with display rebuild, `display_change`, forced `space_change` | §9.2 | yes |
| Q13 | `mouse.scrolled.global` and global click handling only on empty bar/popup area; clicks on empty area do nothing | §6.2/6.3 | yes |
| Q14 | Click on a slider item outside its track: no click_script, no `mouse.clicked` | `bar_item_on_click` | yes |
| Q15 | `bounce` and `overshoot` curves are linear | `animation_setup` | see Open Question 2 |
| Q16 | `wifi_change` fires on IPv4 global state changes, without dedup; SSID may be empty | `wifi.m` | yes |
| Q17 | Brightness dedup state is global across displays | `display.c` | yes |
| Q18 | `--add event` with an existing name is silently ignored (notification not updated) | `custom_events_append` | yes |
| Q19 | JSON key typo `modfier_code` in click/scroll INFO | `bar_item.c`, `bar_manager.c` | **yes, exact** |
| Q20 | Space items run their script on `space_change` only when their selection flips (or forced) | §5.2.1 | yes |
| Q21 | `--trigger` of space/display/space_windows/volume/media/wifi/power runs the forced OS handler and ignores passed vars; `--trigger brightness_change`/`front_app_switched` do *not* query the OS | `handle_domain_trigger` | yes |
| Q22 | `event_post` on main thread is synchronous/re-entrant (`--reload` mid-batch) | `event_post` | yes for ordering |
| Q23 | INFO JSON strings are not JSON-escaped (app names, SSIDs); media escapes only `"` and LF | §5 | yes (byte-exact) |
| Q24 | `space_windows_change` on Space switch emits one event per space of every display | §5.11 | yes |
| B1 | Space item inheriting from defaults/clone gets `SID = ancestor's DID` | `bar_item_inherit_from_item` | no (use ancestor SID) — or yes for exactness; trivial |
| B2 | `space_change` INFO buffer `19*n+4` may truncate | `bar_manager_handle_space_change` | no |
| B3 | `display_change` INFO buffer 3 bytes | `bar_manager_handle_display_change` | no |
| B4 | Float snap on cancel uses int-typed call (garbage), immediately overwritten | `animator_cancel` | no (snap then set) |
| B5 | Hotload while a display link exists with 0 animations leaks a running link (`animator_destroy` only releases when count > 0) | `animator_destroy` | no |
| B6 | Distributed observers are never removed; after hotload, re-adding an event with the same notification registers a second observer → each notification triggers the event twice; if two events share one notification name, only the first is triggered (but once per observer registration) | `workspace.m`, `custom_events.c` | no (deliver exactly once per notification to the first matching event; document) — see Open Question 4 |
| B7 | Removing an item does not cancel its animations (use-after-free) | `bar_manager_remove_item` | no (cancel) |
| B8 | `power_handler` would crash if `IOPSGetProvidingPowerSourceType` returned NULL | `power.c` | no (treat as no event) |

---

## 12. Open questions

1. **vfork env leak (Q4).** On macOS a vfork child shares the parent's address space, so `setenv` in the child very
   likely persists in the daemon's `environ`; then e.g. `$INFO`, `$BUTTON`, `$SCROLL_DELTA` of a previous event are
   visible to later scripts that did not set them. Needs empirical confirmation on a Mac (e.g. subscribe an item to
   `mouse.clicked` and `routine`, print `$INFO` in the routine run). mbar must decide: replicate (maintain a
   daemon-wide accumulated env map that every spawned script inherits) or spawn with clean per-event env.
2. **bounce / overshoot.** This checkout maps them to linear. Upstream docs list them as real curves; the shallow clone
   has no history to recover older formulas. Decide: byte-exact (linear) or implement documented easing (would be an
   mbar extension).
3. **Brightness callback thread.** `DisplayServicesRegisterForBrightnessChangeNotifications` delivery thread is
   undocumented; `event_post` handles both cases, so semantics do not depend on it, but mbar's macOS layer must
   marshal to its main loop.
4. **Duplicate distributed observers after hotload (B6)** — confirm empirically whether NSDistributedNotificationCenter
   delivers twice for a duplicate (observer, selector, name) registration; mbar recommendation: once.
5. **SkyLight notify ids** (1327/1328 space, 1325/1326 window create/destroy, 815/816 unhide/hide, 1401, capture
   gating 904/905/1508/1322) are private; their exact trigger conditions on current macOS (14–26) should be verified
   when implementing `mbar-macos`.
6. **Order of multiple `space_change` sources** (NSWorkspace vs SLS 1327/1328) and whether `bar.sid` is already
   updated when the first one arrives — affects only redundant deliveries, but the first delivery might carry a stale
   INFO. Needs runtime verification.
7. **Carbon double scroll events** (`kEventMouseWheelMoved` + `kEventMouseScroll` for one gesture) — whether both are
   delivered for the same physical event, which the 150 ms throttle would hide; mbar should use one CGEvent scroll
   source plus the same throttle.
8. **MediaRemote availability.** Since macOS 15.4 the private API refuses non-entitled callers; `media_change` is
   effectively dead in SketchyBar. mbar may need an alternative provider (out of scope of 1:1 behavior; payload format
   in §5.10 should be kept).
9. **Tracking rects of overlapping windows.** Whether the bar window receives enter/exit while the cursor moves
   between the bar background and item windows stacked above it (SkyLight `SLSAddTrackingRect`). The 1-px inset
   check in `event_mouse_exited` suggests the bar *does* get an exit when the cursor moves onto an item window; mbar
   must reproduce the resulting `mouse.entered.global`/`mouse.exited.global` semantics (fire on entering/leaving the
   union of bar + popups), not the raw window events.
