# SketchyBar compatibility examples

Test-case catalogue for mbar's 1:1 SketchyBar compatibility. It covers:

1. Every command in the stock `sketchybarrc` and `plugins/*.sh`, with the item state it must produce, the events that fire and the environment variables the plugins read.
2. Documented SketchyBar behaviour that these files do not exercise.
3. Common community config patterns, written as shell tests that run against a headless daemon and check the result with `--query`.

## Provenance tags

Each statement carries a tag that says where it comes from.

| Tag | Source |
|---|---|
| **[FILE]** | Read directly from the checkout: `sketchybarrc`, `plugins/*.sh`, `README.md`, `src/misc/help.h`. |
| **[SRC]** | Checked against the C source in the same checkout (`src/*.c`, `src/misc/*.h`). Use these as authoritative. |
| **[DOCS-MEM]** | Recalled from the official documentation (felixkratz.github.io/SketchyBar). It was not re-checked against the site and may be out of date or imprecise. Where it conflicts with [SRC], [SRC] wins. |
| **[COMMUNITY]** | A common pattern from public dotfiles and discussions, such as the "Setups" and "Plugins" threads, the aerospace and yabai docs, and sketchybar-app-font. |

The checkout is at `scratchpad/sketchybar`. Paths below are relative to its root.

---

## 0. Test harness conventions

All snippets in sections 1 and 3 assume this preamble. `$SB` is the client binary. A `sketchybar -> mbar` symlink must be on `PATH`, because the plugins call `sketchybar` by name. Scripts run asynchronously, so assertions on script side effects must poll.

Plugins run with the **daemon's** environment, not the client's [SRC `sync_exec`: `setenv` in the forked daemon child]. Stub binaries in `$T/bin` are only found if the headless daemon is started **after** the `PATH` export below, for example `PATH="$T/bin:$PATH" mbar --headless &`. The same applies to `T` itself.

```sh
#!/usr/bin/env bash
set -euo pipefail
SB=${SB:-sketchybar}
export T=$(mktemp -d)               # per-test scratch dir (plugin scripts, stub binaries, logs)
export PATH="$T/bin:$PATH"; mkdir -p "$T/bin"

q()  { "$SB" --query "$1" | jq -r "$2"; }          # q <item|bar|defaults|events> <jq-filter>
eq() { local got; got=$(q "$1" "$2"); [ "$got" = "$3" ] || { echo "FAIL $1 $2: got '$got' want '$3'" >&2; exit 1; }; }
wait_eq() {                                         # poll up to ~3s for async script effects
  local i got; for i in $(seq 1 60); do got=$(q "$1" "$2"); [ "$got" = "$3" ] && return 0; sleep 0.05; done
  echo "TIMEOUT $1 $2: got '$got' want '$3'" >&2; exit 1; }
u() { python3 -c 'import sys;sys.stdout.write(chr(int(sys.argv[1],16)))' "$1"; }  # codepoint -> UTF-8
mkscript() { cat > "$T/$1"; chmod +x "$T/$1"; }     # mkscript name.sh <<'EOF' ... EOF
stub()     { cat > "$T/bin/$1"; chmod +x "$T/bin/$1"; }
```

Query output facts used by the assertions [SRC]:

- Booleans serialise as `"on"` / `"off"` strings (`format_bool`). This holds for `drawing`, `highlight`, `scroll_texts`, `ignore_association`, and the bar's `topmost`, `sticky`, `hidden`, `shadow`, `font_smoothing` and `show_in_fullscreen`.
- Colours serialise as `"0x%x"`: lowercase, no zero padding. `0x00000000` becomes `"0x0"`, and `0xff000000` stays `"0xff000000"`.
- Fonts serialise as `"<family>:<style>:<size %.2f>"`, for example `"Hack Nerd Font:Bold:17.00"`.
- `--query <item>` has the following shape:
  - `{name, type, geometry{drawing, position, associated_space_mask, associated_display_mask, ignore_association, y_offset, padding_left, padding_right, scroll_texts, width, background{…, shadow}}, icon{…}, label{…}, scripting{script, click_script, update_freq, update_mask, updates}, bounding_rects{display-N{origin,size}}}`
  - `popup{…}` is present only when the popup has at least one item.
  - `bracket[…]` is present for brackets, `graph{…}` for graphs and `slider{…}` for sliders.
- `geometry.width` is `-1` unless a constant width is set.
- `geometry.position` is one of `left`, `right`, `center`, `q`, `e` or `popup`.
- `geometry.padding_left` and `geometry.padding_right` are the item paddings.
- Text objects (`icon`, `label`) have these fields: `value, drawing, highlight, color, highlight_color, padding_left, padding_right, y_offset, font, width, scroll_duration, align, background{…}`.
- `--query bar` returns `{position, topmost, sticky, hidden, shadow, font_smoothing, show_in_fullscreen, blur_radius, margin, drawing, color, border_color, border_width, height, corner_radius, padding_left, padding_right, x_offset, y_offset, clip, image{…}, items[ names in creation order ]}`. The bar background fields are inlined at the top level.
- `--query events` returns `{ "<event>": { "bit": <u64>, "notification": "<name or (null)>" }, … }`.
- `scripting.updates` is `"on"`, `"off"` or `"when_shown"`.

### Built-in event bits [SRC `custom_events.c`, `custom_events.h`]

`scripting.update_mask` is the OR of these bits. Custom events take the next free bits, starting at `1<<18` (262144), in the order they were added.

| bit | value | event |
|---|---|---|
| 0 | 1 | `front_app_switched` |
| 1 | 2 | `space_change` |
| 2 | 4 | `display_change` |
| 3 | 8 | `system_woke` |
| 4 | 16 | `mouse.entered` |
| 5 | 32 | `mouse.exited` |
| 6 | 64 | `mouse.clicked` |
| 7 | 128 | `mouse.scrolled` |
| 8 | 256 | `system_will_sleep` |
| 9 | 512 | `mouse.entered.global` |
| 10 | 1024 | `mouse.exited.global` |
| 11 | 2048 | `mouse.scrolled.global` |
| 12 | 4096 | `volume_change` |
| 13 | 8192 | `brightness_change` |
| 14 | 16384 | `power_source_change` |
| 15 | 32768 | `wifi_change` |
| 16 | 65536 | `media_change` |
| 17 | 131072 | `space_windows_change` |

Adding an event whose name already exists does nothing.

### Script execution model [SRC `misc/helpers.h`, `bar_item.c`]

- Scripts run as `/usr/bin/env sh -c "<script string>"` in a `vfork` child, with `alarm(60)`. A script running longer than 60 seconds is killed. The string is passed verbatim, so `script="$PLUGIN_DIR/x.sh"` requires `x.sh` to be executable, and arguments inside the string work (for example `script="foo.sh 3"`).
- A leading `~` in `script` or `click_script` is expanded to `$HOME` when the property is set.
- Environment variables reach the child through `setenv` and are layered over the daemon's environment:
  - The daemon environment holds `CONFIG_DIR`, set to the directory of the config file, and `BAR_NAME`, set to `basename(argv[0])` of the daemon, usually `sketchybar` [SRC `sketchybar.c:203`]. The bar name also selects the config directory (`~/.config/<name>/`) and the mach service name. A `sketchybar -> mbar` symlink therefore yields `BAR_NAME=sketchybar` in SketchyBar.
  - Every script invocation sets `NAME`, the item name, and `SENDER`: the event name, `"forced"` for `--update`, or `"routine"` for an `update_freq` tick.
  - Events add `INFO` and other variables, described below.
  - Space items also set `SELECTED`, `SID` and `DID`.
  - Sliders set `PERCENTAGE`.
  - Click scripts and `mouse.clicked` get `BUTTON` and `MODIFIER`.
  - Scroll scripts get `SCROLL_DELTA` and `MODIFIER`.
- Event-specific variables are merged with the item's own variables (`NAME`, plus `SELECTED`/`SID`/`DID` for spaces) before the script is executed.
- An item updates on an event only if its `update_mask` contains that event's bit.
- An item with `updates=off`, or with `update_freq=0` and no sender, is skipped unless the run is forced.
- `updates=when_shown` additionally requires the item to be shown on some bar.

---

## 1. Stock config: line-by-line expectations

### 1.1 `sketchybarrc` [FILE], with semantics marked [SRC] where verified

