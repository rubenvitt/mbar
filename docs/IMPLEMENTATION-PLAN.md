# mbar-core implementation plan

Status of `crates/mbar-core` after the skeleton pass, the four work packages (WP-A to WP-D)
that replace every `todo!()`, the files each package owns, and the contracts between them.

Read first: `docs/ARCHITECTURE.md`, `docs/DESIGN-CORE.md`, `docs/DEVIATIONS.md` (D1–D19),
`docs/EXTENSIONS.md`, `docs/spec/*.md`.

## 1. What is done (shared foundation, do not re-implement)

| file | content | state |
|---|---|---|
| `value.rs` | C-compatible `strtol`/`strtoul`/`strtof`, bools, `split_key` (`get_key_value_pair`), `display_key`, `resolve_path`, `split_list`, `first_byte`, JSON helpers (`json_escape` D13, `json_opt` `(null)`, `fmt_f` `%f`, `fmt_f2`) | complete |
| `color.rs`, `geometry.rs` | `Color` (truncating hex, byte lerp with truncation), `Point`/`Size`/`Rect` | complete |
| `props.rs` | `PropCx` (ANIMATE macros, cancel/snap, chaining, path prefixes), `AnimValue`, `AnimTarget`, `AnimSpec`, `PendingAnim`, `PropEffects`, `PropRequest`, `HiddenRequest`, `PropError` (exact message texts), setter helpers | complete |
| `components/*` | `FontSpec`, `Shadow`, `Image`/`ImageSource`, `Background`, `Text`, `Graph`, `Slider`, `Alias`, `AppMenu`: fields + defaults, `set_prop`, `anim_set`, `write_json`, inheritance helpers, lengths | complete |
| `item.rs` | `ItemId`, `ItemType`, `Position`, `BarItem` (all fields/defaults, item-level `set_prop`/`anim_set`, `set_type`/`set_name`/`set_position`/`set_width`, lengths/heights/shadow extents, `inherit_from`, `to_json`) | complete |
| `bar.rs` | `BarProps` (`--bar` set_prop/anim_set/to_json), `BarState`, `parse_display_pattern` | complete |
| `popup.rs` | `Popup` data, set_prop/anim_set/to_json, `inherited` | complete |
| `model.rs` | `Model` (items, default item, bar props, bars, events, flags), accessors, `create_item`, `draws_item` (incl. D3) | complete |
| `animation.rs` | `Curve` (all formulas, D15), `interpolate`, `Animator` queueing (`add`/`cancel`/`cancel_locked`/`lock_all`/`cancel_target`/`clear`) | complete except stepping (WP-D) |
| `event.rs` | `EventKind` (bit values), `EventMask`, `CustomEvents` registry, `EventInfo`, `is_forced_trigger` | complete except payload builders (WP-D) |
| `script.rs` | `EnvVars` (ordered, `set` = unset+append), `Sender`, constants | complete except builders (WP-D) |
| `provider.rs` | `ProviderKind`, `ProviderConfig` + its `set_prop` | complete except formatting (WP-D) |
| `platform.rs` | `Resources`, `Input`, `OsEvent`, `MouseInput`, `Effect`, `PlatformRequest`, `DisplayInfo`, `SpaceInfo`, `WindowKey`, `WindowUpdate`, `FrameOutput`, `TextMetrics`/`TextKey`, `ImageInfo`/`ImageKey`, `HeadlessResources` | complete |
| `scene.rs` | `Scene`, `Primitive` | complete |

`lib.rs` has a temporary `#![allow(dead_code, unused_variables, unused_imports, unreachable_code)]`
(marked TODO). Remove it when the last package lands and fix what clippy reports.

## 2. Work packages and file ownership

Each package edits **only** its own files. Shared files (section 1 plus `lib.rs`,
`Cargo.toml`) are frozen; if a package needs a change there (a new field, a new helper),
ask the orchestrator: additive changes are batched and merged by one person to avoid
conflicts. Private helpers, extra private types and unit tests go freely into owned files.
Integration tests go into `crates/mbar-core/tests/<wp>_*.rs` (e.g. `tests/wpb_query.rs`),
one file prefix per package.

