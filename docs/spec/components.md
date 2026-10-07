# SketchyBar component spec (text, font, background, image, color, shadow, graph, slider, alias)

Source of truth: SketchyBar C source at commit `5f358ec` ("use opentype directly (#828)").
Files: `src/text.c`, `src/font.c`, `src/background.c`, `src/shadow.c`, `src/image.c`,
`src/color.c`, `src/graph.c`, `src/slider.c`, `src/alias.c` plus the consumers
`src/bar_item.c`, `src/bar.c`, `src/message.c`, `src/window.c`, `src/workspace.m`,
`src/media.m`, `src/animation.[ch]`, `src/misc/helpers.h`, `src/context.c`.

Citations use `file:function`. "Quirk" marks behaviour that looks unintended but is
observable; mbar must reproduce it unless `docs/EXTENSIONS.md` says otherwise.

## 0. Conventions shared by all components

### 0.1 Coordinate system and drawing surface

* Every bar item is drawn into **its own window** (one per display, `bar_item.c:bar_item_get_window`).
  Drawing uses a CoreGraphics bitmap context created by `context.c:context_create`:
  * pixel size = `ceil(size * 2.0)` — the scale is **always 2.0**, independent of the display's
    backing scale (`surface.c:surface_create`, `SLSSetWindowResolution(…, 2.0)`).
  * `CGContextScaleCTM(2,2)`, so all math below is in points.
  * `CGContextSetInterpolationQuality(kCGInterpolationNone)` — images are scaled with
    **nearest neighbour**.
  * `CGContextSetAllowsFontSmoothing(ctx, bar.font_smoothing)` (default off).
  * Pixel format: 8-bit premultiplied ARGB, DeviceRGB.
* CoreGraphics coordinates: origin **bottom-left**, y grows **up**. Every y formula in this
  document is in that system unless noted. mbar's geometry is top-left (`ARCHITECTURE.md`), so
  convert with `y_top = window_height - y_cg` (rects: `y_top = window_height - (y + h)`).
* Item vertical centre: `y = (uint32)(window_height / 2)` (integer division of the bar window
  height, `bar.c:bar_calculate_bounds_top_bottom`), then `content_y = y + item.y_offset`.
* Each item window is `bar_item_display_length + shadow_offsets.x + shadow_offsets.y` wide; the
  item content starts at local `x = max(shadow_offsets.x, 0)` (see §3.8 for the shadow
  offsets).

### 0.2 Property value parsing (`misc/helpers.h`)

| helper | used for | semantics |
|---|---|---|
| `token_to_int` | most ints, colors in text/background/shadow/image | `(int)strtol(s, NULL, 0)` — base auto-detect (`0x..` hex, `0..` octal, else decimal); garbage → 0; trailing garbage ignored. Colors > 0x7fffffff wrap to negative int but keep the same 32 bits. |
| `token_to_uint32t` | graph colors, slider props, alias props, image corner_radius | `strtoul(s, NULL, 0)`; a negative number wraps (e.g. `-1` → 0xffffffff). |
| `token_to_float` | font size, scale, clip, color channels, line_width, border_width (image) | `strtof(s, NULL)`; garbage → 0.0. |
| `evaluate_boolean_state(tok, prev)` | every bool | true for exactly `on`,`yes`,`true`,`1`,`!off`,`!no`,`!false`,`!0`; `toggle` → `!prev`; **anything else (incl. empty) → false**. |
| `get_key_value_pair(s, '.')` | sub-domain dispatch | splits at the **first** `.`; `a.b.c` → key `a`, value `b.c`. No `.` → no pair; trailing `.` → value NULL (treated as no pair). |

The value of a `key=value` pair is everything after the first `=` (`message.c:reformat_batch_key_value_pair`).
An empty value (`label=`) is a zero-length token: strings become `""`, ints/floats become 0,
bools become false.

### 0.3 Animation macros (`animation.h`)

Every setter is invoked through one of three macros. When `--animate <curve> <duration>` is
active (`animator.duration > 0`) the macro cancels *locked* animations for the same
`(target, setter)` and queues a new animation from the current value `p` to target `t`; it
returns "no refresh needed" (the animator refreshes per frame). Otherwise it cancels any running
animation for `(target, setter)` (applying that animation's final value) and calls the setter
directly; the property reports "changed" if either did.

| macro | interpolation |
|---|---|
| `ANIMATE` | integer: `v = (1-s)*a + s*b + 0.5` truncated; final frame exactly `b`. |
| `ANIMATE_FLOAT` | `f32` bit-cast through `int`; `v = (1-s)*a + s*b` (no rounding). |
| `ANIMATE_BYTES` | 32-bit colour: each of the 4 bytes interpolated independently, `byte = (u8)((1-s)*a_i + s*b_i)` (truncation). |

Duration is in frames of a 60 Hz display: seconds = `duration / 60` (`animation.c:animation_setup`).
Properties marked "anim: –" in the tables below are set immediately even inside `--animate`.

### 0.4 Change reporting

Setters return `bool changed`; when true the owning item is flagged `needs_update` and redrawn.
Setters marked "no refresh" in the tables store the value but report `false` (the change shows up
at the next redraw triggered by something else).

### 0.5 Colour hex in JSON

All colours serialize as `"0x%x"`: lowercase, **no zero padding** (`0x0`, `0xff0000`,
`0xffcccccc`). Booleans serialize as `"on"` / `"off"` (`format_bool`). A NULL C string printed
with `%s` yields `(null)` (macOS libc) — this happens for an image that was never loaded.
Strings are **not escaped** in component JSON (a `"` in a label produces invalid JSON) — quirk.

## 1. Color (`color.c`)

### 1.1 Model

```
struct color { f32 r, g, b, a; u32 hex; }   // hex = 0xAARRGGBB
```

* `color_set_hex(hex)`: `a = ((hex>>24)&0xff)/255`, `r = ((hex>>16)&0xff)/255`,
  `g = ((hex>>8)&0xff)/255`, `b = (hex&0xff)/255` (each clamped to [0,1]); then recompute `hex`.
* `color_update_hex`: `hex = (u32)(a*255)<<24 | (u32)(r*255)<<16 | (u32)(g*255)<<8 | (u32)(b*255)`
  — **truncation**, not rounding (`alpha=0.5` → `0x7f`). Returns `prev_hex != hex`; so a
  change of a float channel that does not change the truncated byte reports "unchanged".
* Float setters clamp to [0,1].
* Drawing always uses the float channels (`CGContextSetRGBFillColor/StrokeColor(r,g,b,a)`).

### 1.2 Sub-properties (`color_parse_sub_domain`)

Reachable as `<owner>.<color-name>.<prop>` wherever a colour sub-domain exists (table in §1.3).

| prop | type | parser | anim | effect |
|---|---|---|---|---|
| `hex` | u32 | `token_to_int` | BYTES | `color_set_hex` |
| `alpha` | f32 | `token_to_float` | FLOAT | `a = clamp(v)` |
| `red` | f32 | `token_to_float` | FLOAT | `r = clamp(v)` |
| `green` | f32 | `token_to_float` | FLOAT | `g = clamp(v)` |
| `blue` | f32 | `token_to_float` | FLOAT | `b = clamp(v)` |
| other | – | – | – | responds `[?] Color: Invalid property '<garbage>'\n` (bug: passes a struct to `%s`; mbar prints the property name) |

Important: setting a colour through these sub-properties **never** has the side effects of the
owner's plain `color=` setter (e.g. `background.color.alpha=1` does **not** enable the background,
`shadow.color.hex=…` does **not** enable the shadow, `graph.fill_color.hex` does **not** set
`overrides_fill_color`). Exception: `alias.color.<prop>` sets `color_override` (§8).

### 1.3 Where colour sub-domains exist

| path | colour |
|---|---|
| `icon.color.*`, `label.color.*`, `slider.knob.color.*` | text colour |
| `icon.highlight_color.*`, `label.highlight_color.*` | text highlight colour |
| `*.background.color.*`, `*.background.border_color.*` | background fill / border |
| `*.shadow.color.*` (text, background, image, alias) | shadow colour |
| `*.background.image.border_color.*` | image border |
| `graph.color.*`, `graph.fill_color.*` | graph line / fill |
| `alias.color.*` | alias tint |

## 2. Shadow (`shadow.c`)

### 2.1 Model and defaults (`shadow_init`)

| field | type | default |
|---|---|---|
| `enabled` (`drawing`) | bool | `false` |
| `angle` | u32 degrees | `30` |
| `distance` | u32 | `5` |
| `color` | color | `0xff000000` |
| `offset` (derived) | CGPoint | `x = distance*cos(angle°)`, `y = -distance*sin(angle°)` |

