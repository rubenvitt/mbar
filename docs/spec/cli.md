# SketchyBar CLI / command language: behavioral spec

Source: SketchyBar v2.24.0 (commit `5f358ec`). File references are relative to `src/`.
This spec covers the binary's command line, the IPC transport, the command grammar,
value parsing, every domain and property, `--query` output, error strings, config
lookup, hotload, and the environment that scripts get. Rendering and layout are out
of scope except where they affect parsing.

Conventions:
- "Response" means text returned to the client over IPC. "Daemon log" means the
  daemon's stdout.
- `<x>` is a token, meaning one argv element. `[x]` is optional.
- Strings in `code` are exact. `\n` and `\t` are the literal characters.
- **Quirk** marks behavior that looks accidental but is observable. mbar must
  either reproduce it or document the difference on purpose.

---

## 1. Binary invocation (`sketchybar.c:main`, `parse_arguments`)

### 1.1 Startup sequence
1. `g_name = basename(argv[0])`. This is the **bar name**. It sets the IPC
   service name, the lock file, and the config directory name. If you run the
   binary through a symlink with a different name, you get an independent bar
   instance.
2. If the uid or euid is 0, the binary writes
   `"<g_name>: running as root is not allowed! abort..\n"` to stderr and exits 1.
   This check also applies in client mode.
3. `setenv("BAR_NAME", g_name, 1)`. All children inherit it.
4. If `argc > 1`, the binary calls `parse_arguments`. This always ends in
   `exit()` unless `--config` succeeds.
5. Daemon mode only:
   - **USER check.** If `$USER` is unset, it writes
     `"<g_name>: 'env USER' not set! abort..\n"` to stderr and exits 1.
   - **Lock file.** The path is `/tmp/<g_name>_<USER>.lock`, opened with
     `O_CREAT|O_WRONLY` and mode `0600`. It takes a `fcntl F_SETLK` write lock
     on the whole file.
     - If the file can't be created: `"<g_name>: could not create lock-file! abort..\n"`, exit 1.
     - If the lock fails: `"<g_name>: could not acquire lock-file... already running?\n"`, exit 1.
   - **Signals.** `SIGCHLD` is set to `SIG_IGN`, so children are auto-reaped
     and nobody waits on scripts. `SIGPIPE` is set to `SIG_IGN`.
   - **Bars and IPC server.** It creates the bars and registers the IPC server.
     On failure: `"<g_name>: could not initialize daemon! abort..\n"`, exit 1.
   - **Config and run loop.** It runs the config file (§10), starts the config
     directory watcher (§11), and enters the run loop.

### 1.2 Argument dispatch
Only `argv[1]` is inspected:

| `argv[1]` | Behavior |
|---|---|
| `-v`, `--version` | stdout `sketchybar-v2.24.0\n`, exit 0. The `sketchybar-v` prefix is hard-coded and does not use the bar name. |
| `-h`, `--help` | `printf(help_str, argv[0])` (full `argv[0]`, not the basename). Exit 0. Text is in §1.3. |
| `-m`, `--message` | Client mode with `argv[2..]` (`client_send_message(argc-1, argv+1)`). |
| `-c`, `--config` | If `argc < 3`: stdout `[!] Error: Too few arguments for argument 'config'.\n`, exit 1. Otherwise `set_config_file_path(argv[2])` resolves the path with `realpath()`. On success the daemon starts with that config file and ignores `argv[3..]`. If `realpath` fails: stdout `[!] Error: Specified config file path invalid.\n`, exit 1. |
| anything else | Client mode with `argv[1..]`. There is no `-m` prefix, so `sketchybar --set ...` works the same as `sketchybar -m --set ...`. |
| (no args) | Daemon mode. |

Client mode with zero remaining args (`sketchybar -m`) exits 0 and sends nothing.

### 1.3 Help text (`misc/help.h`), verbatim (`%s` = argv[0])
```
Usage: %s [options]

Startup: 
  -c, --config CONFIGFILE	Read CONFIGFILE as the configuration file
                         	Default CONFIGFILE is ~/.config/sketchybar/sketchybarrc

Set global bar properties, see https://felixkratz.github.io/SketchyBar/config/bar
      --bar <setting>=<value> ... <setting>=<value>

Items and their properties, see https://felixkratz.github.io/SketchyBar/config/items
      --add item <name> <position>	Add item to bar
      --set <name> <property>=<value> ... <property>=<value>
                                  	Change item properties
      --default <property>=<value> ... <property>=<value>
                                  	Change default properties for new items
      --set <name> popup.<popup_property>=<value>
                                  	Configure item popup menu
                                  	See https://felixkratz.github.io/SketchyBar/config/popups
      --reorder <name> ... <name> 	Reorder items
      --move <name> before <reference name>
      --move <name> after <reference name>
                                  	Move item relative to reference item
      --clone <parent name> <name> [optional: before/after]
                                  	Clone parent to create new item
      --rename <old name> <new name>	Rename item
      --remove <name>             	Remove item

Special components, see https://felixkratz.github.io/SketchyBar/config/components
      --add graph <name> <position> <width in points>
                                  	Add graph component
      --push <name> <data point> ... <data point>
                                  	Push data points to a graph
      --add space <name> <position>	Add space component
      --add bracket <name> <member name> ... <member name>
                                  	Add bracket component
      --add alias <application_name> <position>
                                  	Add alias component
      --add slider <name> <position> <width>
                                  	Add slider component

Events and Scripting, see https://felixkratz.github.io/SketchyBar/config/events
      --subscribe <name> <event> ... <event>
                                  	Subscribe to events
      --add event <name> [optional: <NSDistributedNotificationName>]
                                  	Create custom event
      --trigger <event> [optional: <envvar>=<value> ... <envvar>=<value>]
                                  	Trigger custom event

Querying information, see https://felixkratz.github.io/SketchyBar/config/querying
      --query bar               	Query bar properties
      --query <name>            	Query item properties
      --query defaults          	Query default properties
      --query events            	Query events
      --query default_menu_items	Query names of available items for aliases

Animations, see https://felixkratz.github.io/SketchyBar/config/animations
      --animate <linear|quadratic|tanh|sin|exp|circ> <duration> \
                --bar <property=value> ... <property=value>\
                --set <name> <property=value> ... <property=value>
                         	Animate from given source to target property values

Reloading the config
      --hotload <boolean>        	Enable or disable the config hotloader
      --reload [optional: <path>]	Reload the current or the given config

```
Notes:
- The line `Startup: ` has a trailing space. The help ends with two newlines.
- **Quirk:** the help shows `--clone <parent name> <name>`, but the code reads
  `--clone <new name> <parent name>` (§6.8). Follow the code.
- The help does not list `--query item <name>`, `--query displays`, `--update`,
  `--exit`, or `--load-font`.

---

## 2. IPC transport (`mach.c`, `sketchybar.c:client_send_message`, `message.c:handle_message_mach`)

### 2.1 Endpoint
- macOS Mach bootstrap service name: `git.felix.<g_name>` (`MACH_BS_NAME_FMT`).
  The daemon registers it with `bootstrap_register`. The port has queue limit
  `MACH_PORT_QLIMIT_LARGE`, and the server runs on the main CFRunLoop.
- mbar (Linux) needs an equivalent per-bar-name endpoint, for example a Unix
  socket at `$XDG_RUNTIME_DIR/<g_name>.sock`. The bar name must stay part of the
  address.

### 2.2 Request encoding
- The payload is the argv list with each argument NUL-terminated, followed by
  one extra NUL: `arg1\0arg2\0...argN\0\0`.
- **Quirk:** the declared length is `argc + Σ(len(arg_i)+1)` (with argc counted
  including the program slot). That is N bytes more than the content, and the
  extra bytes are uninitialized. Parsing stops at the double NUL, so this has no
  effect.
- The request is sent with a reply port. The client then waits **100 ms**
  (`MACH_RCV_TIMEOUT 100`) for the response.

### 2.3 Client result handling
| Situation | Client behavior |
|---|---|
| Service lookup fails (daemon not running) | Prints nothing, **exit 0**. `mach_send_message` returns NULL. |
| No reply within 100 ms, or a receive error | Response is `""`. Prints nothing, exit 0. |
| `strlen(rsp) > 2 && rsp[1] == '!'` | Whole response to **stderr**, **exit 1**. |
| otherwise | Whole response to stdout (no newline added), exit 0. |

Only the **first** message in the response decides the exit code. A response
that starts with `[?]` or with JSON exits 0, even if a later line is `[!]`.

### 2.4 Server processing
- Each request runs **atomically** on the main thread as one `MACH_MESSAGE`
  event (`event_post`, then `handle_message_mach`). Requests are serialized.
  Before handling any event, `event_execute` also polls the active display
  (`bar_manager_poll_active_display`), which can fire `display_change`.
- The response buffer is an `open_memstream`. Every `respond()` call and every
  `--query` output is appended in command order.
- After all commands run, the response is sent to the request's reply port as
  `length+1` bytes (NUL-terminated). A response is always sent, possibly empty,
  except after `--exit`.
- `respond(rsp, fmt, ...)` writes `fmt` to the response. It also writes to the
  daemon's stdout prefixed with local time `[YYYY-MM-DD HH:MM:SS] `. JSON
  outputs from `--query` are **not** logged.

### 2.5 Per-request state (`handle_message_mach`)
At the start of every request:
1. Reset the animator: `interp_function = '\0'`, `duration = 0`.
2. Freeze the bar manager (`frozen = true`), which suppresses redraws.

At the end of every request:
1. If any `--bar`, `--update` or `--remove` asked for a refresh: if
   `bar_needs_resize`, resize the bars. Then set `bar_needs_update = true`.
2. `animator_lock`: every animation now in the animator, including ones created
   by this request, becomes *locked*. A later request that animates the same
   property cancels locked ones (§8).
3. Unfreeze, then `bar_manager_refresh(false)`.
4. Send the response.

---
## 3. Command grammar and tokenizer

### 3.1 Primitive: `get_token` (`misc/helpers.h`)
The message is a cursor into `tok\0tok\0...\0\0`. `get_token`:
1. Returns the string at the cursor. Its length is the number of bytes up to the
   next NUL.
2. Moves the cursor past that NUL **only if the next byte is not NUL**.
   Otherwise the cursor stays on the NUL.

So once the list ends, every later `get_token` returns the empty token (length
0). All loops stop on a length-0 token.

**Quirk: an empty argv element (`""`) ends the message.** The payload then
contains a double NUL early, and nothing after it is parsed. For example,
`sketchybar --set a label="" --set b ...` stops parsing after `--set a`. Note
that `label=` (key with an empty value) is one non-empty token and is fine.

### 3.2 Domain dispatch loop (`handle_message_mach`)
```
command = get_token()
while command non-empty:
    dispatch on exact string match of command (table 3.4)
    command = get_token()
```
Several commands in one invocation run in order. Each command uses one of three
argument-consumption modes:

**Mode A: key/value list.** Used by `--set` (after the item name), `--default`
and `--bar`.
```
token = get_token()
while token non-empty:
    process token (must be key=value)
    if the byte at the cursor == '-': break   # next token starts with '-': it is the next command
    token = get_token()
```
The list ends at end of message or at the first **following** token whose first
character is `-`.

**Quirk: the first token after the domain (or after the `--set` item name) is
never checked for `-`.** A Mode A domain with no pairs therefore swallows the
next command token as a malformed pair:
- `--set foo --set bar label=x`:
  1. `--set` is reported as `Expected <key>=<value> pair` for `foo`.
  2. `bar` is not a `-` token, so it is reported the same way.
  3. `label=x` is applied **to `foo`**.
- `--bar --set foo x=1`:
  1. `--bar` reports the `--set` token and breaks.
  2. `foo` becomes the next command: `[!] Unknown domain 'foo'`.
  3. The rest of the batch line is skipped.

A *value* that starts with `-` is fine (`y_offset=-5`) because the token starts
with the key. A token that is a bare `-5` (after the first pair) ends the list
and is then read as a command (`[!] Unknown domain '-5'`).

**Mode B: batch line** (`get_batch_line`). Used by every other domain that takes
arguments.
- It scans from the cursor to the first point where a NUL is followed by either
  another NUL (end of message) or `-` (the next token starts with `-`).
- It copies those tokens into a new double-NUL-terminated buffer, then parses
  that buffer with `get_token`.
- The cursor moves to the `-` token, or stays at the end.

Consequences:
- **Quirk:** no argument of these commands can start with `-`. Example:
  `--push g -0.5` sends `-0.5` to the dispatcher, which responds
  `[!] Unknown domain '-0.5'` and then skips to the next `-` token.