| rc line | Command | Required effect |
|---|---|---|
| 6 | `PLUGIN_DIR="$CONFIG_DIR/plugins"` | Plain shell. The daemon must export `CONFIG_DIR` to the config process: the directory of the config file, default `~/.config/sketchybar` [SRC `hotload.c:58`]. All `script=` values below are absolute paths built from it. |
| 15 | `sketchybar --bar position=top height=40 blur_radius=30 color=0x40000000` | Bar query: `.position=="top"`, `.height==40`, `.blur_radius==30`, `.color=="0x40000000"`, `.drawing=="on"`. Setting the color enables the bar background [SRC `background_set_color`]. `position` is parsed by first character only (`t` or `b`) [SRC]. |
| 22–34 | `sketchybar --default padding_left=5 padding_right=5 icon.font="Hack Nerd Font:Bold:17.0" label.font="Hack Nerd Font:Bold:14.0" icon.color=0xffffffff label.color=0xffffffff icon.padding_left=4 icon.padding_right=4 label.padding_left=4 label.padding_right=4` | Modifies the default item. Only items created after this point inherit it [SRC `bar_item_init`, `bar_item_inherit_from_item`]. `--query defaults` must show `.geometry.padding_left==5`, `.geometry.padding_right==5`, `.icon.font=="Hack Nerd Font:Bold:17.00"`, `.label.font=="Hack Nerd Font:Bold:14.00"`, `.icon.color=="0xffffffff"`, `.label.color=="0xffffffff"`, and `.icon.padding_left`, `.icon.padding_right`, `.label.padding_left`, `.label.padding_right` all `4`. |
| 41–58 | Loop over `i=0..9`, with `sid=i+1`: `sketchybar --add space space.$sid left --set space.$sid space=$sid icon=$sid icon.padding_left=7 icon.padding_right=7 background.color=0x40ffffff background.corner_radius=5 background.height=25 label.drawing=off script="$PLUGIN_DIR/space.sh" click_script="yabai -m space --focus $sid"` | Creates 10 items `space.1` … `space.10`, in that order, on the left (see 1.1.1). |
| 64–65 | `--add item chevron left --set chevron icon=` (U+F054, nf-fa-chevron_right) `label.drawing=off` | `chevron`: `.type=="item"`, `.geometry.position=="left"`, `.icon.value=="\uF054"`, `.label.drawing=="off"`. Inherits the defaults. No script, so it never updates. |
| 66–68 | `--add item front_app left --set front_app icon.drawing=off script="$PLUGIN_DIR/front_app.sh" --subscribe front_app front_app_switched` | `front_app`: `.icon.drawing=="off"`, `.scripting.script==CONFIG_DIR+"/plugins/front_app.sh"`, `.scripting.update_mask==1`, `.scripting.update_freq==0`. |
| 81–82 | `--add item clock right --set clock update_freq=10 icon=` (U+F43A, nf-oct-clock) `script="$PLUGIN_DIR/clock.sh"` | `clock`: `.geometry.position=="right"`, `.scripting.update_freq==10`, `.icon.value=="\uF43A"`, `.scripting.update_mask==0`. The script runs every 10 s with `SENDER=routine`. |
| 83–85 | `--add item volume right --set volume script="$PLUGIN_DIR/volume.sh" --subscribe volume volume_change` | `.scripting.update_mask==4096`. Subscribing to `volume_change` starts the CoreAudio listener [SRC `bar_item_parse_subscribe_message` → `begin_receiving_volume_events`]. |
| 86–88 | `--add item battery right --set battery update_freq=120 script="$PLUGIN_DIR/battery.sh" --subscribe battery system_woke power_source_change` | `.scripting.update_freq==120`, `.scripting.update_mask==16392` (8 \| 16384). |
| 91 | `sketchybar --update` | Forced update of everything (see 1.1.2). |

Final state:

- Bar `.items` order: `space.1` … `space.10`, `chevron`, `front_app`, `clock`, `volume`, `battery`.
- Layout: left items are laid out left to right in creation order. Right items are laid out from the right edge inward, so `clock` is rightmost and `battery` is the leftmost of the right group [DOCS-MEM, consistent with the README screenshot].
- All 15 items have `.geometry.padding_left==5` and `.label.font=="Hack Nerd Font:Bold:14.00"`.

#### 1.1.1 Space items (the loop on lines 41–58)

Expected state for `space.N` [SRC]:

- `.type=="space"`, `.geometry.position=="left"`.
- `.geometry.associated_space_mask == 1<<N`, so `space.1` → 2 and `space.10` → 1024. The `space=` property accepts a comma list, and each entry sets bit `1<<n`. For a space-type item, the last entry wins.
- `.icon.value=="N"`, `.icon.padding_left==7`, `.icon.padding_right==7`. These override the default of 4.
- `.geometry.background.color=="0x40ffffff"`.
- `.geometry.background.drawing=="on"`, because setting `background.color` enables drawing.
- `.geometry.background.corner_radius==5`, `.geometry.background.height==25`.
- `.label.drawing=="off"`.
- `.scripting.script==CONFIG_DIR+"/plugins/space.sh"`.
- `.scripting.click_script=="yabai -m space --focus N"`. `$sid` is expanded by the config shell, not by the daemon.
- `.scripting.update_mask` has the `space_change` bit (2) set.

The `--add space` side effects [SRC `bar_item_set_type`]:

- If no script is set yet, the default script is `sketchybar -m --set $NAME icon.highlight=$SELECTED`. Here it is overwritten by the `--set` in the same command.
- `updates` becomes off.
- The item's environment gets `SELECTED=false`, `SID=0` and `DID=0`. Setting `space=N` then sets `SID=N`.

Space selection [SRC `bar_manager_update_space_components`]:

- On a space change, each space item associated with the active Mission Control index of its display gets `SELECTED=true` and `updates=on`. Items whose selection flips to false get `SELECTED=false` and `updates=on`. Unchanged items get `updates=off`.
- So `space.sh` runs only for items whose selection changed, or for all of them when forced.
- `INFO` for `space_change` is a JSON object `{"display-<adid>": <mission-control-index>, …}`.
- Headless: the platform must provide a fake "current space per display". With none (`sid==0`), no space item is selected.

#### 1.1.2 What `--update` triggers [SRC `bar_manager_update(forced=true)`]

The steps run in this order:

1. `space_change` is handled as forced. All space items are re-evaluated, and `space_change` subscribers run with `SENDER=space_change` and `INFO={"display-1": k}`.
2. Forced provider events, each posted as if it had just changed:
   - `wifi_change`: `INFO=<SSID>`, or empty.
   - `volume_change`: `INFO=<0..100>`, an integer rounded from the volume. Mute gives `0`.
   - `brightness_change`: `INFO=<0..100>`.
   - `power_source_change`: `INFO=AC` or `BATTERY`.
   - `front_app_switched`: `INFO=<localized app name>`.
   - `media_change`: `INFO=<JSON>`.
   - `space_windows_change`: `INFO=<JSON>`.

   The front-app event is queued, not run inline. The others call their handlers directly, and those handlers post events.
3. Every item with a non-empty script runs once with `SENDER=forced`, whatever its `update_freq` and `updates` settings, and its update counter is reset.

For the stock config, one `--update` therefore results in:

| Item | Script invocations (env) | Resulting state (real macOS) |
|---|---|---|
| `space.1..10` | `SENDER=forced` (all), plus `SENDER=space_change` for those whose selection flipped. Env: `NAME`, `SELECTED`, `SID`, `DID`. | `background.drawing` = `on` for the selected space and `off` for the others. |
| `front_app` | `SENDER=front_app_switched INFO=<app>` sets the label. `SENDER=forced` is a no-op, because the script checks `SENDER`. | `.label.value==<front app name>`. |
| `clock` | `SENDER=forced`, then `SENDER=routine` every 10 s. | `.label.value==$(date '+%d/%m %H:%M')`. |
| `volume` | `SENDER=volume_change INFO=<n>`. `SENDER=forced` is a no-op. | See 1.2.5. |
| `battery` | `SENDER=power_source_change INFO=AC\|BATTERY`, `SENDER=forced`, and later `SENDER=routine` every 120 s and `SENDER=system_woke`. | See 1.2.1. The script ignores `SENDER` and `INFO`. |
| `chevron` | none | unchanged |

Headless compatibility test for the stock config:

```sh
# run the stock rc against a headless daemon with stubbed system tools
export CONFIG_DIR="$T/cfg"; mkdir -p "$CONFIG_DIR/plugins"
cp scratchpad/sketchybar/sketchybarrc "$CONFIG_DIR/"; cp scratchpad/sketchybar/plugins/*.sh "$CONFIG_DIR/plugins/"
chmod +x "$CONFIG_DIR"/plugins/*.sh
stub pmset <<'EOF'
#!/bin/sh
printf "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1234)\t87%%; discharging; 4:12 remaining present: true\n"
EOF
stub yabai <<'EOF'
#!/bin/sh
echo "$@" >> "$T/yabai.log"
EOF
bash "$CONFIG_DIR/sketchybarrc"

eq bar .position top; eq bar .height 40; eq bar .blur_radius 30; eq bar .color 0x40000000
eq bar '.items|join(",")' "space.1,space.2,space.3,space.4,space.5,space.6,space.7,space.8,space.9,space.10,chevron,front_app,clock,volume,battery"
eq defaults .icon.font "Hack Nerd Font:Bold:17.00"
eq defaults .label.font "Hack Nerd Font:Bold:14.00"
eq defaults .geometry.padding_left 5
for n in 1 5 10; do
  eq space.$n .type space
  eq space.$n .geometry.associated_space_mask $((1<<n))
  eq space.$n .icon.value "$n"
  eq space.$n .icon.padding_left 7
  eq space.$n .label.drawing off
  eq space.$n .geometry.background.color 0x40ffffff
  eq space.$n .geometry.background.corner_radius 5
  eq space.$n .geometry.background.height 25
  eq space.$n .scripting.click_script "yabai -m space --focus $n"
  eq space.$n .scripting.script "$CONFIG_DIR/plugins/space.sh"
done
eq chevron .icon.value "$(u f054)"
eq chevron .label.drawing off
eq front_app .icon.drawing off
eq front_app .scripting.update_mask 1
eq clock .scripting.update_freq 10
eq clock .icon.value "$(u f43a)"
eq volume .scripting.update_mask 4096
eq battery .scripting.update_mask 16392
eq battery .scripting.update_freq 120
wait_eq clock .label.value "$(date '+%d/%m %H:%M')"   # may flake at a minute boundary; retry once
```