| package | owns | scope |
|---|---|---|
| **WP-A** layout + scene | `layout.rs` | bar frames, horizontal/vertical bars, notch, brackets, popups (anchors, bounds, nested), auto heights, component bounds, scroll-text draw offsets, scenes for bars and popups, hit-test geometry |
| **WP-B** command + query | `command.rs`, `query.rs` | tokenizer (`get_token`, Mode A/B/C), `parse`, BRE translation, `--query` dispatch, events/displays/menu items/stats/menus JSON |
| **WP-C** runtime | `runtime.rs`, `group.rs` | message execution, every command's semantics, `PropRequest`s, subscriptions, item updates and scripts, timers, OS events, mouse, displays/spaces, redraw decisions, animation application, providers/aliases/app_menu/Lua glue |
| **WP-D** animation + events + script + providers | `animation.rs` (stepping, `marquee`), `event.rs` (builders, scroll throttle), `script.rs` (env builders), `provider.rs` (formatting/defaults) | frame stepping, INFO payloads, env construction (D1/D16), mach payload, provider templates |

Merge order: A, B and D are leaves (they only call the finished data model) and can land in
any order. C integrates them; until they land, C can implement and unit-test everything
that does not reach a `todo!()` of another package, then finish its integration tests.

## 3. Every `todo!()` and its owner

Spec references: `I` = `docs/spec/item.md`, `C` = `cli.md`, `B` = `bar.md`,
`K` = `components.md`, `E` = `events.md`, `X` = `docs/EXTENSIONS.md`, `D` = `DEVIATIONS.md`.

### WP-A — `layout.rs`

| function | spec |
|---|---|
| `bar_frame` | B §4.1 |
| `layout` | B §4, §5.2 (layout per redrawn bar), I §4–6, D10, D11 |
| `layout_bar_horizontal` | B §4.4, I §4.5, D10 (RTL clamp, centre start clamp, `W - disp` clamp) |
| `layout_bar_vertical` | B §4.5, I §4.6 |
| `side_length` | B §4.3 |
| `item_calculate_bounds` | I §4.3, K §10.2 (graph height uses current-pass background, D11) |
| `text_calculate_bounds` | K §4.8 |
| `background_calculate_bounds` | K §5.3 (writes `bg.height` back; D10 clamp) |
| `image_calculate_bounds` | K §6.4 (media artwork 32 pt) |
| `bracket_bounds` | I §5.2, B §4.6 (D7 clones have members) |
| `anchor_popup` | I §6.3, B §4.7, D4 |
| `popup_bounds` | I §6.4, §5.3 |
| `bar_scene` | B §5.1, K §5.4–5.6 (clip holes, Q11) |
| `popup_scene` | I §6.5 (shadow forced off) |
| `item_scene` | K §4.9, §5.5, §6.5, §7.4, §8.5, §9.8, §10.3; app_menu (X) |
| `window_at` | I §9.2 (z-order emulation) |
| `bar_at_point`, `popup_at_point` | I §9.2 (half-open) |
| `item_local_point` | I §9.3, D2 |
| `slider_track_contains` | K §7.6, D2 |
| `app_menu_title_at` | X |

`item_at_point` is implemented (global order, inclusive edges).

### WP-B — `command.rs`, `query.rs`

| function | spec |
|---|---|
| `Tokens::next_token` | C §3.1 (empty argv element ends the message) |
| `Tokens::next_starts_with_dash` | C §3.2 Mode A |
| `Tokens::batch_line` | C §3.2 Mode B |
| `parse` | C §3.2–3.4, §5–§9, §11; extensions `--monitor`, `--menu`, `--menubar`, `--query stats|menus` (X) |
| `bre_to_regex` | C §3.5 (POSIX BRE, unanchored; `//` → error) |
| `query::query` | C §9 (keyword shadowing, both "not found" texts) |
| `events_json` | E §3.4 |
| `displays_json` | B §9.2 |
| `default_menu_items` | K §9.4 |
| `stats_json`, `menus_json` | X |