With defaults: `offset = (4.330127, -2.5)` — shadow goes right and **down** (CG y-up).
`deg_to_rad = 2π/360`; computed in double, stored as CGFloat. Recomputed whenever angle or
distance changes.

There is **no blur**: a shadow is a hard-edged copy of the shape drawn in the shadow colour,
translated by `offset` (`shadow_get_bounds` returns `rect` translated by `offset`, same size).

### 2.2 Properties (`shadow_parse_sub_domain`)

Reachable as `icon.shadow.*`, `label.shadow.*`, `<…>.background.shadow.*`,
`<…>.background.image.shadow.*`, `slider.knob.shadow.*`, `alias.shadow.*`.

| prop | type | parser | anim | notes |
|---|---|---|---|---|
| `drawing` | bool | bool | – | enable/disable |
| `distance` | u32 | `token_to_int` (negative wraps) | INT | recompute offset |
| `angle` | u32 | `token_to_int` | INT | recompute offset |
| `color` | u32 | `token_to_int` | BYTES | **also sets `enabled = true`** (`shadow_set_color`) |
| `color.<hex/alpha/red/green/blue>` | | | | colour sub-domain; does **not** enable |
| other `x.y` | | | | `[!] Shadow: Invalid subdomain '<x>'\n` |
| other | | | | `[!] Shadow: Invalid property '<p>'\n` |

### 2.3 JSON (`shadow_serialize`), fields at indent `I`

```
I"drawing": "off",
I"color": "0xff000000",
I"angle": 30,
I"distance": 5
```
(no trailing newline; `angle`/`distance` printed with `%u`).

## 3. Font (`font.c`)

### 3.1 Model and defaults (`font_init`)

| field | default | notes |
|---|---|---|
| `family` | `"Hack Nerd Font"` | |
| `style` | `"Bold"` | |
| `size` | `14.0` (f32) | |
| `features` | `NULL` | comma separated feature list |
| `typographical_width` | `false` | |
| `font_changed` | false | dirty flag; CTFont rebuilt lazily |
| `ct_font` | built in `font_init` | |

### 3.2 CTFont construction (`font_create_ctfont`)

1. Attributes dict: `kCTFontFamilyNameAttribute = family`, `kCTFontStyleNameAttribute = style`,
   `kCTFontSizeAttribute = size (Float32)`.
2. `CTFontDescriptorCreateWithAttributes`.
3. If `features` set: split on `,` (`strtok`, so empty entries are skipped). For each entry:
   * if it matches `"%d:%d"` → AAT feature: `{kCTFontFeatureTypeIdentifierKey: n,
     kCTFontFeatureSelectorIdentifierKey: m}` (both `int`).
   * else: optional leading `+` (value 1) or `-` (value 0), default value 1; the remainder must be
     exactly 4 bytes (`strlen == 4`) else the entry is skipped silently →
     `{kCTFontOpenTypeFeatureTag: tag, kCTFontOpenTypeFeatureValue: value}`.
   * If ≥ 1 setting: `CTFontDescriptorCreateCopyWithAttributes(desc, {kCTFontFeatureSettingsAttribute: [settings]})`.
4. `CTFontCreateWithFontDescriptor(desc, 0.0, NULL)`.

Fallback: there is no explicit fallback logic. If the family/style does not exist, CoreText's
descriptor matching picks its default substitute font; missing glyphs are rendered by CoreText's
automatic font cascade inside `CTLine`. mbar must use CoreText the same way (descriptor matching
+ CTLine) to get identical substitution.

The font is rebuilt only when `font_changed` is set and a layout happens
(`text.c:text_prepare_line` / `text_get_length`, §4.4). `font_init` builds immediately.

### 3.3 `font=<Family>:<Style>:<Size>` (`font_set`)

`sscanf(value, "%254[^:]:%254[^:]:%f", family, style, &size)` with `size` pre-initialised to
**10.0** and both strings pre-initialised to `""`:

| input | family | style | size |
|---|---|---|---|
| `Hack Nerd Font:Bold:17.0` | `Hack Nerd Font` | `Bold` | 17.0 |
| `SF Pro:Semibold` | `SF Pro` | `Semibold` | **10.0** |
| `Menlo` | `Menlo` | `""` | **10.0** |
| `:Bold:12` | `""` | `""` | 10.0 (scan stops at the empty first field) |
| `Fam::12` | `Fam` | `""` | 10.0 (empty second field stops scan) |

Then `family` and `style` are set with string-equality change detection (non-forced), and size
with `==`. Returns true if any changed. `font=` is **not animated** (size jumps). Fields longer
than 254 bytes are truncated.

### 3.4 Font sub-properties (`font_parse_sub_domain`) — `icon.font.*`, `label.font.*`, `slider.knob.font.*`

| prop | type | anim | notes |
|---|---|---|---|
| `family` | string | – | string compare |
| `style` | string | – | string compare |
| `size` | f32 | FLOAT | `token_to_float` |
| `features` | string | – | e.g. `+tnum,-liga,1:0`; compared as string |
| `typographical_width` | bool | – | see §4.3 |
| other | | | `[!] Text: Invalid property '<p>'\n` |

### 3.5 `--load-font <path>` (`font_register`)

`CFURLCreateWithString(path)` (a URL *string*, not `CFURLCreateWithFileSystemPath`) →
`CTFontManagerRegisterFontsForURL(url, kCTFontManagerScopeProcess, NULL)`. No response on
failure. (Open question: plain absolute paths yield a scheme-less relative URL; whether
CoreText accepts it must be verified — mbar should accept both `file://` URLs and plain paths.)

### 3.6 Serialization

Only inside text JSON: `"font": "<family>:<style>:<size %.2f>"` e.g. `"Hack Nerd Font:Bold:14.00"`.
`features` and `typographical_width` are **not** serialized.

## 4. Text (`text.c`) — `icon`, `label`, `slider.knob`

### 4.1 Model and defaults (`text_init`)

| field | type | default | property |
|---|---|---|---|
| `string` | UTF-8 string | `""` | `string` (and shorthand `icon=…`, `label=…`, `slider.knob=…`) |
| `drawing` | bool | `true` | `drawing` |
| `highlight` | bool | `false` | `highlight` |
| `color` | color | `0xffffffff` | `color` |
| `highlight_color` | color | `0xff000000` | `highlight_color` |
| `padding_left` | i32 | 0 | `padding_left` |
| `padding_right` | i32 | 0 | `padding_right` |
| `y_offset` | i32 | 0 | `y_offset` |
| `font` | font | §3.1 | `font`, `font.*` |
| `has_const_width` / `custom_width` | bool / u32 | false / 0 | `width` |
| `align` | char | `'l'` | `align` |
| `max_chars` | u32 | 0 (= unlimited) | `max_chars` |
| `scroll_duration` | u32 frames | 100 | `scroll_duration` |
| `scroll` | f32 | 0 | internal (scroll animation offset) |
| `shadow` | shadow | §2.1 | `shadow.*` |
| `background` | background | §5.1 | `background.*` |
| derived: `width` (f32, integral), `bounds` (CGRect), `line` (CTLine, ascent, descent) | | | |

### 4.2 Property dispatch (`text_parse_sub_domain`), checked in this order

| prop | type / parser | anim | change result | semantics |
|---|---|---|---|---|
| `color` | u32 `token_to_int` | BYTES | changed | `color_set_hex` |
| `highlight` | bool | special | `old != new` | see §4.6 |
| `font` | `Family:Style:Size` (rest of message) | – | changed | §3.3 |
| `highlight_color` | u32 `token_to_int` | BYTES | changed | |
| `padding_left` | i32 `token_to_int` | INT | changed | |
| `padding_right` | i32 | INT | changed | |
| `y_offset` | i32 | INT | changed | |
| `scroll_duration` | i32 | – | **no refresh** | negative ignored; else stored |
| `width` | `dynamic` or i32 | INT | changed | §4.5 |
| `drawing` | bool | – | `old != new` | |
| `align` | first byte of value | – | `old != new` | `l`/`c`/`r` meaningful; any byte stored (empty → `'\0'`) |
| `string` | string | special | changed | §4.4, §4.5 |
| `max_chars` | u32 `token_to_int` | – | see below | §4.7 |
| `background.<p>` | | | | §5 |
| `shadow.<p>` | | | | §2 |
| `font.<p>` | | | | §3.4 |
| `color.<p>` / `highlight_color.<p>` | | | | §1.2 |
| other `x.y` | | | | `[!] Text: Invalid subdomain '<x>' \n` |
| other | | | | `[!] Text: Invalid property '<p>'\n` |

### 4.3 Line preparation (`text_prepare_line`) — run whenever the string is set (or forced)