Portability caveats for running the stock plugins on a Linux CI host [FILE + shell semantics]:

- `battery.sh` uses `grep -Eo "\d+%"`. BSD/macOS grep accepts `\d` in ERE. GNU grep does not: it prints "stray \ before d", matches nothing, and the script exits 0, leaving the item unchanged. Put a `grep` wrapper on the stub `PATH` that rewrites `\d` to `[0-9]`, or assert "unchanged" on Linux.
- `battery.sh` has a `#!/bin/sh` shebang but uses `[[ … ]]`. On macOS `/bin/sh` is bash, so this works. On dash it fails with "[[: not found" and the condition is false, so the charging icon is never applied. The daemon always runs `sh -c "<path>"`, so the shebang decides the interpreter.
- `pmset` and `yabai` must be stubbed.

#### 1.1.3 How the rc itself is executed [SRC `hotload.c`, `sketchybar.c`]

- Lookup order, unless `-c/--config FILE` is given [FILE help.h + SRC]:
  1. `$XDG_CONFIG_HOME/<bar_name>/sketchybarrc`
  2. `$HOME/.config/<bar_name>/sketchybarrc`
  3. `$HOME/.sketchybarrc`
- Before running the rc, the daemon:
  - sets `CONFIG_DIR` to the rc's directory in its own environment (inherited by every script),
  - `chdir`s there,
  - adds the executable bit to the rc if it is missing,
  - runs it with `/usr/bin/env sh -c "<rc path>"`, which is fire-and-forget: the daemon does not wait.
- `BAR_NAME` is exported by the daemon at startup.
- The stock rc has **no shebang** and uses bash arrays (`default=( … )`, `"${!SPACE_ICONS[@]}"`). It only works because macOS `/bin/sh` is bash. mbar's headless and Linux runners must execute a shebang-less rc with bash, not dash, to stay compatible.
- `--reload [path]` re-runs this procedure after removing all items. `--hotload on|off` watches the config directory and reloads on change, rate-limited to about one reload per second.

### 1.2 Plugins [FILE]

Every plugin is a POSIX `sh` script that calls back into `sketchybar --set "$NAME" …`. Glyphs are listed by codepoint, with their Nerd Font names.

#### 1.2.1 `plugins/battery.sh`

- **Env read:** `NAME`. It reads neither `SENDER` nor `INFO`.
- **External commands:** `pmset -g batt` (twice), `grep`, `cut`.
- **Logic:**
  - `PERCENTAGE` is the first `\d+%` match in the `pmset` output, without the `%`.
  - `CHARGING` is set when the output contains `AC Power`.
  - Empty `PERCENTAGE` → `exit 0`, and nothing is set (desktop Macs).
- **Command:** `sketchybar --set "$NAME" icon="$ICON" label="${PERCENTAGE}%"`

| PERCENTAGE | CHARGING | icon | label |
|---|---|---|---|
| 90–99, 100 | no | U+F240 (nf-fa-battery_full) | `N%` |
| 60–89 | no | U+F241 (battery_three_quarters) | `N%` |
| 30–59 | no | U+F242 (battery_half) | `N%` |
| 10–29 | no | U+F243 (battery_quarter) | `N%` |
| 0–9 (and anything else) | no | U+F244 (battery_empty) | `N%` |
| any | yes ("AC Power" in output) | U+F0E7 (nf-fa-bolt) | `N%` |
| "" | – | unchanged | unchanged |

Triggers in the stock rc: `--update` (forced), `update_freq=120` (routine), `system_woke`, `power_source_change`.

```sh
# battery: one case per row; a grep wrapper makes the BSD-only \d work on GNU hosts
stub grep <<'STUB'
#!/bin/sh
args=""; for a in "$@"; do a=$(printf '%s' "$a" | sed 's/\\d/[0-9]/g'); args="$args '$a'"; done
eval exec /usr/bin/grep $args
STUB
mkscript battery.sh < scratchpad/sketchybar/plugins/battery.sh
"$SB" --add item battery right --set battery script="$T/battery.sh" --subscribe battery system_woke power_source_change
batt() {  # batt <percent> <"Battery Power"|"AC Power">
  printf '#!/bin/sh\nprintf "Now drawing from %s\\n -InternalBattery-0 (id=1)\\t%s%%%%; x; y present: true\\n"\n' "'$2'" "$1" > "$T/bin/pmset"
  chmod +x "$T/bin/pmset"; }
batt 95 "Battery Power"; "$SB" --trigger system_woke
wait_eq battery .label.value "95%"; eq battery .icon.value "$(u f240)"
batt 75 "Battery Power"; "$SB" --update; wait_eq battery .icon.value "$(u f241)"
batt 45 "Battery Power"; "$SB" --update; wait_eq battery .icon.value "$(u f242)"
batt 15 "Battery Power"; "$SB" --update; wait_eq battery .icon.value "$(u f243)"
batt 5  "Battery Power"; "$SB" --update; wait_eq battery .icon.value "$(u f244)"; eq battery .label.value "5%"
batt 50 "AC Power";      "$SB" --update; wait_eq battery .icon.value "$(u f0e7)"   # needs bash as /bin/sh ([[ ]])
```

Note on `--trigger` and built-in events [SRC `message.c handle_domain_trigger`]:

- `--trigger system_woke` is not special-cased. It goes through the generic custom-event path, so subscribers run with `SENDER=system_woke` plus any `KEY=VALUE` pairs given on the command line.
- `--trigger power_source_change` calls `forced_power_event()`. It re-reads the real power source, and user-supplied variables are **ignored**.
- The same holds for `space_change`, `display_change`, `space_windows_change`, `volume_change`, `media_change` and `wifi_change`.
- A headless platform must therefore expose fake providers for those events. User-supplied `INFO=` has no effect on them.
- Every other name, including `front_app_switched`, `system_woke`, `system_will_sleep`, the `mouse.*` events and custom events, delivers the user's `KEY=VALUE` pairs as environment variables.

#### 1.2.2 `plugins/clock.sh`

- **Env read:** `NAME`.
- **Command:** `sketchybar --set "$NAME" label="$(date '+%d/%m %H:%M')"`, which gives a label such as `07/10 14:05`.
- **Triggers:** `--update` (`SENDER=forced`) and every `update_freq=10` seconds (`SENDER=routine`).
- `update_freq` is counted in daemon ticks of 1 s [DOCS-MEM: "time in seconds between routine script executions"]. A routine run requires `updates` on (the default), or, with `updates=when_shown`, that the item is shown.

```sh
mkscript clock.sh < scratchpad/sketchybar/plugins/clock.sh
"$SB" --add item clock right --set clock update_freq=1 script="$T/clock.sh"
wait_eq clock .label.value "$(date '+%d/%m %H:%M')"     # no --update needed: routine tick after ~1s
"$SB" --set clock update_freq=0 label=frozen; sleep 2; eq clock .label.value frozen  # freq 0 => no routine runs
```

#### 1.2.3 `plugins/front_app.sh`

- **Env read:** `SENDER`, `INFO`, `NAME`.
- **Command:** `sketchybar --set "$NAME" label="$INFO"`, but only when `SENDER=front_app_switched`.
- `INFO` holds the localized name of the frontmost application (`NSRunningApplication.localizedName`) [SRC `workspace.m`].
- A forced or routine run is a no-op.

```sh
mkscript front_app.sh < scratchpad/sketchybar/plugins/front_app.sh
"$SB" --add item front_app left --set front_app icon.drawing=off script="$T/front_app.sh" --subscribe front_app front_app_switched
"$SB" --trigger front_app_switched INFO="Ghostty"        # generic trigger path: user INFO is delivered
wait_eq front_app .label.value "Ghostty"
NAME=front_app SENDER=forced INFO=X "$T/front_app.sh"; eq front_app .label.value "Ghostty"   # forced: no-op
```

#### 1.2.4 `plugins/space.sh`

