# SketchyBar item behavior spec (for mbar)

Source: SketchyBar checkout at commit `5f358ec` (2026-09-16). This spec covers
bar items. It does not cover bar-level rendering, text/font rendering or image
loading, except where item behavior depends on them. Citations use
`file:function`.

Conventions in this document:

- **ADID**: the arrangement display id. It is 1-based and equals `bar->adid`.
  **SID**: the 1-based Mission Control index of a space, counted across all
  displays (`helpers.h:mission_control_index`).
- **Window coordinates** (`window->origin`, frames) are global screen
  coordinates with the origin at the top-left and y pointing down (CGS). Drawing
  coordinates inside an item window come from a CoreGraphics bitmap context:
  the origin is at the bottom-left and y points up. The `y` values passed to
  `*_calculate_bounds` are in drawing coordinates.
- **uint32 math**: most geometry fields are `uint32_t`. Section 13 lists the
  places where C's unsigned wraparound or float-to-unsigned truncation changes
  the result.
- `max`/`min` are the naive macros `(a > b ? a : b)`. When the operands have
  mixed types, the C usual arithmetic conversions apply.
- `format_bool(b)` returns `"on"` or `"off"`.

---

## 1. Data model and defaults

### 1.1 `struct bar_item` fields (bar_item.h, bar_item.c:bar_item_init)

| Field | Type | Default | Notes |
|---|---|---|---|
| `type` | char | `'i'` (item) | `'s'` space, `'a'` alias, `'b'` bracket/group, `'g'` graph, `'t'` slider. `'p'` (plugin) exists but is unused. |
| `name` | string | NULL | Set by `--add`. The default item is named `"defaults"`. |
| `counter` | u32 | 0 | Tick counter for `update_freq` and `scroll_texts`. |
| `needs_update` | bool | true | Dirty flag: the item must be redrawn. |
| `updates` | bool | true | `updates=on/off`. Space items force it to false and then toggle it themselves (8.6). |
| `updates_only_when_shown` | bool | false | `updates=when_shown`. |
| `lazy` | bool | true | **Unused.** The property `lazy` is not parsed and produces "Invalid property". |
| `selected` | bool | false | Space items only: whether the space is selected. |
| `mouse_over` | bool | false | Hover state for `mouse.entered` deduplication. |
| `ignore_association` | bool | false | |
| `overrides_association` | bool | false | Space items: set when `display=` is given explicitly. |
| `drawing` | bool | true | |
| `shadow` | bool | false | macOS window shadow of the item window(s). This is not the drawn shadow. |
| `has_const_width` / `custom_width` | bool / u32 | false / 0 | `width=`. |
| `scroll_texts` | bool | false | |
| `align` | char | `'l'` | Set to the position character whenever the position is set (2.3). |
| `blur_radius` | u32 | 0 | Blur of the item window(s). |
| `associated_to_active_display` | bool | false | `display=active`. |
| `associated_bar` | u32 bitmask | 0 | Bit `adid-1` is set when the item was drawn on that bar in the last draw pass. |
| `associated_display` | u32 bitmask | 0 | Bit `n` means display ADID `n`. 0 means all displays. |
| `associated_space` | u32 bitmask | 0 | Bit `n` means SID `n`. 0 means all spaces. |
| `update_frequency` | u32 | 0 | `update_freq`, in clock ticks (1 tick is 1 s). |
| `script` / `click_script` | string | NULL | |
| `signal_args.env_vars` | env map | empty | Per-item persistent environment (8.4). |
| `position` | char | `'l'` | `l`,`r`,`c`,`q` (center-left),`e` (center-right),`p` (popup). |
| `y_offset` | int | 0 | |
| `background` | background | see 1.3 | Item padding lives here (`background.padding_left/right`). |
| `icon`, `label` | text | see 1.2 | |
| `has_graph`/`graph` | | false | Set by type. |
| `has_alias`/`alias` | | false | Set by type. |
| `has_slider`/`slider` | | false (zeroed) | Set by type. `bar_item_init` does not reset `has_slider`; it starts 0 because the struct is memset. |
| `group` | ptr | NULL | Brackets own a group. Members point to the most recent group that added them. |
| `update_mask` | u64 | 0 | Subscribed event bits (8.2). |
| `windows[]` / `num_windows` | | empty | One window per ADID, created lazily (`bar_item_get_window`). |
| `popup` | popup | see 1.4 | |
| `parent` | ptr | NULL | The popup host item when this item is a popup member. |
| `event_port` | mach port | 0 | `mach_helper=<bootstrap name>`. |

### 1.2 Text (`icon`, `label`, slider `knob`) defaults (text.c:text_init, font.c:font_init)

| Property | Default |
|---|---|
| string | `""` |
| drawing | on |
| highlight | off |
| color | `0xffffffff` |
| highlight_color | `0xff000000` |
| padding_left / padding_right | 0 / 0 |
| y_offset | 0 |
| font | family `Hack Nerd Font`, style `Bold`, size `14.0`; `typographical_width` off; features none |
| width | dynamic (`has_const_width=false`, `custom_width=0`) |
| max_chars | 0 (no truncation) |
| align | `'l'` |
| scroll_duration | 100 |
| scroll (internal) | 0.0 |
| background | as 1.3 |
| shadow | as 1.5 |

### 1.3 Background defaults (background.c:background_init)

| Property | Default |
|---|---|
| drawing (`enabled`) | off |
| color | `0x00000000` |
| border_color | `0x00000000` |
| border_width | 0 |
| height | 0 (`overrides_height=false`) |
| corner_radius | 0 |
| padding_left / padding_right | 0 |
| x_offset / y_offset | 0 |
| clip | 0.0 |
| image | disabled, scale 1.0, border_color `0xcccccccc`, border_width 0, corner_radius 0, padding 0, y_offset 0, shadow as 1.5 |
| shadow | as 1.5 |

Setting `background.color`, or `clip` > 0, also turns `drawing` on
(background.c:background_set_color, background_set_clip). Setting
`border_color` does not.

### 1.4 Popup defaults (popup.c:popup_init)

| Property | Default |
|---|---|
| drawing | off |
| horizontal | off |
| align | `'l'` |
| height (`cell_size`) | 30, `overrides_cell_size=false`. While not overridden, layout recomputes it from the host window (6.3). |
| y_offset | 0 |
| blur_radius | 0 |
| topmost | **on** |
| background | Disabled. `color=0x44000000` and `border_color=0xffff0000` are preset but do not show until the background is enabled (`background.drawing=on` or `background.color=...`). |

### 1.5 Shadow defaults (shadow.c:shadow_init)

`drawing=off`, `angle=30` (degrees), `distance=5`, `color=0xff000000`.
The derived offset is `offset.x = distance*cos(angle)` and
`offset.y = -distance*sin(angle)`, so the defaults give (4.330, -2.5).
Setting `shadow.color` also enables the shadow.

### 1.6 Type-specific defaults

| Component | Defaults |
|---|---|
| graph (graph.c:graph_init) | `line_width=0.5`, `fill=true`, `color` (line) `0xffcccccc`, `fill_color` `0xffcccccc` with `overrides_fill_color=false`. Unless `fill_color` is set, the fill is the line color with alpha×0.2. Width comes from `--add`. |
| slider (slider.c:slider_init) | `percentage=0`, track width 100 (`--add` overwrites it, see 2.1), `highlight_color` (foreground) `0xff0000ff`, track background color `0xff000000`. Both backgrounds are enabled. Track background height is 0 (not overridden). Knob is a text with defaults. |
| alias (alias.c:alias_init) | `update_freq=1`, color `0xffff0000` (`color_override=false`), image scale 1. |
| space (bar_item.c:bar_item_set_type) | If no script was inherited, script is `sketchybar -m --set $NAME icon.highlight=$SELECTED`. `update_mask |= space_change`, `updates=false`, `updates_only_when_shown=false`. Env vars: `SELECTED=false`, `SID=0`, `DID=0`. |
| bracket | `group` created with `members[0]` set to the bracket itself. |

### 1.7 The default item and inheritance

- `g_bar_manager.default_item` is initialized with `bar_item_init(..., NULL)`
  and named `"defaults"`. `--default k=v ...` runs the normal `--set` parser
  on it (message.c:handle_domain_default). `graph.*`, `alias.*` and `slider.*`
  are accepted on the default item even though it has no such component
  (bar_item.c:bar_item_parse_set_message).
- Each new item is created with `bar_item_create` (zeroed), then
  `bar_item_init(item, &default_item)`. `bar_item_init` sets the defaults
  above and then calls `bar_item_inherit_from_item(item, default_item)`, so
  **everything** set on the default item at that moment is copied (5.1).
  Changing the defaults later does not affect existing items.
- `<anything>.reset` is not a property. The bare property name `reset`
  (COMMAND_DEFAULT_RESET) in any `--set` or `--default` (for example
  `--set foo reset=1` or `--default reset=x`) re-runs
  `bar_item_init(&default_item, NULL)` on the default item. This resets the
  defaults and sets the default item's name to NULL, so later
  `--query defaults` prints `"name": "(null)"`. `--default reset` without `=`
  fails with "Expected <key>=<value> pair".

---

