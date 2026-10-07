# SketchyBar bar / bar_manager / display / window spec

Source of truth: SketchyBar v2.24.0, commit `5f358ec` ("use opentype directly").
Files covered: `src/bar.c`, `src/bar_manager.c`, `src/display.c`, `src/display_nsscreen.m`,
`src/window.c`, `src/workspace.m`, `src/app_windows.c`, `src/layer.m`, `src/surface.c`,
`src/context.c`, plus the parts of `message.c`, `event.c`, `sketchybar.c`, `background.c`,
`bar_item.c`, `group.c`, `popup.c`, `animation.c`, `misc/helpers.h` that the bar depends on.

Citations use `file:function`. "Quirk" marks behaviour that looks unintended but is what the C code
does; a 1:1 re-implementation must reproduce it unless the project decides otherwise.

## 0. Conventions and coordinate systems

| Concept | Meaning |
|---|---|
| `did` | CoreGraphics `CGDirectDisplayID`. |
| `adid` | "Arrangement display id": 1-based index of the display's UUID in `SLSCopyManagedDisplays(cid)`. 0 = unknown. With exactly one active display it is always 1 (`display.c:display_arrangement`, `display_active_display_adid`). |
| `dsid` | 64-bit SkyLight space id (`SLSManagedDisplayGetCurrentSpace`). |
| `sid` | "Mission-control index": 1-based position of a `dsid` when walking `SLSCopyManagedDisplaySpaces(cid)` (all displays in array order, all spaces of each display in order, fullscreen spaces included). 0 = not found (`misc/helpers.h:mission_control_index`). |
| Screen coords | CG global coordinates in points: origin at top-left of the main display, y grows downward. All window origins (`window.origin`) and `CGDisplayBounds` use these. |
| Drawing coords | Each window draws into a `CGBitmapContext` (`context.c:context_create`) whose CTM is scaled by 2.0 and is NOT flipped: origin bottom-left, y grows upward, units = points. |
| `g_nirvana` | `{-9999, -9999}` (`misc/helpers.h`). Windows that must be invisible are moved there instead of being ordered out. |
| `min`/`max` | Macros `(a < b ? a : b)` / `(a > b ? a : b)` with C usual-arithmetic conversions (unsigned wrap matters, see §4). |

Integer parsing used by `--bar`: `token_to_int` = `(int) strtol(s, NULL, 0)` (base 0: `0x..` hex, leading `0` octal,
so `08` parses as `0`; garbage parses as 0; empty value parses as 0). `token_to_uint32t` = `strtoul(s, NULL, 0)`.
Floats: `strtof`.

Boolean parsing (`misc/helpers.h:evaluate_boolean_state(token, previous)`): exact, case-sensitive match of
`on`, `yes`, `true`, `1`, `!off`, `!no`, `!false`, `!0` → true; `toggle` → `!previous`; anything else (including
empty, `off`, `ON`) → false.

## 1. Global state (`struct bar_manager g_bar_manager`)

Initialised by `bar_manager.c:bar_manager_init`, then `bar_manager_begin` creates the bars.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `clock` | `CFRunLoopTimerRef` | 1 s repeating timer, first fire at now+1 s, added to main run loop in `kCFRunLoopCommonModes` | Posts `SHELL_REFRESH` (→ `bar_manager_update(false)`). |
| `frozen` | bool | false | When true `bar_manager_refresh` is a no-op and `bar_manager_update(false)` returns early. Not a counter (see §8.4). |
| `sleeps` | bool | false | Set on system-will-sleep; `bar_manager_update` returns early while true. |
| `shadow` | bool | **false** | Bar window system shadow. |
| `topmost` | bool | false | Bar above other windows; also disables menu-bar avoidance. |
| `sticky` | bool | **true** | Windows live in a private "all spaces" space. |
| `font_smoothing` | bool | false | `CGContextSetAllowsFontSmoothing` for newly created bitmap contexts. |
| `any_bar_hidden` | bool | false | Result of the last `hidden=` command (see §2). |
| `needs_ordering` | bool | false | Re-run z-ordering of item windows on next redraw. |
| `might_need_clipping` | bool | false | Becomes true (forever) once any `background.clip` is set on any item. |
| `bar_needs_update` | bool | false | Bar background must be redrawn and every bar re-laid-out. |
| `bar_needs_resize` | bool | uninitialised (0 from static storage) | Bar window frames must be recomputed. |
| `show_in_fullscreen` | bool | false | Show bar on native fullscreen spaces. |
| `displays` | uint32 | `DISPLAY_ALL_PATTERN` = `UINT32_MAX` | Bit `n-1` set ⇒ bar on arrangement display `n`; 0 = `DISPLAY_MAIN_PATTERN`. |
| `position` | char | `'t'` | `'t'` top, `'b'` bottom, `'l'` left, `'r'` right; any other char behaves like top. |
| `margin` | int | 0 | Horizontal inset (top/bottom bar) or x-distance from screen edge (left/right bar). |
| `blur_radius` | uint32 | 0 | Window background blur of bar windows. |
| `notch_width` | uint32 | **200** | Gap reserved around screen centre on built-in displays for `q`/`e` items. |
| `notch_offset` | uint32 | 0 | Extra y offset on built-in displays. |
| `notch_display_height` | uint32 | 0 | If >0, bar window height on built-in displays. |
| `active_adid` | uint32 | `display_active_display_adid()` | adid of the "active" display (§6.3). |
| `window_level` | uint32 | `kCGBackstopMenuLevel` (= -20) | Level of bar and item windows. |
| `bars` / `bar_count` | `struct bar**` / uint32 | NULL / 0 | One bar per selected display. |
| `active_displays` | uint32 | – | Bitmask `1 << bar->adid` over all bars (written in `bar_manager_begin`, never read elsewhere). |
| `bar_items` / `bar_item_count` | array | NULL / 0 | Global ordered item list; order = drawing/layout order. |
| `default_item` | `struct bar_item` | `bar_item_init(.., NULL)`, name `"defaults"` | Template for `--default`. |
| `background` | `struct background` | see below | The bar's own background (colour, border, height, paddings, offsets). |
| `custom_events` | – | `custom_events_init` | Event registry. |
| `animator` | `struct animator` | `animator_init` (creates and starts a CVDisplayLink immediately) | §8.5. |
| `current_artwork` | `struct image` | `image_init` | Media cover. |

`g_bar_manager.background` defaults (`background_init` then overrides in `bar_manager_init`):

| Field | Default | Notes |
|---|---|---|
| `enabled` | **false** | Only affects query output; `bar_draw` forces it true on a copy (§5.1). |
| `bounds.size.height` | **25** | This *is* the bar height (`--bar height`). |
| `overrides_height` | true | `height` reported in query only if true. |
| `padding_left` / `padding_right` | **20 / 20** | Inner padding before first left item / after last right item. |
| `color` | `0x44000000` | Set via `color_set_hex` directly (does not flip `enabled`). |
| `border_color` | `0xffff0000` | |
| `border_width` | 0 | |
| `corner_radius` | 0 | |
| `x_offset` / `y_offset` | 0 / 0 | `y_offset` is the bar's `--bar y_offset`. |
| `clip` | 0.0 | Cannot be set for the bar. |
| `shadow` | `shadow_init` (disabled, angle 30, distance 5, color `0xff000000`) | Parsed but never drawn for the bar. |
| `image` | `image_init` (disabled, scale 1.0, path NULL) | |

Per-bar state (`bar.h:struct bar`, created by `bar.c:bar_create(did)`):

| Field | Init | Meaning |
|---|---|---|
| `did` | arg | CG display id. |
| `adid` | set by caller after `bar_create` | Arrangement index. |
| `dsid` | `display_space_id(did)` | Current space id of that display. |
| `sid` | `mission_control_index(dsid)` | Current mission-control index. |
| `shown` | `SLSSpaceGetType(cid, dsid) != 4` | False on fullscreen spaces. **Quirk:** `show_in_fullscreen` is ignored here; only `bar_manager_handle_space_change` honours it. |
| `hidden` | false | `--bar hidden`. |
| `mouse_over` | false | For `mouse.entered.global` / `mouse.exited.global`. |
| `window` | `bar_create_window` | The bar background window (§3). |

`bar_create` also sets `g_bar_manager.bar_needs_update = true`.

## 2. `--bar` command

### 2.1 Tokenisation (`message.c:handle_message_mach`)

Client argv is sent as NUL-separated tokens. For domain `--bar`, each following token is processed until the
message ends or the *next* token starts with `-`:

1. Token must contain `=`; it is split at the first `=` (`get_key_value_pair`). A token without `=` → response
   `[!] Bar: Expected <key>=<value> pair, but got: '<token>'\n` and the `--bar` loop **breaks**; the next token is
   then interpreted as a new domain (usually yielding `[!] Unknown domain '...'`).
2. `key=` with empty value → value is the empty string (numeric 0, boolean false).
3. `handle_domain_bar(key, value)` is called; its boolean result is OR-ed into `bar_needs_refresh`.
4. Loop stops before a token whose first char is `-` (so values may start with `-`, e.g. `y_offset=-5`, but a
   *key* can never start with `-`).

Values are applied immediately and sequentially; resets (`display`, `shadow`, `sticky`, `topmost`) destroy and
recreate all windows in the middle of the message.

At the end of the whole message (all domains): if `bar_needs_refresh` then (if `bar_needs_resize`
`bar_manager_resize()`) and `bar_needs_update = true`. Then `animator_lock`, `bar_manager_unfreeze`,
`bar_manager_refresh(false)`; the response buffer is returned to the client. The manager is frozen for the whole
message (`bar_manager_freeze` at start), so intermediate refreshes are suppressed (but see §8.4).

### 2.2 Property table (`message.c:handle_domain_bar`)

Checked in this order; first match wins. "Anim" = goes through the `ANIMATE` macro (§8.5) and can be animated
with `--animate`. "Refresh" = what the setter returns (true ⇒ `bar_needs_update` at end of message).
When animated (`--animate` duration > 0) the setter is not called synchronously and the property contributes
`false` to refresh; the animator drives refresh itself.