- **Env read:** `NAME`, `SELECTED` (`"true"` or `"false"`).
- **Command:** `sketchybar --set "$NAME" background.drawing="$SELECTED"`
- **Result:** `.geometry.background.drawing` is `"on"` when `SELECTED=true`, otherwise `"off"`. All boolean parsers accept `on|yes|true|1|!off|!no|!false|!0` as true and `toggle` as a flip; anything else is false [SRC `evaluate_boolean_state`].
- **Also available but unused here:** `SID` (the space index from `space=`) and `DID` (the display index, `0` until associated) [SRC].

```sh
mkscript space.sh < scratchpad/sketchybar/plugins/space.sh
"$SB" --add space space.1 left --set space.1 space=1 background.color=0x40ffffff script="$T/space.sh" \
      --add space space.2 left --set space.2 space=2 background.color=0x40ffffff script="$T/space.sh"
eq space.1 .geometry.background.drawing on         # color set => drawing on
# headless platform hook: make Mission-Control index 2 active on display 1, then:
"$SB" --trigger space_change
wait_eq space.2 .geometry.background.drawing on
wait_eq space.1 .geometry.background.drawing off
# Direct script contract (no platform needed):
NAME=space.1 SELECTED=true  "$T/space.sh"; eq space.1 .geometry.background.drawing on
NAME=space.1 SELECTED=false "$T/space.sh"; eq space.1 .geometry.background.drawing off
```

#### 1.2.5 `plugins/volume.sh`

- **Env read:** `SENDER`, `INFO`, `NAME`.
- **Logic:** acts only when `SENDER=volume_change`, with `VOLUME=$INFO` (an integer 0–100).
- **Command:** `sketchybar --set "$NAME" icon="$ICON" label="$VOLUME%"`

| INFO | icon |
|---|---|
| 60–99, 100 | U+F057E (nf-md-volume_high) |
| 30–59 | U+F0580 (nf-md-volume_medium) |
| 1–9, 10–29 | U+F057F (nf-md-volume_low) |
| 0 / anything else | U+F0581 (nf-md-volume_off) |

How `INFO` is produced [SRC `volume.c`, `bar_manager_handle_volume_change`]:

- `INFO = (int)(volume*100 + 0.5)` from the left channel (or main) of the default output device.
- `0` if muted.
- The event fires only when the value moves by more than 0.01.
- Forced runs (`--update`, `--trigger volume_change`) always fire.
- The first `--subscribe … volume_change` installs the listeners.

```sh
mkscript volume.sh < scratchpad/sketchybar/plugins/volume.sh
"$SB" --add item volume right --set volume script="$T/volume.sh" --subscribe volume volume_change
# Real event requires the headless volume provider; set fake volume=0.73, then:
"$SB" --trigger volume_change
wait_eq volume .label.value "73%"; eq volume .icon.value "$(u f057e)"
# Script contract table (no provider needed):
for v in 100:f057e 60:f057e 59:f0580 30:f0580 29:f057f 1:f057f 0:f0581; do
  NAME=volume SENDER=volume_change INFO=${v%%:*} "$T/volume.sh"
  eq volume .label.value "${v%%:*}%"; eq volume .icon.value "$(u ${v##*:})"
done
NAME=volume SENDER=forced INFO=5 "$T/volume.sh"; eq volume .label.value "0%"   # no-op for other senders
```

### 1.3 `src/misc/help.h`: full CLI surface [FILE], with notes [SRC]

| Syntax | Notes |
|---|---|
| `-c, --config FILE` | Startup only. |
| `--bar <k>=<v> …` | Several pairs are allowed. Parsing of the domain stops at the next token that starts with `-`. |
| `--add item <name> <position>` | Positions are `left`, `right`, `center`, `q` (left of the notch), `e` (right of the notch) and `popup.<parent>` [DOCS-MEM + SRC]. Only the first character is checked (`l`, `r`, `c`, `q`, `e`, `p`), so `lefty` parses as left [SRC `bar_item_set_position`]. An existing name gives `[?] Add: Item '<n>' already exists`. An illegal position gives `[!] Add <n>: Illegal position '<p>'`. |
| `--set <name> <k>=<v> …` | `<name>` may be a regex `/…/` matching several items [SRC `get_bar_items_for_regex`]. The regex is POSIX **basic** (`regcomp(…, 0)`), unanchored and case-sensitive, so `|` and `+` are literal characters. |
| `--default <k>=<v> …` | Applies only to items created afterwards. `--default reset` restores the built-in defaults [SRC `COMMAND_DEFAULT_RESET`]. |
| `--set <name> popup.<k>=<v>` | Popup properties (section 2.5). |
| `--reorder <n> … <n>` | |
| `--move <n> before\|after <ref>` | |
| `--clone <parent> <name> [before\|after]` | Copies all properties and the script. For a space item, `SID` and `DID` are copied from the parent's `DID` (sic, `bar_item.c:792`). |
| `--rename <old> <new>` | |
| `--remove <name>` | Also accepts a `/regex/`. |
| `--add graph <name> <pos> <width>` | `--push <name> <v> …` pushes data points in `[0,1]`. |
| `--add space <name> <pos>` | |
| `--add bracket <name> <member> …` | Members may be `/regex/`. There is no position argument. |
| `--add alias <app_name>[,<window_name>] <pos>` | For example `"Control Center,Battery"`. |
| `--add slider <name> <pos> <width>` | |
| `--subscribe <name> <event> …` | An unknown event gives `[?] Event: '<e>' not found`, and the known ones are still applied. An unknown item gives `[!] Subscribe: Item not found '<n>'`. |
| `--add event <name> [<NSDistributedNotificationName>]` | |
| `--trigger <event> [<k>=<v> …]` | See the note under 1.2.1. |
| `--query bar\|<name>\|item <name>\|defaults\|events\|default_menu_items` | An unknown name gives `[!] Query: Invalid query, or item '<n>' not found `. `displays` also exists in source [SRC `COMMAND_QUERY_DISPLAYS`] but is not in `help.h`. |
| `--animate <linear\|quadratic\|tanh\|sin\|exp\|circ> <duration> --bar … --set …` | The curve is chosen by its **first character** only (`l`, `q`, `t`, `s`, `e`, `c`; anything else is linear). Duration is in 60 Hz frames: 30 → 0.5 s [SRC `animation.c`]. |
| `--hotload <bool>` | |
| `--reload [path]` | |

Present in source but **not** in `help.h` [SRC `misc/defines.h`]: `--update`, `--exit`, `--load-font <path>`, `--query displays`.

Animation semantics [SRC `animation.c`, `animation.h`, `message.c`]:

- The `--animate` context lasts until the end of the current client message (one `sketchybar …` invocation). Every animatable `--set` or `--bar` property after it in that message is animated from its current value to the target.
- Setting the **same** property twice in one animated message chains the animations: the second starts from the first one's final value after it finishes. This is the documented "bounce" idiom, `--animate sin 10 --set x y_offset=10 y_offset=0`.
- A later animated message for a property that is still animating cancels the old animation and starts again from the current interpolated value.
- A **non-animated** `--set` of an animating property cancels the animation: the property jumps to the old target and then to the new value.
- Colors are interpolated per byte (ARGB channels separately).
- `--query` during an animation reports the current interpolated value. After `duration/60` s it reports the target.

### 1.4 `README.md` claims [FILE]

The README makes these behavioural claims:

- Every element can be added, removed or changed at runtime.
- Animation system.
- Event-driven scripting.
- Mouse support.
- Aliases for menu bar apps.
- Graphs.
- On-demand popups.

Related projects named: SbarLua (Lua API), sketchybar-app-font, SketchyBarHelper (a C header for talking over mach directly, related to the `mach_helper` property). Default config location: `~/.config/sketchybar/`.

---

## 2. Documented behaviour not exercised by the stock files

Everything in this section is **[DOCS-MEM]** unless it is tagged otherwise. [SRC] marks a point that was cross-checked against the C source in the checkout; where they differ, follow [SRC]. Page names follow the site navigation.

### 2.1 Setup

- Install with `brew tap FelixKratz/formulae && brew install sketchybar`, then run it as a service with `brew services start sketchybar`.
- Start from the example config:
  ```sh
  mkdir -p ~/.config/sketchybar/plugins
  cp $(brew --prefix)/share/sketchybar/examples/sketchybarrc ~/.config/sketchybar/sketchybarrc
  cp -r $(brew --prefix)/share/sketchybar/examples/plugins/ ~/.config/sketchybar/plugins/
  ```
- The stock config needs a Nerd Font: `brew install --cask font-hack-nerd-font`.
- To hide the native menu bar, enable System Settings → Control Center/Desktop & Dock → "Automatically hide and show the menu bar" → Always.
- Plugin scripts must be executable (`chmod +x`). The rc is made executable automatically [SRC `ensure_executable_permission`].
- For logs, run `sketchybar` in a terminal instead of as a service. Errors from commands are printed to the calling client's stdout; examples are `[!] Set: Item not found '<n>'` and `[!] Item (<n>): Invalid property '<p>'` [SRC].
- Use `brew services restart sketchybar` or `sketchybar --reload` after changing the config.