- **Quirk:** the scan starts *at* the cursor. If the first argument itself starts
  with `-`, it is swallowed into the batch line. For example, `--add --set x`
  treats `--set` as the `--add` type.
- Any extra tokens in the batch line beyond what the command reads are ignored
  silently.

**Mode C: fixed tokens.** `--animate` reads exactly 2 tokens with `get_token`.
`--hotload` and `--load-font` read 1. `--update` and `--exit` read 0. These
commands do not check for `-`, so `--animate --set x` consumes `--set` and `x`
as its arguments.

### 3.3 `key=value` token handling (`get_key_value_pair`, `reformat_batch_key_value_pair`, `pack_key_value_pair`)
- The token is split at the **first** `=`. The key is everything before it. The
  value is everything after it and may contain more `=`, spaces, or any byte
  except NUL.
- If the token has no `=`, it is not a pair. Each domain then reports its
  "Expected <key>=<value> pair" error (§5, §6.2, §6.3).
- `key=` (empty value) gives value NULL. It is packed as `key\0\0\0`, so the
  property handler reads an **empty string** value. What an empty value does:
  - integers and floats become 0;
  - booleans become false (`evaluate_boolean_state("")`);
  - strings become empty;
  - list properties become an empty list: bar `display=` gives pattern 0
    (main display only), and item `display=`/`space=` clear the mask (all).
- `=value` gives the empty key `""`. That is then an invalid property.
- The pair is packed as `key\0value\0\0`. The property handler gets the key as
  "property" and a cursor positioned on the value.
- **Value semantics:** a property handler reads only the *first* `get_token` of
  the value, which is the whole value because it was packed as one token. The
  one exception is `font`, which uses the raw value string (same thing).