## 2. Item creation, types, positions

### 2.1 `--add` (message.c:handle_domain_add)

Syntax: `--add <type> <name> <position> [extra...]`. The `event` type
(`--add event <name> [<notification>]`) registers a custom event and is not
covered here. The steps run in this order:

1. If an item named `<name>` exists, respond `[?] Add: Item '<name>' already exists\n` and stop.
2. `bar_manager_create_item`: append a new item to `bar_items` (it goes last
   in the global order), initialize it from the defaults, and set
   `needs_ordering`.
3. `bar_item_set_type(type)`:
   - `space`, `alias`, `bracket`, `graph` and `slider` set the matching type.
   - Anything else (including `component`) becomes `item`. If the word was not
     `item`, respond `[?] Add <name>: Invalid type '<type>', assuming 'item'\n`
     and continue.
4. If the type is not bracket, call `bar_item_set_position(position)`. If it
   returns false (missing or empty token, or a first character not in
   `l q c e r p`), respond `[!] Add <name>: Illegal position '<pos>'\n`,
   remove the item and stop. Brackets skip this step and keep the inherited
   position (normally `'l'`).
5. `bar_item_set_name(name)`. If the name is empty, respond
   `[!] Add: Illegal name '<name>'\n`, remove the item and stop. Setting the
   name also sets env `NAME=<name>`.
6. Type-specific setup, only when `type` is non-empty and not `item`:
   - **graph**: the next token is the width. `graph_setup(width)` allocates
     `width` floats, all 0.0. If the token is missing the width is 0, which
     later divides by zero; treat it as invalid.
   - **slider**: the next token is the width. `slider_setup(width)` sets the
     track width and enables both backgrounds. If the token is missing the
     width becomes **0**, not the default 100.
   - **alias**: `<name>` is parsed as `owner,name`, split at the first `,`.
     Without a comma, the owner is the whole name and the alias window name is
     NULL. `alias_setup` requests Screen Recording permission and captures
     once.
   - **bracket**: the `<position>` token and every token after it are member
     specs (2.4).
7. If `position[0] == 'p'` and the position has the form `p….<host>` (split at
   the first `.`):
   - If the host is not found, respond
     `[!] Add (Popup) <name>: Item '<host>' is not a valid popup host\n` and
     remove the item.
   - Otherwise `popup_add_item(host.popup, item)`.
   - This check also runs for brackets, against the first member token. A
     bracket whose first member spec starts with `p` and contains `.` is
     wrongly treated as a popup request.
8. `needs_update = true`.

Positions are validated only by their **first character**: `left`, `l`, `lol`
and `lxyz` are all left. `popup.<host>` works, and so does `p.<host>`.

### 2.2 Type semantics summary

| Type (`--add`) | Internal | Draws | Specials |
|---|---|---|---|
| `item` | `'i'` | background, icon, label | |
| `space` | `'s'` | background, icon, label | Space selection logic (8.6). Space association does not hide it (7.1). Env `SELECTED`, `SID`, `DID`. |
| `alias` | `'a'` | background, icon, alias image, label | Mirrors a menu-bar extra. Recaptured on the clock tick while shown. |
| `graph` | `'g'` | background, icon, graph, label | `--push <name> v1 v2 ...` appends samples. `graph.rtl` is set by layout (true for positions `r` and `q`). |
| `slider` | `'t'` | background, icon, slider (track, fill, knob), label | Drag and click handling (9.6). Env `PERCENTAGE`. |
| `bracket` | `'b'` | background only | Bounding box over its members (section 5). Skipped in side-length sums and normal layout. |

Draw order inside an item window (bar_item.c:bar_item_draw):

1. Item background.
2. Brackets stop after the background.
3. Icon (text background first, then text).
4. Label.
5. Alias, then graph, then slider.

### 2.3 Position semantics (bar_item.c:bar_item_set_position)

- A first character that is not in `l q c e r p` returns false and changes
  nothing.
- If `item.parent != NULL`, first `popup_remove_item(parent.popup, item)`.
  This does **not** clear `item.parent` (see `--set position` below).
- `position = c0`.
- If `c0 != 'p'`, then `align = c0`. Consequences:
  - Right items are right-aligned inside a widened slot.
  - Center items are centered.
  - `q` and `e` items behave as left-aligned, because only `'c'` and `'r'`
    are special.
  - Popup items keep their previous `align`.

`--set <item> position=<v>` (bar_item.c:bar_item_parse_set_message):

1. Call `bar_item_set_position(v)`. The return value is ignored; an invalid
   value is a silent no-op.
2. Split `v` at the first `.`.
   - If both halves exist and the key starts with `p`, look up the host. If it
     is missing, respond
     `[!] Item Position (<name>): Item '<host>' is not a valid popup host\n`
     and **return early**: no refresh, and the position stays `'p'` with no
     membership, so the item is not drawn. If found,
     `popup_add_item(host.popup, item)`.
   - If both halves exist and the key does not start with `p`, set
     `parent = NULL`.
   - Without a `.`, `parent` is not touched. It stays stale after moving from
     a popup back to the bar. This is harmless for drawing, because the parent
     is only consulted when `position=='p'`.
3. `needs_update = true`.

Re-setting `position=popup.<host>` on an item already in that popup removes it
and appends it again, which moves it to the **end** of the popup order.

### 2.4 Bracket members (message.c:handle_domain_add, group.c:group_add_member)

For each member token (the `<position>` token and all tokens after it):

- `/regex/` (length > 1, starts and ends with `/`): POSIX **basic** regex
  (`regcomp` flags 0), unanchored, matched against every item name in global
  order.
  - Compile error: `[!] Regex: Could not compile regex '<tok>'\n`.
  - No match: `[?] Regex: No match found for regex '<tok>'\n`.
- Otherwise an exact name lookup. If not found, respond
  `[?] Add (Group) <bracket>: Failed to add member '<tok>', item not found\n`.
- For the **first** item matched by the **first** successful token: if that
  item has `position=='p'`, the bracket is added to that item's parent popup
  and its position becomes `'p'`.
- Every matched item is passed to `group_add_member(bracket.group, item)`:
  - Already a member: no-op.
  - The item is itself a bracket (`item.group.members[0]==item`): its members
    `[1..]` are added recursively, so groups flatten.
  - Otherwise the item is appended and `item.group = group`. This overwrites
    any previous group pointer, although the item stays in the old group's
    member list.
- If the first member token yields nothing, the bracket is removed and
  parsing stops. In C this then touches the freed item (UB); treat it as
  "bracket not created".

---

## 3. `--set` property reference

### 3.1 Parsing rules (message.c:handle_message_mach, helpers.h)

- Arguments are NUL-separated tokens. `--set <name|/regex/> k=v [k=v ...]`.
  Each token is split at the **first** `=`; the value may contain `=`.
  - A token without `=`: respond
    `[!] Set (<item>): Expected <key>=<value> pair, but got: '<tok>'\n` and
    stop this `--set`.
  - `k=` (empty value) gives an empty value token.
- Parsing of the `--set` ends when the next token starts with `-`.
- An unknown item name responds `[!] Set: Item not found '<name>'\n` and skips
  the rest of the batch line, up to the next `--` token. With a regex, each
  `k=v` is applied to all matching items, in global item order.
- Property names are dotted paths, split at the **first** `.` at each level.
  For example `popup.background.color` resolves as popup → background →
  color.
- **Booleans** (helpers.h:evaluate_boolean_state):
  - True: `on yes true 1 !off !no !false !0`.
  - `toggle` negates the current value.
  - **Anything else is false.**
- **Integers**: `strtol(v, NULL, 0)`, so `0x` hex and leading-`0` octal work
  and garbage gives 0. Unsigned values use `strtoul(...,0)`. Colors are
  `strtol` cast to int, so `0xAARRGGBB` works. Floats use `strtof`.
- Single-character enums (`align`, `popup.align`, text `align`) take the
  **first character** of the value, with no validation (an empty value gives
  `'\0'`).
- Animatable properties (the "Anim" column below) go through
  `ANIMATE*(setter, obj, current, target)` (animation.h):
  - If `--animate <curve> <frames>` set a duration > 0 earlier in the same
    message, the setter is driven by an animation.
  - Otherwise any running animation of the same (object, setter) is cancelled
    (snapping it to its final value) and the setter is applied immediately.
  - `needs_refresh` is the OR of the cancel result and the setter result.
- After a property is applied, `needs_refresh` true sets
  `item.needs_update = true`. At the end of the mach message, one
  `bar_manager_refresh(false)` runs.

### 3.2 Item-level properties (bar_item.c:bar_item_parse_set_message)