### 2.2 Bar (`--bar`)

| Property | Values | Notes |
|---|---|---|
| `color`, `border_color` | `0xAARRGGBB` | |
| `position` | `top`, `bottom` | |
| `height` | int | Animatable. |
| `notch_display_height` | int | Bar height used on notched displays [SRC]. |
| `margin` | int | Horizontal inset of the bar from the screen edges. Animatable. |
| `y_offset` | int | Animatable. |
| `corner_radius`, `border_width` | int | |
| `blur_radius` | int | Animatable. |
| `padding_left`, `padding_right` | int | Space between the screen edge and the first item. |
| `notch_width` | int | Default 200. |
| `notch_offset` | int | |
| `display` | `main`, `all`, `<n>[,<n>…]` | `n` is the 1-based arrangement index [SRC]. |
| `hidden` | bool, `current` | `current` toggles the bar on the active display [SRC]. |
| `topmost` | bool, `window` | `window` puts the bar above windows but below popups and menus [SRC]. |
| `sticky` | bool | Stays on screen during space switches. |
| `font_smoothing` | bool | |
| `shadow` | bool | |
| `show_in_fullscreen` | bool | |

- Other `background.*`-style keys, such as `image` and `image.*`, are accepted on `--bar` directly. `--bar` falls back to the background parser, except for `clip`, which is rejected with `[!] Bar: Invalid property 'clip'` [SRC].

### 2.3 Items (`--add item`, `--set`)

**Geometry**

| Property | Notes |
|---|---|
| `drawing` | |
| `position` | Also settable after `--add`. |
| `space` / `associated_space` | Comma list. |
| `display` / `associated_display` | Comma list, or `active` [SRC]. |
| `ignore_association` | |
| `y_offset` | |
| `padding_left`, `padding_right` | |
| `width` | int or `dynamic`. |
| `scroll_texts` | Marquee for text longer than `max_chars`. |
| `blur_radius` | |
| `shadow` | |
| `align` | |
| `background.*` | |

**Text** (`icon.*`, `label.*`; `icon=<s>` and `label=<s>` set `.string`)

| Property | Notes |
|---|---|
| `drawing`, `highlight`, `color`, `highlight_color` | |
| `padding_left`, `padding_right`, `y_offset` | |
| `font="Family:Style:Size"` | Sub-properties `font.family`, `font.style`, `font.size` (animatable), and `font.features`, `font.typographical_width` [SRC]. |
| `width` | int or `dynamic`. |
| `scroll_duration`, `max_chars` | |
| `align` | `left`, `center`, `right`. |
| `background.*`, `shadow.*` | |
| `string` | |

**Scripting**

| Property | Notes |
|---|---|
| `script`, `click_script` | |
| `update_freq` | Seconds. `0` means events only. |
| `updates` | `on`, `off`, `when_shown`. |
| `mach_helper` | Mach port name. Events are sent to the helper as serialised env vars instead of fork/exec [SRC]. |

**Background** (`background.*`)

| Property | Notes |
|---|---|
| `drawing`, `color`, `border_color`, `border_width`, `height`, `corner_radius` | |
| `padding_left`, `padding_right`, `x_offset`, `y_offset` | |
| `clip` | 0–1. Cuts a transparent hole through the bar [SRC: `clip>0` enables drawing]. |
| `image`, `image.*` | |
| `shadow.*` | |

**Image** (`background.image`, `icon.background.image`, …)

| Property / value | Notes |
|---|---|
| `drawing`, `scale` | |
| `border_color`, `border_width`, `corner_radius` | |
| `padding_left`, `padding_right`, `y_offset` | |
| `shadow.*` | |
| `string` (the bare value), path to a png/jpg | `~` is expanded. |
| `app.<bundle-id>` | Application icon [SRC]. |
| `media.artwork` | Current now-playing artwork, linked live [SRC]. |
| `space.<id>` | Space capture [SRC]. |

**Shadow** (`shadow.*`): `drawing`, `color`, `angle`, `distance`.

**Colours**: any colour property also takes sub-properties `.hex`, `.alpha` (float 0–1), `.red`, `.green`, `.blue` [SRC `color.c`], for example `icon.color.alpha=0.5`.

**Defaults**: `--default` applies to items created afterwards. `--default reset` restores the built-in defaults [SRC].

**Item operations**:

- `--reorder a b c`
- `--move a before|after b`
- `--clone parent new [before|after]` [DOCS-MEM: the documented way to make templates]
- `--rename`
- `--remove` (supports `/regex/`)

### 2.4 Components

- **graph**
  - Created with `--add graph <name> <pos> <width>`.
  - Properties: `graph.color`, `graph.fill_color`, `graph.line_width`.
  - Data: `--push <name> <v1> [v2 …]` with values in 0–1. The newest value is drawn at the right edge.
  - Query: `.graph{color, fill_color, line_width (string), data[ "%f" strings ]}` [SRC].
- **space**
  - Created with `--add space <name> <pos>`, associated via `space=<mission-control index>` and `display=<n>`.
  - Script env: `SELECTED`, `SID`, `DID`.
  - Default script: `icon.highlight=$SELECTED` [SRC].
  - Only works with native Mission Control spaces; aerospace users use plain items (section 3.1).
- **bracket**
  - Created with `--add bracket <name> <member|/regex/>…`.
  - Draws one background behind the span of its members. It has its own `background.*`, and member items keep their own.
  - Query: `.bracket[ member names ]` [SRC].
  - A bracket whose first member is a popup item lives inside that popup [SRC].
- **alias**
  - Created with `--add alias "<Owner>[,<Name>]" <pos>`. Names come from `sketchybar --query default_menu_items`.
  - Properties: `alias.color`, `alias.scale`, `alias.update_freq`, `alias.shadow.*` [SRC].
  - Needs the Screen Recording permission.
- **slider**
  - Created with `--add slider <name> <pos> <width>`.
  - Properties: `slider.percentage`, `slider.highlight_color`, `slider.width`, `slider.background.*`, `slider.knob=<string>` and `slider.knob.*`.
  - Subscribe to `mouse.clicked` to receive `PERCENTAGE` [SRC `bar_item_cancel_drag`].
  - Query: `.slider{highlight_color, percentage (string), width (string), background{…}, knob{…}}` [SRC].
- **event providers / helpers**: the docs and FelixKratz's dotfiles show compiled helpers (`cpu_load`, `network_load`) that call `--trigger <custom event> KEY=VAL` on a timer. This replaces the `update_freq` polling scripts.

### 2.5 Popups

- Add an item to a popup with `--add item <name> popup.<host>`. A host that does not exist gives `[!] Add (Popup) <n>: Item '<h>' is not a valid popup host` [SRC].
- Host properties:

  | Property | Notes |
  |---|---|
  | `popup.drawing` | bool or `toggle`. |
  | `popup.horizontal` | |
  | `popup.topmost` | |
  | `popup.height` | Cell height. `-1` in the query when unset [SRC]. |
  | `popup.blur_radius` | |
  | `popup.y_offset` | |
  | `popup.align` | `left`, `right`, `center`. |
  | `popup.background.*` | |

- The `popup` key appears in the host's query only once it has at least one item. Its fields are `{drawing, horizontal, height, blur_radius, y_offset, align, background{…}, items[…]}` [SRC].
- Popup items report `.geometry.position=="popup"` [SRC].

### 2.6 Events & scripting

Variables available to every script:

| Variable | Content |
|---|---|
| `NAME` | Item name. |
| `SENDER` | Event name, `forced` or `routine`. |
| `CONFIG_DIR` | |
| `BAR_NAME` | |
| `INFO` | Event-specific payload. |

Event payloads [SRC where noted]:

| Event | INFO / extra vars |
|---|---|
| `front_app_switched` | App name. |
| `space_change` | `{"display-<n>": <sid>, …}` [SRC]. |
| `space_windows_change` | `{"space": <n>, "apps": {"<App>": <count>, …}}` [SRC `app_windows.c`]. |
| `display_change` | New active display arrangement id [SRC]. |
| `volume_change` | 0–100 [SRC]. |
| `brightness_change` | 0–100 [SRC]. |
| `wifi_change` | SSID [SRC]. |
| `media_change` | `{"state": "playing"\|"paused", "title", "album", "artist", "app"}` [SRC `media.m` keys]. |
| `power_source_change` | `AC` \| `BATTERY` [SRC]. |
| `system_will_sleep`, `system_woke` | None. On wake a second `system_woke` follows about 0.5 s later [SRC]. |
| `mouse.entered`, `mouse.exited` | Only for the item under the cursor. A global exit also fires `mouse.exited` on every subscribed item [SRC]. |
| `mouse.entered.global`, `mouse.exited.global` | Fire when the cursor enters or leaves the bar. |
| `mouse.scrolled.global` | Scroll anywhere on the bar. |
| `mouse.clicked` | `BUTTON` = `left`\|`right`\|`other`; `MODIFIER` = comma-joined `shift,ctrl,alt,cmd,fn` or `none`; `INFO={"button","button_code","modifier","modfier_code"}`. The key is misspelled `modfier_code` in the source and must be kept [SRC]. |
| `mouse.scrolled` | `SCROLL_DELTA`, `MODIFIER`, `INFO={"delta","modifier","modfier_code"}` [SRC]. |
| custom (`--add event <name> [<NSDistributedNotification>]`) | With a notification name, the event fires on that distributed notification and `INFO` carries its userInfo. Docs examples include `com.apple.screenIsLocked` and `com.spotify.client.PlaybackStateChanged`. |