| Key | Value syntax / type | Default | Anim | Setter / effect | Refresh |
|---|---|---|---|---|---|
| `margin` | int (`strtol` base 0) | 0 | yes | `bar_manager_set_margin`: if changed → `margin = v`, `bar_needs_resize = true`. | true iff changed |
| `y_offset` | int | 0 | yes | `bar_manager_set_y_offset`: writes `background.y_offset`, `bar_needs_resize = true`. | true iff changed |
| `blur_radius` | int → uint32 | 0 | yes | `bar_manager_set_background_blur`: if changed, store and call `window_set_blur_radius` on every bar window immediately. | **always false** |
| `font_smoothing` | bool | off | no | `bar_manager_set_font_smoothing`: store. Only affects bitmap contexts created later (window creation/resize), **quirk**: existing windows keep old setting. | true iff changed |
| `shadow` | bool | off | no | `bar_manager_set_shadow`: if changed → store and `bar_manager_reset` (recreate all windows). | true iff changed |
| `notch_width` | int → uint32 | 200 | yes | `bar_manager_set_notch_width`: store (no resize flag). | true iff changed |
| `notch_offset` | int → uint32 | 0 | yes | `bar_manager_set_notch_offset`: store, `bar_needs_resize = true`. | true iff changed |
| `notch_display_height` | int → uint32 | 0 | yes | `bar_manager_set_notch_display_height`: store, `bar_needs_resize = true`. | true iff changed |
| `hidden` | bool \| `current` | off | no | See §2.3. | always true (unless `current` fails) |
| `topmost` | `window` \| bool | off | no | See §2.4. | always true |
| `sticky` | bool | **on** | no | `bar_manager_set_sticky`: if changed → store and `bar_manager_reset`. | true iff changed |
| `display` | `main` \| `all` \| comma list of 1-based arrangement indices | `all` | no | See §2.5; `bar_manager_set_displays` → `bar_manager_reset` if changed. | true iff changed |
| `position` | string; **only first char used**, no validation | `top` | no | `bar_manager_set_position(value[0])`: if changed → `bar_needs_resize = true`. Empty value → ignored. `top`→`t`, `bottom`→`b`, `left`→`l`, `right`→`r`; e.g. `center`→`c` behaves as top. | true iff changed |
| `clip` | – | – | – | Rejected: response `[!] Bar: Invalid property 'clip'\n`. | false |
| `height` | int (cast to uint32) | 25 | yes | `bar_manager_set_bar_height`: `bar_needs_resize |= background_set_height(&background, h)`; `background_set_height` sets `bounds.size.height = h`, `overrides_height = (h != 0)`. | returns current `bar_needs_resize` (may be true even if height unchanged) |
| `show_in_fullscreen` | bool | off | no | `bar_manager_set_show_in_fullscreen`: store. **Quirk:** `bar->shown` is only recomputed on the next space change / forced update. | true iff changed |
| *anything else* | – | – | – | Forwarded to `background.c:background_parse_sub_domain(&g_bar_manager.background, key, value)` (§2.6). | as returned |

### 2.3 `hidden`

`message.c:handle_domain_bar` + `bar_manager.c:bar_manager_set_hidden(adid, hidden)` + `bar.c:bar_set_hidden`.

* `hidden=current`: `adid = display_active_display_adid()`. If `0 < adid <= bar_count`:
  `bar_manager_set_hidden(adid, !bars[adid-1]->hidden)`; else prints `No bar on display <adid> \n` to stdout
  (not to the client) and nothing changes. **Quirk:** the bar is looked up by array index `adid-1`, which is
  only correct when every display has a bar; with `display=2` the single bar sits at index 0 and `current`
  either targets the wrong bar or fails.
* otherwise: `bar_manager_set_hidden(0, evaluate_boolean_state(value, any_bar_hidden))` (toggle is relative to
  `any_bar_hidden`).

`bar_manager_set_hidden(adid, hidden)`:
1. `any_bar_hidden = false`.
2. `adid > 0`: `bar_set_hidden(bars[adid-1], hidden)`; `any_bar_hidden |= hidden`. Else for every bar
   `bar_set_hidden(bar, hidden)` and `any_bar_hidden |= hidden`. (So after a per-display toggle,
   `any_bar_hidden` only reflects that one bar.)
3. If `hidden`: `popup_set_drawing(&item->popup, false)` for every item (closes all popups).
4. `bar_needs_update = true`; return true.

`bar_set_hidden(bar, hidden)`: no-op if unchanged; else store; hidden → `window_move(&bar->window, g_nirvana)`;
visible → `bar_resize(bar)`. Item windows are moved to nirvana by the next `bar_draw` because
`bar_draws_item` is false for hidden bars.

`any_bar_hidden` is also consulted by `bar_manager_begin` (all/pattern mode only): every newly created bar is
hidden if it is true. In `display=main` mode it is **not** applied (quirk) although query still reports it.

### 2.4 `topmost`

`bar_manager_set_topmost(level, topmost)`:

| Value | `level` arg | `topmost` | `window_level` |
|---|---|---|---|
| `window` | `'w'` | true | `kCGFloatingWindowLevel` (3) |
| bool true (`on`, …; `toggle` from off) | `'a'` | true | `kCGStatusWindowLevel` (25) |
| bool false | `'a'` | false | `kCGBackstopMenuLevel` (-20) |

`toggle` evaluates against `topmost` and, if it turns on, always selects `'a'` (status level).
Steps: set `window_level`; `bar_manager_reset()`; **then** `topmost = topmost`; return true. **Quirk:** the
reset (window creation, `bar_get_frame`) runs with the *old* `topmost` value, so the menu-bar offset of the new
windows is stale until the next resize (`bar_needs_resize` is not set). Always resets, even if unchanged.

### 2.5 `display`

`token_split(value, ',')`; pattern starts at 0; for each element in order:
`all` → pattern = `UINT32_MAX`; `main` → pattern = 0; else pattern |= `1 << (strtoul(elem, NULL, 0) - 1)`.
Consequences: `main,2` = display 2 only; `2,main` = main; `0` or non-numeric → `strtoul` gives 0, shift count `0 - 1` (unsigned long)
(undefined; on x86-64/arm64 the 32-bit shift count is masked to 31 → bit 31). Empty value → pattern 0 = main.
If pattern differs from current → `bar_manager_reset`.

### 2.6 Background passthrough (`background.c:background_parse_sub_domain`)

Applies to `g_bar_manager.background`; same parser as item backgrounds.

| Key | Type | Anim | Effect on the bar |
|---|---|---|---|
| `drawing` | bool | no | `background_set_enabled`. **No visual effect** for the bar (bar_draw forces enabled). Changes query `drawing`. |
| `color` | int (ARGB hex, e.g. `0xff1e1e2e`) | yes (per-byte) | `background_set_color`: also sets `enabled = true`. Fill colour. |
| `border_color` | int ARGB | yes (per-byte) | Stroke colour. |
| `border_width` | int → uint32 | yes | Stroke width; also reduces default item background height (§4.4). |
| `corner_radius` | int → uint32 | yes | Rounded-rect radius (clamped, §5.1). |
| `padding_left` / `padding_right` | int | yes | Layout start/end insets (§4). |
| `x_offset` | int | yes | Shifts the drawn background rect inside the bar window (window does not move). |
| `height` / `y_offset` / `clip` | – | – | Never reached (intercepted above). |
| `image` | path string, `app.<bundle id or app name>`, `space.<sid>` or `media.artwork` (see image spec) | no | Background image, drawn at the bar window's bottom-left with its natural size × scale (bounds are never laid out for the bar). |
| `image.<prop>` | see image spec | | Image sub-properties (`drawing`, `scale`, `corner_radius`, `border_width`, `border_color`, `padding_*`, `y_offset`, `string`, …). |
| `shadow.<prop>` | see shadow spec | | Parsed and stored; **never drawn** for the bar (bar_draw disables shadow). |
| `color.<prop>`, `border_color.<prop>` | `alpha`/`red`/`green`/`blue` floats, `hex` | | Colour channel setters. |
| other `<a>.<b>` | | | `[!] Background: Invalid subdomain '<a>'\n` |
| other | | | `[!] Background: Invalid property '<key>'\n` |

Note: the plain key `shadow` is intercepted by §2.2 (window shadow); only dotted `shadow.*` reaches the background.

## 3. Window model

### 3.1 Which windows exist

* One **bar window** per bar (`bar->window`, embedded struct), drawn with the bar background only.
* One **item window per item per bar**: `bar_item->windows[adid-1]`, created lazily by
  `bar_item.c:bar_item_get_window(item, adid)` the first time anything (layout, draw, ordering, group
  computation) asks for it. `bar_draw` iterates *all* items (popup members included) on every redrawn bar and
  `bar_order_item_windows` all non-popup items, so in practice every item owns a window on every bar, parked
  at `g_nirvana` where it is not drawn. New windows are opened at
  `{g_nirvana, 1×1}`, get `window_disable_shadow` unless `item.shadow`, `window_set_blur_radius(item.blur_radius)`,
  and set `needs_ordering` (`parent->popup.needs_ordering` for popup members, else `g_bar_manager.needs_ordering`).
  `windows[]` is grown with `realloc` to `adid` entries (NULL-filled).