| Property | Value | Anim | Refresh | Semantics |
|---|---|---|---|---|
| `icon` | string | width-anim (3.3) | if changed | Same as `icon.string`. |
| `label` | string | width-anim | if changed | Same as `label.string`. |
| `drawing` | bool | – | if changed | Item visibility. When not drawn: windows go to nirvana (-9999,-9999), no hit-testing, and no popup lookup for this host. |
| `updates` | `when_shown` \| bool | – | no | `when_shown` sets `updates=true, only_when_shown=true`. A bool sets `updates=bool, only_when_shown=false`. |
| `update_freq` | uint | – | no | Ticks of the 1 s clock. 0 disables routine updates. |
| `script` | string | – | no | Leading `~` is replaced by `$HOME` (helpers.h:resolve_path). Setting the same string is a no-op. `""` disables. |
| `click_script` | string | – | no | Same handling as `script`. |
| `position` | see 2.3 | – | yes | |
| `align` | char | – | if changed | `c` centers, `r` right-aligns, anything else left-aligns the content in a widened slot (4.3). |
| `width` | `dynamic` \| int | yes | per setter | 4.2. A negative int means dynamic. |
| `y_offset` | int | yes | if changed | Vertical offset of all item content and background. Positive is up in drawing coordinates. |
| `padding_left` | int | yes | if changed | Writes `background.padding_left`. Alias of `background.padding_left`. |
| `padding_right` | int | yes | if changed | Alias of `background.padding_right`. |
| `blur_radius` | int | yes | (setter returns true if changed) | Applied immediately to all existing item windows. |
| `shadow` | bool | – | if changed | Window shadow. On change, **all item windows are destroyed** and recreated on the next draw, with the shadow disabled unless on. |
| `ignore_association` | bool | – | always | 7.1. |
| `associated_space` / `space` | list of ints, comma-separated | – | if mask changed | Reset the mask to 0, then set bit `strtoul(n)` for each entry. An empty value clears it. Space items keep only the **last** bit and set env `SID` (8.6). |
| `associated_display` / `display` | list of ints or `active` | – | if mask changed | Reset the mask and the active flag. `active` sets `associated_to_active_display`. Each other entry sets bit `strtoul(n)`. `main` is **not** special: it parses as 0 → bit 0, which matches no bar. Space items keep the last bit, set `overrides_association=true` and env `DID`. |
| `scroll_texts` | bool | – | no | 8.7. |
| `mach_helper` | bootstrap name | – | no | `event_port = mach_get_bs_port(name)`. |
| `reset` | any | – | no | Resets the **default item** (1.7). |
| `icon.*` / `label.*` | | | | Text sub-properties (3.3). |
| `background.*` | | | | 3.4. |
| `popup.*` | | | | 3.5. |
| `graph.*` / `alias.*` / `slider.*` | | | | 3.6. On a non-matching item type, respond `[!] Item (<name>): Trying to set a graph property on a non-graph item\n` (the alias and slider messages are analogous: "an alias property on a non-alias item" and "a slider property on a non-slider item"). |
| unknown `x.y` | | | | `[!] Item (<name>): Invalid subdomain '<x>'\n` |
| unknown | | | | `[!] Item (<name>): Invalid property '<p>' \n` (note the space before `\n`) |

Not properties, even though they are defined in defines.h: `lazy`,
`cache_scripts`. Both produce "Invalid property".

### 3.3 Text sub-properties (`icon.`, `label.`, `slider.knob.`) (text.c:text_parse_sub_domain)

| Property | Value | Anim | Returns refresh | Semantics |
|---|---|---|---|---|
| `string` (also the bare `icon=`/`label=`) | string | special | if changed | Sets the string and re-lays out the line. With animation active and a changed natural length (`text_get_length(false)`), the text width is pinned to the old length, animated to the new length, then a 0-frame animation sets it to `-1` (dynamic). |
| `drawing` | bool | – | if changed | A hidden text has length 0 and height 0. |
| `color` | hex | bytes | if changed | |
| `highlight` | bool | special | if changed | With animation active, cross-fades color ↔ highlight_color (text.c). Drawing uses `highlight_color` while highlighted. |
| `highlight_color` | hex | bytes | | |
| `font` | `family:style:size` | – | if changed | `sscanf("%254[^:]:%254[^:]:%f")`. A missing size defaults to **10.0**. Takes the **rest of the message** (`string_copy(message)`), not a token. |
| `font.family` / `font.style` | string | – | if changed | |
| `font.size` | float | float | | |
| `font.features` | `feat,...` | – | | OpenType tags (`+liga`, `-calt`) or `type:selector` pairs. |
| `font.typographical_width` | bool | – | | Width uses the typographic advance instead of the glyph bounds. |
| `padding_left` / `padding_right` | int | yes | | |
| `y_offset` | int | yes | | Glyph draw offset only. Does not move the text background. |
| `width` | `dynamic` \| int | yes | | The same pattern as item width, on the text: an int pins `custom_width` (starting from `text_get_length(false)`). `dynamic` animates from `custom_width` to `text_get_length(true)`, then a 0-frame animation sets `-1`. |
| `align` | char | – | if changed | Only used when the text has a const width: `c` centers, `r` right-aligns. |
| `max_chars` | int | – | | Truncates the measured width to the first N UTF-8 code points. The draw is clipped to that width. |
| `scroll_duration` | int | – | **never** | Negative values are ignored. |
| `background.*` | | | | Text background (3.4). |
| `shadow.*` | | | | 3.7. |
| `color.{hex,alpha,red,green,blue}`, `highlight_color.{...}` | | | | Color channels. alpha/red/green/blue are floats in 0..1. |
| unknown | | | | `[!] Text: Invalid property '<p>'\n` / `[!] Text: Invalid subdomain '<x>' \n` |

Text metrics (text.c:text_prepare_line), as needed by item layout:

- `bounds = CTLineGetBoundsWithOptions(GlyphPathBounds)`.
- `bounds.w = (u32)(w+1.5)`, `bounds.h = (u32)(h+1.5)`.
- `bounds.origin` is rounded: `(i32)(v+0.5)`.
- `width` is `bounds.w`, or `(u32)(typographic_width+0.5)` when
  `typographical_width` is on.
- When `max_chars>0`, `width` is the bounds width of the truncated string (the
  same +1.5 rule).
- `ascent`/`descent` come from the typographic bounds.

`text_get_length(text, override)`:

```
if !drawing: return 0
len = width + padding_left + padding_right          (int)
if (!has_const_width || override) && background.enabled && background.image.enabled:
    if image_size(background.image).w > len: return image_w
if has_const_width && !override: return custom_width
return max(len, 0)
```

`text_get_height = drawing ? bounds.h : 0`.

### 3.4 Background sub-properties (background.c:background_parse_sub_domain)

These apply to `background.*` on items and text, `popup.background.*`, and
`slider.background.*`.

| Property | Value | Anim | Semantics |
|---|---|---|---|
| `drawing` | bool | – | Enables or disables the background. Disabling a clipping background resets its clips and forces a bar redraw. |
| `color` | hex | bytes | **Also enables the background.** |
| `border_color` | hex | bytes | |
| `border_width` | int | yes | |
| `height` | int | yes | Sets the height. `overrides_height = (h != 0)`; 0 returns to automatic height. |
| `corner_radius` | int | yes | Clamped at draw time to `min(w,h)/2` of the inset rect. |
| `padding_left` / `padding_right` | int | yes | For item and text backgrounds this is the **item/text padding** used in layout. |
| `x_offset` / `y_offset` | int | yes | Draw offset of the background rect. |
| `clip` | float | float | Cuts the bar background under this rect with alpha `clip`. A value > 0 enables the background. |
| `image` | path | – | Loads the image (`app.<name>`, `space.<n>`, `media.artwork`, file path; an empty string destroys it). |
| `image.*` | | | `string`, `drawing`, `scale`, `corner_radius`, `padding_left/right`, `y_offset`, `border_width`, `border_color`, `border_color.*`, `shadow.*`. |
| `shadow.*` | | | 3.7. |
| `color.*` / `border_color.*` | | | Color channels. |

### 3.5 Popup sub-properties (popup.c:popup_parse_sub_domain)