1. If `font.font_changed`: rebuild CTFont, clear flag.
2. Attributes: `{kCTFontAttributeName: ct_font, kCTForegroundColorFromContextAttributeName: true}`
   — the glyph colour comes from the context fill colour at draw time.
3. `CFStringCreateWithCString(string, UTF8)`; on invalid UTF-8 the line is built from the literal
   `"Warning: Malformed UTF-8 string"` instead (the stored `string` is unchanged).
4. `line = CTLineCreateWithAttributedString`.
5. `typo_w = CTLineGetTypographicBounds(line, &ascent, &descent, NULL)` (descent is positive).
6. `bounds = CTLineGetBoundsWithOptions(line, kCTLineBoundsUseGlyphPathBounds)` (ink box), then
   * `bounds.w = (u32)(bounds.w + 1.5)`, `bounds.h = (u32)(bounds.h + 1.5)` ("+1 px against clipping"),
   * `bounds.x = (i32)(bounds.x + 0.5)`, `bounds.y = (i32)(bounds.y + 0.5)` (both overwritten later by layout).
7. `width = typographical_width ? (u32)(typo_w + 0.5) : bounds.w`.
8. If `max_chars > 0`: truncated width (overrides step 7 even with `typographical_width`): take the
   byte prefix containing the first `max_chars` code points (a byte starts a code point iff
   `(b & 0xC0) != 0x80`), build a second CTLine with the same attributes and set
   `width = (u32)(glyph_path_bounds(prefix).w + 1.5)`.

Consequences (must be reproduced):
* Width is the **ink** width: leading/trailing spaces do not add width unless
  `font.typographical_width=on`; the ink's left bearing (`bounds.x` from step 6) is *not*
  compensated when drawing (text is drawn at the pen origin).
* Empty string: ink box is empty → `width = 1`, `bounds.h = 1` (typographical: `width = 0`).
* `ascent`/`descent` are typographic (font-wide), independent of the string's glyphs.

### 4.4 Length and height

`text_get_length(text, override)`:
```
if !drawing: return 0
if font.font_changed: text_set_string(text, text.string, forced=true)   // relayout
len = width + padding_left + padding_right                              // i32
if (!has_const_width || override) && background.enabled && background.image.enabled:
    iw = image_get_size(background.image).width
    if iw > len: return iw
if has_const_width && !override: return custom_width
return max(len, 0)
```
`text_get_height(text) = drawing ? bounds.h : 0` (ink height + 1.5, truncated).
Note: font changes only re-layout lazily at the next `text_get_length` call.

`text_set_string(text, s, forced)`: no-op (returns false) if `!forced` and `s == string`;
otherwise replace and run §4.3; returns true.

### 4.5 Width property and string-change animation

`text_set_width(w)`: `w < 0` → `has_const_width = false` (changed iff it was true). Else if
`custom_width == w && has_const_width` → unchanged; else `custom_width = w, has_const_width = true`.

* `width=<n>`: `ANIMATE(text_set_width, from = text_get_length(false), to = n)`.
* `width=dynamic`: `ANIMATE(text_set_width, from = custom_width, to = text_get_length(true))`,
  then **always** append a zero-duration linear animation to `-1`. Without `--animate` the
  first step applies immediately and the `-1` step is applied by the animator on the next
  display-link frame (so the end state is "dynamic", `custom_width` keeps the natural length).
  With `--animate` the `-1` step is chained after the width animation.
* `string=<s>` while `--animate` is active and the string changed: `pre = text_get_length(false)`
  (before), `post = text_get_length(false)` (after); if different: `text_set_width(pre)`,
  `ANIMATE(text_set_width, pre → post)`, then chain the `-1` step. Effect: the text cell width
  animates; the glyphs change instantly. If the text had a constant width, `pre == post ==
  custom_width` and nothing animates.

### 4.6 `highlight`

`new = evaluate_boolean_state(value, highlight)`. If `--animate` is active:
* on → off: cancel `color` animations of this text; `target = color.hex`;
  `color = highlight_color` (immediately); `ANIMATE_BYTES(color: highlight_color → target)`.
* off → on: cancel `highlight_color` animations; `target = highlight_color.hex`;
  `highlight_color = color`; `ANIMATE_BYTES(highlight_color: color → target)`.

Then `highlight = new` immediately. Drawing uses `highlight ? highlight_color : color`.
Result: an animated cross-fade between the two colours.

### 4.7 `max_chars` (`text_set_max_chars`)

```
if max_chars == n: return false
max_chars = n
if strlen(string) > n: text_set_string(string, forced)   // relayout, recompute truncated width
return strlen(string) > n
```
`strlen` counts **bytes**, the truncation counts code points. Quirk: raising `max_chars` to a
value ≥ the byte length (or to 0 when the string is empty) does **not** relayout, so the old
truncated `width` stays until the next string/font change; setting 0 with a non-empty string
relayouts (untruncated).

### 4.8 Layout (`text_calculate_bounds(x, y)`)

`x` is the left edge of the text cell (item-local, CG coords), `y` the vertical centre.

```
natural = text_get_length(text, override=true)
if align == 'c' && has_const_width: bounds.x = x + ((i32)custom_width - natural) / 2   // int division
elif align == 'r' && has_const_width: bounds.x = x + custom_width - natural
else: bounds.x = x
bounds.y = (u32)(y - (ascent - descent) / 2)          // baseline: typographic box centred on y
if background.enabled:
    h = background.overrides_height ? background.bounds.h : bounds.h
    background_calculate_bounds(background, x, y, text_get_length(text, false), h)
```
`align` only matters with a constant width. The text background always starts at the
**unaligned** cell `x` and spans the full (constant) cell width. `text.y_offset` moves only the
glyphs (and their shadow), not the text background.

### 4.9 Drawing (`text_draw`)

```
if !drawing: return
if background.enabled: background_draw(background)          // §5.5
save
if max_chars > 0:
    clip to rect (bounds.x + padding_left, -9999, width, 19998)   // horizontal clip only
if shadow.enabled:
    fill = shadow.color
    pen = (bounds.x + shadow.offset.x + padding_left, bounds.y + shadow.offset.y + y_offset)
    CTLineDraw(line)
fill = highlight ? highlight_color : color
pen = (bounds.x + padding_left - scroll, bounds.y + y_offset)
CTLineDraw(line)
restore
```
* Truncation is a clip of the full line — no ellipsis.
* Quirk: the shadow ignores `scroll` (it stays put while the text scrolls) but is clipped.
* The text pen x/y are not pixel-snapped beyond the integer bounds arithmetic.

### 4.10 Scroll animation (`text_animate_scroll`)