- **`click_script` vs `mouse.clicked`.** A `click_script` runs with `INFO`, `BUTTON`, `MODIFIER` and the item's own variables such as `NAME`, but **without** `SENDER` [SRC: the click env does not set `SENDER`]. If the item is also subscribed to `mouse.clicked`, its `script` runs as well, with `SENDER=mouse.clicked` [SRC].
- **Custom-event subscription.** `--subscribe` to an event that was not added first fails with `[?] Event: '<e>' not found`. Order matters: run `--add event` before `--subscribe`.

### 2.7 Querying

- `--query bar`, `--query <item>`, `--query defaults`, `--query events`, `--query default_menu_items` (alias names, a JSON array of `"Owner,Name(<1-based index>)"`; the `(n)` suffix is printed by the source [SRC `alias.c`], while the docs show bare `"Owner,Name"` for use with `--add alias`), and `--query displays` [SRC].
- The docs recommend `jq` for parsing, for example `sketchybar --query bar | jq -r '.items[]'`.

### 2.8 Animations

- Syntax: `sketchybar --animate <curve> <duration> --set … --bar …`.
- Curves: `linear`, `quadratic`, `tanh`, `sin`, `exp`, `circ`.
- `<duration>` is a frame count at 60 Hz [SRC].
- Every numeric and colour property with a transition is animatable:
  - offsets, paddings, widths, heights, font sizes,
  - colours (per channel),
  - bar `height`, `margin`, `y_offset`, `blur_radius`, `notch_*`.
- Repeating a property inside one animated invocation queues the steps (see 1.3).
- Docs example: `sketchybar --animate tanh 25 --set label label.y_offset=5 label.y_offset=0`.

### 2.9 Tips & Tricks

- **Batch commands.** Put as many `--add`, `--set` and `--subscribe` commands as possible into one `sketchybar` invocation. Each invocation is one mach round-trip and one redraw.
- **Use events instead of polling.** Prefer events to `update_freq`, and `updates=when_shown` for hidden items.
- **Multiple bars.** Copy or symlink the binary under another name, for example `bottom_bar`. Each name has its own config dir `~/.config/<name>/` and its own mach port. Scripts should call `$BAR_NAME` instead of a hard-coded `sketchybar` [SRC: name = `basename(argv[0])`].
- **Color picker.** The docs page hosts an ARGB colour picker. Colours are always `0xAARRGGBB`.
- **Item templates.** Combine `--default` and `--clone`.
- **Regex `--set`.** `--set '/space\..*/' icon.color=0xffff0000` changes every matching item.
- **Lua.** SbarLua offers the same API in-process.

### 2.10 FAQ

- **Icons show as boxes.** The font is missing or its name is mistyped. The font string must match the installed family and style, e.g. `"Hack Nerd Font:Bold:17.0"`.
- **The bar is hidden behind the notch or the menu bar.** Auto-hide the native menu bar and use `notch_width` or `display=`.
- **A script does not run.** It is not executable, its path is wrong (use `$CONFIG_DIR`), or it is not subscribed to the event.
- **Items do not update on a space switch with yabai or aerospace.** Use custom events triggered from the window manager (section 3).
- **"could not acquire lock-file… already running?".** Only one instance per bar name may run [SRC].
- **Running as root is refused** [SRC].

---

## 3. Community config patterns as executable tests [COMMUNITY]

Each test assumes the section-0 preamble and a fresh headless daemon with no items. The tests only rely on behaviour tagged [SRC] above, plus these headless-platform hooks where noted:

- a fake space/display provider,
- a fake volume provider,
- a frame clock for animations.

Glyphs are written through `u <hex>` so the files stay ASCII-safe.

### 3.1 AeroSpace workspaces via a custom event and `--trigger`

This is the pattern from the AeroSpace "SketchyBar integration" guide.

`aerospace.toml`, not executed by the test:

```toml
exec-on-workspace-change = ['/bin/bash', '-c',
  'sketchybar --trigger aerospace_workspace_change FOCUSED_WORKSPACE=$AEROSPACE_FOCUSED_WORKSPACE PREV_WORKSPACE=$AEROSPACE_PREV_WORKSPACE']
```

```sh
stub aerospace <<'STUB'
#!/bin/sh
case "$1" in
  list-workspaces) printf '1\n2\n3\nB\n' ;;
  workspace) echo "$2" >> "$T/aerospace.log" ;;
esac
STUB
mkscript aerospace.sh <<'PLUGIN'
#!/bin/sh
# $1 = workspace id baked into the script string; FOCUSED_WORKSPACE comes from --trigger
if [ "$1" = "$FOCUSED_WORKSPACE" ]; then
  sketchybar --set "$NAME" background.drawing=on label.color=0xff000000
else
  sketchybar --set "$NAME" background.drawing=off label.color=0xffffffff
fi
PLUGIN

"$SB" --add event aerospace_workspace_change
for sid in $(aerospace list-workspaces --all); do
  "$SB" --add item "space.$sid" left \
        --subscribe "space.$sid" aerospace_workspace_change \
        --set "space.$sid" background.color=0x44ffffff background.corner_radius=5 background.height=20 \
              background.drawing=off label="$sid" icon.drawing=off \
              click_script="aerospace workspace $sid" \
              script="$T/aerospace.sh $sid"
done

# event registry
eq events '.aerospace_workspace_change.bit' 262144          # first custom event = 1<<18
eq space.1 .scripting.update_mask 262144
eq space.B .scripting.script "$T/aerospace.sh B"            # args preserved verbatim
eq space.1 .geometry.background.drawing off                 # explicit drawing=off after color

"$SB" --trigger aerospace_workspace_change FOCUSED_WORKSPACE=2 PREV_WORKSPACE=1
wait_eq space.2 .geometry.background.drawing on
wait_eq space.1 .geometry.background.drawing off
wait_eq space.2 .label.color 0xff000000

"$SB" --trigger aerospace_workspace_change FOCUSED_WORKSPACE=B
wait_eq space.B .geometry.background.drawing on
wait_eq space.2 .geometry.background.drawing off

# click_script is stored verbatim; execute it as the daemon would (sh -c, env NAME)
NAME=space.3 sh -c "$(q space.3 .scripting.click_script)"; grep -qx 3 "$T/aerospace.log"

# initial state on load: community configs call this once at the end of sketchybarrc
"$SB" --trigger aerospace_workspace_change FOCUSED_WORKSPACE=1
wait_eq space.1 .geometry.background.drawing on
```

Variants seen in the wild, which need no extra daemon features:

- A single `aerospace.sh` without `$1` that calls `aerospace list-workspaces --focused` itself.
- Per-monitor workspaces via `display=<n>` on each item.
- Hiding empty workspaces via `drawing=off` when `aerospace list-windows --workspace $1` is empty.

### 3.2 yabai integration (signals → custom events)

```sh
# yabai side (not executed):
#   yabai -m signal --add event=window_focused   action="sketchybar --trigger window_focus"
#   yabai -m signal --add event=window_title_changed action="sketchybar --trigger title_change"
#   yabai -m signal --add event=space_changed    action="sketchybar --trigger yabai_space YABAI_SPACE=\$YABAI_SPACE_INDEX"
stub yabai <<'STUB'
#!/bin/sh
# yabai -m query --windows --window  -> JSON for the focused window
[ "$3" = "--windows" ] && printf '{"app":"Safari","title":"Docs","is-floating":false,"stack-index":0}\n'
STUB
mkscript yabai_window.sh <<'PLUGIN'
#!/bin/sh
W=$(yabai -m query --windows --window)
case "$SENDER" in
  window_focus|title_change)
    TITLE=$(printf '%s' "$W" | jq -r .title)
    FLOAT=$(printf '%s' "$W" | jq -r '."is-floating"')
    [ "$FLOAT" = true ] && ICON=float || ICON=tiled     # real configs put Nerd Font glyphs here
    sketchybar --set "$NAME" label="$TITLE" icon="$ICON" ;;
esac
PLUGIN
"$SB" --add event window_focus --add event title_change --add event yabai_space \
      --add item window left --set window script="$T/yabai_window.sh" \
      --subscribe window window_focus title_change
eq events '.window_focus.bit' 262144; eq events '.title_change.bit' 524288; eq events '.yabai_space.bit' 1048576
eq window .scripting.update_mask $((262144|524288))
"$SB" --trigger title_change
wait_eq window .label.value Docs; eq window .icon.value tiled

# Native-space items + yabai focus (stock-rc style): space_change is provider-driven, not --trigger env
"$SB" --add space s1 left --set s1 space=1 click_script="yabai -m space --focus 1"
eq s1 .scripting.script 'sketchybar -m --set $NAME icon.highlight=$SELECTED'   # default space script
eq s1 .icon.highlight off
# headless: set fake active space = 1 on display 1, then
"$SB" --trigger space_change
wait_eq s1 .icon.highlight on
```