`Selector::parse`/`pattern` and `query::item_json` are implemented.

### WP-C — `runtime.rs`, `group.rs`

| function | spec |
|---|---|
| `Runtime::begin` | B §6.6 `bar_manager_begin`, E §8.3 |
| `Runtime::handle` | E §1.1 (poll active display first), dispatch of every `Input` |
| `Runtime::frame` | B §5.1–5.2 (redraw decision, `associated_bar` bits, nirvana), E §10.6–10.7 |
| `Runtime::next_deadline` | E §8.1 (1 s clock, missed fires skipped), wake re-post, animations |
| `handle_message`, `exec` | C §2.5, §3.4 (refresh flag for `--bar`/`--update`/`--remove`; `--exit` sends no reply; `--hotload`, `--load-font`) |
| `select` | C §3.5 (regex errors/no-match texts) |
| `exec_set` | C §6.2 (token-major, malformed message names the first item, continue) |
| `exec_default` | C §6.3 (malformed ends the domain) |
| `exec_bar` | B §2 |
| `apply_requests` | `PropRequest` docs; I §2.3 (`AddToPopup` failure: respond, no `needs_update`), B §2.3–2.5, `ResetDefaultItem` |
| `exec_add` | C §6.1, I §2.1, K §7.1/§8.1/§9.1, D8 (missing bracket member skipped, bracket kept), app_menu registers `menus_change` |
| `exec_clone` | I §10.1, D6, D7 |
| `remove_item` | I §10.5, C §6.4 (detach popup children), D18 |
| `exec_move`, `exec_reorder`, `exec_rename`, `exec_push` | I §10.2–10.6, D5, D8 |
| `popup_add_item`, `popup_remove_item` | I §6.1 |
| `exec_query` | C §9 (builds `QueryCx`) |
| `reload` | C §11, E §9.4 (mach helpers `"k"`, `animator.clear()`, events reset, listeners kept, `Effect::RunConfig`) |
| `update_item` | E §4.2, I §8.3 (counter, marquee via `animation::marquee`, gating, D1) |
| `trigger_event` | E §4.1, D16 |
| `exec_subscribe` | E §3.3 (lazy listeners) |
| `exec_trigger` | E §7, Q21 |
| `exec_update` | E §8.2 |
| `routine_tick` | E §8.1, K §9.7 (alias recapture) |
| `handle_os_event` | E §2, §5 |
| `handle_space_change` | E §5.2, B §6.4–6.5 |
| `handle_display_change`, `poll_active_display`, `displays_changed` | E §5.3, §9.3 |
| `system_will_sleep`, `system_woke` | E §9.1–9.2 |
| `handle_mouse`, `on_click`, `on_scroll`, `on_enter_exit`, `on_drag` | E §6, I §9, D2 |
| `begin_bars`, `set_hidden`, `bar_needs_redraw` | B §6.6, §2.3, §5.2 |
| `apply_anim_steps`, `mark_dirty` | E §10.7 |
| `configure_provider`, `provider_sample` | X providers |
| `alias_image` | K §9.6 |
| `menu_titles` | X app_menu |
| `handle_lua` | `LuaRequest` docs |
| `item_under` | hover tracking |
| `group::add_member`, `remove_member`, `destroy_group` | I §2.4, §10.5 |

### WP-D — `animation.rs`, `event.rs`, `script.rs`, `provider.rs`

| function | spec |
|---|---|
| `Animator::step` | E §10.3–10.6, D12 (`n/60` s wall clock) |
| `Animator::next_deadline` | E §10.6 |
| `animation::marquee` | K §4.10, E §10.9 |
| `event::space_change_info` | E §5.2 (B2: no truncation) |
| `event::display_change_info` | E §5.3 (B3) |
| `event::level_info` | E §5.6–5.7 |
| `event::modifier_description` | E §6.2 |
| `event::click_env` | E §6.2 (`modfier_code`) |
| `event::scroll_env`, `scroll_global_env` | E §6.3 |
| `event::trigger_env` | E §7 |
| `ScrollThrottle::feed` | E §6.3 |
| `script::build_update_env` | E §4.2/4.4, D1, Q3 |
| `script::build_click_script_env` | E §6.2 |
| `script::serialize_for_mach` | E §4.6 |
| `provider::format_template`, `sample_info_json`, `apply_sample` | X |
| `ProviderKind::default_format`, `default_freq` | X (document the chosen defaults in `docs/EXTENSIONS.md`) |