### 3.4 Domain table
| Command token | Arg mode | Handler | Sets end-of-request refresh flag |
|---|---|---|---|
| `--set` | name, then Mode A | §6.2 | no (per-item `needs_update` only) |
| `--default` | Mode A | §6.3 | no |
| `--bar` | Mode A | §5 | yes if any property reports a change |
| `--animate` | Mode C (2) | §8 | no |
| `--add` | Mode B | §6.1, §7.2 | no |
| `--clone` | Mode B | §6.8 | no |
| `--subscribe` | Mode B | §7.3 | no |
| `--push` | Mode B | §6.10 | no |
| `--update` | Mode C (0) | `bar_manager_update(forced=true)` (§7.6) | always |
| `--trigger` | Mode B | §7.4 | no |
| `--query` | Mode B | §9 | no |
| `--reorder` | Mode B | §6.6 | no |
| `--move` | Mode B | §6.5 | no |
| `--remove` | Mode B | §6.4 | always |
| `--rename` | Mode B | §6.7 | no |
| `--exit` | Mode C (0) | Destroys the bar manager (sends `"k"` to every item's `mach_helper` port), then `exit(0)`. No response is sent, so the client times out with empty output and exit 0. | n/a |
| `--hotload` | Mode C (1) | `g_hotload = evaluate_boolean_state(tok, g_hotload)` (§11). No output. | no |
| `--load-font` | Mode C (1) | Registers a font file for this process: `CTFontManagerRegisterFontsForURL(CFURLCreateWithString(tok))`, process scope. No output, even on failure. Not in the help text. | no |
| `--reload` | Mode B | §11.2 | no |
| anything else | Mode B (skipped) | Response `[!] Unknown domain '<token>'\n`. The rest of its batch line is discarded. | no |

Matching is exact and case-sensitive. There are no short aliases for domains.
`--add component` (`COMMAND_ADD_COMPONENT`) is defined but never used.
`--add event` is a sub-command of `--add`.

### 3.5 Item addressing and regex selection
- **By name:** exact `strcmp` against the item names in `bar_items` order. The
  first match wins. Names are unique because add, clone and rename enforce it.
- **By regex:** supported **only** for `--set <target>`, `--remove <target>` and
  bracket members in `--add bracket`. A target is a regex when
  `length > 1 && first == '/' && last == '/'`. The pattern is the text between
  the slashes. It is compiled with `regcomp(&re, pattern, 0)`, which means
  **POSIX Basic RE**: no `REG_EXTENDED`, no `REG_ICASE`, unanchored search. It
  is then tested with `regexec` against every item name in current `bar_items`
  order. In BRE, `+`, `?`, `|`, `{`, `(` are literal unless escaped. mbar must
  implement BRE semantics, or document that it uses ERE.
  - Compile error: `[!] Regex: Could not compile regex '<target incl. slashes>'\n`.
    Selection is empty.
  - Match error other than NOMATCH: `[!] Regex: Regex match failed '<regerror text>'\n`.
    Selection is empty.
  - No match: `[?] Regex: No match found for regex '<target incl. slashes>'\n`.
    Selection is empty.
  - `//` gives the empty pattern. That is platform-defined: macOS `regcomp`
    returns `REG_EMPTY`, which produces the compile error.
- `--subscribe`, `--push`, `--move`, `--rename`, `--clone`, `--query` and
  `--reorder` accept **literal names only**. A `/x/` there is just a name that
  is not found.

---
## 4. Shared value parsing rules

### 4.1 Scalar parsers (`misc/helpers.h`)
| Parser | C implementation | Semantics to replicate |
|---|---|---|
| int (`token_to_int`) | `(int)strtol(s, NULL, 0)` | Skip leading whitespace, take an optional sign, then auto-detect the base: `0x`/`0X` is hex, a leading `0` is octal, anything else is decimal. Parsing stops at the first invalid char. Garbage or empty gives 0. Overflow saturates to `LONG_MIN`/`LONG_MAX` (64-bit), then the value is truncated to the **low 32 bits** as a two's-complement `i32`. So `0xffffffff` is `-1` as int, which is bit pattern `0xffffffff`. |
| uint (`token_to_uint32t`) | `(uint32_t)strtoul(s, NULL, 0)` | Same base rules. A leading `-` negates modulo 2^64, then the value is truncated to the low 32 bits. Saturates at `ULONG_MAX`. |
| float (`token_to_float`) | `strtof(s, NULL)` | Decimal, exponent, hex-float, `inf`, `nan`. Garbage gives 0.0. |
| bool (`evaluate_boolean_state(tok, prev)`) | see 4.2 | |
| string (`token_to_string`) | copy | Verbatim, may be empty. |
| char (`.text[0]`) | first byte | Used for `align`, positions and `--animate`. Empty token gives `'\0'`. |

### 4.2 Booleans (`evaluate_boolean_state`)
Exact, case-sensitive matching:
- **true**: `on`, `yes`, `true`, `1`, `!off`, `!no`, `!false`, `!0`
- **toggle**: `toggle` gives `!previous`
- **false**: everything else, including `off`, `no`, `false`, `0`, `!on`, `!yes`,
  `!true`, `!1`, `ON`, `True`, `2`, and the empty string.

Booleans are serialized as `"on"`/`"off"` (`format_bool`).

### 4.3 Colors
- A color value is an **int** (4.1), read as 32-bit `0xAARRGGBB`. Examples:
  `0xff00ff00`, decimal `4278255360`, octal `0377...`.
  - **Quirk:** `ff00ff00` without the `0x` prefix parses as 0 (fully transparent).
  - **Quirk:** `0xff0000` is a 6-digit value, so alpha is 0 and the color is invisible.
- Stored channels: `a=((hex>>24)&0xff)/255`, `r=((hex>>16)&0xff)/255`,
  `g=((hex>>8)&0xff)/255`, `b=(hex&0xff)/255` (`color.c:color_set_hex`).
- The `hex` field is recomputed from the float channels as
  `((u32)(a*255)<<24) + ((u32)(r*255)<<16) + ((u32)(g*255)<<8) + (u32)(b*255)`.
  This truncates, not rounds.
- Sub-domain `<color>.<prop>` (`color_parse_sub_domain`):

| Property | Type | Effect |
|---|---|---|
| `hex` | int | Same as setting the color directly. Animated byte-wise. |
| `alpha` | float | `a = clamp(v, 0, 1)`. Animated as float. |
| `red` | float | `r = clamp(v, 0, 1)` |
| `green` | float | `g = clamp(v, 0, 1)` |
| `blue` | float | `b = clamp(v, 0, 1)` |
| other | | `[?] Color: Invalid property '<prop>'\n`. **Quirk:** the C code passes a struct to `%s`, which is UB. In practice it prints the property text. |

  **Quirk:** setting through the sub-domain (for example `background.color.alpha=1`)
  does *not* run the parent's setter. So it does not auto-enable the background,
  shadow, or alias override the way `background.color=...` does. Exception:
  `alias.color.*` sets the override (§6.11.7).
- Serialized as `"0x%x"` of `hex`: lowercase, no zero padding (for example
  `"0x0"`, `"0x44000000"`).

### 4.4 Fonts (`font.c`)
- `<text>.font=<Family>:<Style>:<Size>`. Parsed with
  `sscanf("%254[^:]:%254[^:]:%f")`. Fields that don't parse stay `""`, and size
  defaults to **10.0**.
  - `"Hack Nerd Font:Bold"` gives size 10.0.
  - `":Bold:14"` matches nothing (the first field can't be empty), so family is
    `""`, style is `""` and size is 10.0.
  - Family and style are each capped at 254 bytes.
  - Family, style and size are each set independently, so an unchanged part does
    not count as a change.
- Sub-properties `<text>.font.<p>`:

| p | Type | Notes |
|---|---|---|
| `family` | string | |
| `style` | string | |
| `size` | float | Animated as float. |
| `features` | string | Comma-separated. Each entry is either `N:M` (AAT feature type and selector ints), or a 4-char OpenType tag optionally prefixed `+` (on) or `-` (off), for example `+liga,-calt,ss01`. Other entries are ignored silently. |
| `typographical_width` | bool | When on, the text width is the typographic advance rounded (`+0.5`) instead of glyph bounds `+1.5`. |
| other | | `[!] Text: Invalid property '<p>'\n` |

- Default font: family `Hack Nerd Font`, style `Bold`, size `14.0`,
  `typographical_width` off, no features.
- Serialized as `"%s:%s:%.2f"`, for example `"Hack Nerd Font:Bold:14.00"`. The
  features are not serialized.

### 4.5 Paths
`script`, `click_script` and `background.image` (file form) values that start
with `~` are rewritten to `$HOME` + rest (`resolve_path`). Only a leading `~` is
handled, so `~user` becomes `$HOMEuser`. The path is capped at 511 bytes.

### 4.5.1 Relative paths
Every relative path is resolved by the **daemon** against **its** cwd, not the
client's. After the first config run, that cwd is the config directory (§10.2).
This applies to:
- `--reload <path>` (`realpath` in the daemon);
- `--load-font`;
- relative image files;
- relative paths inside `script` strings, which are run by `sh` in that cwd.

`--config <path>` is the exception: it is resolved by the starting process,
against the shell's cwd.

### 4.6 Dot notation (sub-domains)
Property keys are resolved **recursively, one segment at a time**. Each level
splits the remaining key at its first `.` (`get_key_value_pair(key, '.')`).
- Example: `icon.background.shadow.color.alpha` goes item, then `icon` (text),
  then `background`, then `shadow`, then `color`, then `alpha`.
- At each level the handler first compares the *whole remaining key* against its
  leaf property names. Only if none matches does it split off a sub-domain.
- **Quirk:** a key ending in `.` (for example `icon.`) splits into key `icon`
  with a NULL value. It is then compared as the full original length and fails:
  `[!] Item (<name>): Invalid property 'icon' \n`.

---
## 5. `--bar` domain (`message.c:handle_domain_bar`)

Syntax: `--bar <key>=<value> ...` (Mode A). Each pair is handled independently.
A token without `=` responds `[!] Bar: Expected <key>=<value> pair, but got: '<token>'\n`
and **breaks out of the list**.

**Quirk:** after that break, the *next* token is read as a command. So
`--bar foo height=30` also produces `[!] Unknown domain 'height=30'\n` and skips
to the next `-` token.

Properties are checked in this order. Anything not listed falls through to the
bar's background (§6.11.2).

| Key | Value | Default | Animated | Effect / notes |
|---|---|---|---|---|
| `margin` | int | 0 | yes | Horizontal inset from the screen edges. Resize. |
| `y_offset` | int | 0 | yes | Stored in `background.y_offset`. Resize. |
| `blur_radius` | int | 0 | yes | Window blur. **Quirk:** the setter returns false, so it never asks for a refresh by itself. |
| `font_smoothing` | bool | off | no | |
| `shadow` | bool | off | no | Recreates all bars (`bar_manager_reset`) when it changes. This is the bar's window shadow (bool). `shadow.<p>` goes to the background shadow (fallthrough). |
| `notch_width` | int | 200 | yes | Changing it does **not** set the resize flag. |
| `notch_offset` | int | 0 | yes | Resize. |
| `notch_display_height` | int | 0 | yes | Resize. |
| `hidden` | bool \| `current` | off | no | See below. |
| `topmost` | bool \| `window` | off | no | See below. Always recreates the bars, even if unchanged. |
| `sticky` | bool | on | no | Recreates the bars if it changed. |
| `display` | `main` \| `all` \| list | `all` | no | See below. Recreates the bars if it changed. |
| `position` | char (first byte) | `t` | no | Only the first char is stored, with no validation. Meaningful values: `top`/`t`, `bottom`/`b`, plus `left`/`l` and `right`/`r`, which make a vertical bar (`bar.c`). Empty value: ignored. |
| `clip` | — | — | — | Always responds `[!] Bar: Invalid property 'clip'\n`. |
| `height` | int | 25 | yes | `background_set_height`. 0 clears "overrides height". Resize. |
| `show_in_fullscreen` | bool | off | no | |
| *(other)* | — | — | — | `background_parse_sub_domain(&bar.background, key, value)` (§6.11.2). This is how `color` (default `0x44000000`), `border_color` (default `0xffff0000`), `border_width`, `corner_radius`, `padding_left` and `padding_right` (default **20**, the inset of the first and last items), `x_offset`, `image`, `image.*`, `shadow.*`, `color.*`, `border_color.*` and `drawing` are set. Unknown keys respond `[!] Background: Invalid property '<key>'\n` or `[!] Background: Invalid subdomain '<seg>'\n`. |

Bar background notes:
- The bar always draws its background, whatever `drawing` says, because
  `bar.c:bar_draw` forces `enabled = true`.
- Setting `color` also sets `drawing=on`, so the query then shows `"drawing": "on"`.

`hidden`:
- `hidden=current`:
  1. Get `adid` = the arrangement index of the active display.
  2. If `1 <= adid <= bar_count`, toggle `bars[adid-1].hidden`.
  3. Otherwise log `No bar on display <adid> \n` to the daemon stdout only. This
     is not in the response.
  - **Quirk:** indexing uses `bars[adid-1]`. That assumes bars line up with
    arrangement indices, which isn't true when `display` is a subset.
- Otherwise: `v = evaluate_boolean_state(value, any_bar_hidden)`. `toggle` is
  relative to "any bar hidden". Apply `v` to **all** bars, and set
  `any_bar_hidden = v`.
- If the result is hidden, close every item's popup (`popup.drawing=off`).
- Always marks the bar for update.

`topmost`:
- `window`: window level `kCGFloatingWindowLevel`, `topmost = true`.
- Otherwise `v = bool(value, prev topmost)`. On gives `kCGStatusWindowLevel`.
  Off gives `kCGBackstopMenuLevel` (the default).
- The query reports `topmost` as `"on"`/`"off"` only. `window` and `on` look the same.

`display`:
- The value is split on `,`. Entries are processed left to right into a 32-bit
  pattern that starts at 0:
  - `all` sets the pattern to `0xFFFFFFFF`, overwriting it.
  - `main` sets the pattern to `0`, overwriting it.
  - Anything else does `pattern |= 1 << (strtoul(entry) - 1)`. Displays are
    1-based **arrangement** indices.
- Pattern `0` means "only the main display": one bar on `CGMainDisplayID`.
- **Quirk:** an empty value (`display=`) gives pattern 0, which means main.
- **Quirk:** `0` or a non-number gives shift `-1`, which is UB in C. mbar should
  ignore such entries.
- **Quirk:** this bit numbering (bit n-1 for display n) differs from the item
  `display` property, which uses bit n (§6.11.1).

---
## 6. Items

### 6.1 `--add` (`message.c:handle_domain_add`)
Syntax (Mode B):
```
--add item    <name> <position>
--add space   <name> <position>
--add graph   <name> <position> <width>
--add slider  <name> <position> [<width>]
--add alias   <owner>[,<window name>] <position>
--add bracket <name> <member> [<member> ...]
--add event   <name> [<NSDistributedNotificationName>]        (see §7.2)
```
The first token is the *type*. `event` is handled before everything else
(§7.2). For all other types the algorithm is:

1. `name = tok2`, `position = tok3`.
2. **Duplicate check.** If an item called `name` exists, respond
   `[?] Add: Item '<name>' already exists\n` and stop. This is `[?]`, so the
   client exit code is 0.
3. **Create.** Create the item and append it at the **end** of `bar_items`. It
   is initialized as a copy of the default item (§6.3).
4. **Type.** `bar_item_set_type(type)`:
   - Recognized types: `item`, `space`, `alias`, `bracket`, `graph`, `slider`.
     Matching is exact.
   - Anything else (including empty) becomes `item`, with the response
     `[?] Add <name>: Invalid type '<type>', assuming 'item'\n`. Processing
     continues.
   - Side effects for `space`:
     - If no script is set, `script = "sketchybar -m --set $NAME icon.highlight=$SELECTED"`.
       It is literally `sketchybar`, not the bar name. **Quirk:** "no script"
       means none after inheriting from the default item, so a `--default script=…`
       suppresses this built-in script.
     - `update_mask |= space_change`.
     - `updates = off`, and "when_shown" is cleared.
     - Env vars `SELECTED=false`, `SID=0`, `DID=0`.
   - Side effects for the other types: `alias` sets `has_alias`, `graph` sets
     `has_graph`, `slider` sets `has_slider`. `bracket` creates a group whose
     first member is the bracket itself.
5. **Position.** Skipped for brackets. `bar_item_set_position(position)`
   accepts any string whose **first char** is one of:

   | Char | Meaning |
   |---|---|
   | `l` | left |
   | `q` | left of the notch (center-left) |
   | `c` | center |
   | `e` | right of the notch (center-right) |
   | `r` | right |
   | `p` | popup |

   Anything else, or an empty value, responds
   `[!] Add <name>: Illegal position '<position>'\n`. The item is removed and
   processing stops. On success the item's `align` is also set to the same char,
   unless the char is `p`.
6. **Name.** `bar_item_set_name(name)`. An empty name responds
   `[!] Add: Illegal name '<name>'\n` and removes the item. Setting the name
   also sets the env var `NAME=<name>`.
7. **Type-specific arguments.** These run when the type is not literally `item`
   and is non-empty:
   - **graph:** `width = uint(tok4)`. That many samples are allocated, all 0.0.
     A missing width gives 0 samples. **Quirk:** pushing to a 0-width graph is a
     modulo by zero in C. mbar must guard against it.
   - **slider:** `slider.width = uint(tok4)`. A missing width gives **0**. It is
     not the init default of 100, because the setup always overwrites it.
   - **alias:** `name` is split at the first `,`. Before the comma is the owner
     (process name); after it is the window name.
     - With no comma (or nothing after it), the owner is the whole name and the
       window name is NULL.
     - The item's name stays the full string, for example
       `Control Center,Battery`.
     - Matching rules are in §6.11.7.
   - **bracket:** members start at `tok3`, the "position" slot. For each member
     token:
     - If it is a regex `/…/`, it matches all items by regex (§3.5).
     - Otherwise it is an exact name. If the name is not found, respond
       `[?] Add (Group) <bracketname>: Failed to add member '<member>', item not found\n`.
     - For the **first** member token that resolves to ≥1 item: if that item's
       position is `p`, the bracket is added to the same popup and gets position
       `p`.
     - Each resolved item is added with `group_add_member`. An item that already
       belongs to a bracket is moved to this one, since the group pointer is
       overwritten. If the resolved item is itself a bracket, its members are
       added recursively instead.
     - If the **first** member token resolves to nothing, the bracket is removed
       and the loop ends. **Quirk (C UB):** processing then continues on the
       freed item. mbar should just stop.
     - A bracket's own `position` is whatever the default item has (`l` unless
       `--default position=` was used), because step 5 is skipped.
8. **Popup host.** This step runs if `position` starts with `p`. Note that for
   brackets it checks the first member token, which is a **quirk**.
   - The position string is split at the first `.`. If both halves are
     non-empty, the second half names the popup host:
     - Host not found: respond
       `[!] Add (Popup) <name>: Item '<host>' is not a valid popup host\n`,
       remove the item and stop.
     - Host found: append the item to the host's popup item list. The item's
       `parent` becomes the host.
   - So valid popup positions are `popup.<host>`, `p.<host>`, `pXYZ.<host>`, and
     so on. A bare `popup` gives position `p` with no host, and the item is
     never shown.
   - **Quirk:** a bracket whose first member name starts with `p` and contains a
     `.` goes through this logic too.
9. Mark the item `needs_update`.

### 6.2 `--set` (`handle_message_mach`)
Syntax: `--set <name|/regex/> <key>=<value> ...`

Algorithm:
1. **Target.** Get the target token.
   - Regex (§3.5): select every matching item.
   - Otherwise exact name. If not found, respond `[!] Set: Item not found '<name>'\n`.
2. **Empty selection.** Consume and discard the rest of the batch line (Mode B
   scan), then continue with the next command.
   - **Quirk:** the scan starts at the cursor. If the missing target is directly
     followed by another command (`--set missing --bar height=10`), that command
     is swallowed too.
3. **Apply pairs.** Otherwise iterate the pairs in Mode A. For each token, and
   for each selected item in order (token-major, item-minor), apply
   `bar_item_parse_set_message(item, "key\0value\0\0")` (§6.11.1).
   - A token without `=` responds once:
     `[!] Set (<first item name>): Expected <key>=<value> pair, but got: '<token>'\n`.
     Unlike `--bar`/`--default`, it continues with the next token.
   - Properties are applied **in argument order**. A later pair overrides an
     earlier one for the same property.

### 6.3 `--default` and the default item
- Syntax: `--default <key>=<value> ...` (Mode A).
- Each pair is applied with `bar_item_parse_set_message(&default_item, ...)`.
  It accepts **exactly the same keys as `--set`**.
- A token without `=` responds
  `[!] Set (default): Expected <key>=<value> pair, but got: '<token>'\n` and
  **breaks**. As with `--bar`, the next token is then read as a command.
- The default item:
  - is not in `bar_items`;
  - is named `defaults`, so its errors read `[!] Item (defaults): ...`;
  - is a plain `item`.
- **Inheritance:** at creation (`--add`, `--clone`), a new item becomes a **deep
  copy of the default item's current state** (`bar_item_inherit_from_item`).
  - Copied: every scalar, the icon, label and knob text (string, font family,
    style, size, typographical_width), script, click_script, the background
    images, and also position, align, update_freq, associated space/display,
    drawing, and so on.
  - Not copied: the env vars (reset, then NAME is set), popup items, the bracket
    group, and windows.
  - **Quirk:** `text_copy` does not copy `font.features`. The bitwise struct copy
    leaves the pointer shared, then clears it. So features are **not
    inherited**: `features` ends up NULL on the new item.
  - Later `--default` changes do not affect existing items.
- `graph.*`, `alias.*` and `slider.*` keys are accepted on the default item even
  though it is not that type. Every other item gets the "non-graph/alias/slider"
  error (§6.11.1).
- **Reset.** The key `reset` resets the default item to factory state:
  `bar_item_init(&default_item, NULL)`.
  - **Quirk:** it needs a value, as in `--default reset=1`. A bare
    `--default reset` hits the "Expected <key>=<value>" error.
  - **Quirk:** `--set foo reset=1` *also* resets the **default** item, not `foo`.
  - **Quirk:** after a reset the default item's name is NULL, so it queries as
    `"name": "(null)"` and errors print `(null)`.

### 6.4 `--remove <name|/regex/>` (Mode B)
- Not found (literal name): respond `[!] Remove: Item '<name>' not found\n`.
- For each selected item, call `bar_manager_remove_item`:
  - If the item's position is `p`, remove it from **every** item's popup list.
  - Remove it from `bar_items`.
  - Destroy it. A bracket releases its members. A bracket member is removed from
    its bracket. The item's own popup (and its windows) is destroyed.
- Sets the end-of-request refresh flag even if nothing was removed.
- Popup children of a removed host are **not** removed. Their `parent` pointer
  dangles in C. mbar should detach them.

### 6.5 `--move <name> before|after <reference>` (Mode B)
- If either item is missing, respond
  `[!] Move: Item '<name>' or '<reference>' not found\n`.
- The direction is `before` only if the token is exactly `before`. **Anything
  else means `after`.**
- Algorithm: rebuild `bar_items` without the item, inserting it immediately
  before or after the reference.
- **Quirk (UB):** if `name == reference`, the item drops out of the list. mbar
  should make this a no-op.
- Marks the moved item `needs_update`. Only the order of the global item list
  changes. Position, popup and bracket membership are unaffected.

### 6.6 `--reorder <name> [<name> ...]` (Mode B)
- Names that aren't found respond `[!] Order: Item '<name>' not found\n` and are
  skipped.
- Algorithm (`bar_manager_sort`):
  ```
  index = 0
  for i in 0..len(bar_items):
      if bar_items[i] is in ordering:
          if bar_items[i] != ordering[index]:
              bar_items[i] = ordering[index]
              mark it needs_update
          index += 1
  ```
  The slots that the listed items already occupy are kept, and they are refilled
  in the listed order. Items not listed keep their exact slots.
- **Quirk:** duplicate names in the list can duplicate or lose items. mbar should
  deduplicate first, keeping the first occurrence.

### 6.7 `--rename <old> <new>` (Mode B)
- If `old` is missing or `new` already exists, respond
  `[!] Rename: Failed to rename item: <old> -> <new>\n`.
- Otherwise rename the item and update env `NAME`.
- An empty `new` name is rejected silently (no message).

### 6.8 `--clone <new name> <parent> [before|after]` (Mode B)
Note the argument order. The help text gets it wrong.

1. If the parent is not found, respond
   `[!] Clone: Parent Item '<parent>' not found\n` and stop.
2. If `new name` exists, respond `[?] Clone: Item '<new name>' already exists\n`
   and stop.
3. Create the item (default-initialized). Then deep-copy **the parent's full
   state** onto it, including:
   - `type`, `position`, `align`
   - `update_mask` (subscriptions), `update_freq`, scripts
   - associations and all visual properties
   - `has_graph`, `has_slider`, `has_alias`
4. For spaces it then sets `SELECTED=false`, `SID=<parent's DID>` and
   `DID=<parent's DID>`. **Quirk:** SID is set from the parent's DID.
5. Set the name.
6. If the modifier is exactly `before` or `after`, move the clone next to the
   parent. Otherwise it is appended at the end.

Clone notes:
- **Quirk:** the graph sample buffer, the alias owner/name strings, the bracket
  group pointer and the popup parent pointer are shallow-copied in C.
  - Graph samples are shared.
  - A clone of a popup child has `parent` set, but is **not** in the host's popup
    list.
  - mbar should deep-copy, and decide whether to also insert popup clones into
    the host's popup. The recommended choice is yes: append after the parent.
- A missing `new name` (empty token) creates an item with a NULL name (C bug).
  mbar should reject it.

### 6.9 Item types summary
| Type | Created by | Extra state | Query `type` |
|---|---|---|---|
| item | `--add item` | — | `"item"` |
| space | `--add space` | Auto script, `SELECTED`/`SID`/`DID` env, implicit `space_change` subscription | `"space"` |
| graph | `--add graph` | Ring buffer of `width` floats | `"graph"` |
| slider | `--add slider` | percentage, knob text, foreground/background | `"slider"` |
| alias | `--add alias` | Captured menu-bar window image | `"alias"` |
| bracket | `--add bracket` | Member list (first member = self) | `"bracket"` |

### 6.10 `--push <name> <float> [<float> ...]` (Mode B)
- Item not found: respond `[!] Push: Item '<name>' not found\n`.
- Item is not a graph: respond `[!] Push: Item '<name>' not a graph\n`.
- Each value is parsed as a float (`strtof`) and written at the ring cursor:
  `y[cursor] = v; cursor = (cursor+1) % width`. The newest value is drawn at the
  right.
- Values are not clamped. They are expected in 0..1.
- **Quirk:** negative values like `-0.5` cannot be passed (§3.2).
- Marks the item `needs_update`.

### 6.11 Item property reference

The **Anim** column says whether `--animate` applies. The kinds are `int`,
`float`, and `bytes` (a color animated per channel). Every handler compares the
new value with the old one and sets `needs_update` only on a change, unless
noted otherwise.

#### 6.11.1 Item level (`bar_item.c:bar_item_parse_set_message`)
The key is first split at its first `.`. If both parts are non-empty, the left
part is a **sub-domain**:

| Sub-domain | Target | Restriction |
|---|---|---|
| `icon.` | text §6.11.3 | |
| `label.` | text §6.11.3 | |
| `background.` | background §6.11.2 | |
| `popup.` | popup §6.11.6 | |
| `graph.` | graph §6.11.8 | Only items with `has_graph`, or the default item. Otherwise `[!] Item (<name>): Trying to set a graph property on a non-graph item\n`. |
| `alias.` | alias §6.11.7 | Same rule. `[!] Item (<name>): Trying to set an alias property on a non-alias item\n` |
| `slider.` | slider §6.11.9 | Same rule. `[!] Item (<name>): Trying to set a slider property on a non-slider item\n` |
| other | — | `[!] Item (<name>): Invalid subdomain '<left part>'\n` |

Keys without a sub-domain:

| Key | Value | Default | Anim | Semantics |
|---|---|---|---|---|
| `icon` | string | `""` | width\* | Same as `icon.string`. |
| `label` | string | `""` | width\* | Same as `label.string`. |
| `drawing` | bool | on | – | Hides the item everywhere when off. |
| `updates` | bool \| `when_shown` | on | – | `when_shown` sets `updates=on` and `only_when_shown=on`. Any other value sets `updates = bool(v, updates)` and `only_when_shown=off`. Never marks `needs_update`. |
| `scroll_texts` | bool | off | – | Enables marquee scrolling of icon, label and knob when `max_chars` truncates them. No redraw flag. |
| `width` | int \| `dynamic` | dynamic | int | See below. |
| `script` | string | none | – | Shell command (§12). A leading `~` is expanded. Setting the identical string is a no-op. An empty string disables it. |
| `click_script` | string | none | – | Same rules as `script`. Runs on mouse-up over the item (§12.3). |
| `update_freq` | uint | 0 | – | Routine update period in seconds (1 s tick). 0 disables routine updates. |
| `position` | position | `l` | – | See below. Always marks `needs_update` (except on the popup-host error). |
| `align` | char | same as position | – | Horizontal alignment of the content inside a fixed `width`: `c` centers, `r` right-aligns, any other char left-aligns. Any first char is stored. |
| `associated_space`, `space` | list of uint | 0 (all) | – | See below. |
| `associated_display`, `display` | list of uint \| `active` | 0 (all) | – | See below. |
| `y_offset` | int | 0 | int | Vertical offset of the item content. |
| `padding_left` | int | 0 | int | **The same field as `background.padding_left`.** |
| `padding_right` | int | 0 | int | **The same field as `background.padding_right`.** |
| `blur_radius` | int | 0 | int | Item window blur. |
| `shadow` | bool | off | – | Item window shadow. On a change, all item windows are destroyed and recreated. |
| `ignore_association` | bool | off | – | Draw regardless of display or space association. Always marks `needs_update`. |
| `mach_helper` | string | none | – | Bootstrap (IPC) service name. Every time the item's script would run, the env is also sent there (§12.5). A failed lookup silently stores "none". An empty value is ignored. |
| `reset` | any | — | – | Resets the **default item** (§6.3 quirk). |
| other | — | — | — | `[!] Item (<name>): Invalid property '<key>' \n` (note the space before `\n`). This includes the unimplemented names `lazy`, `cache_scripts`, `string`, `color`, and so on. |

\* The `icon`/`label` string change is not animated. When an animation is
active and the text width changes, the width is animated (§6.11.3 `string`).

`width`:
- `width=<n>`, n ≥ 0: fixed item width n. This is the **total** width including
  padding: when a fixed width is set, `padding_left` and `padding_right` are not
  added to the item's footprint. The animation goes from the current footprint
  to n.
- `width=<n>`, n < 0: back to dynamic width with no animation.
- `width=dynamic`:
  1. Animate the fixed width from the current value to the natural content
     length plus both paddings.
  2. Then queue a zero-duration animation that sets "dynamic" (value −1).
  So `dynamic` takes effect on the next animator frame. With no `--animate` it
  is effectively immediate.
- Query: `geometry.width` is the fixed width, or `-1` when dynamic.

`position` (`--set`):
- `bar_item_set_position(value)`. The first char must be one of `l q c e r p`.
  Otherwise the call is **silently ignored**.
- On success:
  - If the item currently has a popup parent, it is removed from that parent's
    popup list. The C code does not clear `parent`; mbar should.
  - `position = c`, and `align = c` unless `c == 'p'`.
- Then the value is split at the first `.`:
  - If the left part starts with `p` and the right part is non-empty, the right
    part names the host.
    - Host not found: respond
      `[!] Item Position (<name>): Item '<host>' is not a valid popup host\n`.
      The handler returns early, so `needs_update` is not set. **Quirk:** the
      item already has position `p` from the first step.
    - Host found: `popup_add_item(host.popup, item)`. This removes the item from
      any previous popup and sets `parent = host`.
  - If the left part does not start with `p` (for example `left.x`): `parent = NULL`.

`associated_space` / `space`:
- The mask is reset to 0. The value is split on `,`, and for each entry `n`:
  `mask |= 1 << strtoul(n, 0)`. The bit is **n**, not n−1. Space numbers are
  1-based Mission Control indices across all displays (`mission_control_index`).
- Mask 0 means "all spaces".
- An empty value clears the mask.
- An entry that isn't a number gives bit 0, which is never a real space.
- For **space-type** items, each entry *replaces* the mask, so the last one wins.
  The item's `SID` env var is set to the index of the lowest set bit (= n).
  Space items ignore the space mask when deciding whether to draw; it decides
  `SELECTED` instead (§12.4).
- `needs_update` is set only if the mask changed.
- The query shows `associated_space_mask` as an unsigned decimal, for example
  `space=1,3` gives `10`.

`associated_display` / `display`:
- The mask is reset to 0 and `associated_to_active_display = false`. The value
  is split on `,`:
  - `active` sets `associated_to_active_display = true`.
  - Otherwise `mask |= 1 << strtoul(n)`. Again bit **n**; displays are 1-based
    arrangement ids.
- The item is drawn on a bar only if (mask == 0 or bit `adid` is set) **and**
  (not `active`, or the bar is on the active display). `ignore_association=on`
  bypasses both checks (`bar.c:bar_draws_item`).
- For space items, each entry replaces the mask (last wins), sets
  `overrides_association` (so the automatic display association from the space
  is no longer applied) and sets the `DID` env var to n.
- `needs_update` is set only if the mask changed. Toggling only `active` does
  not count as a change.

#### 6.11.2 Background (`background.c:background_parse_sub_domain`)
Used for `background.*` on items, `icon.background.*`, `label.background.*`,
`popup.background.*`, `slider.background.*`, `slider.knob.background.*`, and the
bar's fallthrough keys.

| Key | Value | Default (item) | Anim | Semantics |
|---|---|---|---|---|
| `drawing` | bool | off | – | Enable or disable. Disabling resets the clip state. |
| `color` | color | `0x00000000` | bytes | **Also sets `drawing=on`.** |
| `border_color` | color | `0x00000000` | bytes | Does not enable. |
| `border_width` | int (u32) | 0 | int | |
| `height` | int | 0 | int | 0 means "auto" (bar height minus bar border − 1). A non-zero value fixes the height. The query shows `0` when auto. |
| `corner_radius` | int | 0 | int | |
| `padding_left` | int | 0 | int | |
| `padding_right` | int | 0 | int | |
| `x_offset` | int | 0 | int | |
| `y_offset` | int | 0 | int | |
| `clip` | float | 0.0 | float | 0..1. This background punches a hole of this alpha through the bar background. A value > 0 also enables the background. |
| `image` | image source | none | – | See below. Same as `image.string`. |
| `shadow.<p>` | | | | §6.11.4 |
| `image.<p>` | | | | §6.11.5 |
| `color.<p>` | | | | §4.3 |
| `border_color.<p>` | | | | §4.3 |
| other with `.` | | | | `[!] Background: Invalid subdomain '<seg>'\n` |
| other | | | | `[!] Background: Invalid property '<key>'\n` |

Image source (`image.c:image_load`). Checked in this order:
1. `app.<Application Name>`: the app icon from NSWorkspace, at 32 pt × screen
   scale; the image size is divided by scale². If the app is unknown, respond
   `[!] Image: Invalid application name: '<name>'\n` and make no change.
2. `space.<n>`: a screenshot of Mission Control space n (`atoi`). On failure,
   respond `[!] Image: Invalid Space ID: '<n>'\n`.
3. `media.artwork`: link to the now-playing artwork. Starts media listening,
   enables the image, and the image is scaled to 32 px height.
4. An existing file path (after `~` expansion): decoded as PNG if the path ends
   in `.png`, otherwise as JPEG.
   - If the data provider fails: `[!] Image: Invalid Image Format: '<…>'\n`.
     **Quirk:** this prints the second half of the dot split, which may be "(null)".
   - If decoding fails: response *and* daemon log get
     `Could not open image file at: <path>\n`. Note there is no `[!]` prefix,
     so the client exits 0.
5. Empty string: destroys the image (clears it and its path). Returns false.
6. Anything else: `[!] Image: File '<resolved path>' not found\n`.

On success the image is `enabled` (`drawing=on`). The `value` field of the
query is the raw string as given. The path is stored *before* the checks, so it
is updated even when loading fails.

#### 6.11.3 Text: icon, label, knob (`text.c:text_parse_sub_domain`)
| Key | Value | Default | Anim | Semantics |
|---|---|---|---|---|
| `string` | string | `""` | width | Sets the text. If it changed while an animation is active and the rendered width changed, the text width is animated from the old to the new width, then set back to dynamic. Invalid UTF-8 renders as `Warning: Malformed UTF-8 string`. |
| `color` | color | `0xffffffff` | bytes | |
| `highlight` | bool | off | bytes | When on, the text draws in `highlight_color` instead of `color`. With an animation active, the color cross-fades. |
| `highlight_color` | color | `0xff000000` | bytes | |
| `font` | `Family:Style:Size` | `Hack Nerd Font:Bold:14.0` | – | §4.4 |
| `font.<p>` | | | | §4.4 (`family`, `style`, `size`, `features`, `typographical_width`) |
| `padding_left` | int | 0 | int | |
| `padding_right` | int | 0 | int | |
| `y_offset` | int | 0 | int | |
| `width` | int \| `dynamic` | dynamic | int | Fixed text width, including the text paddings. Negative means dynamic. `dynamic` works like the item width (two-step). |
| `align` | char | `l` | – | Within a fixed width: `c` centers, `r` right-aligns, else left. The query maps `l r c b t` to names and anything else to `"invalid"`. |
| `drawing` | bool | on | – | |
| `max_chars` | int | 0 | – | Truncates the *displayed* width to the first N UTF-8 code points (counting non-continuation bytes). 0 means unlimited. With `scroll_texts=on` the full text scrolls. |
| `scroll_duration` | int | 100 | – | Marquee duration in 60 Hz frames. Negative values are ignored. Never triggers a redraw. |
| `background.<p>` | | | | §6.11.2 |
| `shadow.<p>` | | | | §6.11.4 |
| `color.<p>` | | | | §4.3 |
| `highlight_color.<p>` | | | | §4.3 |
| other with `.` | | | | `[!] Text: Invalid subdomain '<seg>' \n` |
| other | | | | `[!] Text: Invalid property '<key>'\n` |

#### 6.11.4 Shadow (`shadow.c:shadow_parse_sub_domain`)
| Key | Value | Default | Anim | Semantics |
|---|---|---|---|---|
| `drawing` | bool | off | – | |
| `color` | color | `0xff000000` | bytes | **Also enables the shadow.** |
| `angle` | int (u32) | 30 | int | Degrees. Offset = (d·cos θ, −d·sin θ). |
| `distance` | int (u32) | 5 | int | |
| `color.<p>` | | | | §4.3 |
| other with `.` | | | | `[!] Shadow: Invalid subdomain '<seg>'\n` |
| other | | | | `[!] Shadow: Invalid property '<key>'\n` |

#### 6.11.5 Image (`image.c:image_parse_sub_domain`)
| Key | Value | Default | Anim | Semantics |
|---|---|---|---|---|
| `string` | image source | none | – | Same as `background.image=…` (§6.11.2) |
| `drawing` | bool | off (on once loaded) | – | |
| `scale` | float | 1.0 | float | |
| `corner_radius` | uint | 0 | int | |
| `border_width` | float | 0 | float | |
| `border_color` | color | `0xcccccccc` | bytes | |
| `padding_left` | int | 0 | int | |
| `padding_right` | int | 0 | int | |
| `y_offset` | int | 0 | int | |
| `border_color.<p>` | | | | §4.3 |
| `shadow.<p>` | | | | §6.11.4 |
| other with `.` | | | | `[?] Image: Invalid subdomain: <seg> \n` |
| other | | | | `[?] Image: Unknown property: <key> \n` |

#### 6.11.6 Popup (`popup.c:popup_parse_sub_domain`)
| Key | Value | Default | Anim | Semantics |
|---|---|---|---|---|
| `drawing` | bool | off | – | Shows the popup. Off closes the window. Every change resets the popup's display. |
| `horizontal` | bool | off | – | Lays the items out in a row instead of a column. |
| `align` | char | `l` | – | `l`, `c`, `r` relative to the host. Any char is stored. |
| `height` | int | 30 (auto) | int | Fixed cell height. Without it, the cell height follows the host's height. The query shows `-1` until it is set. |
| `y_offset` | int | 0 | int | |
| `blur_radius` | int | 0 | int | Never requests a redraw. |
| `topmost` | bool | on | – | Window level: popup-menu level when on, backstop+1 when off. |
| `background.<p>` | | | | §6.11.2. Default popup background is color `0x44000000` and border `0xffff0000`, with drawing **off**. |
| other with `.` | | | | `[!] Popup: Invalid subdomain '<seg>'\n` |
| other | | | | `[!] Popup: Invalid property '<key>'\n` |

Popup items are added through `position=popup.<host>` (§6.11.1) or
`--add … popup.<host>` (§6.1).

#### 6.11.7 Alias (`alias.c:alias_parse_sub_domain`)
| Key | Value | Default | Semantics |
|---|---|---|---|
| `color` | uint color | `0xffff0000` | Tints the captured image (mask fill) and turns the color override on. |
| `scale` | float | 1.0 | Image scale. |
| `update_freq` | uint | 1 | Re-capture period in seconds. 0 disables re-capture. |
| `shadow.<p>` | | | §6.11.4, applied to the image shadow. |
| `color.<p>` | | | §4.3. Also turns the color override on. |
| other with `.` | | | `[!] Alias: Invalid subdomain '<seg>'\n` |
| other | | | `[!] Alias: Invalid property '<key>' \n` |

None of these are animated.

Alias window matching (`alias_find_window`):
- The candidates are every on-screen window at the menu-bar layer, excluding
  owner `Window Server`. They are sorted by x **descending** (rightmost first),
  and `i` below is the 0-based index in that sorted list.
- A window matches when the owner name is equal **and** one of these holds:
  - the window name is equal;
  - the alias name equals `"<window name>(<i+1>)"` (the indexed form that
    `--query default_menu_items` prints);
  - no alias name was given and the window name is `""`.

#### 6.11.8 Graph (`graph.c:graph_parse_sub_domain`)
| Key | Value | Default | Semantics |
|---|---|---|---|
| `color` | uint color | `0xffcccccc` | Line color. |
| `fill_color` | uint color | `0xffcccccc` | Fill under the line. Sets an "overrides fill" flag. |
| `line_width` | float | 0.5 | |
| `color.<p>` | | | §4.3 on the line color |
| `fill_color.<p>` | | | §4.3. Does *not* set the override flag. |
| other with `.` | | | `[!] Graph: Invalid subdomain '<seg>'\n` |
| other | | | `[!] Graph: Invalid property '<key>'\n` |

None of these are animated. The color setters report a change only when the
value differs. `line_width` always reports a change.

#### 6.11.9 Slider (`slider.c:slider_parse_sub_domain`)
| Key | Value | Default | Anim | Semantics |
|---|---|---|---|---|
| `percentage` | uint | 0 | int | Clamped to 0..100. **Ignored while the user is dragging.** |
| `width` | uint | width from `--add` (0 if omitted) | int | Track length. Ignored while dragging. |
| `highlight_color` | uint color | `0xff0000ff` | bytes | Foreground (filled part) color. |
| `knob` | string | `""` | – | Same as `knob.string`. The knob is a text (§6.11.3). |
| `knob.<p>` | | | | §6.11.3 |
| `background.<p>` | | | | §6.11.2, applied to **both** the track background and the foreground. The foreground color is then re-asserted to `highlight_color`. The returned change flag is the track's. Default track color is `0xff000000`, enabled by `--add`. |
| other with `.` | | | | `[!] Slider: Invalid subdomain '<seg>' \n` |
| other | | | | `[!] Slider: Invalid property '<key>'\n` |

---
## 7. Events

### 7.1 Event registry (`custom_events.c`)
- Events live in an ordered list. An event's **bit** is `1 << index`, and an
  item's `update_mask` is the OR of the bits it is subscribed to.
- The built-in events are registered at init in this fixed order. The indices
  must match (`custom_events.h`):

| Bit index | Name | `bit` value in `--query events` |
|---|---|---|
| 0 | `front_app_switched` | 1 |
| 1 | `space_change` | 2 |
| 2 | `display_change` | 4 |
| 3 | `system_woke` | 8 |
| 4 | `mouse.entered` | 16 |
| 5 | `mouse.exited` | 32 |
| 6 | `mouse.clicked` | 64 |
| 7 | `mouse.scrolled` | 128 |
| 8 | `system_will_sleep` | 256 |
| 9 | `mouse.entered.global` | 512 |
| 10 | `mouse.exited.global` | 1024 |
| 11 | `mouse.scrolled.global` | 2048 |
| 12 | `volume_change` | 4096 |
| 13 | `brightness_change` | 8192 |
| 14 | `power_source_change` | 16384 |
| 15 | `wifi_change` | 32768 |
| 16 | `media_change` | 65536 |
| 17 | `space_windows_change` | 131072 |

INFO payloads per event are in §12.2.

- Custom events are appended after these, starting at index 18.
- **Limit:** 64 events in total, because of the 64-bit mask. Index ≥ 64 is UB
  in C. mbar should reject further events with an error.

### 7.2 `--add event <name> [<notification>]` (Mode B)
- Appends a custom event. If one or more tokens follow `<name>`, the next token
  is the notification name. On macOS that is an `NSDistributedNotificationCenter`
  name. The daemon observes it; when it fires, the event is triggered with
  `INFO` = the notification's `userInfo` as pretty-printed JSON, if serializable.
- **A duplicate name is ignored silently,** even if it has a different
  notification. Built-in names can't be re-added.
- No response in any case. An empty name is appended as an event called `""`.
- mbar on Linux has no NSDistributedNotificationCenter. It may map the
  notification name to a D-Bus signal, or ignore it. Either way it must store the
  name and report it in `--query events`.

### 7.3 `--subscribe <name> <event> [<event> ...]` (Mode B)
- Item not found: respond `[!] Subscribe: Item not found '<name>'\n` and stop.
  Literal name only, no regex.
- For each event token:
  - `flag` = the event's bit, or 0 if unknown.
  - Some events start a listener lazily, once per process, at the first
    subscription:
    - `volume_change`: audio volume listener;
    - `brightness_change`: brightness listener;
    - `media_change`: now-playing listener;
    - `space_windows_change`: window tracking.
  - `update_mask |= flag`.
  - Unknown event: respond `[?] Event: '<event>' not found\n` and continue with
    the next token.
- There is **no unsubscribe** command. The mask is only cleared by removing the
  item or reloading.
- A clone inherits the subscriptions. Space items are implicitly subscribed to
  `space_change`.

### 7.4 `--trigger <event> [<KEY>=<value> ...]` (Mode B)
- Every following token that contains `=` with a **non-empty value** becomes an
  env var: split at the first `=`, later duplicates win. Other tokens are ignored
  silently.
  - `=v` (empty key) is accepted, but the later `setenv("")` fails, so it has no
    effect.
  - `K=` (empty value) is dropped.
- Built-in events with forced handlers. For these, the user env vars are
  **ignored**:

| Event | Action |
|---|---|
| `space_change` | `handle_space_change(forced=true)`: recompute spaces, update space items (forced `SELECTED` refresh), fire `space_change` with the computed INFO. |
| `display_change` | Fire `display_change` with INFO = active display index. |
| `space_windows_change` | Only if the window listener is active (some item subscribed): recompute all spaces and fire for each. Otherwise nothing. |
| `volume_change` | Reset the cached volume and re-read it, so it always fires. |
| `media_change` | Clear the cached media info and re-query now-playing. Fires if info is available. |
| `wifi_change` | Re-read the SSID and fire. |
| `power_source_change` | Reset the cached source, re-read it and fire. |

- Any other name, including the remaining built-ins such as
  `front_app_switched`, `system_woke`, `mouse.*`, `brightness_change` and
  custom events, calls `custom_events_trigger(name, env)`:
  - For every item, in `bar_items` order, whose `update_mask & bit(name)` is
    non-zero, call `bar_item_update(item, sender=name, forced=false, env)`
    (§12.1).
  - An **unknown event name triggers nothing, with no error**.
  - **Quirk:** the same env object is reused across items. Each item's own env
    vars (`NAME`, `SID`, `SELECTED`, …) are merged into it, so vars from an
    earlier item **leak into later items' scripts** unless overwritten. mbar
    should build a fresh env per item, unless bug-compatibility is wanted.

### 7.5 Routine updates and the clock
- A 1 s repeating timer posts `SHELL_REFRESH`, which runs
  `bar_manager_update(forced=false)`:
  - Skipped while frozen or while the system sleeps.
  - Calls `bar_item_update(item, NULL, false, NULL)` for every item.
  - Re-captures aliases that are shown.
- `bar_item_update(item, sender, forced, env)` (`bar_item.c`):
  1. If the item is shown, `scroll_texts=on`, and `counter % 15 == 0`, run the
     text scroll animations.
  2. `counter += 1`.
  3. If `(!updates || (update_freq == 0 && sender == NULL)) && !forced`, return.
  4. Run the update if `((counter >= update_freq || sender != NULL) && (only_when_shown ? shown : true)) || forced`.
     - Set `counter = 0`.
     - If a script is non-empty or `mach_helper` is set, build the env (§12.1)
       and run the script and/or send to `mach_helper`.
- Space items have `updates` toggled internally: on when their selection changed,
  off otherwise. That is why space scripts run only on a selection change during
  `space_change` (§12.4).

### 7.6 `--update`
- Calls `bar_manager_update(forced=true)`. This works even while frozen, but not
  while asleep.
- It fires these forced events in order:
  1. space change
  2. wifi
  3. volume
  4. brightness
  5. power source
  6. front app
  7. media
  8. space windows
- It then runs **every** item's script with `SENDER=forced`. This ignores
  `updates`, `update_freq` and `when_shown`.
- Then it does a forced full refresh, which happens at the end of the request
  because the bar is frozen during the request.

---

## 8. `--animate <curve> <duration>` (Mode C)
- `interp = first char of the curve token`:

  | Char | Curve |
  |---|---|
  | `l` | linear `x` |
  | `q` | quadratic `x²` |
  | `t` | tanh `0.52·tanh(2·atanh(1/1.04)·(x−0.5)) + 0.5` |
  | `s` | sin `sin(πx/2)` |
  | `e` | exp `x·e^(x−1)` |
  | `c` | circ `sqrt(1−(x−1)²)` |
  | any other (including `b` bounce, `o` overshoot) | linear |

  So `linear`, `quadratic`, `tanh`, `sin`, `exp`, `circ`, and any word with the
  same first letter, all work.
- `duration = uint(token)`, in frames at 60 Hz (`seconds = duration / 60`).
  Wall-clock based on the display link.
- The setting stays in effect **for the rest of this request only**. It applies
  to every animatable property set by later `--set`, `--bar` and `--default`
  commands. It is reset at the start of each request. Use `--animate <c> 0` to
  turn it off mid-request.
- With duration 0, any running animation for the same (target, property) is
  cancelled and **jumps to its final value** first. Then the new value is applied
  immediately.
- With duration > 0:
  1. *Locked* animations (from earlier requests) on the same (target, property)
     are removed without jumping. The new animation starts from the property's
     **current** value.
  2. If another animation for the same (target, property) already exists in
     *this* request, the new one is **chained**: it waits for the previous one
     to finish and starts from its final value. So
     `--animate sin 30 --set a y_offset=10 y_offset=0` bounces.
- Values are interpolated per kind:
  - int: rounded `(1−s)·a + s·b + 0.5`, and exactly `b` on the final frame;
  - float: linear in float;
  - bytes: each of the 4 bytes of a color separately, truncated.

---
## 9. `--query` (`message.c:handle_domain_query`)

Syntax (Mode B): `--query <what> [<name>]`.

| `<what>` | Output |
|---|---|
| `default_menu_items` | §9.6 |
| `item <name>` | Item JSON (§9.1). Not found: `[!] Query: Item '<name>' not found\n` |
| `bar` | Bar JSON (§9.3) |
| `defaults` | Item JSON of the default item (§9.1), `"name": "defaults"` |
| `events` | §9.4 |
| `displays` | §9.5 |
| anything else | Treated as an item name. Not found: `[!] Query: Invalid query, or item '<token>' not found \n` (note the space before `\n`). |

The keywords come first. An item literally named `bar`, `events`, `defaults`,
`displays`, `default_menu_items` or `item` can only be queried with
`--query item <name>`.

General formatting rules. **Byte-exact compatibility requires these:**
- Indentation uses TAB characters. Lines end with `\n`.
- `%f` prints 6 decimals (`1.000000`), `%u`/`%d` print decimal, and colors print
  as `"0x%x"` (lowercase, unpadded).
- Booleans are the strings `"on"`/`"off"`.
- Strings are printed **unescaped**, except `script` and `click_script`, which
  escape only `"` (as `\"`) and newline (as `\n`). Backslashes are not escaped.
  So labels containing `"` produce invalid JSON. mbar may escape properly, but
  must say so.
- NULL strings print as `(null)`. This is macOS printf behavior and appears for
  an unset `script`, `click_script`, image `value`, and event `notification`.
- Every output ends with `\n`.

### 9.1 Item JSON (`bar_item_serialize`)
Template. `→` marks a TAB. Placeholders are `{…}`.
```
{
→"name": "{name}",
→"type": "{item|alias|bracket|slider|graph|space}",
→"geometry": {
→→"drawing": "{on|off}",
→→"position": "{left|right|center|q|e|popup}",
→→"associated_space_mask": {u32},
→→"associated_display_mask": {u32},
→→"ignore_association": "{on|off}",
→→"y_offset": {int},
→→"padding_left": {int},
→→"padding_right": {int},
→→"scroll_texts": "{on|off}",
→→"width": {fixed width or -1},
→→"background": {
{BACKGROUND indent=→→→ detailed}
→→}
→},
→"icon": {
{TEXT indent=→→}
→},
→"label": {
{TEXT indent=→→}
→},
→"scripting": {
→→"script": "{escaped script|(null)}",
→→"click_script": "{escaped|(null)}",
→→"update_freq": {u32},
→→"update_mask": {u64 decimal},
→→"updates": "{on|off|when_shown}"
→},
→"bounding_rects": {
{RECTS}
→}{POPUP}{TYPE_EXTRA}
}
```
- Position `q` and `e` print as the single letters `"q"` and `"e"`.
- `{RECTS}`:
  - For each existing item window, in display-index order and joined by `,\n`:
    ```
    →→"display-{adid}": {
    →→→"origin": [ {x %f}, {y %f} ],
    →→→"size": [ {w %f}, {h %f} ]
    →→}
    ```
  - Because of the surrounding template, with no windows the block is
    `"bounding_rects": {\n\n→}`.
  - The coordinates are the item window's global screen origin and size.
    Windows not yet placed sit at (-9999, -9999).
- `{POPUP}` is present only if the item hosts ≥ 1 popup item:
  `,\n→"popup": {\n` + POPUP(indent `→→`) + `\n→}`.
- `{TYPE_EXTRA}`:

  | Type | Text |
  |---|---|
  | bracket | `,\n→"bracket": [\n` + member names (excluding itself), each `→→"{name}"`, joined by `,\n` + `\n→]` |
  | graph | `,\n→"graph": {\n` + GRAPH(`→→`) + `\n→}` |
  | slider | `,\n→"slider": {\n` + SLIDER(`→→`) + `\n→}` |
  | other types | (nothing) |

Fragment templates. `I` is the given indent. None of them ends with a newline.

BACKGROUND(I, detailed) (`background_serialize`):
```
I"drawing": "{on|off}",
I"color": "0x{hex}",
I"border_color": "0x{hex}",
I"border_width": {u32},
I"height": {height if fixed else 0},
I"corner_radius": {u32},
I"padding_left": {int},
I"padding_right": {int},
I"x_offset": {int},
I"y_offset": {int},
I"clip": {float %f},
I"image": {
I→"value": "{raw image string|(null)}",
I→"drawing": "{on|off}",
I→"scale": {float %f}
I}
```
If `detailed`, this is appended:
```
,
I"shadow": {
SHADOW(I→)
I}
```

SHADOW(J) (`shadow_serialize`):
```
J"drawing": "{on|off}",
J"color": "0x{hex}",
J"angle": {u32},
J"distance": {u32}
```

TEXT(T) (`text_serialize`):
```
T"value": "{string, unescaped}",
T"drawing": "{on|off}",
T"highlight": "{on|off}",
T"color": "0x{hex}",
T"highlight_color": "0x{hex}",
T"padding_left": {int},
T"padding_right": {int},
T"y_offset": {int},
T"font": "{family}:{style}:{size %.2f}",
T"width": {custom_width},
T"scroll_duration": {int},
T"align": "{left|right|center|bottom|top|invalid}",
T"background": {
BACKGROUND(T→, detailed)
T},
T"shadow": {
SHADOW(T→)
T}
```
**Quirk:** text `width` prints the stored fixed width even when the width is
dynamic. That is `0` initially, or the last fixed value. Compare the item
`width`, which prints `-1` when dynamic.

POPUP(P) (`popup_serialize`):
```
P"drawing": "{on|off}",
P"horizontal": "{on|off}",
P"height": {cell height if set else -1},
P"blur_radius": {u32},
P"y_offset": {int},
P"align": "{left|right|center|bottom|top|invalid}",
P"background": {
BACKGROUND(P→, detailed)
P},
P"items": [
P→ "{name}",            <- TAB then SPACE before the quote; entries joined by ",\n"
P]
```

GRAPH(G) (`graph_serialize`):
```
G"color": "0x{line hex}",
G"fill_color": "0x{fill hex}",
G"line_width": "{%f}",         <- quoted string
G"data": [
G→"{%f}",                      <- each sample a quoted string; joined by ",\n"
G]
```
**Quirk:** `data` lists the raw ring buffer `y[0..width)`, *not* in
chronological order. A zero-width graph gives `"data": [\n\nG]`.

SLIDER(S) (`slider_serialize`):
```
S"highlight_color": "0x{hex}",
S"percentage": "{%d}",          <- quoted
S"width": "{%d}",               <- quoted
S"background": {
BACKGROUND(S→, not detailed)
S},
S"knob": {
TEXT(S→)
S}
```

### 9.2 Example: fresh `--add item foo left` with factory defaults
```
{
	"name": "foo",
	"type": "item",
	"geometry": {
		"drawing": "on",
		"position": "left",
		"associated_space_mask": 0,
		"associated_display_mask": 0,
		"ignore_association": "off",
		"y_offset": 0,
		"padding_left": 0,
		"padding_right": 0,
		"scroll_texts": "off",
		"width": -1,
		"background": {
			"drawing": "off",
			"color": "0x0",
			"border_color": "0x0",
			"border_width": 0,
			"height": 0,
			"corner_radius": 0,
			"padding_left": 0,
			"padding_right": 0,
			"x_offset": 0,
			"y_offset": 0,
			"clip": 0.000000,
			"image": {
				"value": "(null)",
				"drawing": "off",
				"scale": 1.000000
			},
			"shadow": {
				"drawing": "off",
				"color": "0xff000000",
				"angle": 30,
				"distance": 5
			}
		}
	},
	"icon": {
		"value": "",
		"drawing": "on",
		"highlight": "off",
		"color": "0xffffffff",
		"highlight_color": "0xff000000",
		"padding_left": 0,
		"padding_right": 0,
		"y_offset": 0,
		"font": "Hack Nerd Font:Bold:14.00",
		"width": 0,
		"scroll_duration": 100,
		"align": "left",
		"background": {
			"drawing": "off",
			"color": "0x0",
			"border_color": "0x0",
			"border_width": 0,
			"height": 0,
			"corner_radius": 0,
			"padding_left": 0,
			"padding_right": 0,
			"x_offset": 0,
			"y_offset": 0,
			"clip": 0.000000,
			"image": {
				"value": "(null)",
				"drawing": "off",
				"scale": 1.000000
			},
			"shadow": {
				"drawing": "off",
				"color": "0xff000000",
				"angle": 30,
				"distance": 5
			}
		},
		"shadow": {
			"drawing": "off",
			"color": "0xff000000",
			"angle": 30,
			"distance": 5
		}
	},
	"label": { …same as icon… },
	"scripting": {
		"script": "(null)",
		"click_script": "(null)",
		"update_freq": 0,
		"update_mask": 0,
		"updates": "on"
	},
	"bounding_rects": {
		"display-1": {
			"origin": [ 120.000000, 900.000000 ],
			"size": [ 2.000000, 25.000000 ]
		}
	}
}
```
The `bounding_rects` numbers are illustrative.

### 9.3 Bar JSON (`bar_manager_serialize`)
```
{
→"position": "{top|bottom}",
→"topmost": "{on|off}",
→"sticky": "{on|off}",
→"hidden": "{on|off}",
→"shadow": "{on|off}",
→"font_smoothing": "{on|off}",
→"show_in_fullscreen": "{on|off}",
→"blur_radius": {u32},
→"margin": {int},
BACKGROUND(→, not detailed),
→"items": [
→→ "{name}",                <- TAB TAB SPACE quote; joined by ",\n"; all items in bar_items order
→]
}
```
- `position` prints `"bottom"` only for `b`. Everything else, including vertical
  `l`/`r`, prints `"top"`.
- `hidden` is `any_bar_hidden`.
- The bar's background fields (`drawing`, `color`, …, `image`) appear **flattened
  at the top level**, with no `shadow` block.
- With no items: `→"items": [\n\n→]\n}\n`.
- Factory values:
  - `color` `0x44000000`, `border_color` `0xffff0000`, `height` `25`;
  - `padding_left`/`padding_right` `20`, `blur_radius` `0`, `margin` `0`;
  - `sticky` `on`, everything else `off`;
  - `drawing` `off` until `color=` is set (§5).
- `notch_*` and `display` are **not** serialized.

### 9.4 Events JSON (`custom_events_serialize`)
```
{
→"front_app_switched": {
→→"bit": 1,
→→"notification": "(null)"
→},
→"space_change": {
→→"bit": 2,
…
→"my_event": {
→→"bit": 262144,
→→"notification": "com.example.note"
→}
}
```
- Each entry is `→"{name}": {\n→→"bit": {1<<i},\n→→"notification": "{n|(null)}"\n`.
  `→},\n` follows every entry except the last; after the last comes `→}\n}\n`.
- The order is registration order (§7.1).

### 9.5 Displays JSON (`display.c:display_serialize`)
One object per active display, in arrangement order 1..N:
```
[
→{
→→"arrangement-id":{int},
→→"DirectDisplayID":{u32 as %d},
→→"UUID":"{uuid|<unknown>}",
→→"frame":{
→→"x":{%.4f},
→→"y":{%.4f},
→→"w":{%.4f},
→→"h":{%.4f}
→→}
→},          <- the last object closes with "→}\n" instead
]
```
- There is no space after the colons. The `frame` members are indented at the
  same level as `frame` itself.
- If the display list can't be obtained, the output is empty.
- On Linux, mbar maps "DirectDisplayID" to its own output id, "UUID" to a stable
  output identifier, and "frame" to the global logical geometry.

### 9.6 `default_menu_items` (`alias.c:print_all_menu_items`)
- Without screen-recording permission (macOS 11+): respond
  `[!] Query (default_menu_items): Screen Recording Permissions not given. Restart SketchyBar after granting permissions.\n`.
- Otherwise the candidates are all menu-bar-layer windows (excluding
  `Window Server`), sorted by x descending. The output is:
  ```
  [
  →"{owner},{window name}({k})", 
  →"…"
  ]
  ```
  - The separator between entries is `", \n"`: a comma, a **space**, then a
    newline.
  - `k` is the 1-based index in the sorted list. It is the same index the alias
    matcher accepts.
  - If there are no candidates, nothing is printed.
- mbar (Linux) equivalent: StatusNotifierItem/tray entries. The format should
  stay the same.

---
## 10. Config file (`hotload.c`)

### 10.1 Lookup (`get_config_file`, `exec_config_file`)
If no path was given with `--config`, the first **existing regular file**
(`stat` ok and not a directory) in this list is used:
1. `$XDG_CONFIG_HOME/<g_name>/sketchybarrc`, but only if `XDG_CONFIG_HOME` is
   set and non-empty.
2. `$HOME/.config/<g_name>/sketchybarrc`.
3. `$HOME/.sketchybarrc`. The leading dot is on the file name, and this one does
   not use `g_name`.

Notes:
- The directory name follows the bar name. The file name is always `sketchybarrc`.
- If `HOME` is unset, steps 2 and 3 are skipped.
- If nothing is found, the daemon log gets `could not locate config file..\n`
  and the bar runs with no config.
- **Quirk:** the buffer is still left holding the last path tried. A later
  `--reload` (with no path) then reports `file '<that path>' does not exist..\n`,
  and the watcher (§11) watches that path's directory.
- `--config <path>` is resolved through `realpath`, so the stored path is
  absolute and symlink-free.

### 10.2 Execution
1. If the file doesn't exist: log `file '<path>' does not exist..\n` and return.
2. `setenv("CONFIG_DIR", dirname(path))` in the daemon process. All later
   children inherit it.
3. `chdir(dirname(path))`. **The daemon's cwd, and therefore every script's cwd,
   is the config directory.**
4. Make sure the owner execute bit is set: `chmod(mode | S_IXUSR)` if missing.
   On failure: log `could not set the executable permission bit for '<path>'\n`.
5. Run the file with `fork_exec(path, NULL)`, which execs
   `/usr/bin/env sh -c <path>`.
   - **Quirk:** the path is a *shell command string*, so a path containing spaces
     or shell metacharacters breaks.
   - The config runs **asynchronously**. The daemon does not wait for it, and
     the config talks back through the IPC client (`sketchybar --bar …`).
   - Like every child, the config gets `alarm(60)`, so it is **killed by SIGALRM
     after 60 s** if still running.
     - A pending alarm survives `exec` but is *not* inherited across `fork`. So
       only the direct `sh` process is affected, for example a config that ends
       in a foreground loop or a `wait`. Processes it backgrounds are not
       affected.
     - The same 60 s limit applies to every item `script` and `click_script`.
     - mbar should reproduce this: SIGALRM default action means termination
       after 60 s for the directly spawned shell.
   - If the fork fails: log `failed to execute file '<path>'\n`.
   - All these messages go to the daemon's stdout. None of them reach any client.

---

## 11. Hotload and reload

### 11.1 Hotload (`--hotload <bool>`)
- `g_hotload` starts **off**. `--hotload on|off|toggle|…` sets it (§4.2).
- The watcher is created once at startup with
  `FSEventStream(dirname(config_path), latency 0.5 s, NoDefer|FileEvents)`.
  - It watches the **config directory recursively**.
  - Any file event under it (create, modify, delete, rename) posts `HOTLOAD` if
    `g_hotload` is on.
  - Rate limit: at most one reload per 2^30 ns (≈1.074 s). Events inside that
    window are dropped, not deferred.
- **Quirk:** the watcher is never re-pointed. After `--reload <other path>`, the
  original directory is still the one watched.
- `g_hotload` survives reloads, because it is not reset by `bar_manager_init`.
- mbar: use inotify recursively (or the `notify` crate) on the config directory
  with the same rate limit.

### 11.2 `--reload [<path>]` (Mode B)
- With a path: `realpath(path)`. If that fails, respond
  `[?] Reload: Invalid config path '<path>'\n` and **do not reload**. On success
  the stored config path is replaced.
- Then the reload runs **synchronously, inside the current request**
  (`event_hotload`):
  1. Destroy the bar manager. Every item's `mach_helper` gets `"k"`, all items
     and bars are destroyed, and custom events are dropped.
  2. Re-init. Everything returns to factory defaults: bar properties, the
     default item, the event list (built-ins only), and the animator.
  3. Recreate the bars.
  4. Run the config file (§10.2).
- Commands after `--reload` in the same request act on the fresh, empty state.
- `HOTLOAD` from the watcher does the same thing outside a request.

---
## 12. Script execution and environment

### 12.1 How a script runs (`bar_item_update`, `helpers.h:fork_exec`)
When an item update fires (§7.5) and the item has a non-empty `script` or a
`mach_helper`:

1. **Pick the env set.**
   - If the caller passed no env (routine/forced update, `mouse.entered`,
     `mouse.exited`, `*.global` enter/exit, `system_woke`, `system_will_sleep`),
     use **the item's own persistent env set**.
   - Otherwise use the caller's env. All of the item's persistent vars are
     copied into it, then `NAME=<item name>` is set.
2. **SENDER.** Set `SENDER` to the event name, or to `forced`/`routine` when
   there is no sender.
   - **Quirk:** in the no-env case this writes `SENDER` *into the item's
     persistent set*, so it later shows up in that item's `click_script` env.
3. **Run.** `fork_exec(script, env)`:
   - `vfork()`. In the child: `alarm(60)`, then `setenv(k, v, 1)` for every var,
     then `execvp("/usr/bin/env", ["/usr/bin/env", "sh", "-c", script])`.
   - The parent does not wait; `SIGCHLD` is ignored.
   - The child inherits the daemon env (including `BAR_NAME` and `CONFIG_DIR`)
     and cwd (the config dir).
   - The script string is interpreted by `sh`, so it may be any shell command
     line, for example `"$CONFIG_DIR/plugins/clock.sh"` or `echo hi`.
   - **Quirk (Darwin vfork):** the child's `setenv` calls run in the parent's
     address space. So every var set for any script (`NAME`, `SENDER`, `INFO`,
     custom trigger vars, …) **stays in the daemon's environment** and is
     inherited by every later child, including scripts of other items and
     events that don't set that var.
     - Example: a script fired by `mouse.entered` sees the `INFO` from the last
       event that set one.
     - mbar should give each child an env built from the daemon's *startup*
       environment plus the event vars. Scripts must not rely on stale vars.
4. **mach_helper.** If set, send the env as `k\0v\0k\0v\0…\0` (one extra
   trailing NUL) to the `mach_helper` port, one-way. On bar destruction (exit
   or reload) the helper gets the 2-byte message `"k\0"`.

### 12.2 Variables
| Variable | Set for | Value |
|---|---|---|
| `BAR_NAME` | everything (process env) | `g_name` (basename of argv[0]) |
| `CONFIG_DIR` | everything (process env, after the config runs) | directory of the config file |
| `NAME` | every item script, click_script, mach_helper | item name |
| `SENDER` | item scripts | event name, `routine` (update_freq tick), or `forced` (`--update`) |
| `INFO` | see the table below | event payload |
| `SELECTED` | space items | `true` / `false` |
| `SID` | space items | Mission Control index n from `space=n` (initially `0`) |
| `DID` | space items | display index from `display=n`, else `0`. Not updated by automatic display association. |
| `DID` | `mouse.scrolled.global` | arrangement index of the bar or popup that was scrolled |
| `BUTTON` | `mouse.clicked`, click_script | `left`, `right` or `other` (from the mouse-up event type) |
| `MODIFIER` | `mouse.clicked`, `mouse.scrolled[.global]`, click_script | Comma-joined subset of `shift,ctrl,alt,cmd,fn`, in that order, or `none` |
| `SCROLL_DELTA` | `mouse.scrolled[.global]` | Integer vertical delta (accumulated, see below) |
| `PERCENTAGE` | slider items, after a click or drag release | `0`–`100`. Persists in the item's env. |
| *custom* | `--trigger <ev> K=V…` | as given |

`INFO` per event:

| Event | INFO |
|---|---|
| `front_app_switched` | Localized name of the newly active app (unset if unknown). |
| `space_change` | `{\n\t"display-<adid>": <space index>,\n …}`. One line per bar, `,` after every line except the last, no trailing newline after `}`. Index 0 means unknown. |
| `display_change` | Arrangement index of the active display as decimal (at most 2 chars). |
| `volume_change` | `(int)(volume*100+0.5)`. Muted reads as 0. Fires on a change > 1%. |
| `brightness_change` | `(int)(brightness*100+0.5)` |
| `wifi_change` | SSID string (empty if none) |
| `power_source_change` | `AC` or `BATTERY` (fires only on a change) |
| `media_change` | `{\n\t"state": "playing\|paused",\n\t"title": "…",\n\t"album": "…",\n\t"artist": "…",\n\t"app": "…"\n}`. Title, album and artist are escaped as `"`→`\"` and newline→`\n`. Fires only when the info changes. |
| `space_windows_change` | `{\n\t"space": <index>,\n\t"apps": {\n\t\t"<App>": <window count>,\n …\n\t}\n}\n` |
| `mouse.clicked` / click_script | `{\n\t"button": "<left\|right\|other>",\n\t"button_code": <n>,\n\t"modifier": "<MODIFIER>",\n\t"modfier_code": <flags>\n}\n`. The key really is misspelled `modfier_code`. |
| `mouse.scrolled`, `mouse.scrolled.global` | `{\n\t"delta": <d>,\n\t"modifier": "<MODIFIER>",\n\t"modfier_code": <flags>\n}\n` |
| custom with notification | Pretty-printed JSON of the notification's userInfo (if JSON-serializable) |
| `--trigger` | Whatever `INFO=` the caller passed |
| `mouse.entered`/`exited`(`.global`), `system_woke`, `system_will_sleep` | not set (stale, see the 12.1 quirk) |

### 12.3 Mouse event semantics that affect scripts
- **Click.** On mouse-up over an item window, or over the item under the cursor
  if the window belongs to a bracket:
  1. Build `INFO`, `BUTTON` and `MODIFIER`.
  2. Slider items: only clicks inside the track (or ending a drag) count. They
     update `PERCENTAGE`. Clicks elsewhere on the slider item do nothing at all.
  3. Run `click_script` (if non-empty) with those vars plus the item's
     persistent vars (`NAME`, …). There is no explicit `SENDER`.
  4. If subscribed to `mouse.clicked`, also run `script` with `SENDER=mouse.clicked`,
     **forced** (ignores `updates=off`).
- **Scroll.** Events within 150 ms of the last delivered one are accumulated and
  not delivered. `SCROLL_DELTA` is the sum.
  - Over an item subscribed to `mouse.scrolled`, the script runs forced.
  - Over bar or popup background (no item), every subscriber of
    `mouse.scrolled.global` runs, with `DID`.
- **Enter/exit.**
  - `mouse.entered` fires once per entry (tracked by `mouse_over`). `mouse.exited`
    fires on exit. Both are forced and use no env (item vars only).
  - `mouse.entered.global` and `mouse.exited.global` fire when the pointer enters
    or leaves the union of bars and popups. Each runs subscribers non-forced.
  - Exiting globally also sends `mouse.exited` to every item subscribed to it.

### 12.4 Space items
On every `space_change`:
- **Display association.** Each space item that has no `display` override gets
  `associated_display = 1 << arrangement(display of space SID)`, or `1<<30` if
  that display is unknown.
- **Selection.** For each bar whose display bit is in the item's display mask,
  with `sid` = that bar's current space index:
  - If `space mask & (1<<sid)` and (not selected yet, or forced): `SELECTED=true`,
    `updates=on`.
  - Else if the space bit is not set and (selected, or forced): `SELECTED=false`,
    `updates=on`.
  - Else `updates=off`.
- Then `space_change` subscribers run. Space items run only if `updates` is on,
  so in practice only items whose selection changed, or all of them when forced.
- The default script `sketchybar -m --set $NAME icon.highlight=$SELECTED` sets
  the highlight.

### 12.5 `mach_helper` (event provider protocol)
- `mach_helper=<bootstrap name>` makes the daemon push the item's update env,
  serialized as `KEY\0VALUE\0…\0`, to that Mach service on every update. It does
  this whether or not there is a script.
- Exit and reload send `"k"`.
- mbar may replace this with a Unix-socket push using the same payload, or drop
  it. Either way it must accept the key.

---
## 13. Message catalog (exact strings)

### 13.1 Response messages
These go into the response and are also logged with a timestamp. `%s` is the
argument shown in the Meaning column. For the nested property errors (Text,
Background, Shadow, Image, Popup, Graph, Slider, Alias, Color), `%s` is the
**remaining key at that nesting level**: `icon.foo=1` produces
`[!] Text: Invalid property 'foo'`, and `icon.foo.bar=1` produces
`[!] Text: Invalid subdomain 'foo' `.

The client prefix rule: `[!]` gives exit 1 (if it comes first). `[?]` and
unprefixed text give exit 0.

| String | Meaning |
|---|---|
| `[!] Unknown domain '%s'\n` | Unknown command token |
| `[!] Set: Item not found '%s'\n` | `--set` target missing |
| `[!] Set (%s): Expected <key>=<value> pair, but got: '%s'\n` | item name, token |
| `[!] Set (default): Expected <key>=<value> pair, but got: '%s'\n` | `--default` |
| `[!] Bar: Expected <key>=<value> pair, but got: '%s'\n` | `--bar` |
| `[!] Bar: Invalid property 'clip'\n` | `--bar clip=…` |
| `[!] Regex: Could not compile regex '%s'\n` | the full `/…/` token |
| `[!] Regex: Regex match failed '%s'\n` | `regerror` text |
| `[?] Regex: No match found for regex '%s'\n` | the full `/…/` token |
| `[?] Add: Item '%s' already exists\n` | |
| `[?] Add %s: Invalid type '%s', assuming 'item'\n` | name, type |
| `[!] Add %s: Illegal position '%s'\n` | name, position |
| `[!] Add: Illegal name '%s'\n` | |
| `[?] Add (Group) %s: Failed to add member '%s', item not found\n` | bracket, member |
| `[!] Add (Popup) %s: Item '%s' is not a valid popup host\n` | item, host |
| `[!] Item Position (%s): Item '%s' is not a valid popup host\n` | item, host |
| `[!] Item (%s): Invalid property '%s' \n` | item, key |
| `[!] Item (%s): Invalid subdomain '%s'\n` | item, segment |
| `[!] Item (%s): Trying to set a graph property on a non-graph item\n` | |
| `[!] Item (%s): Trying to set an alias property on a non-alias item\n` | |
| `[!] Item (%s): Trying to set a slider property on a non-slider item\n` | |
| `[!] Text: Invalid property '%s'\n` | Text and font leaves |
| `[!] Text: Invalid subdomain '%s' \n` | |
| `[!] Background: Invalid property '%s'\n` | |
| `[!] Background: Invalid subdomain '%s'\n` | |
| `[!] Shadow: Invalid property '%s'\n` | |
| `[!] Shadow: Invalid subdomain '%s'\n` | |
| `[?] Image: Unknown property: %s \n` | No quotes. |
| `[?] Image: Invalid subdomain: %s \n` | No quotes. |
| `[!] Image: Invalid application name: '%s'\n` | |
| `[!] Image: Invalid Space ID: '%s'\n` | |
| `[!] Image: Invalid Image Format: '%s'\n` | |
| `[!] Image: File '%s' not found\n` | Resolved path. |
| `Could not open image file at: %s\n` | No prefix. Exit 0. |
| `[!] Popup: Invalid property '%s'\n` | |
| `[!] Popup: Invalid subdomain '%s'\n` | |
| `[!] Graph: Invalid property '%s'\n` | |
| `[!] Graph: Invalid subdomain '%s'\n` | |
| `[!] Slider: Invalid property '%s'\n` | |
| `[!] Slider: Invalid subdomain '%s' \n` | |
| `[!] Alias: Invalid property '%s' \n` | |
| `[!] Alias: Invalid subdomain '%s'\n` | |
| `[?] Color: Invalid property '%s'\n` | |
| `[!] Subscribe: Item not found '%s'\n` | |
| `[?] Event: '%s' not found\n` | `--subscribe` with an unknown event |
| `[!] Push: Item '%s' not found\n` | |
| `[!] Push: Item '%s' not a graph\n` | |
| `[!] Rename: Failed to rename item: %s -> %s\n` | |
| `[!] Clone: Parent Item '%s' not found\n` | |
| `[?] Clone: Item '%s' already exists\n` | |
| `[!] Remove: Item '%s' not found\n` | |
| `[!] Move: Item '%s' or '%s' not found\n` | |
| `[!] Order: Item '%s' not found\n` | `--reorder` |
| `[!] Query: Item '%s' not found\n` | `--query item x` |
| `[!] Query: Invalid query, or item '%s' not found \n` | `--query x` |
| `[!] Query (default_menu_items): Screen Recording Permissions not given. Restart SketchyBar after granting permissions.\n` | |
| `[?] Reload: Invalid config path '%s'\n` | |

### 13.2 Daemon-log-only messages (stdout)
- `No bar on display %u \n`
- `could not locate config file..\n`
- `file '%s' does not exist..\n`
- `could not set the executable permission bit for '%s'\n`
- `failed to execute file '%s'\n`
- `ERROR (id): No active display detected!\n` (`display.c`)

### 13.3 Process-level messages
| Stream | String | Exit |
|---|---|---|
| stderr | `%s: running as root is not allowed! abort..\n` | 1 |
| stderr | `sketchybar-msg: 'env USER' not set! abort..\n` (client mode; literal `sketchybar-msg`) | 1 |
| stderr | `%s: 'env USER' not set! abort..\n` (daemon) | 1 |
| stderr | `%s: could not create lock-file! abort..\n` | 1 |
| stderr | `%s: could not acquire lock-file... already running?\n` | 1 |
| stderr | `%s: could not initialize daemon! abort..\n` | 1 |
| stdout | `[!] Error: Too few arguments for argument 'config'.\n` | 1 |
| stdout | `[!] Error: Specified config file path invalid.\n` | 1 |
| stdout | `sketchybar-v2.24.0\n` | 0 |

Here `%s` is `g_name`.

---
## 14. Notes for the mbar implementation

1. **Tokenizer.**
   - Implement `get_token`, Mode A, Mode B and Mode C exactly as in §3.2,
     including the `-` prefix rules. Scripts in the wild depend on
     `--set a k=v --set b k=v` chaining.
   - Fixing the "first token not checked" and "empty argv ends the message"
     quirks is low-risk. If you fix them, document it here.
2. **Atomicity.**
   - One IPC request is one transaction: freeze, apply all commands in order,
     lock animations, refresh once, reply.
   - Requests are processed serially on the main loop.
3. **Response contract.**
   - Concatenate all messages in order.
   - The client's exit code is decided by `rsp[1] == '!'` on the first message.
   - The client prints to stderr on failure and to stdout otherwise, with no
     added newline.
   - Silent exit 0 if the daemon is not reachable. Consider an opt-in error
     message, but keep exit 0 by default for compatibility.
   - Keep the reply timeout (100 ms) or make it larger. **Recommendation:**
     larger. 100 ms is easily exceeded by `--query` on a busy system, and then
     queries come back empty.
4. **Numbers.** Use C-compatible `strtol`/`strtoul`/`strtof` semantics: prefix
   detection, partial parse, garbage gives 0, truncation to 32 bits (§4.1).
   Colors depend on this.
5. **Item store.**
   - An ordered `Vec` of items (`bar_items` order matters for drawing order
     inside each position, for regex iteration order, and for `--query bar`)
     plus a name → index map.
   - Store the default item as a full item value. Creation clones it, except for
     the not-inherited fields listed in §6.3.
6. **Event bits.** Keep the registration order of §7.1 so that `--query events`
   bits and `update_mask` values match SketchyBar.
7. **JSON output.** Reproduce the hand-formatted output byte-for-byte (tabs,
   `0x%x`, `%f`, `(null)`, quoted graph floats, `modfier_code`). Third-party
   tools parse it, sometimes with regex.
8. **Env for children.** Build the env explicitly per spawn: daemon base env +
   `BAR_NAME` + `CONFIG_DIR` + item vars + event vars + `SENDER`. Set cwd to the
   config dir. Exec `/usr/bin/env sh -c <cmd>`. Apply a 60 s `alarm`/kill. Do
   not wait, but reap children.

## 15. Open questions / unverified points
- **vfork env leakage (§12.1).** This assumes the Darwin `vfork` child shares
  the parent's address space, so `setenv` in the child mutates the daemon's
  `environ`. Verify empirically: does a script see an `INFO` that its event did
  not set? Decide whether mbar replicates it. The recommendation is no.
- **`--load-font`.** `CFURLCreateWithString` with a plain filesystem path
  probably produces a relative URL that CoreText cannot load. It is unclear
  whether this domain works at all with plain paths. mbar on Linux should
  accept a file path and register it with fontconfig (`FcConfigAppFontAddFile`).
- **Regex flavor.** Exact macOS BRE behavior for edge patterns (`//`, `\+`)
  needs a test on a Mac. mbar needs a BRE→Rust-regex translator, or a decision
  to use ERE.
- **`display_change` INFO.** It is truncated to 2 chars (`char adid_str[3]`).
  That is irrelevant in practice.
- **Undefined behavior in C.** These paths should be specified by mbar instead
  of copied:
  - removing the bracket after a failed first member (use-after-free);
  - `--move x before x`;
  - `display=0` on the bar;
  - more than 64 events;
  - pushing to a 0-width graph;
  - the shared graph buffer between clones.
- **Clone of a popup child.** Should the clone be inserted into the host popup?
  The C code doesn't insert it, but leaves `parent` set.
- **Linux mapping of macOS-only sources.** Distributed notifications (`--add event
  <name> <notification>`), `default_menu_items`, alias components, `space.<n>`
  and `app.<name>` images, and `mach_helper` all need a Linux design decision.
  The command syntax and query shapes should be kept regardless.