Plugins run under `sh -c`, so they cannot use harness shell functions such as `u`. Glyphs must be literal in the plugin file.

### 3.3 sketchybar-app-font: icon mapping

The pattern:

- Install the font with `brew install --cask font-sketchybar-app-font`, or download `sketchybar-app-font.ttf` and `icon_map.sh`.
- `icon_map.sh` defines `__icon_map()`, which sets `icon_result=":<app_slug>:"`.
- The font renders those ligatures as app logos.

```sh
mkscript icon_map.sh <<'MAP'
#!/bin/bash
# excerpt in the shape of sketchybar-app-font's generated icon_map.sh
function __icon_map() {
  case "$1" in
    "Safari") icon_result=":safari:" ;;
    "Code" | "Visual Studio Code") icon_result=":code:" ;;
    "Ghostty") icon_result=":ghostty:" ;;
    "Finder") icon_result=":finder:" ;;
    *) icon_result=":default:" ;;
  esac
}
__icon_map "$1"; echo "$icon_result"
MAP

# (a) front app icon
mkscript front_app_icon.sh <<PLUGIN
#!/bin/sh
[ "\$SENDER" = front_app_switched ] && sketchybar --set "\$NAME" label="\$INFO" icon="\$($T/icon_map.sh "\$INFO")"
PLUGIN
"$SB" --add item front_app left \
      --set front_app icon.font="sketchybar-app-font:Regular:16.0" script="$T/front_app_icon.sh" \
      --subscribe front_app front_app_switched
eq front_app .icon.font "sketchybar-app-font:Regular:16.00"
"$SB" --trigger front_app_switched INFO="Visual Studio Code"
wait_eq front_app .icon.value ":code:"
eq front_app .label.value "Visual Studio Code"
"$SB" --trigger front_app_switched INFO="Unknown App"
wait_eq front_app .icon.value ":default:"

# (b) per-space app strip from space_windows_change (INFO JSON) -- script contract, run directly
mkscript space_windows.sh <<PLUGIN
#!/bin/bash
[ "\$SENDER" = space_windows_change ] || exit 0
space=\$(echo "\$INFO" | jq -r .space)
apps=\$(echo "\$INFO" | jq -r '.apps | keys[]')
strip=""
if [ -n "\$apps" ]; then
  while read -r app; do strip+=" \$($T/icon_map.sh "\$app")"; done <<< "\$apps"
else strip=" —"; fi
sketchybar --set space.\$space label="\$strip"
PLUGIN
"$SB" --add item space.2 left --set space.2 label.font="sketchybar-app-font:Regular:16.0" \
      --add item space_windows left --set space_windows drawing=off script="$T/space_windows.sh" \
      --subscribe space_windows space_windows_change
NAME=space_windows SENDER=space_windows_change INFO='{"space": 2, "apps": {"Finder": 1, "Safari": 2}}' "$T/space_windows.sh"
eq space.2 .label.value " :finder: :safari:"      # jq keys[] sorts keys
# event path (needs headless window provider): provider reports space 2 with Ghostty -> --trigger space_windows_change
```

### 3.4 Nerd Fonts and font sub-properties

```sh
"$SB" --add item nf right --set nf icon="$(u f240)" icon.font="Hack Nerd Font:Bold:17.0" label="x"
eq nf .icon.value "$(u f240)"
eq nf .icon.font "Hack Nerd Font:Bold:17.00"
"$SB" --set nf icon.font.size=20;            eq nf .icon.font "Hack Nerd Font:Bold:20.00"
"$SB" --set nf icon.font.style=Regular;      eq nf .icon.font "Hack Nerd Font:Regular:20.00"
"$SB" --set nf icon.font.family="JetBrainsMono Nerd Font"; eq nf .icon.font "JetBrainsMono Nerd Font:Regular:20.00"
# 5-hex-digit Material Design codepoints (plane 15 PUA) must survive the round-trip unchanged
"$SB" --set nf icon="$(u f057e)";            eq nf .icon.value "$(u f057e)"
# SF Symbols / SF Pro is the other common choice; the query echoes the requested family even if unresolved
"$SB" --set nf label.font="SF Pro:Semibold:15.0"; eq nf .label.font "SF Pro:Semibold:15.00"
```

The query echoes the font as it was set, not the font that was resolved [SRC `text_serialize` prints `family:style:size`]. A headless daemon must therefore keep the requested strings even when the font is missing.

### 3.5 Popup menu with a click toggle

```sh
"$SB" --add item apple left \
      --set apple icon="$(u f179)" label.drawing=off \
            click_script='sketchybar --set $NAME popup.drawing=toggle' \
            popup.background.color=0xff1e1e2e popup.background.corner_radius=8 popup.background.border_width=2 \
            popup.align=left popup.height=30 \
      --add item apple.prefs popup.apple --set apple.prefs icon="$(u f013)" label="Preferences" \
            click_script='open -a "System Settings"; sketchybar --set apple popup.drawing=off' \
      --add item apple.lock  popup.apple --set apple.lock  icon="$(u f023)" label="Lock Screen" \
            click_script='pmset displaysleepnow; sketchybar --set apple popup.drawing=off'

eq apple.prefs .geometry.position popup
eq apple '.popup.items|join(",")' "apple.prefs,apple.lock"
eq apple .popup.drawing off
eq apple .popup.align left
eq apple .popup.height 30
eq apple .popup.background.color 0xff1e1e2e
eq apple .scripting.click_script 'sketchybar --set $NAME popup.drawing=toggle'   # single quotes: $NAME kept literal

click() { NAME="$1" BUTTON=left MODIFIER=none sh -c "$(q "$1" .scripting.click_script)"; }   # emulate daemon click
click apple; wait_eq apple .popup.drawing on
click apple; wait_eq apple .popup.drawing off
"$SB" --set apple popup.drawing=on
stub open <<'STUB'
#!/bin/sh
:
STUB
click apple.prefs; wait_eq apple .popup.drawing off

# auto-close when the cursor leaves the bar (mouse.exited.global goes through the generic trigger path)
mkscript popup_close.sh <<'PLUGIN'
#!/bin/sh
case "$SENDER" in
  mouse.exited.global) sketchybar --set "$NAME" popup.drawing=off ;;
  mouse.clicked)       sketchybar --set "$NAME" popup.drawing=toggle ;;
esac
PLUGIN
"$SB" --set apple click_script="" script="$T/popup_close.sh" --subscribe apple mouse.clicked mouse.exited.global
eq apple .scripting.update_mask $((64|1024))
"$SB" --trigger mouse.clicked;        wait_eq apple .popup.drawing on
"$SB" --trigger mouse.exited.global;  wait_eq apple .popup.drawing off

# a popup host with no items has no "popup" key at all
"$SB" --add item lonely left --set lonely popup.drawing=on
[ "$(q lonely 'has("popup")')" = false ]
# invalid host
"$SB" --add item orphan popup.nope | grep -q "is not a valid popup host"
```

### 3.6 Hover effects via `mouse.entered` / `mouse.exited`

```sh
mkscript hover.sh <<'PLUGIN'
#!/bin/sh
case "$SENDER" in
  mouse.entered) sketchybar --animate tanh 10 --set "$NAME" background.color=0x40ffffff label.color=0xff000000 icon.y_offset=2 ;;
  mouse.exited)  sketchybar --animate tanh 10 --set "$NAME" background.color=0x00000000 label.color=0xffffffff icon.y_offset=0 ;;
esac
PLUGIN
"$SB" --add item h1 right --set h1 label=hover background.corner_radius=6 background.height=24 script="$T/hover.sh" \
      --subscribe h1 mouse.entered mouse.exited \
      --add item h2 right --set h2 label=other script="$T/hover.sh" --subscribe h2 mouse.entered mouse.exited
eq h1 .scripting.update_mask 48

# --trigger broadcasts to ALL subscribers (real pointer events only target the hovered item)
"$SB" --trigger mouse.entered
wait_eq h1 .geometry.background.color 0x40ffffff
wait_eq h2 .geometry.background.color 0x40ffffff
eq h1 .geometry.background.drawing on       # setting a colour enabled drawing
wait_eq h1 .icon.y_offset 2
"$SB" --trigger mouse.exited
wait_eq h1 .geometry.background.color 0x0   # serialised without padding
wait_eq h1 .label.color 0xffffffff
eq h1 .geometry.background.drawing on       # colour 0 does not switch drawing off

# headless pointer hook (if provided): move pointer into h1 bounds => only h1 gets mouse.entered; repeated
# entered without exit must not re-fire (bar_item_mouse_entered checks mouse_over) [SRC]
```