## 4. Cross-package contracts

WP-C is the only caller of the other packages' `todo!()` functions; A, B and D only use
the finished data model.

| caller | callee | contract |
|---|---|---|
| C | A `layout(model, res) -> Layout` | Writes every item's frame on every bar (nirvana when not drawn), component bounds, popup state. `BarLayout.items` = items drawn on that bar (C sets `associated_bar` bit `adid-1` for them and clears it for all others). Calls `BarItem::ensure_layout` itself. |
| C | A `bar_frame(bar, display, menu_bar_visible)` | C stores the result in `BarState.frame` when `bar_needs_resize` (bar_resize). |
| C | A `bar_scene`, `popup_scene` | Pure: read the model + layout, return the window scene. |
| C | A `window_at`, `item_at_point`, `bar_at_point`, `popup_at_point` | Replace C's `wid` lookups (`window_at`) and point lookups. |
| C | A `item_local_point`, `slider_track_contains`, `app_menu_title_at` | Slider click/drag (`Slider::handle_drag` takes the local point), app_menu clicks. |
| C | B `parse(args) -> Vec<Command>` | State-independent; C emits all state-dependent messages. |
| C | B `bre_to_regex` | `Ok(regex)` used with `regex::Regex::new`; `Err` (or a `Regex::new` failure) → `[!] Regex: Could not compile regex '<token>'\n`. |
| C | B `query(target, &QueryCx) -> String` | Full response text incl. trailing `\n` or error message. |
| C | D `Animator::step(now) -> Vec<AnimStep>` | Steps in order; C applies each via `anim_set` on the target (`Bar` → `model.bar`, `Default` → `model.default_item`, `Item(id)` → item) with a `PropEffects`, marks dirty on `true`. |
| C | D `marquee(target, prefix, text)` | C adds the returned `PendingAnim`s: `cancel_locked` + `add` for the first and third (ANIMATE_FLOAT), plain `add` for the 0-frame jump. |
| C | D `build_update_env`, `build_click_script_env`, `serialize_for_mach` | C turns the env into `Effect::RunScript { env: env.into_vec() }` / `PlatformRequest::MachSend`. |
| C | D `event::*` builders, `trigger_env`, `ScrollThrottle::feed` | Builders return the exact env/INFO texts; C only routes them. |
| C | D `provider::apply_sample`, `default_freq` | C sets `label`/`icon` through `BarItem::set_prop` (so animations/widths behave like `--set`) and runs the script with `SENDER=provider`. |
| B | data model | `BarItem::to_json` (via `item_json`), `BarProps::to_json(&model.item_names())`, `CustomEvents::events()`. |
| A | data model | `BarItem::{length, content_length, height, shadow_extents, set_frame}`, `Text::length/height`, `Image::reserved_size`, `Graph::sample/effective_fill`, `Slider` fields, `Model::draws_item`. |

Property-layer conventions every package relies on:

* All property writes go through `set_prop(key, value, &mut PropCx)` with
  `cx.set_target(AnimTarget::…)` and `cx.anim` set from the message's `--animate`.
  `Ok(true)` → `needs_update` (items) or the `--bar` refresh flag; `Err(e)` → append
  `e.to_string()` to the response (and `Effect::Log`). `cx.response` (non-fatal texts) is
  appended in order; `cx.fx` is merged into `Model` flags; `cx.requests` are executed by
  `apply_requests` right after the call.