| Property | Value | Anim | Returns refresh | Semantics |
|---|---|---|---|---|
| `drawing` | bool | – | if changed | On change: when turning off, close the popup window. Always: `drawing=v`, `adid=0` (forces re-anchoring). |
| `horizontal` | bool | – | **always** | Lay out the items in a row instead of a column. |
| `align` | char | – | always | `c` centered under the host, `l` left (subtracts the host's `padding_left`), anything else right-aligned. For nested popups: `l` places it left of the parent popup, else right of it. |
| `height` | int | yes | | Cell height. Sets `overrides_cell_size=true` permanently: there is no way back to automatic, and a negative value wraps to a huge `u32`. |
| `y_offset` | int | yes | | Added to `anchor.y` (screen coordinates, positive moves **down**). |
| `blur_radius` | int | yes | **never** | Applied to the popup window immediately. |
| `topmost` | bool | – | if changed | Window level: `kCGPopUpMenuWindowLevel` when on, `kCGBackstopMenuLevel+1` when off. Sets `needs_ordering`. |
| `background.*` | | | | Popup background. The **shadow is never drawn** for popups. |
| unknown | | | | `[!] Popup: Invalid property '<p>'\n` / `[!] Popup: Invalid subdomain '<x>'\n` |

### 3.6 Component sub-properties (short)

- **graph.*** (graph.c):
  - `color` (line, uint): no animation, returns `color_set_hex`.
  - `fill_color`: sets `overrides_fill_color=true`.
  - `line_width` (float): always refreshes.
  - `color.*`, `fill_color.*`.
  - Unknown: `[!] Graph: Invalid property/subdomain`.
- **slider.*** (slider.c):
  - `percentage` (u32, animatable): **ignored while dragging**. The setter
    clamps the stored value to ≤100, but compares with the unclamped value
    for change detection.
  - `highlight_color` (bytes animation): foreground color.
  - `width` (animatable, ignored while dragging): track width.
  - `knob` (string): knob text string.
  - `knob.*`: knob text sub-properties.
  - `background.*`: applied to the **foreground** first, then the foreground
    color is restored to `highlight_color`, then applied to the track
    background. The return value comes from the track background only.
- **alias.*** (alias.c):
  - `color` (uint): sets `color_override`.
  - `color.*`: sets the override.
  - `scale` (float): image scale.
  - `update_freq` (uint): recapture period in clock ticks.
  - `shadow.*`: image shadow.

### 3.7 Shadow sub-properties (shadow.c:shadow_parse_sub_domain)

| Property | Value | Anim | Semantics |
|---|---|---|---|
| `drawing` | bool | – | |
| `color` | hex | bytes | **Also enables the shadow.** |
| `angle` | uint (degrees) | yes | Recomputes the offset. |
| `distance` | uint | yes | Recomputes the offset. |
| `color.*` | | | |


---

## 4. Item geometry

### 4.1 Lengths and heights (bar_item.c)

```
content_length(item) =                                   // bar_item_get_content_length
    text_get_length(icon,false) + text_get_length(label,false)
  + (has_graph  ? graph.enabled ? graph.width : 0 : 0)
  + (has_slider ? slider.track_width : 0)
  + (has_alias  ? (alias.image_ref ? alias.image.bounds.w : 0) : 0)
  clamped to >= 0

get_length(item, ignore_override):                       // bar_item_get_length
  c = content_length(item)
  if background.enabled && background.image.enabled:
      c = max(c, image_size(background.image).w)
  if has_const_width && (!ignore_override || custom_width > c): return custom_width
  return c
```

- `get_length(false)` is the **slot length**: `custom_width` when the width is
  constant, else the content length.
- `get_length(true)` is the **display length**: `max(custom_width, c)` when the
  width is constant, else `c`.
- Item padding is **not** part of either length.
- `image_size(img)`:
  - `w = img.bounds.w + img.padding_left + img.padding_right + (img.shadow.enabled ? img.shadow.offset.x : 0)`
  - `h = img.bounds.h + 2*|img.y_offset|`

```
get_height(item) =                                       // bar_item_get_height
  max( max(text_get_height(label), text_get_height(icon)),
       alias_height,                       // alias image height or 0
       background.enabled ? max(bg.image.enabled ? image_size(bg.image).h : 0,
                                bg.bounds.h) : 0 )
```

Here `bg.bounds.h` is the **current** background height: either the
override, or the value computed by the last layout. This makes the result
depend on the previous layout pass.

### 4.2 `width` property (bar_item.c:bar_item_set_width)

- The setter `set_width(w)`:
  - `w < 0`: `has_const_width=false`. Returns whether the flag changed.
  - Otherwise: if `custom_width==w` and the width is already constant,
    return false. Else set `custom_width=w`, `has_const_width=true` and return
    true.
- `width=<int>`: `ANIMATE(set_width, from = get_length(false) + (const ? 0 : pl+pr), to = int)`.
  The int is the **total slot width including item padding** (see the
  advance rules in 4.4).
- `width=dynamic`:
  1. `ANIMATE(set_width, from = custom_width, to = get_length(true) + pl + pr)`.
  2. **Always** add a 0-duration animation `custom_width → -1`. It chains
     after any running animation for the same (item, setter).
  - Without `--animate`: step 1 applies immediately (the item becomes
    const-width at its natural size). Step 2 switches it back to dynamic on the
    next animator frame.
  - With `--animate`: the width animates from the **current `custom_width`**
    to the natural width, then turns dynamic. If the item was already dynamic,
    the start value is a stale `custom_width` (often 0), so the animation
    visibly starts from that value.

### 4.3 Internal layout (bar_item.c:bar_item_calculate_bounds)

`calculate_bounds(item, bar_height, x, y) -> slot_length`. All values are
u32; `x` and `y` are drawing coordinates inside the item window.

```
length  = get_length(false)
content = content_length()
content_x = x
if length > content:
    if align == 'c': content_x += (length - content) / 2       // integer div
    elif align == 'r': content_x += length - content
icon_x  = content_x
mid_x   = icon_x + text_get_length(icon,false)            // "sandwich" position
label_x = mid_x + (has_graph ? graph_len : has_alias ? alias_len : has_slider ? slider_len : 0)
ty = y + item.y_offset

text_calculate_bounds(icon,  icon_x,  ty)
text_calculate_bounds(label, label_x, ty)
if has_alias:  alias_calculate_bounds(mid_x, ty)          // image bounds: x = mid_x + img.padding_left,
                                                          //   y = ty - img.h/2 + img.y_offset
if has_slider: slider_calculate_bounds(mid_x, ty)
if has_graph:
    gh = background.enabled ? (bg.bounds.h - bg.border_width - 1)
                            : (bar_height - (BAR.border_width + 1))
    graph_calculate_bounds(mid_x, ty, gh)                 // origin.y = ty - gh/2 + line_width
if background.enabled:
    bh = bg.overrides_height ? bg.height : (bar_height - (BAR.border_width + 1))
    background_calculate_bounds(bg, x, ty, length, bh)    // NOTE: x, not content_x
return length
```

Here `BAR` is `g_bar_manager.background`, i.e. the bar's own background.

- `background_calculate_bounds(bg, x, y, w, h)` sets the rect to
  `origin=(x, y - h/2)` (u32 math, `h/2` is integer division) and `size=(w,h)`.
  If the background image is enabled, it also lays out the image at `(x, y)`.
- The item background spans the **slot** (`length`) starting at `x`. Item
  padding lies **outside** the background.
- **Double subtraction quirk.** The bar passes
  `bar_height = H - (BAR.border_width+1)`, where `H` is the bar window height.
  The background and graph then subtract `(BAR.border_width+1)` again. So the
  automatic item background height is `H - 2*(border_width+1)`, which is
  `H-2` with the default border of 0.

`text_calculate_bounds(text, x, y)` (text.c):

```
if align=='c' && has_const_width: origin.x = (int)x + ((int)custom_width - (int)text_get_length(text,true)) / 2
elif align=='r' && has_const_width: origin.x = (int)x + (int)custom_width - (int)text_get_length(text,true)
else origin.x = x
origin.y = (u32)(y - (ascent - descent)/2)               // double math, truncated
if background.enabled:
    h = bg.overrides_height ? bg.height : bounds.h
    background_calculate_bounds(bg, x, y, text_get_length(text,false), h)   // unaligned x, no text y_offset
```

Text is drawn at `(origin.x + padding_left - scroll, origin.y + text.y_offset)`.
When `max_chars>0`, drawing is clipped to x ∈ `[origin.x+padding_left, +width)`.
If the text shadow is enabled, it is drawn first at the same position plus
`shadow.offset` (text.c:text_draw).

Slider (slider.c:slider_calculate_bounds):

- The track is at `(x, y)` with the track width and the track height.
- The foreground is at the same position with width `track_w*pct/100`.
- The knob text is at `x + clamp(pct/100*track_w - knob.bounds.w/2, 0, track_w - (knob.bounds.w+1))`.

### 4.4 Shadow extents (bar_item.c:bar_item_calculate_shadow_offsets)

The function returns a `CGPoint` whose fields actually mean **left extent**
(`.x`) and **right extent** (`.y`). Only the x components of the offsets are
used.

```
L = (int)( [bg.shadow.enabled]         * max(-bg.shadow.offset.x, 0)
         + [icon.shadow.enabled]       * max(-icon.shadow.offset.x, 0)
         + [icon.bg.shadow.enabled]    * max(-icon.background.shadow.offset.x, 0)
         + [label.bg.shadow.enabled]   * max(-label.background.shadow.offset.x, 0)
         + [label.shadow.enabled]      * max(-label.shadow.offset.x, 0)
         + [bg.enabled]                * max(-bg.x_offset, 0) )
R = same with +offset.x and +bg.x_offset
```

- A shadow counts whenever its `shadow.enabled` is set, even if the owning
  background is disabled.
- Image, alias and slider shadows are not counted.
- The sum is computed in double and truncated to int once.

### 4.5 Bar layout, horizontal bars (top/bottom) (bar.c:bar_calculate_bounds_top_bottom)

Inputs:

- `W, H`: bar window size.
- `notch = builtin_display ? bar.notch_width : 0`.
- `BAR.padding_left/right`: the bar background paddings.

Cursors (u32):

```
left   = max(BAR.padding_left, 0)
right  = W - max(BAR.padding_right, 0)
center = (W - center_len) / 2          // W is double; result truncated to u32
c_right(e) = (W + notch) / 2
c_left(q)  = (W - notch) / 2
center_len = Σ over items with position=='c', type != bracket, drawn on this bar:
             get_length(false) + (has_const_width ? 0 : pl + pr)
y = (u32)(H / 2)
```

Items are processed in **global `bar_items` order**. Skipped items: those not
drawn on this bar (7.1), brackets, and `position=='p'`. For each remaining
item, where `pl/pr` are the item `background.padding_left/right`:

```
disp = get_length(true)
cur  = cursor for position (l→left, c→center, r→right, e→c_right, q→c_left)
rtl  = position in {r, q}
if rtl:  cur = min(cur - disp - pr  /*u32 wrap*/,  W - disp /*double*/)
else:    cur = cur + max(-(int)cur, pl)            // i.e. max(0, cur + pl)
graph.rtl = rtl
(L, R) = shadow extents
slot = calculate_bounds(item, H - (BAR.border_width + 1), L, y)
frame = { x: bar.origin.x + cur - L, y: bar.origin.y, w: disp + L + R, h: H }
window_set_frame(item.window[adid], frame)
if item.popup.drawing: anchor popup (6.3)
advance:
  rtl & const:   cur += disp + pr - custom_width     // net: cursor moves left by custom_width
  rtl & !const:  cur -= pl                           // net: left by disp + pr + pl
  ltr & const:   cur += custom_width - pl            // net: right by custom_width
  ltr & !const:  cur += slot + pr                    // net: right by pl + slot + pr
```

Consequences:

- Right (`r`) and center-left (`q`) items fill from their start point
  **leftwards**, with the first item in global order outermost.
- Left (`l`), center (`c`) and center-right (`e`) items fill rightwards.
- With a constant width, the slot is exactly `custom_width`. `padding_left`
  only shifts the content start, and for LTR items `padding_right` is ignored.
- For RTL items, the `min(..., W - disp)` keeps the item inside the bar. When
  `cur - disp - pr` underflows as u32, it becomes huge and the right-hand side
  wins.

A second pass handles brackets (section 5). The window frame's `x` uses the
item's left shadow extent; content starts at `L` inside the window.

### 4.6 Bar layout, vertical bars (left/right) (bar.c:bar_calculate_bounds_left_right)

Positions map to the vertical axis, where "length" is the item height:

```
left=max(BAR.pl,0); right = Hbar - max(BAR.pr,0)
center = (Hbar - 2*margin - center_len)/2 - 1     // center_len sums get_height(item) + (const?0:pl+pr)
c_right = c_left = Hbar/2                          // notch is always 0 here
for each eligible item (same filter as 4.5):
  dh = get_height(item); disp = get_length(true)
  rtl as in 4.5; cursor update as in 4.5 with dh in place of disp and Hbar in place of W
  calculate_bounds(item, bar_height = dh,
                   x = (u32)((BAR.height - disp)/2. + L),  y = (u32)(dh/2.))
  frame = { x: bar.origin.x - L,
            y: bar.origin.y + cur - max(-y_offset, 0),
            w: BAR.height,                         // bar thickness setting
            h: dh + |y_offset| }
  advance as in 4.5 with dh in place of disp/slot
```

Brackets are **not** laid out in vertical bars: no group pass runs, so
bracket windows are never positioned.


---

## 5. Brackets (group.c)

### 5.1 Structure

- `group.members[0]` is the bracket item itself. `members[1..]` are the
  members.
- Brackets are excluded from side-length sums and from the normal layout pass.
- They draw only their background (`bar_item_draw` returns after the
  background).
- Brackets have their own `drawing`, `associated_*` and `ignore_association`
  settings. `bar_draws_item(bar, bracket)` must be true for the bracket to be
  laid out and drawn.

### 5.2 Bounding box (group.c:group_calculate_bounds), run per bar after all normal items

```
first = member (index>=1, bar_draws_item true) with min window[adid].origin.x   // strict <, first wins ties
last  = member (same filter) with max origin.x + frame.w                        // strict >, first wins ties
if num_members == 1 or no drawn members:
    group.bounds.origin = (-9999,-9999)    // size unchanged
    (window frame then set to these bounds: hidden)
    return
len = max(last.x + last.w + last.padding_right + first.padding_left - first.x, 0)
(L, R) = shadow extents of the BRACKET item (4.4)
group.bounds = { x: first.x - first.padding_left,
                 y: first.y,
                 w: len + L + R,
                 h: first.window.h }
background_calculate_bounds(bracket.background,
                            x = max(L,0), y = y_bar + bracket.y_offset,
                            w = len, h = bracket.background.bounds.h)
bracket.window[adid].frame = group.bounds
if bracket.popup.drawing: anchor popup (6.3)
```

Notes:

- The member window x and width already include the members' own shadow
  extents. The bracket background therefore starts `L` px inside its window,
  and the window itself is **not** shifted left by `L`.
- The bracket background height is the **current**
  `bracket.background.bounds.h`. For brackets this is never computed
  automatically: it is 0 unless `background.height` was set on the bracket or
  inherited from the defaults. So a bracket without an explicit height has a
  zero-height background.
- `y_bar` is `H/2` in a bar.
- Ordering: bracket windows are ordered **below the first item window**
  (bar.c:bar_order_item_windows), so they appear behind their members.

### 5.3 Brackets in popups (popup.c:popup_calculate_bounds)

After the normal popup items are placed, and only if `popup.adid > 0`, each
drawn bracket item in the popup gets:

- `cell = popup.cell_size`. If `group.num_members > 2`, use
  `cell = max(get_height(members[1]), cell_size)`. A bracket with exactly one
  real member uses the plain `cell_size`.
- `ih = horizontal ? row_height : cell`.
- `group_calculate_bounds(group, bar, ih/2)`, then set the window frame to
  `group.bounds`.


---

## 6. Popups (popup.c, bar.c)

### 6.1 Membership

- `popup_add_item(host.popup, item)`:
  - No-op if the item is already a member.
  - Otherwise remove it from its old `parent`'s popup, append it,
    set `item.parent = host` and `popup.needs_ordering = true`.
  - If it is now the only item, call `popup_draw` (normally a no-op until the
    popup has an `adid`).
- `popup_remove_item`:
  - When the last item is removed, the item list is freed and the popup
    window is **closed**. `popup.drawing` stays on.
  - Otherwise the item is removed and the order is kept.
- When the host is destroyed, `popup_destroy` removes and destroys **all
  popup items** (bar_manager_remove_item for each), then closes the window.
- When a popup item is removed (`--remove`), it is removed from every item's
  popup list (bar_manager.c:bar_manager_remove_item).
- A popup item is an ordinary member of `bar_items` (it keeps a global order
  slot). It is filtered out of bar layout and laid out by its host's popup.

### 6.2 Visibility of popup items (bar.c:bar_draws_item, rule P)

A `position=='p'` item is drawn on a bar only if all of these hold:

- `parent != NULL`.
- `parent.popup.drawing` is on.
- `bar.adid == active_adid`.

Rules 1–3 of 7.1 also apply (relative to the active bar's space). Popups
therefore only exist on the **active display**, which is the display with the
cursor or keyboard focus (`display_active_display_adid`). It is polled before
every event (event.c:event_execute → bar_manager_poll_active_display), and a
change emits `display_change`.

### 6.3 Anchor computation

**Host in a bar** (bar.c:bar_calculate_popup_anchor_for_bar_item). This runs
for every laid-out bar item or bracket with `popup.drawing`, only when
`bar.adid == active_adid`:

```
win = host.window[adid]
if !popup.overrides_cell_size:
    popup.cell_size = (bar vertical) ? win.w : win.h
popup_calculate_bounds(popup)                      // pass 1: compute the size (frames use the old anchor/adid)
a = win.origin
if horizontal bar:
    if popup.align=='c': a.x += (win.w - popup.w)/2
    elif popup.align=='l': a.x -= host.padding_left
    else:                  a.x += win.w - popup.w
    a.y += (bar position=='b') ? -popup.h : win.h   // below a top bar, above a bottom bar
else (vertical bar):
    if align=='c': a.y += (win.h - popup.h)/2
    elif align=='l': a.y -= host.padding_left
    else:            a.y += win.h - popup.h
    a.x += (bar position=='r') ? -popup.w : win.w
popup_set_anchor(popup, a, adid)                    // a.y += popup.y_offset; if adid changed:
                                                    //   needs_ordering, mark all popup items needs_update
popup_calculate_bounds(popup)                       // pass 2: real frames
```

`win` includes the host's shadow extents, so the anchor follows the window,
not the background.

**Host inside a popup** (popup.c:popup_calculate_popup_anchor_for_bar_item).
This runs from the parent popup's layout for a member with `popup.drawing`,
only if `parent_popup.adid == active_adid`:

```
win = item.window[parent_popup.adid]
if !sub.overrides_cell_size: sub.cell_size = win.h
popup_calculate_bounds(sub)                         // only pass: frames use the PREVIOUS anchor (1-refresh lag)
a = win.origin
if item.position != 'p' || parent_popup.horizontal:
    same as the horizontal-bar rules above (align c/l/else; below or above by bar position)
elif item.parent:
    hp = item.parent.popup
    a.x = hp.window.origin.x
    a.x += (sub.align=='l') ? -sub.w : hp.window.w   // flyout to the left or right of the parent popup
    a.y -= hp.background.border_width
popup_set_anchor(sub, a, parent_popup.adid)
```

### 6.4 Popup layout (popup.c:popup_calculate_bounds)

Coordinates are screen coordinates (y down). `bw` is
`popup.background.border_width`. Items are visited in **popup order**, and
only drawn (`drawing`) non-bracket items count.

```
y = bw; x = 0; width = 0; row_h = 0; total = 0
has_img = bg.enabled && bg.image.enabled
if has_img: width = image_size(bg.image).w + 2*bw
if horizontal:
    for item: total += pl + pr + get_length(false)
              row_h = max(row_h, max(get_height(item), cell_size))
    if has_img: row_h = max(row_h, image_size.h); x = (width - total)/2   // u32 math
for item:
    cell = max(get_height(item), cell_size)
    ix   = max((int)x + pl, 0)
    ih   = horizontal ? row_h : cell
    iw   = pl + pr + calculate_bounds(item, ih, 0, ih/2)     // x=0: shadow extents ignored in popups
    if popup.adid > 0:
        item.window[popup.adid].frame = { anchor.x + ix, anchor.y + y, get_length(true), ih }
    if item.popup.drawing: anchor nested popup (6.3)
    if vertical: width = max(width, iw); y += cell
    else:        x += iw
(bracket pass, 5.3, only if adid > 0)
if horizontal: if !has_img: width = x + bw;  y += row_h
else:          if !has_img: width += bw
y += bw
popup.background.bounds.size = (width, y)                    // origin stays (0,0)
image_calculate_bounds(bg.image, bw, bw + bg.image.bounds.h/2)
if adid > 0: popup.window.frame = { anchor, (width, y) }
```

Notes:

- Vertical popups stack top to bottom. Each cell is
  `max(item height, cell_size)` tall, and the item is vertically centered in
  its cell (`y = ih/2`).
- **Border quirk.** The width gets `bw` added only **once**, on the right.
  The items start at x=`pl`, not at `bw + pl`. The height includes the border
  on both top and bottom.
- Item windows in popups are `get_length(true)` wide, with no shadow extents.
- `cell_size` defaults to the host window height, so popup rows match the bar
  height unless `popup.height` is set.

### 6.5 Drawing and ordering (popup.c:popup_draw, popup_order_windows)

`bar_draw` calls `popup_draw(host.popup)` for every item drawn on the
**active** bar whose `popup.drawing` is on. This happens before the host's own
redraw check.

`popup_draw`:

1. Return if `!drawing || adid < 1 || num_items == 0`.
2. If the popup window does not exist, create it. Its shadow is disabled and
   its blur is set to `popup.blur_radius`. The default item's popup never
   gets a window.
3. If the window frame did not change and `!host.needs_update`, return.
4. Clear the window and install a mouse tracking area over the whole popup.
5. Draw `popup.background` **with its shadow forced off**.
6. Flush. If `needs_ordering` is set, order the windows.

Window order:

- The popup window gets level `topmost ? kCGPopUpMenuWindowLevel (101) : kCGBackstopMenuLevel+1`.
- Each popup item window gets the same level and is ordered above the
  previous one, starting above the popup window.
- Bracket windows are ordered below the first item window.

Popup item windows are drawn by the normal `bar_draw` loop (they are in
`bar_items`).

### 6.6 Lifecycle notes

- `popup.drawing=off` closes the window. Every change of `drawing` resets
  `adid=0`, so the popup is re-anchored on the next layout.
- `--bar hidden=on` sets `popup.drawing=off` for **all** items
  (bar_manager.c:bar_manager_set_hidden).
- If the host stops being drawn (for example `drawing=off`) while
  `popup.drawing` stays on, the popup is no longer re-laid out or redrawn.
  However, its window is not closed, and its items still pass rule P
  (`parent.popup.drawing` is on), so they keep their last frames.
  Hit-testing via `get_popup_by_wid` ignores popups of hosts that are not
  drawing. *(Derived from the code paths; this is an observable leftover.)*
- `popup.topmost` defaults to **on**.


---

## 7. Visibility, association and redraw

### 7.1 `bar_draws_item(bar, item)` (bar.c)

The rules are evaluated in order. The first rule that matches returns false.

1. `!item.drawing || !bar.shown || bar.hidden` → false.
   - `bar.shown` is false on native fullscreen spaces unless
     `show_in_fullscreen` is set.
   - `bar.hidden` is set by `--bar hidden`.
2. Display association, applied only when `!ignore_association`. Return false
   if either holds:
   - `associated_display != 0` and bit `adid` is not set;
   - `associated_to_active_display` and `bar.adid != active_adid`.

   When **both** a mask and `active` are set, the item shows only on the
   active display, and only if that display is in the mask (intersection).
3. Space association: if `associated_space != 0`, bit `bar.sid` is not set,
   `!ignore_association`, and `type != space` → false. Space items are never
   hidden by their space mask. Their display mask drives them instead (8.6).
4. Rule P for popup items (6.2). It is **not** bypassed by
   `ignore_association`.
5. Otherwise → true.

### 7.2 Shown state (bar_item.c:bar_item_is_shown)

- In `bar_draw(bar)`, each item that passes `bar_draws_item` gets bit
  `adid-1` of `associated_bar` set. Each item that fails has the bit cleared,
  and its window on that bar is moved to `(-9999,-9999)` if it is not
  already there.
- `is_shown = associated_bar != 0`, i.e. the item was drawn on at least one
  bar in its most recent draw.
- The mask is reset to 0 on a forced refresh and on `bar_manager_reset`.
- `is_shown` drives `updates=when_shown`, `scroll_texts`, alias recapture and
  the media cover refresh.

### 7.3 Redraw decision (bar_manager.c:bar_manager_bar_needs_redraw)

A bar is recalculated and redrawn if `bar_needs_update` is set, or if any item
satisfies one of the following. Here `mask = 1<<bar.adid` and
`drawn_here = (associated_bar<<1) & mask`.

- `needs_update && bar_draws_item(bar,item)`
- `!drawing && associated_bar != 0`. A disabled item still has a drawn bit,
  so it must be hidden.
- Unless `ignore_association`:
  - Drawable, `associated_display & mask`, and `!drawn_here`.
  - Drawable, `active`, `active_adid==bar.adid`, and `!drawn_here`.
  - `!active`, `associated_display != 0`, `!(associated_display & mask)`, and
    `drawn_here`.
  - `drawing`, `active`, `drawn_here`, and `bar.adid != active_adid`.
  - For non-space items:
    - `associated_space != 0`, `!(associated_space & 1<<sid)`, and
      `drawn_here`;
    - drawable, `associated_space & 1<<sid`, and `!drawn_here`.

After every refresh, all `needs_update` flags, `needs_ordering` and
`bar_needs_update` are cleared (bar_manager.c:bar_manager_clear_needs_update).

### 7.4 Per-item draw (bar.c:bar_draw)

For each item in global order, with its window for this bar (created lazily):

1. If not drawn on this bar: move the window to nirvana, clear the bar bit and
   continue.
2. Set the bar bit. If `popup.drawing` and this is the active bar, call
   `popup_draw`.
3. If the bar background is being redrawn, apply clipping (`background.clip`
   of the item, icon and label backgrounds).
4. `resized = window_apply_frame(window)`. If neither resized nor
   `needs_update`, continue.
5. If subscribed to `mouse.entered` or `mouse.exited`, (re)assign the mouse
   tracking area to the window frame. It is only assigned during a redraw, so
   a subscription made after the last redraw takes effect at the next redraw
   of that item.
6. Clear the window, `bar_item_draw`, flush.

Window creation (bar_item.c:bar_item_get_window):

- The window opens 1×1 at nirvana, with the window shadow disabled unless
  `item.shadow`, and blur `item.blur_radius`.
- It sets `parent.popup.needs_ordering` if the item has a parent, else the
  global `needs_ordering`.

Global z-order (bar.c:bar_order_item_windows):

- Non-popup item windows are stacked above the bar window in global
  `bar_items` order. Later items are **above** earlier ones.
- Bracket windows go below the first item window.
- The level is the bar window level.


---

## 8. Updates, scripts, events

### 8.1 The clock

- A `CFRunLoopTimer` fires every **1.0 s**, first 1 s after init. It posts
  `SHELL_REFRESH` → `bar_manager_update(forced=false)`
  (bar_manager.c:clock_handler).
- `bar_manager_update(forced)`:
  1. Return if (`frozen && !forced`) or `sleeps` (between `system_will_sleep`
     and wake).
  2. If `forced` (only from `--update`):
     - `handle_space_change(true)`;
     - forced network (`wifi_change`), volume, brightness, power,
       front-app, media and space-windows events. Each emits its event as if
       the value changed.
  3. For **every** item in global order:
     - `bar_item_update(item, sender=NULL, forced, env=NULL)`;
     - if the item is an alias and shown, run `alias_update(alias, false)`.
       If the image changed, mark the item and request a refresh.
  4. If needed or forced, call `bar_manager_refresh(forced)`. A forced refresh
     resets the bar associations, marks all items dirty and resizes the bars.
- `bar_item_update` always returns false, so script runs never cause a
  refresh by themselves. Scripts change the bar by sending `--set` messages.

### 8.2 Subscriptions (bar_item.c:bar_item_parse_subscribe_message)

`--subscribe <item> ev1 ev2 ...`:

- If the item is not found, respond `[!] Subscribe: Item not found '<name>'\n`.
- For each event:
  - `flag = 1<<index` of the event in the custom-event registry
    (registration order).
  - An unknown event responds `[?] Event: '<ev>' not found\n` and adds
    nothing.
  - `update_mask |= flag`.
  - Side effects: subscribing to `volume_change`, `brightness_change`,
    `media_change` or `space_windows_change` starts the corresponding system
    observer.
- Subscriptions accumulate. There is no unsubscribe, except removing the item.
- The mask is copied by `--clone` and by default inheritance.

Built-in events and their bits (custom_events.c:custom_events_init):

| Bit | Name | Bit | Name |
|---|---|---|---|
| 0 | front_app_switched | 9 | mouse.entered.global |
| 1 | space_change | 10 | mouse.exited.global |
| 2 | display_change | 11 | mouse.scrolled.global |
| 3 | system_woke | 12 | volume_change |
| 4 | mouse.entered | 13 | brightness_change |
| 5 | mouse.exited | 14 | power_source_change |
| 6 | mouse.clicked | 15 | wifi_change |
| 7 | mouse.scrolled | 16 | media_change |
| 8 | system_will_sleep | 17 | space_windows_change |

- Custom events (`--add event <name> [<NSDistributedNotification name>]`) get
  bits 18 and up. Registering an existing name is a no-op. The bit field is
  u64, so at most 64 events are supported in total.
- `update_mask` is serialized as a decimal u64.

### 8.3 `bar_item_update(item, sender, forced, env)` (bar_item.c) — exact algorithm

```
is_shown = associated_bar != 0
if is_shown && scroll_texts && counter % 15 == 0:
    text_animate_scroll(icon); text_animate_scroll(label)
    if type == slider: text_animate_scroll(slider.knob)
counter += 1
if (!updates || (update_freq == 0 && sender == NULL)) && !forced:
    return
scheduled = update_freq <= counter
should    = updates_only_when_shown ? is_shown : true
if ((scheduled || sender != NULL) && should) || forced:
    counter = 0
    if (script non-empty) || event_port:
        if env == NULL:
            env = &item.env                              // the item's persistent env
        else:
            for (k,v) in item.env (insertion order): env.set(k, v)   // item vars OVERRIDE event vars
            env.set("NAME", item.name)
        env.set("SENDER", sender ?? (forced ? "forced" : "routine"))
    if script non-empty: fork_exec(script, env)
    if event_port: mach_send(event_port, serialize(env))   // "k\0v\0k\0v\0...\0"
```

Consequences:

- **Routine** updates (`SENDER=routine`) need `updates` on and
  `update_freq > 0`. The counter is incremented by **every** call: clock ticks,
  and also every event delivered to the item and every mouse event. Any
  executed update resets it to 0.
  - The first routine run after creation happens on the tick where
    `counter >= update_freq`. With the counter starting at 0, that is after
    `update_freq` seconds.
  - An event run resets the routine timer.
- With `updates=off`, **all non-forced deliveries** are suppressed: custom
  events, `--trigger`, system events, and the `.global` mouse events.
  Only forced calls still run: `--update` and the item-level `mouse.entered`,
  `mouse.exited`, `mouse.clicked` and `mouse.scrolled`.
- `updates=when_shown`: non-forced runs (routine and events) only happen while
  `is_shown`. While hidden, the counter keeps growing past `update_freq`, so
  the item updates on the first tick after it becomes shown. Events that
  arrive while it is hidden are dropped, not queued.
- `update_freq == 0` only blocks **routine** runs. Events still run.
- `--update` (`forced=true`) runs every item that has a script or mach helper
  with `SENDER=forced`, regardless of `updates`, `update_freq` and the shown
  state.
- When `env == NULL` (routine, forced, `mouse.entered`/`mouse.exited`, and
  `.global` events without info), `SENDER` is written into the item's
  **persistent** env. It stays there and leaks into later `click_script` runs
  (8.5).
- Event env sharing: `bar_manager_custom_events_trigger` passes the **same**
  env object to each subscribed item in turn. Each item adds its own vars and
  `NAME`/`SENDER` to it. **Variables of an earlier item leak to later items**
  that do not define the same key. For example, during `space_change` a
  non-space item that comes after a space item sees that space item's
  `SELECTED`, `SID` and `DID`. Item vars also override caller-supplied vars
  with the same key; for example `--trigger ev NAME=x` is overwritten by the
  item's `NAME`.

### 8.4 Environment variables

The persistent item env (`signal_args.env_vars`) is an ordered map. `set`
removes an existing key and appends the new pair at the end.

| Var | Set by | Value |
|---|---|---|
| `NAME` | `bar_item_set_name`, and per event delivery | Item name |
| `SENDER` | every executed update | Event name, `routine`, or `forced` |
| `SELECTED` | space items | `true`/`false` |
| `SID` | space items (`space=`) | Space index (decimal). `0` until set. |
| `DID` | space items (`display=`) | ADID (decimal). `0` until set. |
| `PERCENTAGE` | slider: end of drag or click | 0..100 |
| `INFO` | event payload (per event, not persistent) | see below |
| `BUTTON`, `MODIFIER` | click | 9.3 |
| `SCROLL_DELTA`, `MODIFIER` | scroll | 9.4 |
| `DID` | `mouse.scrolled.global` payload | ADID of the bar/popup |
| `BAR_NAME`, `CONFIG_DIR` | process env (sketchybar.c, hotload.c) | Binary basename; config directory |

Event `INFO` payloads (bar_manager.c):

| Event | INFO |
|---|---|
| `space_change` | JSON `{\n\t"display-<adid>": <sid>,\n ...}` for every bar (separator `,`; the last entry has none) |
| `display_change` | Active ADID (decimal) |
| `volume_change`, `brightness_change` | `(int)(v*100+0.5)` |
| `wifi_change` | SSID |
| `power_source_change` | State string |
| `media_change` | JSON string from the media source |
| `front_app_switched` | App name (if any) |
| `space_windows_change` | JSON string |
| distributed-notification custom events | Notification info string (if any) |
| `--trigger <ev> K=V ...` | The user-given K=V pairs, split at the first `=`; tokens without `=` are ignored |

`--trigger` with one of the names `space_change`, `display_change`,
`space_windows_change`, `volume_change`, `media_change`, `wifi_change` or
`power_source_change` re-runs the system handler, and user env pairs are
discarded. Any other name (including `front_app_switched`, `system_woke` and
mouse events) is delivered as a custom event with the user pairs. An unknown
name reaches no item.

### 8.5 Script execution (helpers.h:fork_exec, sync_exec)

- `vfork()`. In the child: `alarm(60)`, `setenv` every env pair in order
  (overwriting), then `execvp("/usr/bin/env", ["env","sh","-c",script])`.
- `alarm(60)` survives `exec`, so **a script is killed by SIGALRM after
  60 s** unless it handles or ignores it.
- `SIGCHLD` is ignored in the parent, so there are no zombies and the exit
  status is never observed.
- The working directory is the config file's directory (hotload.c).
- `vfork` shares memory with the parent, so the child's `setenv` calls also
  modify the bar process's own environment. Variables from earlier script
  runs (for example `INFO`, `BUTTON`, `SELECTED`) therefore **persist into
  all later scripts** that do not set them. mbar should decide whether to
  replicate this; see the open questions.
- Scripts run asynchronously. Nothing serializes multiple runs of one item.

### 8.6 Space items (bar_manager.c:bar_manager_update_space_components, handle_space_change)

On every space change (from the system, `--trigger space_change`, `--update`,
or a display change, the last three being forced):

1. Freeze. For each bar:
   - Recompute `dsid` and `sid`.
   - `shown = (space type != fullscreen(4)) || show_in_fullscreen`.
   - Request a forced refresh if `shown` changed.
2. Build `INFO`.
3. For each space item:
   - If `!overrides_association`, set `associated_display` to
     `1 << arrangement(display_of_space(lowest set bit of associated_space))`.
     If the space no longer exists, use `1<<30`.
   - For each bar whose ADID bit is in `associated_display` and whose
     `sid != 0`:
     - If `(!selected || forced)` and the space bit `sid` is in the mask:
       `selected=true`, `updates=true`, `SELECTED=true`.
     - Else if `(selected || forced)` and the bit is not in the mask:
       `selected=false`, `updates=true`, `SELECTED=false`.
     - Else `updates=false`.

     When several bars match, the **last bar wins**.
4. Trigger `space_change`. Only space items whose `updates` was just set to
   true run their script (with `SELECTED`).
5. Unfreeze and refresh.

Notes:

- The user's `updates=` on a space item is overwritten by this mechanism.
  Other events a space item subscribes to are delivered only while its
  `updates` happens to be true.
- Before the first space change, `associated_display` is 0, so the item shows
  on all displays.
- The default script (`icon.highlight=$SELECTED`) is only assigned when no
  script was inherited from the defaults.

### 8.7 `scroll_texts`

- On each `bar_item_update` call where `counter % 15 == 0` and the item is
  shown, `text_animate_scroll` runs on the icon, the label and, for sliders,
  the knob.
- It only acts on a text that meets all of these:
  - `max_chars > 0`;
  - `scroll == 0`;
  - not (`has_const_width && custom_width < width`);
  - `width != 0` and `width != bounds.w` (the text is truncated).
- It queues three float animations of `scroll`:
  1. Linear, from 0 to `bounds.w`, over
     `scroll_duration * bounds.w/width` frames.
  2. A 0-frame jump to `-width`.
  3. Linear, back to 0, over `scroll_duration` frames.
- It then resets the global animator duration and curve. Duration is in
  60 Hz frames.
- The routine timer resets the counter, so in practice a scroll starts on the
  call right after each executed update, or every 15 calls.


---

## 9. Mouse handling (mouse.c, event.c, bar_item.c)

### 9.1 Event sources

- Carbon handlers are installed for MouseUp, MouseDragged, MouseEntered,
  MouseExited, MouseWheelMoved and MouseScroll (mouse.c). Each is converted
  to a CGEvent and posted to the main thread.
- **There is no mouse-down handling.** Clicks fire on button **release**.
- `wid` is `CGEventGetIntegerValueField(ev, 0x33)`, the window number under
  the pointer.
- `point` is the global location (y down).
- Entered and exited events only exist for windows with a tracking area:
  - bar windows (always);
  - popup windows (assigned on draw);
  - item windows of items subscribed to `mouse.entered` or `mouse.exited`
    (7.4).

### 9.2 Hit testing (bar_manager.c)

- `get_item_by_wid(wid)`: the first item in global order, with
  `drawing=on`, that has any window (any ADID) whose id equals `wid`.
  Popup items are included.
- `get_item_by_point(p)`: the first item in global order, with `drawing=on`,
  that has a window whose rect `[origin, origin+size]` contains `p`.
  The rect test is **inclusive** on all edges.
- `get_bar_by_wid` / `get_bar_by_point`: matching bar window (the point test
  is `CGRectContainsPoint`: half-open).
- `get_popup_by_wid` / `get_popup_by_point`: popups of items with
  `drawing && popup.drawing`, matched against the popup window.

### 9.3 Click (event.c:event_mouse_up → bar_item.c:bar_item_on_click)

```
item = get_item_by_wid(wid)
if !item || item.type == bracket: item = get_item_by_point(point)
if !item && !popup_by_wid && !bar_by_wid: return
local = item && window ? point - window.origin : (0,0)
on_click(item, cg_event_type, button_number, flags, local)   // returns at once if !item
if item.needs_update: refresh
```

Clicking a bracket's own window (the area between or around members) usually
resolves to the bracket itself via the point search, so the bracket's click
handling runs.

`on_click`:

1. Build a fresh env with these keys:
   - `INFO` = the exact text
     `"{\n\t\"button\": \"<b>\",\n\t\"button_code\": <n>,\n\t\"modifier\": \"<m>\",\n\t\"modfier_code\": <flags>\n}\n"`.
     The misspelling `modfier_code` is in the source. `<flags>` is the raw
     `CGEventFlags` truncated to u32, so it includes bits such as `0x100`.
   - `BUTTON = <b>`: `left` for LeftMouseUp, `right` for RightMouseUp,
     `other` otherwise.
   - `MODIFIER = <m>`: a comma-joined subset of `shift,ctrl,alt,cmd,fn` in
     that order, or `none` when empty.
2. Sliders: if `slider.is_dragged` or `local` is inside the track background
   bounds:
   - `slider_handle_drag(local)` sets the percentage from x:
     `round(max(local.x - track.x, 0)/track_w*100)`, capped at 100.
   - `bar_item_cancel_drag`: set persistent env `PERCENTAGE=<pct>` and
     `is_dragged=false`.

   Otherwise **return**: a click on a slider item outside its track runs
   neither `click_script` nor `mouse.clicked`. The containment test uses the
   y-down `local` against the y-up drawing bounds; it is symmetric for a
   vertically centered track.
3. If `click_script` is non-empty, copy the item's persistent env into the
   click env and `fork_exec(click_script, env)`. Notes:
   - `NAME` is present because it lives in the persistent env.
   - `SENDER` is **stale**: whatever the last update stored, or absent.
4. If subscribed to `mouse.clicked`, run
   `bar_item_update(item, "mouse.clicked", forced=true, env)`. This runs
   `script` (and the mach helper) with `SENDER=mouse.clicked`.

   Both `click_script` and `script` run if both conditions hold. `script` is
   never run on click without the subscription.

### 9.4 Scroll (event.c:event_mouse_scrolled)

- `delta = kCGScrollWheelEventDeltaAxis1` (vertical, integer).
- Coalescing, with a 150 ms timeout. `ts` and `acc` are global:

  ```
  now = monotonic ns
  if ts + 150ms > now: acc += delta; return            // swallowed, accumulated
  if ts + 300ms < now: acc = 0                          // discard a stale accumulation
  ts = now
  total = delta + acc; acc = 0 (after dispatch)
  ```

- Target selection:

  ```
  item = get_item_by_wid(wid); if !item || bracket: item = get_item_by_point(point)
  if !item:
      if bar = bar_by_wid: if bar.mouse_over && !mouse_over_any_popup: scrolled_global(total, bar.adid)
                           acc = 0; return
      if popup = popup_by_wid: if popup.mouse_over && !mouse_over_any_bar: scrolled_global(total, popup.adid)
                           acc = 0; return
  on_scroll(item, total, flags)       // no-op when item is NULL
  ```

- `on_scroll` builds a fresh env with these keys:
  - `INFO` = `"{\n\t\"delta\": <d>,\n\t\"modifier\": \"<m>\",\n\t\"modfier_code\": <flags>\n}\n"`;
  - `SCROLL_DELTA = <d>`;
  - `MODIFIER = <m>`.

  If subscribed to `mouse.scrolled`, it runs
  `bar_item_update(item, "mouse.scrolled", forced=true, env)`. There is no
  scroll script analogous to `click_script`.
- `mouse.scrolled.global` builds an env with `SCROLL_DELTA`, `INFO` (same
  format), `DID=<adid>` and `MODIFIER`, and delivers it as a **non-forced**
  custom event to all subscribers.
  - It only fires when the pointer is over the bar background or the popup
    background, not over an item window.
  - It only fires while the matching `mouse_over` flag is set.

### 9.5 Enter and exit (event.c:event_mouse_entered, event_mouse_exited)

**Entered** (`wid`):

- Bar window: if `!bar.mouse_over && !mouse_over_any_popup`, set
  `bar.mouse_over = true` and trigger `mouse.entered.global` (non-forced,
  env NULL). Return.
- Popup window: if `!popup.mouse_over && !mouse_over_any_bar`, set
  `popup.mouse_over = true` and trigger `mouse.entered.global`. Return.
- Otherwise `item = get_item_by_wid(wid)`, then `bar_item_mouse_entered`:
  if subscribed to `mouse.entered` and `!item.mouse_over`, run a forced update
  with `SENDER=mouse.entered` (persistent env). Then
  `item.mouse_over = true`, regardless of subscription.

**Exited** (`wid`, `point`):

- If `wid` is a bar window: the target is the popup under `point`.
  If `wid` is a popup window: the target is the bar under `point`.
- For bar and popup windows, compute
  `over_origin = point ∈ inset(origin_window_rect, 1, 1)` and act as follows:
  - `!over_origin && !over_target`: clear `mouse_over` of the origin and
    trigger `mouse.exited.global`. Then call `bar_item_mouse_exited` on
    **every** item: each item subscribed to `mouse.exited` gets a forced
    `mouse.exited` run, whether or not it was hovered, and every item's
    `mouse_over` is cleared.
  - `!over_origin && over_target`, coming from a bar: clear
    `bar.mouse_over` and set `target_popup.mouse_over`. No events.
  - `!over_origin && over_target`, coming from a popup: set
    `target_bar.mouse_over` and clear `popup.mouse_over`. If the popup host
    subscribes to `mouse.exited` or `mouse.exited.global`, and the item under
    `point` is not the host, call `bar_item_mouse_exited(host)`.
  - `over_origin`: nothing.

  Then return.
- An item window: if the item subscribes to `mouse.exited.global` and the
  popup under `point` is the item's **own** popup, ignore the exit (hover
  carries over into its popup). Otherwise call `bar_item_mouse_exited(item)`:
  if subscribed to `mouse.exited`, run a forced update with
  `SENDER=mouse.exited` (no `mouse_over` precondition), then set
  `mouse_over = false`.

### 9.6 Drag (event.c:event_mouse_dragged)

- `item = get_item_by_wid(wid)`. Only sliders respond.
- `local = point - window.origin`. `slider_handle_drag` sets
  `is_dragged=true` and the percentage. If the percentage changed, the item
  is marked dirty and refreshed.
- No script runs during the drag.
- On release, the click path (9.3) finalizes the percentage, sets
  `PERCENTAGE`, clears `is_dragged`, and runs `click_script` and
  `mouse.clicked`.
- While dragging, `slider.percentage=` and `slider.width=` from scripts are
  ignored.