Also seen in the wild: hover on bracket members, which subscribes every member and animates the bracket's `background.border_color`. Another variant uses `mouse.entered.global` / `mouse.exited.global` to fade the whole bar in and out:

```sh
"$SB" --add item fader left --set fader drawing=off script='case "$SENDER" in
  mouse.entered.global) sketchybar --animate sin 15 --bar color=0xff1e1e2e ;;
  mouse.exited.global)  sketchybar --animate sin 15 --bar color=0x40000000 ;; esac' \
  --subscribe fader mouse.entered.global mouse.exited.global
"$SB" --trigger mouse.entered.global; wait_eq bar .color 0xff1e1e2e
"$SB" --trigger mouse.exited.global;  wait_eq bar .color 0x40000000
```

The inline multi-line `script=` works because the script is passed to `sh -c` verbatim [SRC]. An item with `drawing=off` still runs its scripts, because `updates` defaults to `on`, not `when_shown`.

### 3.7 Animations with `--animate`

These tests need the headless frame clock to advance in real time at 60 Hz, or a test hook that advances frames.

```sh
"$SB" --add item a left --set a label=anim y_offset=0 background.color=0xff000000

# final value after the duration (30 frames = 0.5 s)
"$SB" --animate linear 30 --set a y_offset=20
v=$(q a .geometry.y_offset); [ "$v" -ge 0 ] && [ "$v" -le 20 ]      # immediately: start or early frame
sleep 0.25; v=$(q a .geometry.y_offset); [ "$v" -gt 0 ] && [ "$v" -lt 20 ]   # mid-flight value is interpolated
wait_eq a .geometry.y_offset 20

# chaining inside one invocation: bounce 0 -> 10 -> 0
"$SB" --set a y_offset=0
"$SB" --animate sin 15 --set a y_offset=10 y_offset=0
sleep 0.2;  v=$(q a .geometry.y_offset); [ "$v" -gt 0 ]                       # first leg running
wait_eq a .geometry.y_offset 0

# non-animated set cancels a running animation and wins immediately
"$SB" --animate linear 120 --set a y_offset=50
"$SB" --set a y_offset=5;  eq a .geometry.y_offset 5; sleep 0.3; eq a .geometry.y_offset 5

# colour animation interpolates per channel; endpoint exact
"$SB" --animate quadratic 20 --set a background.color=0xffff0000
wait_eq a .geometry.background.color 0xffff0000

# bar properties animate too
"$SB" --animate tanh 20 --bar height=50 y_offset=4 margin=10
wait_eq bar .height 50; eq bar .y_offset 4; eq bar .margin 10

# curve is matched on first letter only (l,q,t,s,e,c); anything else -> linear (must not error)
"$SB" --animate zzz 10 --set a y_offset=1; wait_eq a .geometry.y_offset 1

# --animate scope is a single invocation: the next call is instant
"$SB" --animate linear 600 --set a padding_left=30
"$SB" --set a padding_right=7; eq a .geometry.padding_right 7       # not animated
"$SB" --set a padding_left=30                                        # cancel long animation (jumps to target)
eq a .geometry.padding_left 30

# font size animation (ANIMATE_FLOAT) ends exactly on target
"$SB" --set a label.font="Hack Nerd Font:Bold:14.0"
"$SB" --animate linear 10 --set a label.font.size=18; wait_eq a .label.font "Hack Nerd Font:Bold:18.00"
```

### 3.8 Brackets with regex members (status "pills")

```sh
"$SB" --add item status.cpu right --add item status.mem right --add item disk right \
      --add bracket status '/status\..*/' disk \
      --set status background.color=0xff313244 background.corner_radius=9 background.height=26
eq status .type bracket
eq status '.bracket|join(",")' "status.cpu,status.mem,disk"   # regex matches in item order, then explicit members
# regex is POSIX *basic* (regcomp flags 0) and unanchored [SRC message.c get_bar_items_for_regex]:
# '/cpu|mem/' matches only a literal "cpu|mem"; '/mem/' also matches "status.mem".
"$SB" --add bracket empty '/nomatch/' | true             # no members -> bracket removed
q bar '.items|index("empty")' | grep -qx null
```

### 3.9 Graph fed by `--push` (CPU graph pattern)

```sh
"$SB" --add graph cpu.graph right 50 \
      --set cpu.graph graph.color=0xffa6e3a1 graph.fill_color=0x40a6e3a1 graph.line_width=1.5 label="cpu"
eq cpu.graph .type graph; eq cpu.graph .graph.color 0xffa6e3a1
"$SB" --push cpu.graph 0.25 0.5 0.75
q cpu.graph '.graph.data|length' | grep -qx 50                 # data = raw ring buffer of <width> slots [SRC graph.c]
q cpu.graph '.graph.data|map(tonumber)|.[0:3]|map(.*100|round)|join(",")' | grep -qx "25,50,75"
# slots are in buffer order, not chronological: after width+1 pushes slot 0 holds the newest value
```

### 3.10 Slider (volume slider pattern)

```sh
mkscript vslider.sh <<'PLUGIN'
#!/bin/sh
case "$SENDER" in
  volume_change) sketchybar --set "$NAME" slider.percentage="$INFO" ;;
  mouse.clicked) echo "$PERCENTAGE" > "$T/set_volume" ;;   # real config: osascript -e "set volume output volume $PERCENTAGE"
esac
PLUGIN
"$SB" --add slider vol right 100 \
      --set vol slider.highlight_color=0xff89b4fa slider.background.height=5 slider.background.corner_radius=3 \
                slider.background.color=0xff45475a slider.knob="$(u f111)" script="$T/vslider.sh" \
      --subscribe vol volume_change mouse.clicked
eq vol .type slider; eq vol .slider.width 100; eq vol .slider.percentage 0
NAME=vol SENDER=volume_change INFO=42 "$T/vslider.sh"; eq vol .slider.percentage 42
# headless click inside the slider track at 75% => mouse.clicked with PERCENTAGE=75 (pointer hook)
```

### 3.11 Template items via `--default` and `--clone`

```sh
"$SB" --default icon.color=0xffcdd6f4 label.drawing=off
"$SB" --add item tpl left --set tpl drawing=off icon=T background.color=0xff11111b script="$T/hover.sh"
"$SB" --clone tpl c1 --clone tpl c2 after
eq c1 .icon.value T; eq c1 .geometry.background.color 0xff11111b; eq c1 .scripting.script "$T/hover.sh"
"$SB" --set '/c[12]/' drawing=on icon.color=0xfff38ba8
eq c1 .geometry.drawing on; eq c2 .icon.color 0xfff38ba8; eq tpl .geometry.drawing off
"$SB" --default reset; "$SB" --add item fresh left; eq fresh .label.drawing on
"$SB" --rename c2 c3; eq c3 .name c3
"$SB" --remove '/c[13]/'; q bar '.items|map(select(test("^c[0-9]$")))|length' | grep -qx 0
```

### 3.12 Distributed-notification custom events

```sh
"$SB" --add event lock com.apple.screenIsLocked --add event unlock com.apple.screenIsUnlocked
eq events '.lock.notification' com.apple.screenIsLocked
eq events '.front_app_switched.notification' '(null)'   # built-ins have no notification; printf("%s", NULL) on macOS
mkscript lock.sh <<'PLUGIN'
#!/bin/sh
[ "$SENDER" = lock ] && sketchybar --bar hidden=on
[ "$SENDER" = unlock ] && sketchybar --bar hidden=off
PLUGIN
"$SB" --add item locker left --set locker drawing=off script="$T/lock.sh" --subscribe locker lock unlock
"$SB" --trigger lock;   wait_eq bar .hidden on
"$SB" --trigger unlock; wait_eq bar .hidden off
# headless: posting the distributed notification "com.apple.screenIsLocked" must have the same effect,
# with INFO = notification userInfo (if any)
```

### 3.13 Error responses (exact strings) [SRC]

```sh
"$SB" --set nope label=x            | grep -qF "[!] Set: Item not found 'nope'"
"$SB" --add item a left             | grep -qF "[?] Add: Item 'a' already exists"
"$SB" --add item z middle           | grep -qF "[!] Add z: Illegal position 'middle'"
"$SB" --subscribe a no_such_event   | grep -qF "[?] Event: 'no_such_event' not found"
"$SB" --subscribe nope front_app_switched | grep -qF "[!] Subscribe: Item not found 'nope'"
"$SB" --query nope                  | grep -qF "[!] Query: Invalid query, or item 'nope' not found"
"$SB" --set a bogus=1               | grep -qF "[!] Item (a): Invalid property 'bogus'"
"$SB" --bar clip=1                  | grep -qF "[!] Bar: Invalid property 'clip'"
"$SB" --add foo f left              | grep -qF "[?] Add f: Invalid type 'foo', assuming 'item'"
```

These snippets were written against the [SRC] behaviour and have not yet been run against a real SketchyBar instance. Exact message text and whitespace should be confirmed against the binary before they are pinned as golden tests.