Triggered from `bar_item.c:bar_item_update` for `icon`, `label` and (slider items) `slider.knob`
when the item is shown, the item property `scroll_texts=on`, and `item.counter % 15 == 0`
(checked *before* `counter++`; `counter` is incremented on each `bar_item_update` call — the
1 s routine tick and every event delivery — and reset to 0 whenever the item's script runs).

```
if max_chars == 0 || scroll != 0: return
if has_const_width && custom_width < width: return     // quirk
if width == 0 || width == bounds.w: return            // not truncated
D1 = (u32)(scroll_duration * (bounds.w / width))
A1: scroll 0 → bounds.w, linear, D1 frames
A2: chained after A1: jump to -width (duration 0)
A3: chained after A2: -width → 0, linear, scroll_duration frames
```
Visual: the text slides left until its end reaches the clip start, re-enters from the right
edge of the clip window and slides back to rest. Speed is `width / scroll_duration` points per
60 Hz frame in both phases. A new cycle can start only after `scroll` is back to exactly 0.
If `scroll_duration == 0` nothing visible happens.

### 4.11 JSON (`text_serialize`), fields at indent `T`

```
T"value": "<string, unescaped>",
T"drawing": "on",
T"highlight": "off",
T"color": "0xffffffff",
T"highlight_color": "0xff000000",
T"padding_left": 0,
T"padding_right": 0,
T"y_offset": 0,
T"font": "Hack Nerd Font:Bold:14.00",
T"width": 0,                     // raw custom_width (%d) even when not constant (stale value)
T"scroll_duration": 100,
T"align": "left",                // l→left r→right c→center b→bottom t→top else "invalid"
T"background": {
<background_serialize(detailed=true) at T+\t>
T},
T"shadow": {
<shadow_serialize at T+\t>
T}
```
No trailing newline after the final `}`. `max_chars`, `font.features`,
`font.typographical_width` are not serialized.

### 4.12 Inheritance (`text_copy`, used by `--default` inheritance and `--clone`)

The whole item struct is `memcpy`'d from the template, pointers are cleared, then:
family, style (forced), size, typographical_width and string are copied; the forced string set
rebuilds the CTFont and the line immediately.
`font.features` is **not** copied (quirk: features are lost on inherited/cloned items).
Background images: `CGImageCreateCopy` of the template image; `path` is **not** copied (it
serializes as `(null)`), `enabled`/`size`/`bounds`/`scale`/`link` are copied by the memcpy.

## 5. Background (`background.c`)

Used by: item (`background.*`, the item's own `padding_left/right` live here), `icon.background`,
`label.background`, `slider.knob.background`, slider track/fill (§7), brackets, popups
(`popup.background.*`) and the bar itself (`--bar color=…` etc.).

### 5.1 Model and defaults (`background_init`)

| field | type | default | property |
|---|---|---|---|
| `enabled` | bool | `false` | `drawing` |
| `color` | color | `0x00000000` | `color` |
| `border_color` | color | `0x00000000` | `border_color` |
| `border_width` | u32 | 0 | `border_width` |
| `bounds.size.height` + `overrides_height` | f64 / bool | 0 / false | `height` |
| `corner_radius` | u32 | 0 | `corner_radius` |
| `padding_left` / `padding_right` | i32 | 0 / 0 | `padding_left` / `padding_right` |
| `x_offset` / `y_offset` | i32 | 0 / 0 | `x_offset` / `y_offset` |
| `clip` | f32 | 0.0 | `clip` |
| `shadow` | shadow | §2.1 | `shadow.*` |
| `image` | image | §6.1 | `image`, `image.*` |
| `clips[]` | per-display cached copies | empty | internal (clip change detection) |

Other owners override defaults after `background_init`: bar background (`bar_manager_init`):
`height=25` (overrides), `padding_left=padding_right=20`, `border_color=0xffff0000`,
`color=0x44000000`; popup background (`popup.c:popup_init`): `border_color=0xffff0000`,
`color=0x44000000`; slider track/fill: §7.

### 5.2 Properties (`background_parse_sub_domain`), checked in this order

| prop | type / parser | anim | side effects |
|---|---|---|---|
| `drawing` | bool | – | `background_set_enabled` (below); returns changed |
| `clip` | f32 `token_to_float` | FLOAT | sets `bar_needs_update`, `might_need_clipping`; if `clip > 0` → `enabled = true`. Not clamped. |
| `height` | `token_to_int` → u32 | INT | `bounds.h = h; overrides_height = (h != 0)` |
| `corner_radius` | u32 | INT | |
| `border_width` | u32 | INT | |
| `color` | u32 `token_to_int` | BYTES | **also sets `enabled = true`** (even for alpha 0) |
| `border_color` | u32 | BYTES | does not enable |
| `padding_left` / `padding_right` | i32 | INT | |
| `x_offset` / `y_offset` | i32 | INT | |
| `image` | path string | – | `image_load` (§6.2); returns its result |
| `shadow.<p>` | | | §2.2 |
| `image.<p>` | | | §6.3 |
| `color.<p>`, `border_color.<p>` | | | §1.2 (no enable) |
| other `x.y` | | | `[!] Background: Invalid subdomain '<x>'\n` |
| other | | | `[!] Background: Invalid property '<p>'\n` |

Bar reuse (`message.c:handle_domain_bar`): any `--bar <prop>` not handled by the bar itself
falls through to `background_parse_sub_domain(bar.background)` (so `--bar color=`,
`border_color`, `border_width`, `corner_radius`, `padding_left/right`, `x_offset`, `drawing`,
`image`, `shadow.*`, … work); `--bar clip=` is rejected with `[!] Bar: Invalid property 'clip'\n`;
`--bar height` and `y_offset` have bar-specific setters. Popups: `popup.background.<p>`.

`background_set_enabled(e)`: unchanged → false. If the background currently clips the bar
(`enabled && clip > 0`), drop all cached clip copies and set `bar_needs_update`. Then set.

### 5.3 Layout (`background_calculate_bounds(x, y, w, h)`, all args **u32**)

```
bounds.x = x
bounds.y = y - h / 2            // u32 arithmetic: integer division; if h/2 > y it WRAPS (≈4.29e9)
bounds.w = w ; bounds.h = h
if image.enabled: image_calculate_bounds(image, x, y)      // §6.4
```
Quirk: a background taller than `2*y` gets a wrapped (huge) y and becomes invisible.

Who calls it with what:

| owner | x | y | w | h |
|---|---|---|---|---|
| item (`bar_item_calculate_bounds`) | item-local start (`max(shadow_offsets.x,0)`) | `y_center + item.y_offset` | item length (`bar_item_get_length(false)`) | `overrides_height ? height : bar_height - (bar.border_width + 1)` where the caller already passed `bar_height = window_h - (bar.border_width + 1)` → effectively `window_h - 2*(bar.border_width+1)` (quirk; popup cells: `cell_h - (bar.border_width+1)`) |
| text (§4.8) | cell x (unaligned) | text centre y | `text_get_length(false)` | `overrides_height ? height : text.bounds.h` |
| slider (§7.3) | slider x | y | track width / fill width | `slider.background.bounds.h` (from `height`) |
| bracket (`group.c:group_calculate_bounds`) | `max(shadow_offsets.x,0)` | `y + bracket.y_offset` | group length | current `bounds.h` |

### 5.4 Rounded-rect primitive (`background.c:draw_rect`)

```
draw_rect(ctx, region, fill, radius: u32, lw: u32, stroke?):
  CGContextSetLineWidth(lw)
  if stroke: CGContextSetRGBStrokeColor(stroke)
  CGContextSetRGBFillColor(fill)
  inset = CGRectInset(region, lw/2, lw/2)               // float
  if radius > inset.h/2 || radius > inset.w/2:
      radius = (u32)(inset.h > inset.w ? inset.w/2 : inset.h/2)   // truncated
  path = CGPathAddRoundedRect(inset, radius, radius)
  CGContextDrawPath(kCGPathFillStroke)                  // fill, then stroke
```
* The stroke is centred on the inset edge, so the border lies entirely inside `region`;
  the fill covers the inset rect, i.e. it continues under the inner half of the border.
* `lw == 0`: Quartz is asked to stroke with width 0 (see open question Q1; observed
  SketchyBar output shows no hairline, so mbar draws no stroke when `lw == 0`).

### 5.5 Drawing (`background_draw`)

```
if !enabled: return
if (border_color.a == 0 || border_width == 0) && color.a == 0
   && !shadow.enabled && !image.enabled: return            // nothing to draw
r = bounds translated by (x_offset, y_offset)
if shadow.enabled:
    draw_rect(r translated by shadow.offset, fill = shadow.color, corner_radius,
              border_width, stroke = shadow.color)          // solid silhouette, even if fill alpha is 0
draw_rect(r, fill = color, corner_radius, border_width, stroke = border_color)
if image.enabled: image_draw(image)                         // image does NOT get x/y_offset
```
Callers: item background first, then icon (its background then glyphs), label, alias/graph/
slider (`bar_item.c:bar_item_draw`). Bracket items draw only their background.
The bar background is drawn with `shadow.enabled` forced off and `enabled` forced on, bounds =
bar window frame with `origin.y -= y_offset` (`bar.c:bar_draw`).

### 5.6 Clip (`background_clip_bar`, `helpers.h:clip_rect`)

`clip ∈ (0,1]` punches a hole of opacity `clip` into the **bar** background under this
background, so a translucent item background shows the desktop instead of the bar colour.
Applies to the item background, `icon.background`, `label.background` (`bar_item_clip_bar`);
not to slider/knob/popup backgrounds.

Executed on the bar window context only when the bar background is redrawn
(`bar_needs_update`), after the bar background:
```
if !(enabled && clip > 0): return
update cached copy clips[adid-1] = *background
r = bounds translated by (item_window.origin.x - bar_window.origin.x + x_offset, y_offset)
radius clamped as in draw_rect but against r itself (no inset)
CGContextSetBlendMode(kCGBlendModeDestinationOut)
fill colour = (0,0,0,clip)
path = rounded rect r ; CGContextDrawPath(kCGPathFillStroke)
CGContextSetBlendMode(kCGBlendModeNormal)
```
Destination-out: `dst *= (1 - src_alpha)`. Quirk: the stroke uses whatever stroke colour/width
the bar context last had (the bar's `border_color`/`border_width` from drawing the bar
background), so with a visible bar border the hole's outline is punched with that alpha too.

Change detection (`background_clip_needs_update`, `bar.c:bar_check_for_clip_updates`, only
while `might_need_clipping`): the bar is fully redrawn (and all holes re-punched) when, for any
item that clips: it is not drawn on this bar but its window is not parked at nirvana
(−9999,−9999); or its window needs move/resize; or `bounds`, `corner_radius`, `x_offset`,
`y_offset` differ from the per-display cached copy (a fresh copy has origin −9999,−9999 so the
first check always triggers).

### 5.7 JSON (`background_serialize(bg, I, rsp, detailed)`)

```
I"drawing": "off",
I"color": "0x0",
I"border_color": "0x0",
I"border_width": 0,
I"height": 0,                 // overrides_height ? (int)bounds.h : 0
I"corner_radius": 0,
I"padding_left": 0,
I"padding_right": 0,
I"x_offset": 0,
I"y_offset": 0,
I"clip": 0.000000,            // %f
I"image": {
<image_serialize at I+\t>
I}
```
If `detailed`, additionally `,\nI"shadow": {\n<shadow at I+\t>\nI}`. No trailing newline.
`detailed = true`: item `geometry.background`, text backgrounds, popup background.
`detailed = false`: `--query bar` (bar background, fields directly in the bar object) and the
slider track.

## 6. Image (`image.c`) — `<…>.background.image`

Images exist only as a background's image (item, icon, label, knob, popup, bar background) and
internally inside an alias (§8). An image is **only drawn and only counted for width when its
background is enabled** (`background_draw` returns early; `bar_item_get_length` /
`text_get_length` check `background.enabled && image.enabled`). Loading an image does **not**
enable the background.

### 6.1 Model and defaults (`image_init`)

| field | type | default | property |
|---|---|---|---|
| `enabled` | bool | `false` | `drawing` |
| `path` | string | `NULL` (JSON `(null)`) | `string` / `background.image=` |
| `scale` | f32 | 1.0 | `scale` |
| `size` | CGSize | (0,0) | logical image size before scale |
| `bounds` | CGRect | `CGRectNull` (size 0) | `size * scale`, origin set in layout |
| `corner_radius` | u32 | 0 | `corner_radius` |
| `border_width` | f32 | 0.0 | `border_width` |
| `border_color` | color | `0xcccccccc` | `border_color` |
| `padding_left` / `padding_right` | i32 | 0 / 0 | `padding_left` / `padding_right` |
| `y_offset` | i32 | 0 | `y_offset` |
| `shadow` | shadow | §2.1 | `shadow.*` |
| `link` | image* | NULL | set by `media.artwork` |
| `image_ref`, `data_ref` | CGImage / CFData | NULL | decoded image and a copy of its bytes (for change detection) |

### 6.2 Loading (`image_load(image, value)`) — `background.image=<v>` or `background.image.string=<v>`

`image.path = copy(v)` is stored **first, unconditionally** (also when loading fails — the JSON
`value` then shows the failed value while the old picture stays; quirk).
`res = v` with a leading `~` replaced by `$HOME` (`resolve_path`, 512-byte buffer).
`(key, val) = split v at the first '.'`. Dispatch in order:

| # | condition | action | logical size (before `scale`) | on failure |
|---|---|---|---|---|
| 1 | `key == "app"` | app icon of `val` (below) | `pixels / s²` where `s` = max `backingScaleFactor` over `NSScreen.screens` (≥ 1) | `[!] Image: Invalid application name: '<val>'\n`, return false |
| 2 | `key == "space"` | `space_capture(atoi(val))` (below) | pixel size (1 px = 1 pt) | `[!] Image: Invalid Space ID: '<val>'\n`, return false |
| 3 | `v == "media.artwork"` | `begin_receiving_media_events()`; `image_set_link(&current_artwork)`; return "link changed" | 32 pt high, see §6.4 | – |
| 4 | `res` exists and is not a directory (`stat`) | `CGDataProviderCreateWithFilename(res)`; if `res` ends in `.png` (case-sensitive) `CGImageCreateWithPNGDataProvider(p, NULL, false, kCGRenderingIntentDefault)` else `CGImageCreateWithJPEGDataProvider(…)` | pixel size | provider NULL: `[!] Image: Invalid Image Format: '<val>'\n`, false. Decode NULL (e.g. GIF/TIFF/HEIC/SVG, `.PNG`): prints `Could not open image file at: <res>\n` to stdout **and** the response, returns **true**, picture unchanged |
| 5 | `res == ""` | `image_destroy`: release picture and **free path** (JSON `(null)`), return false. `enabled`, `size`, `bounds`, `link` are **not** reset (quirk: an enabled-but-empty image keeps reserving its width; a media link keeps drawing the artwork) | – | – |
| 6 | otherwise | – | – | `[!] Image: File '<res>' not found\n`, return false |

Note on #1/#2: any value whose first `.`-segment is `app`/`space` is treated as such, e.g. a
relative file `app.png` is looked up as application `png` (quirk).

On success (1, 2, 4): `image_set_image(image, ref, (0,0,w_px/scale_load, h_px/scale_load), forced=true)`;
`image_load` returns true.

**App icon** (`workspace.m:workspace_icon_for_app(name)`):
1. `url = NSWorkspace.URLForApplicationWithBundleIdentifier(name)`.
2. If nil: scan `NSWorkspace.runningApplications` for the first app with `localizedName == name`;
   take its `bundleIdentifier`, look up the URL again; stop at the first name match. If still
   no URL → failure.
3. `NSImage *img = NSWorkspace.iconForFile(url.path)`; nil → failure.
4. `rect = (0,0,32*s,32*s)`; `CGImageForProposedRect(&rect, context: nil, hints: nil)` (retained).
Intended logical size is **32×32 pt** (the `s²` divisor compensates for AppKit returning
`32·s` points rendered at backing scale `s`). mbar: render the icon at `32·s` px per side and
report a logical size of 32×32 pt (verify on hardware, Q3). Accepts bundle id
(`app.com.apple.Safari`) or a running app's localized name (`app.Safari`).

**Space capture** (`helpers.h:space_capture`): `n = atoi(val)`; walk
`SLSCopyManagedDisplaySpaces(cid)` → for each display dict → `"Spaces"` array → each space's
`"id64"`; the n-th space overall (1-based, display order) gives `dsid` (not found → 0 →
failure). `SLSHWCaptureSpace(cid, dsid, 0)` returns a CFArray; first element retained. One-shot
snapshot at load time (never refreshed).

### 6.3 Properties (`image_parse_sub_domain`) — `<…>.background.image.<p>`

| prop | type / parser | anim | effect |
|---|---|---|---|
| `string` | path | – | `image_load` (§6.2) |
| `drawing` | bool | – | `enabled` |
| `scale` | f32 | FLOAT | `bounds.size = size * scale` (origin kept) |
| `corner_radius` | u32 `token_to_uint32t` | INT | |
| `padding_left` / `padding_right` | i32 | INT | |
| `y_offset` | i32 | INT | |
| `border_width` | f32 | FLOAT | |
| `border_color` | u32 | BYTES | |
| `border_color.<p>` | | | §1.2 |
| `shadow.<p>` | | | §2.2 |
| other `x.y` | | | `[?] Image: Invalid subdomain: <x> \n` |
| other | | | `[?] Image: Unknown property: <p> \n` |

`image_set_image(image, ref, rect, forced)` (shared with alias and media artwork):
```
if ref == NULL: release image_ref/data_ref (set NULL); return false     // enabled untouched
if link: link = NULL                                                    // any real image breaks the media link
new_data = CGDataProviderCopyData(CGImageGetDataProvider(ref))
if !forced && image_ref && data_ref && bytes(data_ref) == bytes(new_data) && size == rect.size:
    release new; return false
size = rect.size
bounds = (0, 0, size.w * scale, size.h * scale)
image_ref = ref; data_ref = new_data
enabled = true                                                          // re-enables drawing
return true
```

### 6.4 Size and layout

`image_get_size(image)` (used for width/height reservation by item, text, popup):
```
w = bounds.w + padding_left + padding_right + (shadow.enabled ? shadow.offset.x : 0)   // offset.x may be negative
h = bounds.h + 2*abs(y_offset)
```
`image_calculate_bounds(image, x, y)` (called by `background_calculate_bounds` with the
background's *un-offset* x / centre y, and by the alias):
```
if link && link.image_ref:                         // media artwork: normalise to 32 pt height
    k = 32 / CGImageGetHeight(link.image_ref)
    size = (CGImageGetWidth(link.image_ref) * k, 32)
    bounds.size = size * scale
bounds.x = x + padding_left
bounds.y = y - bounds.h / 2 + y_offset            // float math; vertically centred on y
```
The image ignores the background's `x_offset`/`y_offset` and its width/height — it is
left-aligned at the background start and vertically centred on the owner centre. The
background's own rect is not resized to the image; instead the *owner's length* grows to
`max(len, image_get_size().w)` (§4.4, item length). Item height uses `image_get_size().h`.

### 6.5 Drawing (`image_draw`)

```
pic = link ? link.image_ref : image_ref
if pic == NULL: return
if shadow.enabled:
    save
    fill = shadow.color
    path = CGPathAddRoundedRect(bounds + shadow.offset, corner_radius, corner_radius)   // NOT clamped
    CGContextDrawPath(kCGPathFillStroke)   // stroke uses the inherited stroke state
    restore
save
rounded = bounds.h > 2*corner_radius && bounds.w > 2*corner_radius
if rounded: clip to CGPathAddRoundedRect(bounds, r, r)
CGContextDrawImage(bounds, pic)            // stretched to bounds, kCGInterpolationNone
if rounded:
    line width = 2 * border_width; stroke = border_color; fill = (0,0,0,0)
    CGPathAddRoundedRect(bounds, r, r); CGContextDrawPath(kCGPathFillStroke)
    // half of the stroke is clipped away → visible border of border_width inside the image
restore
```
* With `corner_radius = 0` the condition holds for any non-empty image, so the clip/border path
  runs; with `2r ≥ w` or `2r ≥ h` there is neither clip nor border.
* The image shadow is a solid rounded rectangle (not the image's alpha silhouette).
* Images are stretched to `bounds` (aspect not preserved except via `size*scale`).

### 6.6 JSON (`image_serialize`) at indent `I`

```
I"value": "<path>",          // raw value as given (before ~ expansion), or (null)
I"drawing": "off",
I"scale": 1.000000           // %f
```
corner_radius, border, padding, y_offset and shadow of the image are **not** serialized.

## 7. Slider (`slider.c`) — item type `slider`

### 7.1 Creation

`--add slider <name> <position> <width>` → `slider_setup(width)`: track width = `width`
(`token_to_uint32t`, missing → 0), track and fill backgrounds enabled
(`message.c:handle_domain_add`). The item gets `has_slider = true`. Item content length adds
`slider_get_length = track width` (between icon and label, §9).

### 7.2 Model and defaults (`slider_init`)

| field | default | property |
|---|---|---|
| `percentage` | 0 (u32, 0..100) | `slider.percentage` |
| `foreground_color` | `0xff0000ff` | `slider.highlight_color` |
| `background` (track) | background with `color=0xff000000` (thus enabled), `bounds.w = 100` | `slider.background.*`, `slider.width` |
| `foreground` (fill) | background with `color=foreground_color` (enabled) | via `slider.background.*` |
| `knob` | text (§4.1 defaults, string `""`) | `slider.knob=<s>`, `slider.knob.*` |
| `is_dragged` | false | internal |

Note the track height defaults to **0** (`overrides_height=false`, `bounds.h = 0`): nothing is
visible and clicks never hit the track until `slider.background.height` is set.

### 7.3 Layout (`slider_calculate_bounds(x, y)`)

`x` = item-local position after the icon, `y` = item centre (`content_y + item.y_offset`).
```
W = track.bounds.w ; H = track.bounds.h
background_calculate_bounds(track, x, y, W, H)
background_calculate_bounds(fill,  x, y, (u32)(W * percentage / 100), H)
raw  = (i32)(percentage/100 * W - knob.bounds.w / 2)
knob_offset = (u32) max(min(raw, W - (knob.bounds.w + 1)), 0)      // float compare
text_calculate_bounds(knob, x + knob_offset, y)
```
`knob.bounds.w` is the knob's ink width (+1.5, §4.3), not including knob padding.
`x_offset`/`y_offset` of the shared background props shift the drawn track/fill rects.

### 7.4 Drawing (`slider_draw`)

`background_draw(track)`, `background_draw(fill)`, `text_draw(knob)` — in that order.

### 7.5 Properties (`slider_parse_sub_domain`) — `slider.<p>`; only on slider items or `--default`

| prop | type | anim | semantics |
|---|---|---|---|
| `percentage` | u32 `token_to_uint32t` | INT | **ignored while dragging**. `slider_set_percentage(p)`: unchanged if `p == percentage` (compared before clamping); else `percentage = min(p, 100)` (negative input wraps → 100) |
| `highlight_color` | u32 | BYTES | `foreground_color = c`; `background_set_color(fill, c)` (enables fill) |
| `width` | u32 | INT | **ignored while dragging**; track width |
| `knob` | string | – | knob text string (same as `slider.knob.string`) |
| `background.<p>` | | | applied to the **fill first**, then fill colour reset to `foreground_color`, then applied to the **track**; return value is the track's. So `height`, `corner_radius`, `border_*`, offsets, `drawing`, `shadow`, `image` are shared; `color` only affects the track |
| `knob.<p>` | | | any text property (§4.2), e.g. `knob.font`, `knob.color`, `knob.drawing` |
| other `x.y` | | | `[!] Slider: Invalid subdomain '<x>' \n` |
| other | | | `[!] Slider: Invalid property '<p>'\n` |

On a non-slider item: `[!] Item (<name>): Trying to set a slider property on a non-slider item\n`.

### 7.6 Mouse interaction

Mouse events come from a Carbon handler (`mouse.c`: `kEventMouseUp`, `kEventMouseDragged`, …)
on the item windows. The event location (global, top-left origin) is converted to window-local
by subtracting the item window origin (also top-left). **Quirk:** this top-left local point is
compared with CG (bottom-left) slider rects without flipping y; it only works because the track
is vertically centred in the window — a `y_offset` breaks the hit test (mbar: replicate by
mirroring, i.e. compare against `window_h - y`, documented as Q5).

`slider_get_percentage_for_point(p)`:
```
d = max(p.x - track.bounds.x, 0)
pct = (u32)(d / track.bounds.w * 100 + 0.5)
return min(pct, 100)
```

* **Drag** (`event.c:event_mouse_dragged`, item window must belong to an item with `has_slider`):
  `slider_handle_drag(p)`: `is_dragged = true`; `slider_set_percentage(pct)`; if changed → item
  redraw. No script/event is sent during drags.
* **Mouse up** (`bar_item.c:bar_item_on_click`):
  ```
  if slider.is_dragged || CGRectContainsPoint(track.bounds, p):
      slider_handle_drag(p)                         // final value
      env PERCENTAGE = "<percentage>" (stored in the item's persistent env vars)
      is_dragged = false
  else:
      return    // click outside the track on a slider item: no click_script, no mouse.clicked
  then: click_script (with INFO/BUTTON/MODIFIER + the item's env vars incl. PERCENTAGE);
        if subscribed: mouse.clicked event (SENDER=mouse.clicked)
  ```
  So scripts observe the new value via `$PERCENTAGE` on `mouse.clicked`/click_script; the
  variable then persists in the item's env for later script runs.
* While `is_dragged`, `slider.percentage` and `slider.width` sets from scripts are ignored.

### 7.7 Scroll texts

With item `scroll_texts=on`, the knob participates in §4.10.

### 7.8 JSON (`slider_serialize`) — item key `"slider"`, fields at indent `S` (= `\t\t`)

```
S"highlight_color": "0xff0000ff",
S"percentage": "0",              // quoted string, %d
S"width": "100",                 // quoted string, (int)track.bounds.w
S"background": {
<background_serialize(track, S+\t, detailed=false)>
S},
S"knob": {
<text_serialize(knob, S+\t)>
S}
```

## 8. Graph (`graph.c`) — item type `graph`

### 8.1 Creation and model

`--add graph <name> <position> <width>` → `graph_setup(width)`: `width` samples (u32
`token_to_uint32t`), zero-filled f32 ring buffer, `cursor = 0`. **Quirk:** `width = 0` makes
every `--push` divide by zero (crash); mbar should reject/ignore pushes when width is 0.
`--clone` of a graph shares the sample buffer pointer (memcpy; quirk — mbar deep-copies).

| field | default | property |
|---|---|---|
| `width` | from `--add` | – (not settable later) |
| `line_color` | `0xffcccccc` | `graph.color` |
| `fill_color` | `0xffcccccc` | `graph.fill_color` |
| `overrides_fill_color` | false | set by `graph.fill_color=` only |
| `line_width` | 0.5 (f32) | `graph.line_width` |
| `fill` | true | – (always true, no property) |
| `enabled` | true | – (no property) |
| `rtl` | false | set by layout, see §8.4 |

Item content length adds `graph_get_length = width` (1 sample = 1 pt).

### 8.2 Data (`--push <name> <v1> [v2 …]`, `message.c:handle_domain_push`)

Item must exist (`[!] Push: Item '<n>' not found\n`) and be a graph
(`[!] Push: Item '<n>' not a graph\n`). Each value is `token_to_float`'d (no clamping; values are
fractions of the graph height, 0..1 expected) and pushed:
```
y[cursor] = v ; cursor = (cursor + 1) % width
```
then the item is marked for redraw. Logical sample `i` (0 = oldest, width-1 = newest):
`graph_get_y(i) = y[(cursor + i) % width]`.

### 8.3 Properties (`graph_parse_sub_domain`) — `graph.<p>`; graph items or `--default` only

| prop | parser | anim | effect |
|---|---|---|---|
| `color` | `token_to_uint32t` | – | `line_color` |
| `fill_color` | `token_to_uint32t` | – | `fill_color`; `overrides_fill_color = true` |
| `line_width` | `token_to_float` | – | always reports changed |
| `color.<p>` | | §1.2 | `line_color` channels |
| `fill_color.<p>` | | §1.2 | channels; does **not** set `overrides_fill_color` |
| other | | | `[!] Graph: Invalid subdomain '<x>'\n` / `[!] Graph: Invalid property '<p>'\n` |

On a non-graph item: `[!] Item (<name>): Trying to set a graph property on a non-graph item\n`.

### 8.4 Layout

`graph.rtl = true` for items in position `right` or `q` (center-left), false otherwise
(`bar.c:bar_calculate_bounds_*`; popup items keep the last value).
Height (`bar_item.c:bar_item_calculate_bounds`):
```
H = item.background.enabled ? item.background.bounds.h - item.background.border_width - 1
                            : bar_height - (bar.border_width + 1)   // bar_height = window_h - (bar.border_width+1)
```
Quirk: `item.background.bounds.h` is the value from the *previous* layout pass unless the
height is overridden (the item background is laid out after the graph).
`graph_calculate_bounds(x = position after icon, y = content_y, H)`:
`bounds.h = H; bounds.x = x; bounds.y = y - H/2 + line_width` (float).

### 8.5 Drawing (`graph_draw`) — exact path

```
x  = (u32)(bounds.x + (rtl ? width : 0)) ; y = (u32)bounds.y ; h = (u32)bounds.h
stroke = line_color
fill   = overrides_fill_color ? fill_color : line_color with alpha * 0.2
lineWidth = line_width
start_x = x
if rtl:
    moveTo(x, y + Y(width-1)*h)
    for i = width-1 down to 1: lineTo(x, y + Y(i)*h); x -= 1
else:
    moveTo(x, y + Y(0)*h)
    for i = width-1 down to 1: lineTo(x, y + Y(i)*h); x += 1
stroke path
// fill (always): continue the same path
lineTo(rtl ? x + 1 : x - 1, y) ; lineTo(start_x, y) ; closeSubpath ; fill    // fill drawn over the line
```
(`Y(i) = graph_get_y(i)`, u32 x arithmetic.) Resulting geometry:
* LTR (left/center/e items): a vertical segment at `x0` from the **oldest** to the **newest**
  value, then samples newest → oldest+1 at `x0 … x0+width-2` (newest on the **left**, sample 0
  appears only as the initial point). Fill polygon closes at `(x0+width-2, y)` → `(x0, y)`.
* RTL (right/q items): newest at `x0+width`, then samples newest → index 1 at
  `x0+width … x0+2` (newest on the **right**). Fill closes at `(x0+2, y)` → `(x0+width, y)`.
* The fill (default 20 % alpha of the line colour) is painted after the stroke, on top of it.

### 8.6 JSON (`graph_serialize`) — item key `"graph"`, fields at indent `G` (= `\t\t`)

```
G"color": "0xffcccccc",
G"fill_color": "0xffcccccc",
G"line_width": "0.500000",        // quoted %f
G"data": [
G\t"0.000000",                    // quoted %f, RAW buffer order y[0..width), not chronological
G\t"0.250000"
G]
```
Elements separated by `,\n`; with `width = 0` the array prints as `[\n\nG]`.

## 9. Alias (`alias.c`) — item type `alias` (mirrors a native menu-bar extra)

### 9.1 Creation (`message.c:handle_domain_add`, `alias_setup`)

`--add alias <spec> <position>`; the item **name is the spec**:
* `"<Owner>,<Name>"` → `owner = Owner`, `name = Name` (split at the first `,`; `Name` may itself
  contain commas).
* `"<Owner>"` (no comma) → `owner = Owner`, `name = NULL`.
* `"<Owner>,"` (comma with nothing after it) → `owner = "<Owner>,"` (the whole spec, comma
  included), `name = NULL` — never matches (quirk).

`alias_setup` then:
1. `alias_get_permission`: macOS ≥ 11: `permission = CGRequestScreenCaptureAccess()` (prompts
   the user once; result stored, never consulted afterwards). Older: a dummy
   `CGWindowListCreateImage(1×1)` to trigger the prompt.
2. `alias_update_image(forced = true)`.

### 9.2 Model and defaults (`alias_init`)

| field | default | property |
|---|---|---|
| `owner`, `name` | from spec | – |
| `update_frequency` | 1 (ticks of the 1 s routine timer) | `alias.update_freq` |
| `counter` | 0 | internal |
| `color_override` / `color` | false / `0xffff0000` | `alias.color`, `alias.color.*` |
| `image` | §6.1 defaults (scale 1.0) | `alias.scale`, `alias.shadow.*` |
| `window` | id 0, frame `CGRectNull` | captured window (id, size, origin) |

### 9.3 Enumerating menu-bar extras (`get_menu_item_list`)

```
list = CGWindowListCopyWindowInfo(kCGWindowListOptionAll, kCGNullWindowID)
for each dict:
    require keys kCGWindowName, kCGWindowOwnerName, kCGWindowOwnerPID,
                 kCGWindowLayer, kCGWindowBounds, kCGWindowNumber      // else skip
    skip unless kCGWindowLayer == 0x19 (25 = kCGStatusWindowLevel, MENUBAR_LAYER)
    skip unless CGRectMakeWithDictionaryRepresentation(bounds) succeeds
    skip if owner == "Window Server"
    item = { owner, name, x_pos = bounds.x, wid = kCGWindowNumber, bounds }
sort by x_pos DESCENDING (selection sort; rightmost first)
```
Sort quirk: the inner loop starts with `best_index = 0` and threshold −9999, so items with
`x ≤ −9999` may be swapped with index 0; otherwise a plain descending sort (ties keep the
first occurrence). Without Screen Recording permission `kCGWindowName` is absent for other
apps' windows, so the list is (nearly) empty.

### 9.4 `--query default_menu_items` (`print_all_menu_items`)

macOS ≥ 11: if `!CGRequestScreenCaptureAccess()` → respond
`[!] Query (default_menu_items): Screen Recording Permissions not given. Restart SketchyBar after granting permissions.\n`
and stop. Otherwise, if the list is non-empty:
```
[
\t"<owner>,<name>(1)", 
\t"<owner>,<name>(2)"
]
```
(separator is `", \n"` — comma, space, newline; index = 1-based position in the sorted list;
closing `\n]\n`). Empty list → no output at all.

### 9.5 Window lookup (`alias_find_window`)

Iterate the sorted list with index `i` (0-based):
```
skip if owner == NULL or owner != item.owner
if name == NULL:            match only if item.name == ""
elif name == item.name:     match
elif name == item.name + "(" + (i+1) + ")": match        // the indexed form printed by §9.4
else skip
on match: window.id = wid; window.frame.size = bounds.size; window.origin = bounds.origin; stop
no match: window.id = 0
```
Note the index is the position among **all** menu-bar extras at lookup time, so indexed names
break when other extras appear/disappear.

### 9.6 Capture (`alias_update_image`, `window.c:window_capture`)

```
if window.id == 0: alias_find_window()
if window.id == 0: return false
ref = window_capture(window, &disabled)
if ref == NULL:
    if !disabled: window.id = 0; image_destroy(image)   // image_ref NULL → length 0; re-lookup next time
    return false
return image_set_image(image, ref, window.frame, forced)
```
`window_capture`:
* Capture is suspended by WindowServer notifications registered with `SLSRegisterNotifyProc`
  (`sketchybar.c:system_events`): event **1322** stores a timestamp → capture disabled for
  `2^30` ns (≈1.07 s); event **905** → disabled indefinitely; events **904, 1401, 1508** →
  re-enabled. While disabled `*disabled = true`, return NULL (image kept).
* `SLSCaptureWindowsContentsToRectWithOptions(cid, &wid, true, CGRectNull, 1 << 8, &image)`.
* `SLSGetScreenRectForWindow(cid, wid, &rect)`; `rect.w = (u32)(rect.w + 0.5)`;
  `window.frame.size = rect.size` → logical image size in points (the captured bitmap has
  the display's pixel density).

### 9.7 Updates (`alias_update(forced)`)

```
if update_frequency == 0: return false        // never updates, even when forced
counter += 1
if forced || counter >= update_frequency:
    counter = 0
    return alias_update_image(forced)
return false
```
Called from `bar_manager_update` (the 1 s `SHELL_REFRESH` CFRunLoopTimer, and `--update` with
forced = false for the alias part) for every alias item **that is currently shown on some bar**;
a true result marks the item for redraw. Unchanged pixels+size → no redraw (byte compare in
`image_set_image`).

### 9.8 Layout and drawing

* Length = `image.image_ref ? image.bounds.w : 0`; height = `image.image_ref ? image.bounds.h : 0`
  (image padding/shadow are not counted). Placed between icon and label.
* `alias_calculate_bounds(x, y)` = `image_calculate_bounds(image, x, y)` (vertically centred).
* `alias_draw`:
  ```
  if color_override:
      save
      image_draw(image)
      CGContextClipToMask(ctx, image.bounds, image.image_ref)
      fill = color ; CGContextFillRect(image.bounds)
      restore
  else: image_draw(image)
  ```
  The tint paints `color` through the captured image used as a mask (Q4: exact mask semantics
  of an RGBA image in `CGContextClipToMask`; mbar uses the image alpha × luminance-free alpha
  mask, i.e. "recolour opaque pixels").

### 9.9 Properties (`alias_parse_sub_domain`) — `alias.<p>`; alias items or `--default`

| prop | parser | effect / result |
|---|---|---|
| `color` | `token_to_uint32t` | `color_set_hex`, `color_override = true`, returns true (not animated) |
| `color.<p>` | §1.2 | `color_override = true`; changed if colour changed or override was off |
| `scale` | `token_to_float` | `image_set_scale` (not animated) |
| `update_freq` | `token_to_uint32t` | stored; no refresh |
| `shadow.<p>` | §2.2 | the alias image's shadow |
| other `x.y` | | `[!] Alias: Invalid subdomain '<x>'\n` |
| other | | `[!] Alias: Invalid property '<p>' \n` |

On a non-alias item: `[!] Item (<name>): Trying to set an alias property on a non-alias item\n`.

### 9.10 JSON

Alias items have `"type": "alias"`; **no alias-specific block** is serialized (owner, name,
color, scale, update_freq are not queryable).

## 10. How components compose inside an item (`bar_item.c`)

### 10.1 Length / height

```
content_length = text_get_length(icon,false) + text_get_length(label,false)
               + (has_graph ? graph.width : 0) + (has_slider ? track.w : 0)
               + (has_alias ? alias_length : 0)                 ; max(…, 0)
item_length(ignore_override):
    L = content_length
    if background.enabled && background.image.enabled: L = max(L, image_get_size(bg.image).w)
    if has_const_width && (!ignore_override || custom_width > L): return custom_width
    return L
item_height = max(icon_h, label_h, alias_h,
                  background.enabled ? max(bg.image.enabled ? image_get_size(bg.image).h : 0, bg.bounds.h) : 0)
```

### 10.2 Placement (`bar_item_calculate_bounds(bar_height, x, y)`)

```
len = item_length(false); content = content_length
cx = x; if len > content: cx += (len-content)/2 if item.align=='c'; += len-content if 'r'
icon_x   = cx
label_x  = icon_x + text_get_length(icon,false)
mid_x    = label_x
label_x += graph.width   if has_graph
        else alias_len   if has_alias
        else track.w     if has_slider
cy = y + item.y_offset
text_calculate_bounds(icon,  icon_x,  cy)
text_calculate_bounds(label, label_x, cy)
alias / slider / graph at (mid_x, cy)
item background at (x, cy, len, …) — §5.3
```
Order left→right: **icon, [graph | alias | slider], label**.

### 10.3 Draw order (`bar_item_draw`)

item background → icon (background, shadow, glyphs) → label (same) → alias → graph → slider
(track, fill, knob). Bracket items draw only their background.

### 10.4 Window expansion for shadows (`bar_item_calculate_shadow_offsets`)

```
left  = (int)( Σ max(-s.offset.x, 0) over enabled shadows of
               {background, icon, icon.background, label.background, label}
             + (background.enabled ? max(-background.x_offset, 0) : 0) )
right = (int)( Σ max( s.offset.x, 0) over the same shadows
             + (background.enabled ? max( background.x_offset, 0) : 0) )
```
(returned as `CGPoint{x = left, y = right}`.) The item window is `item_length + left + right`
wide and content starts at local `x = max(left, 0)`. Image shadows, alias shadows, the
knob, text/background `y` offsets and `text.background.x_offset` are **not** included — such
content can be clipped by the window edge.

### 10.5 Item JSON skeleton (`bar_item_serialize`) — component parts

```
{
	"name": "<name>",
	"type": "item|alias|bracket|slider|graph|space",
	"geometry": {
		…,
		"padding_left": <background.padding_left>,
		"padding_right": <background.padding_right>,
		…,
		"background": {
<background_serialize(item.background, "\t\t\t", detailed=true)>
		}
	},
	"icon": {
<text_serialize(icon, "\t\t")>
	},
	"label": {
<text_serialize(label, "\t\t")>
	},
	"scripting": { … },
	"bounding_rects": { … }
	[,"popup": { … }]
	[,"graph": {\n<graph_serialize "\t\t">\n\t}]       // type graph
	[,"slider": {\n<slider_serialize "\t\t">\n\t}]     // type slider
}
```
The item-level `padding_left/right` and `background.padding_left/right` are the same field.
`--query defaults` prints the default item with this serializer (`"name": "defaults"`).
`--query bar` embeds `background_serialize(bar.background, "\t", detailed=false)` directly in
the bar object (keys `drawing`, `color`, … `clip`, `image`).

### 10.6 Media artwork distribution (`bar_manager_handle_media_cover_change`)

On a `COVER_CHANGED` event (from `media.m`, MediaRemote now-playing artwork decoded with
`CGImageSourceCreateImageAtIndex`): `image_set_image(current_artwork, img, CGRectNull, false)`;
if it changed, every item whose `background.image`, `icon.background.image` or
`label.background.image` links to it is flagged for redraw, and the bar refreshes if any such
item is shown. Media events are only processed after something called
`begin_receiving_media_events` (`media.artwork` image or a `media_change` subscription).
(MediaRemote is locked down since macOS 15.3; artwork may never arrive.)

## 11. Open questions (need verification on macOS hardware)

| # | question | where | proposed mbar behaviour |
|---|---|---|---|
| Q1 | Quartz stroke with `CGContextSetLineWidth(0)` + `kCGPathFillStroke`: hairline or nothing? Matters for backgrounds with `border_width=0` but opaque `border_color`, and for every image (`border_color` default `0xcccccccc`, `border_width` 0). | `draw_rect`, `image_draw` | draw no stroke for width 0 (default SketchyBar images show no grey hairline) |
| Q2 | `CGPathAddRoundedRect` with a radius larger than half the rect (image **shadow** path is not clamped): CG error + nothing, or clamp? | `image_draw` | skip the shadow path when `2r > w` or `2r > h` |
| Q3 | Pixel size returned by `-[NSImage CGImageForProposedRect:]` with nil context for the `32·s` rect (decides whether app icons are 32 pt or 16 pt on Retina). | `workspace_icon_for_app`, `image_load` | 32×32 pt |
| Q4 | `CGContextClipToMask` with a non-grayscale RGBA window capture (alias tint): luminance or alpha mask? | `alias_draw` | alpha mask (tint opaque pixels) |
| Q5 | Slider hit-testing mixes top-left event coordinates with bottom-left CG rects. | `event_mouse_up/dragged`, `bar_item_on_click` | reproduce (no flip) behind a compat flag; correct flip as extension |
| Q6 | `--load-font /abs/path.ttf`: does `CFURLCreateWithString` + `CTFontManagerRegisterFontsForURL` accept a scheme-less path? | `font_register` | accept both plain paths and `file://` URLs |
| Q7 | Glyph-path bounds of an empty `CTLine` (zero rect vs `CGRectNull`) → assumed empty text width 1 / height 1. | `text_prepare_line` | width 1, height 1 |
| Q8 | Owner name of WindowServer status windows on current macOS (`"Window Server"` filter). | `get_menu_item_list` | filter `"Window Server"` exactly |
| Q9 | `SLSCaptureWindowsContentsToRectWithOptions` / `SLSHWCaptureSpace` are private SkyLight APIs; ScreenCaptureKit may be required on newer macOS. | alias, `space.<n>` images | private API first, SCK fallback |
| Q10 | MediaRemote now-playing is locked for third parties since macOS 15.3 (`media.m` comment). | `media.artwork` | keep the link mechanism; provider may never deliver |
| Q11 | `clip_rect` strokes with inherited bar stroke state; exact visual impact when the bar has a border. | `helpers.h:clip_rect` | reproduce: stroke the hole outline with bar border width, alpha = bar border alpha |