* One **popup window** per item with an open popup (`popup.window`, opened in `popup.c:popup_create_window`
  at the anchor with the popup background size; shadow disabled; popup blur). Popup member items use their own
  per-adid item windows (the popup's `adid`).
* Brackets (type `bracket`, `BAR_COMPONENT_GROUP`) also own a per-bar item window that spans their members.

Invisible items are never ordered out; their window is moved to `g_nirvana` (-9999,-9999).

### 3.2 `struct window` (`window.h`)

| Field | Meaning |
|---|---|
| `id` | SkyLight window id (0 = closed). |
| `origin` | Screen position (CG global, top-left). |
| `frame` | Always origin (0,0); `frame.size` = window size in points. |
| `needs_move` / `needs_resize` | Pending changes set by `window_set_frame`, applied by `window_apply_frame`. |
| `parent`, `order_mode` | Last ordering relation (used only on macOS < 13 when reshaping). |
| `refc` | Reference count; `window_apply_frame` bumps it until its deferred update ran (§3.5). |
| `context` | `SLWindowContextCreate` context (only used to clear the window's own backing store). |
| `surface` | `struct surface` (§3.4) – the bitmap context everything is drawn into. |

### 3.3 Creating a window (`window.c:window_open(window, frame)`)

1. `window.origin = frame.origin`; `window.frame = {0,0,frame.size}`.
2. `CGSNewRegionWithRect(&{0,0,w,h}, &region)`; `CGRegionCreateEmptyRegion()` for the opaque shape.
3. `SLSNewWindowWithOpaqueShapeAndContext(cid, kCGBackingStoreBuffered /*2*/, region, empty_region,
   options = 13 | (1 << 18) /*0x4000D*/, &set_tags, x, y, 64 /*tag bits*/, &wid, NULL)` with
   `set_tags = kCGSExposeFadeTagBit (1<<1) | kCGSPreventsActivationTagBit (1<<16)`.
4. `SLSSetWindowResolution(cid, wid, 2.0)`; `SLSSetWindowTags(cid, wid, &set_tags, 64)`;
   `SLSClearWindowTags(cid, wid, &0, 64)`; `SLSSetWindowOpacity(cid, wid, false)` (non-opaque).
5. `window.context = SLWindowContextCreate(cid, wid, NULL)`; clear it (`CGContextClearRect` + `CGContextFlush`);
   `CGContextSetInterpolationQuality(kCGInterpolationNone)`.
6. `window.surface = surface_create(window)` (§3.4).
7. **Sticky** (`g_bar_manager.sticky`): on the first sticky window ever, create a global space
   `g_space = SLSSpaceCreate(cid, 1, 0)`, `SLSSpaceSetAbsoluteLevel(cid, g_space, 0)`,
   `SLSShowSpaces(cid, [g_space])` (CFArray of one SInt32). Then for every window:
   `SLSSpaceAddWindowsAndRemoveFromSpaces(cid, g_space, [wid], 0x7)`. The window therefore appears on all
   spaces of all displays. `g_space` is never destroyed (survives `sticky=off`, resets and hotload).
   Non-sticky windows are left on whatever space SkyLight assigns and are moved explicitly on space change
   (§6.4).

Bar window: `bar.c:bar_create_window` = `window_init` + `window_open(bar_get_frame(bar))` +
`window_assign_mouse_tracking_area(frame)` + `window_set_blur_radius(g_bar_manager.blur_radius)` +
(`!g_bar_manager.shadow` → `window_disable_shadow`).

Other per-window operations:

| Function | Implementation |
|---|---|
| `window_disable_shadow` | `SLSWindowSetShadowProperties(wid, {"com.apple.WindowShadowDensity": CFNumber(0)})`. There is no "enable"; shadow changes recreate windows. |
| `window_set_blur_radius(r)` | `SLSSetWindowBackgroundBlurRadius(cid, wid, r)`; then clear `window.context` if present. |
| `window_assign_mouse_tracking_area(rect)` | `SLSRemoveAllTrackingAreas(cid, wid)`; `SLSAddTrackingRect(cid, wid, rect)` with `rect = window.frame` (local, origin 0). Enables Carbon `kEventMouseEntered/Exited`. |
| `window_set_level(level)` | `windows_freeze()`; macOS ≥ 14: `SLSTransactionSetWindowLevel(tx, wid, level)`; else `SLSSetWindowLevel(cid, wid, level)`. |
| `window_order(w, parent, mode)` | `windows_freeze()`; store `parent`, store `mode` unless `W_OUT`; macOS ≥ 14: `SLSTransactionOrderWindow(tx, wid, mode, parent ? parent.id : 0)`; else `SLSOrderWindow(cid, …)`. `W_ABOVE = 1`, `W_OUT = 0`, `W_BELOW = -1`. |
| `window_move(p)` | `origin = p`; macOS ≥ 12: `windows_freeze()` + `SLSTransactionMoveWindowWithGroup(tx, wid, p)`; else `SLSMoveWindow(cid, wid, &p)` + `SLSReassociateWindowsSpacesByGeometry(cid, [wid])`. |
| `window_send_to_space(dsid)` | `SLSMoveWindowsToManagedSpace(cid, [wid] (SInt32), dsid)`; if `origin == g_nirvana` also `SLSMoveWindow(cid, wid, &g_nirvana)`. |
| `window_close` | if `id`: `SLSOrderWindow(cid, wid, 0, 0)` (order out), `surface_destroy`, `CGContextRelease(context)`, `SLSReleaseWindow(cid, wid)`, zero all fields. |
| `window_destroy` (heap windows) | `--refc`; if still > 0 return (deferred update pending, it will destroy); else close + free. |
| `window_flush` | `surface_flush(surface)`. |
| `window_capture` (aliases) | `SLSCaptureWindowsContentsToRectWithOptions(cid, &wid64, true, CGRectNull, 1<<8, &img)` + `SLSGetScreenRectForWindow`; disabled while `g_disable_capture` (§9). |

### 3.4 Surface / layer pipeline (`surface.c`, `layer.m`, `context.c`)

Content is not drawn into the SkyLight window context; it is drawn into an offscreen bitmap and pushed
through a CoreAnimation remote layer bound to the window as a surface:

`surface_create(window)`:
1. `layer_create(cid, window.frame)`: `[CALayer layer]` with `anchorPoint = (0,0)`, `position = (0,0)`,
   `bounds = frame`, `contentsScale = 2.0`, `opaque = NO`, `geometryFlipped = NO`, `masksToBounds = NO`;
   `[CAContext contextWithCGSConnection:cid options:nil]` (private), `context.layer = calayer`.
2. `SLSAddSurface(cid, wid, &surface_id)` (fail → destroy, return NULL).
3. `SLSBindSurface(cid, wid, surface_id, 0x4, 0, ca_context.contextId)`.
4. `SLSSetSurfaceBounds(cid, wid, sid, window.frame)`; `SLSSetSurfaceResolution(…, 2.0)`;
   `SLSSetSurfaceOpacity(…, false)`; `SLSSetSurfaceColorSpace(…, CGColorSpaceCreateDeviceRGB())`;
   `SLSOrderSurface(cid, wid, sid, W_ABOVE, 0)`; `SLSFlushSurface(cid, wid, sid, 0)`.
5. `surface.context = context_create(window.frame.size, 2.0)`.

`context_create(size, scale=2.0)`: pixel size `ceil(w*2) × ceil(h*2)`; returns NULL if either is 0 (zero-sized
windows draw into NULL, i.e. nothing). `CGBitmapContextCreate(NULL, pw, ph, 8, pw*4, DeviceRGB,
kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Host)`, `CGContextScaleCTM(2, 2)`,
interpolation none, `CGContextSetAllowsFontSmoothing(g_bar_manager.font_smoothing)`. The scale is always 2.0,
independent of the display's backing scale.

`surface_flush`: `CGBitmapContextCreateImage(context)` → `layer.contents = image` (inside
`[CATransaction setDisableActions:YES]`, `contentsScale = 2.0`) → `SLSFlushSurface(cid, wid, sid, 0)`.

`surface_resize(surface, window)`: release + recreate the bitmap context at the new size (new contexts pick up
the current `font_smoothing`); macOS ≥ 26: `SLSTransactionSetSurfaceBounds(tx, wid, sid, frame)` (layer bounds
deferred, §3.5); else `SLSSetSurfaceBounds(cid, …)` + `layer_set_bounds(layer, frame)`
(`setDisableActions:YES`, `bounds = frame`, `position = 0`).

`surface_destroy`: `SLSRemoveSurface(cid, wid, sid)`, `layer_destroy` (`context.layer = nil`,
`[context invalidate]`, release), `CGContextRelease`.

### 3.5 Geometry changes (`window.c:window_set_frame`, `window_apply_frame`)

`window_set_frame(w, frame)` only records: if `needs_move || origin != frame.origin` → `needs_move = true`,
`origin = frame.origin`; if `needs_resize || size != frame.size` → `needs_resize = true`, `frame.size = frame.size`.

`window_apply_frame(w, forced) -> bool resized`:
* If `needs_resize || forced`:
  1. `windows_freeze()`; build `region = CGSNewRegionWithRect({0,0,size})`; `refc++`.
  2. macOS ≥ 26: `SLSTransactionSetWindowShape(tx, wid, origin.x, origin.y, region)`; `window_move(origin)`;
     if `SLSTransactionAddPostDecodeAction` resolved (dlsym, §9) register a block calling
     `window_defer_update(w)`, else call `window_defer_update(w)` directly.
  3. macOS 13–25: `SLSSetWindowShape(cid, wid, g_nirvana.x, g_nirvana.y, region)`; clear `window.context`;
     `window_move(origin)`; `window_defer_update(w)`.
  4. macOS < 13: if `parent`: `SLSOrderWindow(cid, wid, 0, parent.id)` (order out); `SLSSetWindowShape(cid, wid,
     0, 0, region)`; clear; if `parent`: `window_order(w, parent, order_mode)`; `window_move(origin)`;
     `window_defer_update(w)`.
  5. `surface_resize`; clear both flags; return **true**.
* Else if `needs_move`: `window_move(origin)`; clear flag; return false.
* Else return false.

`window_defer_update(w)`: block = { if `!surface` return; if `--refc <= 0` `window_destroy(w)` else
`layer_set_bounds(surface.layer, frame)` + `window_flush(w)` }. On the main thread it is
`dispatch_async(main_queue)` (runs after the current event and its transaction commit); off-main
`dispatch_sync(main_queue)`. Hence after a resize the new content becomes visible one run-loop turn later, in
sync with the new shape. That is why `bar_draw` skips `window_flush` when `resized` (§5.2).

### 3.6 SkyLight transactions (`window.c:windows_freeze/unfreeze`)

* `windows_freeze()`: if no `g_transaction`: (macOS < 26) `SLSDisableUpdate(cid)`; `g_transaction =
  SLSTransactionCreate(cid)`. Called implicitly by every move/level/order/reshape.
* `windows_unfreeze()`: if a transaction exists: `SLSTransactionCommit(tx, 0)` (synchronous), release, NULL;
  (macOS < 26) `SLSReenableUpdate(cid)`.
* `event.c:event_execute` calls `windows_unfreeze()` after **every** event handler, and `main` wraps the initial
  `bar_manager_begin` in freeze/unfreeze. So all window geometry/level/order changes caused by one event are
  committed atomically.

### 3.7 Window levels and z-order

| Window | Level |
|---|---|
| Bar + all bar item windows | `g_bar_manager.window_level`: `kCGBackstopMenuLevel` -20 (default, below normal windows), `kCGFloatingWindowLevel` 3 (`topmost=window`), `kCGStatusWindowLevel` 25 (`topmost=on`). |
| Popup window + popup member windows | `popup.topmost` (default true) ? `kCGPopUpMenuWindowLevel` 101 : `kCGBackstopMenuLevel + 1` (-19) (`popup.c:popup_order_windows`). |

`bar.c:bar_order_item_windows(bar)` (only if `bar.sid >= 1 && bar.adid >= 1 && bar.shown`):
1. `window_set_level(bar.window, window_level)`; `window_order(bar.window, NULL, W_ABOVE)` (front of level).
2. `previous = NULL; first = NULL`. For each item in `bar_items` order, skipping `position == 'p'` (popup):
   * `w = bar_item_get_window(item, bar.adid)` (creates if needed); `window_set_level(w, window_level)`.
   * `if (!first) first = w`.
   * Bracket: `window_order(w, first, W_BELOW)` (since `first` is always set at this point, the `else` branch is
     dead; if the bracket is itself the first item it is ordered below itself); `continue` (brackets do not
     become `previous`).
   * Else: `window_order(w, previous ? previous : bar.window, W_ABOVE)`; `previous = w`.

Result: bar window at the bottom, items stacked in list order (later items above earlier), each bracket just
below the first item window.

Windows created by `window_open` are never explicitly ordered in there; they become visible only when
`bar_order_item_windows` (or `popup_order_windows` for popups) orders them `W_ABOVE`. That is why window
creation, `bar_manager_begin`, item add/move/reorder and a bar becoming `shown` all set `needs_ordering`.
Ordering is skipped for bars that are not `shown`, so their (re)created windows stay invisible until shown. Ordering runs from `bar_manager_refresh` only for bars that are redrawn while
`needs_ordering` is set.

## 4. Geometry and layout

### 4.1 Bar window frame (`bar.c:bar_get_frame`)

Inputs: `B = CGDisplayBounds(bar.did)` (points, global), `H = background.bounds.size.height` (bar height /
thickness), `M = margin`, `Y = background.y_offset`, `builtin = CGDisplayIsBuiltin(did)`,
`NO = builtin ? notch_offset : 0`, `NDH = builtin ? notch_display_height : 0`,
`menu_visible = display_menu_bar_visible()`, `MH = display_menu_bar_rect(did).size.height` (§6.2).

Horizontal bar (`position` not `l`/`r`):
```
x = B.x + M
w = B.w - 2*M
if position == 'b':
    y = B.y + B.h - H - 2*Y - NO          # CGRectGetMaxY(B) - H - 2*Y - NO   (quirk: 2*Y, and uses H even if NDH > 0)
else:
    y = B.y + Y + NO
    if menu_visible && !topmost: y += MH
h = NDH > 0 ? notch_display_height : H   # NDH>0 implies builtin
frame = {x, y, w, h}
```
Vertical bar (`position == 'l'` or `'r'`; notch values ignored):
```
h = B.h - 2*Y
x = B.x + (position == 'r' ? B.w - H - M : M)
y = B.y + Y
if menu_visible && !topmost: y += MH; h -= MH
frame = {x, y, H, h}                     # width = bar height ("thickness")
```

`bar.c:bar_resize(bar)`: if `bar.hidden || !bar.shown` → `window_move(bar.window, g_nirvana)` and return.
Else `window_set_frame(bar.window, bar_get_frame(bar))`; if `window_apply_frame(bar.window, false)` returned
true (size changed) → `window_assign_mouse_tracking_area(frame)` and `bar_needs_update = true`.
`bar_manager_resize` calls `bar_resize` for every bar and clears `bar_needs_resize`.

### 4.2 Item eligibility (`bar.c:bar_draws_item(bar, item)`)

Returns false if any of:
1. `!item.drawing || !bar.shown || bar.hidden`.
2. Display association, unless `item.ignore_association`:
   `(item.associated_display > 0 && !(item.associated_display & (1 << bar.adid)))`
   or `(item.associated_to_active_display && bar.adid != active_adid)`.
   (Display mask bit `n` = arrangement display `n`; set by `associated_display=1,2` / `display=…` as
   `1 << strtoul(n)`; `active` sets `associated_to_active_display`.)
3. Space association: `item.associated_space > 0 && !(item.associated_space & (1 << bar.sid)) &&
   !item.ignore_association && item.type != space`. (Space items are never filtered by space; their display
   mask is derived automatically, §6.5.) Masks are 32-bit: sids ≥ 32 cannot be represented (shift UB).
4. Popup members (`position == 'p'`): `!item.parent || !item.parent.popup.drawing || bar.adid != active_adid`.

### 4.3 Side lengths (`bar_manager.c:bar_manager_length_for_bar_side(bar, side)`)

Sum over items with `position == side`, `type != bracket`, `bar_draws_item(bar, item)`:
`len_i + (item.has_const_width ? 0 : item.padding_left + item.padding_right)` where
`len_i = vertical ? bar_item_get_height(item) : bar_item_get_length(item, false)`.
`bar_item_get_length(item, ignore_override)`: `content = max(icon_len + label_len + graph/slider/alias len, 0)`;
if background enabled with image enabled: `content = max(content, image_width)`; if `has_const_width &&
(!ignore_override || custom_width > content)` return `custom_width`, else `content`.
So `get_length(item, false)` = `custom_width` whenever a width is set; `get_length(item, true)` =
`max(custom_width, content)` when a width is set ("display length", the window never shrinks below content).
`width=<n>` sets `has_const_width`, `custom_width = n`; negative/`dynamic`-end clears `has_const_width`.

### 4.4 Horizontal layout (`bar.c:bar_calculate_bounds_top_bottom`)

Item positions (first char of the item's `position` value, `bar_item.c:bar_item_set_position`): `l` left,
`r` right, `c` center, `q` = `POSITION_CENTER_LEFT` (left of the notch), `e` = `POSITION_CENTER_RIGHT` (right of
the notch), `p` popup (`popup.<host>`). Other first chars are rejected.

All cursors are `uint32_t`; `W`, `H` are the bar window's `frame.size` (CGFloat).
```
notch = CGDisplayIsBuiltin(bar.did) ? notch_width : 0
center_len = bar_manager_length_for_bar_side(bar, 'c')
cur_l = max(bar.background.padding_left, 0)                 # 'l'
cur_r = (u32)(W - max(bar.background.padding_right, 0))     # 'r'
cur_c = (u32)((W - center_len) / 2)                         # 'c'  (double division, truncated)
cur_e = (u32)((W + notch) / 2)                              # 'e'  (right of notch, grows rightwards)
cur_q = (u32)((W - notch) / 2)                              # 'q'  (left of notch, grows leftwards)
y = (u32)(H / 2)

for item in bar_items (global order):
    skip if !bar_draws_item(bar,item) || item.type == bracket || item.position == 'p'
    disp_len = bar_item_get_length(item, true)
    pick cursor by item.position: 'l'→cur_l, 'c'→cur_c, 'r'→cur_r (rtl), 'e'→cur_e, 'q'→cur_q (rtl); other → skip
    if rtl:   # 'r' and 'q'
        cand = (u32)(cur - disp_len - (u32)item.padding_right)          # unsigned wrap!
        cur  = (double)cand < (W - disp_len) ? cand : (u32)(W - disp_len)
    else:
        cur  = cur + max(-(int)cur, item.padding_left)                  # == max(cur + padding_left, 0)
    item.graph.rtl = rtl
    sh = bar_item_calculate_shadow_offsets(item)        # sh.x = left overhang, sh.y = right overhang (both >= 0)
    item_len = bar_item_calculate_bounds(item,
                   bar_height = H - (bar.background.border_width + 1),
                   x = max(sh.x, 0),
                   y = y)                               # = bar_item_get_length(item, false)
    window_set_frame(window(item, bar.adid),
        { bar.origin.x + cur - max(sh.x,0), bar.origin.y,
          disp_len + sh.x + sh.y, H })
    if item.popup.drawing: bar_calculate_popup_anchor_for_bar_item(bar, item)   # §4.7
    if rtl:
        cur += has_const_width ? disp_len + padding_right - custom_width : -padding_left
    else:
        cur += has_const_width ? custom_width - padding_left : item_len + padding_right

for item in bar_items:                                  # second pass: brackets
    skip unless type == bracket && position != 'p' && bar_draws_item(bar, item)
    group_calculate_bounds(item.group, bar, y)          # §4.6
    window_set_frame(window(item.group.members[0] /*= bracket*/, bar.adid), item.group.bounds)
    if item.popup.drawing: bar_calculate_popup_anchor_for_bar_item(bar, item)
```
Effective semantics:
* `l` items flow left→right from `padding_left`; each occupies `padding_left + length + padding_right`
  (const width: exactly `custom_width`, which *includes* both paddings; the item is drawn at
  `start + padding_left`).
* `r` items flow right→left from `W - padding_right` in list order (first `r` item is rightmost).
* `c` items flow left→right starting so that the whole centre group (incl. paddings) is centred on `W/2`.
* `e` items flow left→right from `(W + notch)/2`; `q` items flow right→left from `(W - notch)/2`. On non-built-in
  displays both start at `W/2`. Nothing prevents overlaps between groups.
* Left cursor clamp: an item can never start at x < 0 (negative paddings are clamped at 0).
* Right clamp (quirk, emulate exactly): the candidate `cur - len - padding_right` is computed in `uint32`. If it
  would be negative it wraps to a huge value, so the `min` with `W - len` (a double) selects `W - len`, i.e.
  overflowing right items jump to the far right edge. The same `min` also stops a negative `padding_right` from
  pushing an item past the right edge. If `W - len < 0` the double→uint32 conversion is undefined (arm64:
  saturates to 0; x86-64: wraps modulo 2³²).
* Window frame: x is shifted left by the left shadow overhang and widened by both overhangs; window height is
  always the full bar window height (`notch_display_height` on notch displays); y is the bar's y. The item's
  internal drawing origin is `(max(sh.x,0), H/2)` in window-local, y-up coordinates.
* `bar_item_calculate_shadow_offsets` (bar_item.c): `x = Σ max(-s.offset.x, 0)` over enabled shadows of
  background, icon, icon.background, label.background, label, plus `max(-background.x_offset, 0)` if the item
  background is enabled; `y = Σ max(s.offset.x, 0)` over the same shadows plus `max(background.x_offset, 0)`
  (yes, `.y` is built from `offset.x`; both truncated to int).
* `bar_item_calculate_bounds(item, bar_height, x, y)` positions icon/label/graph/slider/alias/background inside
  the window (content x shifted by `(length - content)/2` for `align=center`, by `length - content` for
  `align=right`; item backgrounds without explicit height get `bar_height` = `H - border_width - 1`; graphs
  without enabled background likewise). Details belong to the item spec.

### 4.5 Vertical layout (`bar.c:bar_calculate_bounds_left_right`)

Used when `position` is `l` or `r`. Same loop structure, but along y (screen y-down) and with item heights:
```
notch = 0
center_len = bar_manager_length_for_bar_side(bar, 'c')     # uses bar_item_get_height
T = background.bounds.size.height                          # bar thickness (window width)
cur_l = max(padding_left, 0);  cur_r = (u32)(Hwin - max(padding_right, 0))
cur_c = (u32)((Hwin - 2*margin - center_len) / 2 - 1)      # quirk: subtracts 2*margin and 1
cur_e = cur_q = (u32)(Hwin / 2)
for item (same skips):
    ih = bar_item_get_height(item);  disp_len = bar_item_get_length(item, true)
    rtl for 'r','q'
    if rtl: cur = min((u32)(cur - ih - padding_right), Hwin - ih)   # same unsigned quirk
    else:   cur = max(cur + padding_left, 0)
    item.graph.rtl = rtl
    sh = shadow offsets
    bar_item_calculate_bounds(item, bar_height = ih,
                              x = (T - disp_len) / 2.0 + max(sh.x,0),   # converted to u32
                              y = ih / 2.0)
    window_set_frame(window, { bar.origin.x - max(sh.x,0),
                               bar.origin.y + cur - max(-item.y_offset, 0),
                               T, ih + abs(item.y_offset) })
    popup anchor if popup.drawing
    if rtl: cur += has_const_width ? ih + padding_right - custom_width : -padding_left
    else:   cur += has_const_width ? custom_width - padding_left : ih + padding_right
```
`Hwin` = bar window height. Items are centred horizontally in the bar thickness; "left" items start at the
top, "right" items stack upward from the bottom. **Quirks:** brackets are never laid out in vertical mode (no
second pass; their windows keep their last frame); the window width ignores shadow overhang (always `T`);
`const width` semantics are applied to heights; `bar_item_clip_bar` offset only accounts for x (§5.1).

### 4.6 Brackets (`group.c:group_calculate_bounds(group, bar, y)`)

`members[0]` is the bracket item itself; members `1..n-1` are the bracketed items.
* `first_item` = drawn member (`bar_draws_item`) with the smallest `window.origin.x`; `last_item` = drawn
  member with the largest `origin.x + frame.width` (strict `<`/`>`, earliest wins ties). NULL if the bracket
  has no other members or none is drawn.
* If either is NULL: `bounds.origin = g_nirvana` (size unchanged) and return.
* `len = max(last.origin.x + last.width + last_item.padding_right + first_item.padding_left - first.origin.x, 0)`.
* `sh = bar_item_calculate_shadow_offsets(bracket)`.
* `bounds = { first.origin.x - first_item.padding_left, first.origin.y, len + sh.x + sh.y, first.height }`.
* `background_calculate_bounds(&bracket.background, x = max(sh.x,0), y = y + bracket.y_offset, width = len,
  height = bracket.background.bounds.size.height)` (bracket background height must be set explicitly;
  otherwise 0).
Note member windows include their own shadow overhang, so bracket edges follow window edges, not content.

### 4.7 Popup anchor (`bar.c:bar_calculate_popup_anchor_for_bar_item`)

Only on the active display bar (`bar.adid == active_adid`). `w = item window on this bar`.
1. Unless `popup.overrides_cell_size`: `popup.cell_size = vertical ? w.width : w.height`.
2. `popup_calculate_bounds(popup, bar)`.
3. Anchor `a = w.origin`; with `PB = popup.background.bounds.size`:
   * Horizontal bar: `popup.align == 'c'`: `a.x += (w.width - PB.w)/2`; `popup.align == 'l'`:
     `a.x -= item.padding_left`; else (right): `a.x += w.width - PB.w`.
     `a.y += bar position == 'b' ? -PB.h : w.height`.
   * Vertical bar: `popup.align 'c'`: `a.y += (w.height - PB.h)/2`; `'l'`: `a.y -= item.padding_left`; else
     `a.y += w.height - PB.h`. `a.x += bar position == 'r' ? -PB.w : w.width`.
4. `popup_set_anchor(popup, a, bar.adid)` (adds `popup.y_offset`; if adid changed: `needs_ordering`, mark all
   members `needs_update`); `popup_calculate_bounds` again.

## 5. Drawing and the redraw cycle

### 5.1 `bar.c:bar_draw(bar, forced)`

Returns immediately if `bar.sid < 1 || bar.adid < 1` (unknown space/display ⇒ the bar is never drawn).
`forced` is always `false` at every call site.

1. If `might_need_clipping`: `bar_check_for_clip_updates(bar)` – only when `bar_needs_update` is still false:
   for each item, `w = item window on this bar`; if the item clips (`background.clip > 0 && enabled` on its
   background, icon.background or label.background):
   * not drawn on this bar but `w.origin != g_nirvana` → `bar_needs_update = true`, stop;
   * drawn → `bar_needs_update |= w.needs_move || w.needs_resize || clip geometry changed` (`bounds`,
     `corner_radius`, `x_offset`, `y_offset` differ from the per-adid clip snapshot); stop when true.
2. If `bar_needs_update` (bar background):
   ```
   bg = copy of g_bar_manager.background
   bg.bounds = bar.window.frame                 # {0,0,W,Hwin}
   bg.bounds.origin.y -= bg.y_offset            # cancels the +y_offset inside background_draw
   bg.shadow.enabled = false; bg.enabled = true
   CGContextClearRect(bar.surface.context, bar.window.frame)
   background_draw(bg, bar.surface.context)
   ```
   `background_draw`: skip if no visible content (`(border alpha == 0 || border_width == 0) && color alpha == 0
   && !shadow && !image`); rect = bounds shifted by `(x_offset, y_offset)`; `draw_rect`: line width
   `border_width`, stroke `border_color`, fill `color`, path = rounded rect of
   `CGRectInset(rect, bw/2, bw/2)` with radius `min(corner_radius, min(w,h)/2)` (radius clamped to half the
   smaller inset side), drawn with `kCGPathFillStroke`; then `image_draw` if the image is enabled.
3. For each item in `bar_items` order (`w = bar_item_get_window(item, bar.adid)`, creating if needed):
   * If `!bar_draws_item(bar, item)`: if `w.origin != g_nirvana` → `window_move(w, g_nirvana)`;
     `bar_item_remove_associated_bar(item, adid)` (clears bit `adid-1`); continue.
   * `bar_item_append_associated_bar(item, adid)` (sets bit `adid-1`).
   * If `item.popup.drawing && bar.adid == active_adid` → `popup_draw(popup)`.
   * If `bar_needs_update` → `bar_item_clip_bar(item, w.origin.x - bar.window.origin.x, bar)`: for each of
     item.background / icon.background / label.background that clips, store a snapshot (per adid) and punch a
     rounded rect (bounds + offset in x, + x_offset/y_offset) out of the bar context using
     `kCGBlendModeDestinationOut` with fill alpha = `clip`.
   * `resized = window_apply_frame(w, forced)`.
   * If `!resized && !item.needs_update` → continue (position-only changes are just moves).
   * If the item subscribes to `mouse.entered` or `mouse.exited` → `window_assign_mouse_tracking_area(w, w.frame)`.
   * `CGContextClearRect(w.surface.context, w.frame)`; `bar_item_draw(item, ctx)` (background, then for
     non-brackets icon, label, alias, graph, slider); `CGContextFlush`; if `!resized` → `window_flush(w)`
     (resized windows flush in the deferred update, §3.5).
4. If `bar_needs_update`: `CGContextFlush(bar ctx)`; `window_flush(bar.window)`.

Note the bar window itself is never re-applied here (its frame changes only in `bar_resize`).

### 5.2 `bar_manager.c:bar_manager_refresh(forced)` – the redraw entry point

```
if frozen: return
if forced:
    every item: associated_bar = 0; needs_update = true
if forced || bar_needs_resize: bar_manager_resize()
for bar in bars:
    if forced || bar_manager_bar_needs_redraw(bar):
        bar_calculate_bounds(bar)        # §4.4/4.5; returns early if sid<1 || adid<1
        bar_draw(bar, false)
        if needs_ordering: bar_order_item_windows(bar)
every item: needs_update = false
needs_ordering = false; bar_needs_update = false
```
Layout is always recomputed in full for a redrawn bar; there is no incremental layout. Layout results stored in
items (text bounds etc.) are per item, not per bar, which is fine because each bar is laid out and drawn
back-to-back.

`bar_manager_bar_needs_redraw(bar)` (mask `m = 1 << bar.adid`; `ab = item.associated_bar << 1`):
returns true if `bar_needs_update`, or for any item:
1. `item.needs_update && bar_draws_item(bar, item)`;
2. `!item.drawing && item.associated_bar != 0` (hidden item still shown on some bar);
3. (skip remaining checks if `ignore_association`)
4. drawn here, has display mask containing this bar, but `!(ab & m)` (should appear, not yet shown);
   or drawn here, `associated_to_active_display`, this is the active bar, `!(ab & m)`;
5. `!associated_to_active_display && display mask > 0 && !(mask & m) && (ab & m)` (shown on a bar it no longer
   belongs to); or `item.drawing && associated_to_active_display && (ab & m) && bar.adid != active_adid`;
6. (skip remaining for space items)
7. `associated_space > 0 && !(associated_space & (1 << bar.sid)) && (ab & m)` (shown on wrong space);
8. drawn here, `associated_space > 0 && (associated_space & (1 << bar.sid)) && !(ab & m)`.

So a space switch causes a redraw of a bar only if some item's visibility on it changes.

`bar_manager_resize()`: `bar_resize` for every bar; `bar_needs_resize = false`.

### 5.3 Who triggers a refresh

| Trigger | Call |
|---|---|
| End of every client message | `bar_manager_refresh(false)` (after optional resize + `bar_needs_update`, §2.1). |
| `--update` | `bar_manager_update(true)` → forced events + `bar_manager_refresh(true)`; also sets `bar_needs_refresh`. |
| `--remove` | sets `bar_needs_refresh`. `--reorder` calls `bar_manager_refresh(false)` directly. |
| `--trigger space_change` / `--trigger display_change` | `bar_manager_handle_space_change(true)` / `bar_manager_handle_display_change()` (real handlers, not just the custom event; extra `KEY=VALUE` args are ignored for these). |
| 1 s clock (`SHELL_REFRESH`) | `bar_manager_update(false)`: routine script updates; refresh only if an alias changed. |
| Animation frame | `bar_manager_animator_refresh` (§8.5). |
| Space change | `bar_manager_handle_space_change` → `bar_manager_refresh(force_refresh)` (§6.4). |
| Display add/remove/move/resize, wake | `bar_manager_display_changed` → reset + `bar_manager_refresh(true)` (§6.6). |
| Active display change | `bar_manager_handle_display_change` → `bar_manager_refresh(false)`. |
| Menu bar autohide toggled | `bar_manager_resize()`, `bar_needs_update = true`, `bar_manager_refresh(false)` (`event.c:event_menu_bar_hidden_changed`). |
| Mouse click / drag / scroll on an item | `bar_manager_refresh(false)` if the item got `needs_update`. |
| Media cover change | `bar_manager_refresh(false)` if a shown item links the artwork. |

There is no frame scheduling/coalescing beyond: (a) the manager is frozen during message processing and space
change handling so only one refresh runs at the end; (b) all SkyLight changes of one event are committed in a
single transaction (§3.6); (c) animations run at display-link rate.

## 6. Displays and spaces

### 6.1 Display enumeration (`display.c`)

| Function | Implementation |
|---|---|
| `display_active_display_count()` | `CGGetActiveDisplayList(0, NULL, &count)`. |
| `display_active_display_list(&count)` | `CGGetActiveDisplayList(n, buf, &count)`. |
| `display_main_display_id()` | `CGMainDisplayID()`. |
| `display_uuid(did)` | `CGDisplayCreateUUIDFromDisplayID(did)` → `CFUUIDCreateString`; NULL if no UUID. |
| `display_bounds(did)` | `CGDisplayBounds(did)`. |
| `display_space_id(did)` | `SLSManagedDisplayGetCurrentSpace(cid, uuid)`; 0 if no UUID. |
| `display_space_list(did, &n)` | Iterate `SLSCopyManagedDisplaySpaces(cid)`; for the dict whose `"Display Identifier"` equals the UUID, return all `"Spaces"[j]["id64"]`. |
| `display_arrangement(did)` | If exactly 1 active display: 1 if `did` is it, else 0. Else 1-based index of the UUID string in `SLSCopyManagedDisplays(cid)`; 0 if absent. |
| `display_arrangement_display_id(n)` | `CGDisplayGetDisplayIDFromUUID(CFUUIDCreateFromString(SLSCopyManagedDisplays(cid)[n-1]))`; 0 if out of range. |
| `display_active_display_id()` | 1 display: that one. Else UUID from `display_active_display_uuid()` → did. |
| `display_active_display_adid()` | 1 display: 1. Else index (1-based) of `display_active_display_uuid()` in `SLSCopyManagedDisplays`; 0 on failure. |
| `display_active_display_uuid()` | If `g_space_management_mode != 1` (1 = "Displays have separate Spaces" enabled, per the yabai convention; so: setting off): display whose `CGDisplayBounds` contains the cursor (`CGEventGetLocation(CGEventCreate(NULL))`), first match in active list; no match → `display_uuid(0)` (NULL). If mode == 1: `SLSCopyActiveMenuBarDisplayIdentifier(cid)`. |
| `display_menu_bar_visible()` | `SLSGetMenuBarAutohideEnabled(cid, &s)`; visible = `!s`. |
| `display_menu_bar_rect(did)` | see §6.2. |
| `display_begin()` | `CGDisplayRegisterReconfigurationCallback(display_handler, NULL)`. |

`g_space_management_mode = SLSGetSpaceManagementMode(cid)` once at startup.

Note `CGGetActiveDisplayList` (count, includes mirrors) and `SLSCopyManagedDisplays` (arrangement) can
disagree; an index without a managed display yields `did = 0`, whose bar has `dsid = 0`, `sid = 0` and is never
drawn.

### 6.2 Menu bar height (`display.c:display_menu_bar_rect`)

* x86-64: `SLSGetRevealedMenuBarBounds(&rect, cid, display_space_id(did))`.
* arm64: `inset = display_nsscreen_top_inset(did)`: find the `NSScreen` whose
  `deviceDescription["NSScreenNumber"] == did`; `inset = NSMaxY(frame) - NSMaxY(visibleFrame) - 1`, clamped to
  ≥ 0, rounded (`(int)(inset + 0.5)`); -1 if no screen matches. If `inset > 0` → height = inset. Else
  `notch = workspace_display_notch_height(did)` (built-in only, macOS ≥ 12: `NSScreen.safeAreaInsets.top`); if
  non-zero → height = `notch + 6`; else height = 24. Width = `CGDisplayPixelsWide(did)`. Only `.size.height` is
  used.

### 6.3 Active display

`active_adid` is refreshed in `bar_manager_begin`, `bar_manager_display_changed` and
`bar_manager_handle_display_change`. When `g_space_management_mode != 1`, `event.c:event_execute` calls
`bar_manager_poll_active_display` before **every** event: if `display_active_display_adid() != active_adid` →
`bar_manager_handle_display_change`. (The 1 s clock event guarantees polling at least once per second; any other event — e.g. a mouse event on a
bar window, a client message — polls too. There is no global mouse-move monitoring.) `NSWorkspaceActiveDisplayDidChangeNotification` also posts
`DISPLAY_CHANGED` → `bar_manager_handle_display_change`.

`bar_manager_handle_display_change`: `active_adid = display_active_display_adid()`; trigger custom event
`display_change` with `INFO` = active adid as decimal (buffer `char[3]`: at most 2 digits); then
`bar_manager_refresh(false)`. Active display matters for: `associated_display=active` items, popups (drawn only
on the active display's bar), `hidden=current`.

### 6.4 Space tracking (`bar_manager.c:bar_manager_handle_space_change(forced)`)

Triggered by `SPACE_CHANGED` (from `NSWorkspaceActiveSpaceDidChangeNotification` and SkyLight notifications
1327/1328 on macOS ≥ 13, `sketchybar.c:space_events`), by `--update`/`bar_manager_update(true)`, by `--trigger space_change` and by
`bar_manager_display_changed` (the notification path uses `forced = false`, the other three `forced = true`). Steps:
```
freeze manager
force_refresh = false
for i, bar in bars:
    dsid = display_space_id(bar.did)
    bar.sid = mission_control_index(dsid)
    was_shown = bar.shown
    bar.shown = SLSSpaceGetType(cid, dsid) != 4 || show_in_fullscreen       # 4 = fullscreen space
    needs_ordering |= !was_shown && bar.shown
    force_refresh |= was_shown != bar.shown
    if bar.dsid != dsid:
        bar.dsid = dsid
        if !sticky && bar.shown: bar_change_space(bar, dsid)
    append  "\t\"display-<adid>\": <sid>" + ("," unless last) + "\n"  to INFO
INFO = "{\n" + lines + "}"                    # no trailing newline
bar_manager_update_space_components(forced)    # §6.5
trigger custom event "space_change" with INFO
unfreeze manager
bar_manager_refresh(force_refresh)
```
A bar whose `shown` flipped is only moved (to its frame or to nirvana) by the forced refresh at the end
(`bar_manager_refresh(true)` → `bar_resize`); its items follow in `bar_draw`.
INFO buffer is `19 * bar_count + 4` bytes; longer lines (2-digit adid with 2+-digit sid, 3-digit sids) are
truncated by `snprintf` (quirk; a reimplementation can just produce the full string).

`bar.c:bar_change_space(bar, dsid)` (non-sticky only; requires `adid >= 1`): for every item
`bar_item_change_space(item, dsid, adid)` = `window_send_to_space(windows[adid-1], dsid)` if that window
exists, plus `popup_change_space` (all popup members' windows on that adid, and the popup window if
drawing); then `window_send_to_space(bar.window, dsid)`.

`bar.sid` semantics: the global mission-control index of the display's current space. An item with
`associated_space = N` (mask bit N) is shown only on bars whose current space index is N.

### 6.5 Space components (`bar_manager.c:bar_manager_update_space_components(forced)`)

For each item of type `space`:
1. Unless `overrides_association` (set when the user assigns `associated_display`/`display` to a space item):
   `space = lowest set bit index of associated_space` (`UINT32_MAX` if none); `did = display_id_for_space(space)`
   (`dsid_from_sid` walks `SLSCopyManagedDisplaySpaces` to the N-th space; `SLSCopyManagedDisplayForSpace(cid,
   dsid)` → UUID → `CGDisplayGetDisplayIDFromUUID`); `associated_display = did ? 1 << display_arrangement(did) :
   1 << 30`.
2. For each bar whose `adid` bit is in `associated_display` and with `sid != 0`:
   * `(!selected || forced) && (associated_space & (1 << bar.sid))` → `selected = true`, `updates = true`,
     env `SELECTED=true`;
   * else `(selected || forced) && !(associated_space & (1 << bar.sid))` → `selected = false`,
     `updates = true`, env `SELECTED=false`;
   * else `updates = false`.
   (`updates` gates whether the following `space_change` event runs the item's script: only on selection
   change or when forced.)

### 6.6 Display reconfiguration (`bar_manager.c:bar_manager_display_changed`)

`display.c:display_handler` (CG reconfiguration callback; flags checked with else-if precedence):
`kCGDisplayAddFlag` → `DISPLAY_ADDED` (+ brightness notification registration if enabled);
`kCGDisplayRemoveFlag` → `DISPLAY_REMOVED` (+ unregister); `kCGDisplayMovedFlag` → `DISPLAY_MOVED`;
`kCGDisplayDesktopShapeChangedFlag` → `DISPLAY_RESIZED`. (Callbacks carrying only
`kCGDisplayBeginConfigurationFlag` post nothing.) All four events call `bar_manager_display_changed`, once per
callback, i.e. possibly several full resets per reconfiguration:
```
active_adid = display_active_display_adid()
freeze; bar_manager_reset(); unfreeze
bar_manager_refresh(true)
bar_manager_handle_display_change()       # display_change event
bar_manager_handle_space_change(true)
animator_renew_display_link()
```
`bar_manager_reset()`: clear every item's `associated_bar`; for each bar: `bar_item_remove_window(item,
bar.adid)` for every item (destroys item windows), `bar_destroy(bar)` (`window_close` + free); `bar_count = 0`;
`bar_manager_begin()`.

`bar_manager_begin()`:
* `displays == 0` (main): one bar on `CGMainDisplayID()`, `adid = display_arrangement(did)`.
* else: `n = display_active_display_count()`; for `index` in `1..=n` with bit `index-1` set:
  `did = display_arrangement_display_id(index)`, `bar_create(did)`, `adid = index`, if `any_bar_hidden` →
  `bar_set_hidden(bar, true)`.
* `active_displays = OR(1 << adid)`; `active_adid = display_active_display_adid()`; `needs_ordering = true`.
After a reset all bar/item window ids change; item windows are recreated lazily on the next refresh/order.
Popup windows are *not* destroyed by a reset (they keep their window and `adid`).

### 6.7 Other system notifications (`workspace.m`, `sketchybar.c`)

| Source | Event | Handling |
|---|---|---|
| `NSWorkspaceActiveSpaceDidChangeNotification` | `SPACE_CHANGED` | §6.4 (`forced = false`). |
| `NSWorkspaceActiveDisplayDidChangeNotification` (string name) | `DISPLAY_CHANGED` | `bar_manager_handle_display_change`. |
| `NSWorkspaceDidActivateApplicationNotification` | `APPLICATION_FRONT_SWITCHED` | `front_app_switched` event, `INFO` = `localizedName`. |
| `NSWorkspaceWillSleepNotification` | `SYSTEM_WILL_SLEEP` | `system_will_sleep` event; destroy display link; `sleeps = true`. |
| `NSWorkspaceDidWakeNotification`, distributed `com.apple.screenIsUnlocked` | `SYSTEM_WOKE` | §8.6. |
| distributed `AppleInterfaceMenuBarHidingChangedNotification` | `MENU_BAR_HIDDEN_CHANGED` | resize all bars, `bar_needs_update`, refresh. |
| SkyLight notify 1327, 1328 (macOS ≥ 13) | `SPACE_CHANGED` | §6.4. |
| SkyLight notify 904, 905, 1401, 1508, 1322 | – | `g_disable_capture`: 1322 → now (alias capture disabled ~1.07 s = 2³⁰ ns), 905 → -1 (disabled until another of these events), others → 0. |
| CG display reconfiguration | `DISPLAY_*` | §6.6. |

All callbacks go through `event.c:event_post`, which runs the handler synchronously on the main thread
(`dispatch_sync(main_queue)` when called off-main), followed by `windows_unfreeze()`.

## 7. Per-space application windows (`app_windows.c`, event `space_windows_change`)

Enabled lazily by `begin_receiving_space_window_events()` the first time any item subscribes to
`space_windows_change` (`bar_item.c:bar_item_parse_subscribe_message`). Global state: `g_windows`,
`g_hidden_windows` (arrays of `{wid: u32, sid: u64 dsid, pid}`; slots with `wid == 0` are free and reused by
`app_windows_add`), `g_space_window_events`.

Registration (`SLSRegisterNotifyProc(handler, event, context)`):

| Event | Handler | Payload |
|---|---|---|
| 1325 | `window_spawn_handler` | `{u64 sid; u32 wid}` – window created on a space |
| 1326 | `window_spawn_handler` | same – window destroyed / removed from a space |
| 815 | `window_hide_handler` | `u32 wid` – window shown again |
| 816 | `window_hide_handler` | `u32 wid` – window hidden (e.g. app hidden/minimised) |
| 1401 | `space_handler` | refresh all spaces **silently** (no event) |
| 1327, 1328 (macOS ≥ 13) | `space_handler` | refresh all spaces and post events |

Then `update_all_spaces(&g_windows, silent = true)`.

* Spawn handler: ignore if `wid == 0 || sid == 0`. 1325 and `app_window_suitable(wid)` →
  `app_windows_update_space(g_windows, sid, false)`. 1326 and `(wid, sid)` present in `g_windows` → update that
  space; then clear the `wid` entry from `g_hidden_windows` if present.
* Hide handler: 816 and wid in `g_windows` → add a copy to `g_hidden_windows` (if not already), update its
  space. 815 and wid in `g_hidden_windows` → update its space, clear the hidden entry,
  `app_windows_register_notifications()`.
* `update_all_spaces(silent)`: for each active display, for each space id in `display_space_list(did)` →
  `app_windows_update_space(sid, silent)`.

`app_windows_update_space(windows, sid, silent)`:
1. Clear all entries with this `sid`.
2. `SLSCopyWindowsWithOptionsAndTags(cid, owner = 0, [sid] (SInt64), options = 0x2, &set_tags = 1,
   &clear_tags = 0)`; if non-empty: `SLSWindowQueryWindows(cid, list, 0)` → `SLSWindowQueryResultCopyWindows`
   → iterate with `SLSWindowIteratorAdvance`. A window is suitable iff
   `SLSWindowIteratorGetParentID == 0 && ((attributes & 0x2) || (tags & 0x0400000000000000)) &&
   ((tags & 0x1) || ((tags & 0x2) && (tags & 0x80000000)))` (tags/attributes from
   `SLSWindowIteratorGetTags/GetAttributes`). For suitable windows: `SLSGetWindowOwner(cid, wid, &owner_cid)`,
   `SLSConnectionGetPID(owner_cid, &pid)`, add `{wid, sid, pid}`.
3. If `!silent` → post the event for this space.
4. `SLSRequestNotificationsForWindows(cid, all non-zero wids of g_windows ∪ g_hidden_windows, count)`.

`app_window_suitable(wid)`: same predicate on a single-window `SLSWindowQueryWindows` query.

Event payload (`app_windows_post_event_for_space`), sent as `SPACE_WINDOWS_CHANGED` → custom event
`space_windows_change` with `INFO`:
```
{
	"space": <mission_control_index(sid)>,
	"apps": {
		"<App Name>": <window count>,
		"<Other App>": <count>
	}
}
```
(trailing `\n` after `}`; with no apps the `apps` object is `{\n\n\t}`). Window counts are grouped by pid in
first-seen order, names via `NSRunningApplication runningApplicationWithProcessIdentifier:pid].localizedName`
(pids without a name are skipped), then entries with identical names are merged (counts summed into the first
one). Names are **not** JSON-escaped.

`forced_space_windows_event()` (from `--update`): if enabled, `update_all_spaces(non-silent)` → one event per
space of every display.

## 8. Update cycle, timers, freezing, animation, lifecycle

### 8.1 Startup (`sketchybar.c:main`)

1. Refuse root; `BAR_NAME` env; parse args; `init_misc_settings`: lock file `/tmp/<name>_<user>.lock`,
   ignore SIGCHLD/SIGPIPE, `CGSetLocalEventsSuppressionInterval(0)`, `CGEnableEventStateCombining(false)`,
   `g_connection = SLSMainConnectionID()`, `g_space_management_mode = SLSGetSpaceManagementMode(cid)`.
2. `load_symbols`: macOS ≥ 26: `dlopen(".../SkyLight.framework/SkyLight")`,
   `dlsym("SLSTransactionAddPostDecodeAction")`.
3. Register SkyLight notify procs 904/905/1401/1508/1322 (capture gating) and 1327/1328 (space change, ≥ 13).
4. Workspace observer object allocated; `bar_manager_init`; `mouse_begin` (Carbon `InstallEventHandler` on
   the event dispatcher for mouse up/dragged/entered/exited/wheel/scroll); `display_begin`; workspace observers
   registered.
5. `windows_freeze(); bar_manager_begin(); windows_unfreeze()`.
6. Mach server; power/network/media sources; run config file; config hot-reload watcher.
7. macOS ≥ 14: `SLSWindowManagementBridgeSetDelegate(NULL)`.
8. `RunApplicationEventLoop()` (Carbon).

All handlers execute on the main thread; there is no other concurrency in bar state.

### 8.2 Clock (`SHELL_REFRESH`, every 1 s) → `bar_manager_update(forced = false)`

```
if (frozen && !forced) || sleeps: return
if forced:
    bar_manager_handle_space_change(true)
    forced_network_event(); forced_volume_event(); forced_brightness_event();
    forced_power_event(); forced_front_app_event(); forced_media_change_event(); forced_space_windows_event()
needs_refresh = false
for item in bar_items:
    bar_item_update(item, sender = NULL, forced, NULL)   # increments counter, runs routine scripts; returns false
    if item.has_alias && item shown (associated_bar != 0) && alias_update(alias, false):
        item.needs_update = true; needs_refresh = true
if needs_refresh || forced: bar_manager_refresh(forced)
```
`bar_item_update`: every 15 ticks (`counter % 15 == 0`) shown items with `scroll_texts` start text scroll
animations; `counter++`; scripts run when `update_freq > 0 && counter >= update_freq` (routine) or on events,
respecting `updates`/`updates=when_shown`. Routine `SENDER` = `routine`, forced = `forced`. Details in the item
spec.

### 8.3 Mouse plumbing touching the bar

Window under the event is identified via `CGEventGetIntegerValueField(event, 0x33)` (undocumented field 51).
Hit tests (`bar_manager_get_item_by_point`) scan items (skipping `!drawing`) and all their per-adid windows with
an inclusive rect test on `{origin, frame.size}`; bar/popup lookups use `CGRectContainsPoint`.
`bar.mouse_over` drives `mouse.entered.global` / `mouse.exited.global` (exit tested against the bar rect inset
by 1 pt); scroll on the bar background (no item) emits `mouse.scrolled.global` with `DID` = bar adid. Scroll
deltas are accumulated within 150 ms windows. Details belong to the events spec.

### 8.4 Freezing

Two independent mechanisms:
* **Manager freeze** (`frozen` bool): set by `bar_manager_freeze`, cleared by `bar_manager_unfreeze`. Used around
  message processing, `bar_manager_handle_space_change`, `bar_manager_display_changed`, and animation frames.
  It is a plain bool, not a counter: e.g. `--update` inside a message calls `handle_space_change`, whose
  unfreeze clears the message's freeze, so later commands in the same message may refresh immediately (quirk).
* **Window freeze** (SkyLight transaction, §3.6): batches server-side window changes per event.

### 8.5 Animation (`animation.c`, `animation.h`)

* `--animate <curve> <duration>` sets `animator.interp_function` (first char: `l`inear, `q`uadratic (x²),
  `s`in (sin(πx/2)), `t`anh (0.52·tanh(2·atanh(1/1.04)(x−0.5))+0.5), `c`irc (√(1−(x−1)²)), `e`xp (x·e^(x−1));
  `b`ounce/`o`vershoot are defined but map to linear) and `duration` (in 60 Hz frames; seconds = d/60). Both
  are reset to 0 at the start of every message.
* `ANIMATE(setter, target, current, new)` with duration > 0: cancel *locked* animations of the same
  (target, setter); create animation from `current` to `new`; if an unlocked animation for the same pair exists
  in this message, the new one starts from its final value and waits for it (chaining). Duration 0: cancel any
  running animation for the pair (applying its final value first), then call the setter directly.
* `ANIMATE_BYTES` (colours): each of the 4 bytes interpolated independently, truncated to `unsigned char`.
  `ANIMATE_FLOAT`: float interpolation. Int: `(1−s)·a + s·b + 0.5` truncated, exact final value on last frame.
* At message end `animator_lock` marks all animations locked (a later message replaces them).
* The CVDisplayLink (`CVDisplayLinkCreateWithActiveCGDisplays`, started immediately, also recreated by
  `animator_add` when absent) calls back on its thread → `dispatch_async(main)` → `ANIMATOR_REFRESH(hostTime)`.
  `t = (hostTime − start) / (duration_s · CVGetHostClockFrequency())`.
* `bar_manager_animator_refresh(time)`: freeze; `animator_update` (apply every animation; a value change on a
  target inside an item marks that item `needs_update`, otherwise — e.g. bar properties — sets
  `bar_needs_update`; finished animations removed; link destroyed when none remain); if anything changed:
  unfreeze, `bar_manager_resize()` if `bar_needs_resize`, `bar_manager_refresh(false)`; unfreeze.
  Animating `margin`, `y_offset`, `height`, `notch_offset`, `notch_display_height` therefore resizes the bar
  windows every frame. `blur_radius` animation applies the blur directly but its setter returns false, so it
  does not by itself cause redraws.
* The link is destroyed on sleep and renewed on display change.

### 8.6 Sleep / wake / hotload / exit

* Will sleep: `system_will_sleep` event, `animator_destroy_display_link`, `sleeps = true`.
* Woke (`bar_manager_handle_system_woke`): if `sleeps`: `sleeps = false` and schedule a second `SYSTEM_WOKE`
  500 ms later (global queue `usleep(500000)` → main queue). Always: `bar_manager_display_changed()` (full
  reset), then `system_woke` event. So each wake causes two full resets; each screen unlock causes one.
* Hotload/`--reload`: `bar_manager_destroy` (send `"k"` to mach helpers, destroy animator, remove all items,
  destroy bars, default item, events, background, clock), `bar_manager_init`, `bar_manager_begin`, re-run the
  config. Note `bar_needs_resize` and `g_space` survive.
* `--exit`: `bar_manager_destroy`, `exit(0)`.

## 9. Queries

### 9.1 `--query bar` (`bar_manager.c:bar_manager_serialize`)

Exact output (tabs shown as `\t`; booleans via `format_bool` → `"on"`/`"off"`):
```
{
\t"position": "<top|bottom>",
\t"topmost": "<on|off>",
\t"sticky": "<on|off>",
\t"hidden": "<on|off>",
\t"shadow": "<on|off>",
\t"font_smoothing": "<on|off>",
\t"show_in_fullscreen": "<on|off>",
\t"blur_radius": <%u>,
\t"margin": <%d>,
\t"drawing": "<on|off>",
\t"color": "0x<%x>",
\t"border_color": "0x<%x>",
\t"border_width": <%u>,
\t"height": <%u>,
\t"corner_radius": <%u>,
\t"padding_left": <%d>,
\t"padding_right": <%d>,
\t"x_offset": <%d>,
\t"y_offset": <%d>,
\t"clip": <%f>,
\t"image": {
\t\t"value": "<path or (null)>",
\t\t"drawing": "<on|off>",
\t\t"scale": <%f>
\t},
\t"items": [
\t\t "<item 1>",
\t\t "<item 2>"
\t]
}
```
Details:
* `position`: `"bottom"` iff `position == 'b'`, otherwise `"top"` — **left/right bars report `"top"`**.
* `topmost`: the bool only (`window` level not distinguishable). `hidden` = `any_bar_hidden`.
* `drawing` = `background.enabled` (default `off`; becomes `on` after `--bar color=…`).
* `color`/`border_color`: lowercase hex without zero padding (`%x`), e.g. `"0x44000000"`, `"0x0"`.
* `height`: `overrides_height ? (int)bounds.size.height : 0` printed with `%u`.
* `clip`: `%f` → `0.000000`. `image.value`: `printf("%s", NULL)` on macOS prints `(null)`; `scale` `%f`
  (`1.000000`). The background `shadow` block is not printed (`detailed = false`).
* `items`: every item name in global order, each line `\t\t "<name>"` (two tabs + one space), names not escaped;
  with zero items the array renders as `[\n\n\t]`.
* Not reported: `display`, `notch_width`, `notch_offset`, `notch_display_height`, the topmost level, per-bar
  state (`hidden` per display, `shown`, `sid`).
* Output ends with `}\n`.

Defaults rendering: position top, topmost off, sticky on, hidden off, shadow off, font_smoothing off,
show_in_fullscreen off, blur_radius 0, margin 0, drawing off, color `0x44000000`, border_color `0xffff0000`,
border_width 0, height 25, corner_radius 0, padding_left 20, padding_right 20, x_offset 0, y_offset 0,
clip 0.000000, image value (null)/off/1.000000.

### 9.2 `--query displays` (`display.c:display_serialize`)

For `i` in `0..count` of `CGGetActiveDisplayList` (the display is looked up by arrangement `i+1`):
```
[
\t{
\t\t"arrangement-id":<display_arrangement(did)>,
\t\t"DirectDisplayID":<did %d>,
\t\t"UUID":"<uuid or <unknown>>",
\t\t"frame":{
\t\t"x":<%.4f>,
\t\t"y":<%.4f>,
\t\t"w":<%.4f>,
\t\t"h":<%.4f>
\t\t}
\t},            <- "\t}\n" for the last entry
]
```
(no space after colons; `frame` = `CGDisplayBounds`).

### 9.3 Item query fields set by the bar

`bounding_rects` in `--query <item>` lists, for each existing per-adid window, `"display-<adid>": { "origin":
[x, y], "size": [w, h] }` (`%f`) using the window's screen origin (nirvana `-9999.000000` when hidden) and size.

## 10. macOS API inventory

Public:
`CGDisplayBounds`, `CGDisplayIsBuiltin`, `CGDisplayPixelsWide`, `CGMainDisplayID`, `CGGetActiveDisplayList`,
`CGDisplayCreateUUIDFromDisplayID` (semi-private, exported by CoreGraphics), `CGDisplayGetDisplayIDFromUUID`,
`CGDisplayRegisterReconfigurationCallback` / `CGDisplayRemoveReconfigurationCallback`,
`CGEventCreate`/`CGEventGetLocation`/`CGEventGetIntegerValueField`, `CGSetLocalEventsSuppressionInterval`,
`CGEnableEventStateCombining`, `CGBitmapContextCreate`/`CGBitmapContextCreateImage`, `CGContext*`
(ClearRect, Flush, ScaleCTM, SetInterpolationQuality, SetAllowsFontSmoothing, SetBlendMode, paths),
`CGColorSpaceCreateDeviceRGB`, `CFRunLoopTimerCreate`/`CFRunLoopAddTimer`, `CVDisplayLink*`,
`CVGetHostClockFrequency`, GCD (`dispatch_async/sync`, main/global queues), `NSScreen` (`screens`,
`deviceDescription["NSScreenNumber"]`, `frame`, `visibleFrame`, `safeAreaInsets`, `backingScaleFactor`),
`NSWorkspace` notifications and `frontmostApplication`, `NSDistributedNotificationCenter`,
`NSRunningApplication`, `CATransaction setDisableActions:`, `CALayer`, Carbon `InstallEventHandler`,
`RunApplicationEventLoop`, `CopyEventCGEvent`.

Private (SkyLight unless noted):

| Area | Symbols |
|---|---|
| Connection | `SLSMainConnectionID`, `SLSGetSpaceManagementMode`, `SLSRegisterNotifyProc`, `SLSWindowManagementBridgeSetDelegate` (≥ 14) |
| Displays | `SLSCopyManagedDisplays`, `SLSManagedDisplayGetCurrentSpace`, `SLSCopyManagedDisplaySpaces`, `SLSCopyManagedDisplayForSpace`, `SLSCopyActiveMenuBarDisplayIdentifier`, `SLSGetMenuBarAutohideEnabled`, `SLSGetRevealedMenuBarBounds` (x86-64) |
| Spaces | `SLSSpaceGetType` (4 = fullscreen), `SLSSpaceCreate`, `SLSSpaceSetAbsoluteLevel`, `SLSShowSpaces`, `SLSSpaceAddWindowsAndRemoveFromSpaces`, `SLSMoveWindowsToManagedSpace`, `SLSHWCaptureSpace` (space images) |
| Windows | `SLSNewWindowWithOpaqueShapeAndContext`, `SLSReleaseWindow`, `SLSSetWindowResolution`, `SLSSetWindowTags`, `SLSClearWindowTags`, `SLSSetWindowOpacity`, `SLWindowContextCreate`, `CGSNewRegionWithRect`, `CGRegionCreateEmptyRegion`, `SLSSetWindowShape`, `SLSMoveWindow`, `SLSReassociateWindowsSpacesByGeometry` (< 12), `SLSOrderWindow`, `SLSSetWindowLevel`, `SLSSetWindowBackgroundBlurRadius`, `SLSWindowSetShadowProperties`, `SLSRemoveAllTrackingAreas`, `SLSAddTrackingRect`, `SLSCaptureWindowsContentsToRectWithOptions`, `SLSGetScreenRectForWindow` |
| Transactions | `SLSTransactionCreate`, `SLSTransactionCommit`, `SLSTransactionMoveWindowWithGroup` (≥ 12), `SLSTransactionSetWindowLevel` / `SLSTransactionOrderWindow` (≥ 14), `SLSTransactionSetWindowShape` / `SLSTransactionSetSurfaceBounds` / `SLSTransactionAddPostDecodeAction` (≥ 26, dlsym), `SLSDisableUpdate` / `SLSReenableUpdate` (< 26) |
| Surfaces | `SLSAddSurface`, `SLSRemoveSurface`, `SLSBindSurface`, `SLSSetSurfaceBounds`, `SLSSetSurfaceResolution`, `SLSSetSurfaceOpacity`, `SLSSetSurfaceColorSpace`, `SLSOrderSurface`, `SLSFlushSurface`; QuartzCore private `CAContext +contextWithCGSConnection:options:`, `.contextId`, `-invalidate` |
| Window queries | `SLSCopyWindowsWithOptionsAndTags`, `SLSWindowQueryWindows`, `SLSWindowQueryResultCopyWindows`, `SLSWindowIteratorGetCount/Advance/GetParentID/GetWindowID/GetTags/GetAttributes`, `SLSGetWindowOwner`, `SLSConnectionGetPID`, `SLSRequestNotificationsForWindows` |
| DisplayServices | `DisplayServicesGetBrightness`, `DisplayServicesCanChangeBrightness`, `DisplayServicesRegister/UnregisterForBrightnessChangeNotifications` |

SkyLight notification ids used: 815, 816, 904, 905, 1322, 1325, 1326, 1327, 1328, 1401, 1508.

## 11. Re-implementation checklist and quirk index

Behaviours a 1:1 port must reproduce (or consciously deviate from):

1. Bar `background.enabled` defaults to off and `--bar drawing=off` has no visual effect (§2.6, §5.1).
2. `--bar shadow.*` and bar background shadows are parsed but never drawn; `--bar shadow=on` means the
   *window* system shadow, toggling recreates all windows.
3. `position` takes the first character only; anything except `b`/`l`/`r` acts as top; query prints only
   `top`/`bottom` (§2.2, §9.1).
4. Bottom bar uses `2*y_offset`; top uses `+y_offset` (§4.1). `notch_display_height` replaces the window height
   on built-in displays for both top and bottom, but the bottom y computation still uses `height`.
5. Notch values apply to every `CGDisplayIsBuiltin` display (with or without a physical notch); `notch_width`
   defaults to 200.
6. Unsigned cursor arithmetic in layout: right/`q` overflow snaps to `W - len`; left cursor clamps at 0 (§4.4).
7. Vertical bars: centre start `(H - 2*margin - len)/2 - 1`; no bracket layout; item windows ignore shadow
   overhang in width (§4.5).
8. `hidden=current` indexes bars by `adid-1` (§2.3); `any_bar_hidden` is overwritten by every `hidden`
   command; new bars inherit it only in non-`main` mode.
9. `topmost` resets windows *before* updating the flag (stale menu-bar offset until next resize) and always
   resets (§2.4).
10. `show_in_fullscreen` and `bar_create` interplay: fullscreen detection only refreshed on space change (§1, §2.2).
11. `font_smoothing` only affects contexts created after the change (§2.2, §3.4).
12. `display` list parsing: later `main`/`all` overwrite earlier numbers; non-numbers shift by -1 (§2.5).
13. Display reconfiguration and every wake/unlock trigger complete window recreation, possibly several times
    (§6.6, §8.6).
14. Manager freeze is a bool, not a counter (§8.4).
15. Item windows exist for every item on every bar and are parked at (-9999, -9999), never ordered out (§3.1).
16. Bitmap contexts are always 2× scale regardless of the display (§3.4).
17. The space-change `INFO` buffer can truncate (§6.4); `display_change` `INFO` is at most 2 digits (§6.3).
18. Space/display masks are 32-bit; `associated_space` bit = mission-control index, `associated_display` bit =
    arrangement index; `associated_bar` uses bit `adid-1` (compared via `<< 1`).

Suggested Rust mapping (non-normative): a `BarManager` owning `Vec<Bar>` and `Vec<Item>`, a per-(item, adid)
`Window` map, a `Platform` trait wrapping the SkyLight/CG calls listed in §10 so layout (§4) and redraw
decisions (§5.2) can be unit-tested on Linux with a fake platform.

## 12. Open questions (not answerable from source alone)

* Exact semantics of SkyLight notification ids (815/816 hide/unhide vs. order-in/out; 1322/905/1508 for
  capture gating) and of window tag/attribute bits in the suitability predicate; taken verbatim from the code.
* With `sticky=off`, which space receives a newly created item window (windows are only moved when a bar's
  `dsid` changes) — depends on SkyLight default placement.
* Whether `SLSSpaceCreate(cid, 1, 0)` + absolute level 0 behaves identically on every supported macOS version
  (the whole "sticky" mechanism relies on it).
* `kCGSExposeFadeTagBit` / option bits `13 | 1<<18` of `SLSNewWindowWithOpaqueShapeAndContext` are undocumented;
  their visible effect (Mission Control fading, no activation, no window-server shadow by default) must be
  verified on hardware.
* Behaviour of `double → uint32` conversion for negative layout results differs by architecture (§4.4); the port
  must pick one (arm64 saturating-to-0 is the dominant platform).