* Animation identity is `(AnimTarget, path)`; `path` mirrors the property path and is
  produced by `PropCx::scoped/enter`. The item key `padding_left` uses path
  `background.padding_left`; `slider.background.<p>` animates `slider.foreground.<p>` and
  `slider.background.<p>`; text width animations (`width=`, animated `string=`) use
  `<text>.width`; marquee uses `<text>.scroll`.
* Text lines are measured at set time (`Resources::text_metrics`); font changes only set
  `Text::font_changed`; call `BarItem::ensure_layout(res)` before reading lengths.
* `Background::height` is C's `bounds.size.height`: layout writes computed heights back.
* Component `bounds` are item-local, y-up (C coordinates); scenes are window-local,
  top-left.

## 5. Spec ambiguities resolved in the skeleton

1. **`anim_slot` → `anim_set`.** Raw field references cannot run setter side effects
   (background enabling, shadow offsets, font relayout flag, clip/resize flags), so
   components expose `anim_set(path, AnimValue, &mut PropEffects) -> bool`.
2. **JSON is hand-written text**, not `serde_json::Value`, to keep SketchyBar's layout and
   key order byte-for-byte; all strings (incl. `script`/`click_script`) are fully JSON-escaped
   (D13). `(null)` stays unquoted text inside the quotes.
3. **Byte colour animation truncates** (`(u8)((1-s)a + s·b)`, E §10.4); `Color::lerp_bytes`
   was rounding and has been fixed.
4. **D9 messages** (no SketchyBar text exists): `[!] Item (<name>): Invalid space '<e>'\n`,
   `[!] Item (<name>): Invalid display '<e>'\n` (entries ≥ 32 skipped, others applied),
   `[!] Bar: Invalid display '<e>'\n` (`0`, non-numeric or > 32 skipped).
5. **Popup parent on `position=` change** stays set like C (`item.md` §2.3); the `cli.md`
   suggestion to clear it is not adopted (not in DEVIATIONS).
6. **Cloned popup members are not attached** to the host popup (C behaviour; DEVIATIONS does
   not list the `cli.md` recommendation).
7. **D8 bracket members:** a member token that resolves to nothing gets the `[?] Add (Group)`
   message and is skipped; the bracket is kept even when the first token fails. The popup
   rule applies to the first token that resolves.
8. **`topmost` reset** sees the new `topmost` value (C's stale menu-bar offset until the next
   resize is not reproduced).
9. **`menus_change`** (app_menu extension) is registered lazily on the first `--add app_menu`
   so plain configs keep SketchyBar's custom-event bits (18+).
10. **Extension query blocks** `"app_menu": {…}` and `"provider": {…}` are appended only for
    such items; the type name of app_menu items is `"app_menu"`; Lua-handled items print
    `"script": "lua:<id>"`.
11. **Item windows are virtual** (`BarItem::frames`, one per display); hit tests emulate the
    window z-order (`layout::window_at`). Item enter/exit is synthesized from
    `MouseKind::Moved`.
12. **Forced OS events** (`--update`, `--trigger <builtin>`) read values synchronously via
    `Resources::query_system`; `media_change` stays asynchronous (`RefreshMedia`).
13. **Image loading** is platform-side with three outcomes (`NotFound`, `InvalidFormat`,
    `DecodeFailed`) mapped to the exact messages; `space.<n>` uses `atoi`.
14. **Font fields > 254 bytes** are truncated at a char boundary (C truncates bytes).
15. **Slider with track width 0:** percentage from a saturating cast (`d/0 → 100`, `0/0 → 0`),
    the arm64 behaviour (D10).
16. **D3** is implemented in `Model::draws_item`: a popup member is drawn only while its host
    is drawn on that bar.
17. **`hidden=`** is executed by the runtime (`PropRequest::BarHidden`); the setter returns
    `false`, `apply_requests` sets the refresh flag (C returns true, or false when `current`
    fails).
18. **`reset=`** re-initialises the default item but keeps its `update_mask` (`bar_item_init`
    does not clear it).
19. **B1** (space clone gets `SID = parent's DID`) is kept; it is not in DEVIATIONS.
