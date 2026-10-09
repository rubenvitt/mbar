# JankyBorders (`borders`): behavioral spec

Source: JankyBorders v1.9.0 (commit `a7297ca`, 2026-05-14), by Felix Kratz. File
references are relative to the repository root. Paths under `src/` are C sources.
The man page is `docs/borders.1.scd`. This spec covers the command line, the IPC
transport, the config file, window tracking, focus detection, border window
creation and drawing, and the yabai proxy integration. With it you should be able
to reimplement `borders` in Rust with the same observable behavior.

The companion FFI reference (every private SkyLight/CGS/AX symbol with its C
signature as declared) lives in
[`borders-skylight.md`](borders-skylight.md). §12 here summarizes it.

Conventions:
- `BR-<area>-<n>` are requirement IDs. Each one states one observable behavior.
- **Quirk** marks behavior that looks accidental but is observable. The quirk index
  in §13 says whether mbar should reproduce each one.
- `w` = the effective `border_width`, `P` = `BORDER_PADDING` = 8.0.
- "SLS event N" means a SkyLight notify-proc event registered with
  `SLSRegisterNotifyProc(handler, N, ctx)`.
- "Main" means the main thread and its CFRunLoop / main dispatch queue. Everything
  runs there unless stated otherwise.
- `stdout`/`stderr` texts are exact. `\n` is a newline.

---

## 1. Overview and process model

`borders` is one binary that runs in one of two roles:

- **Primary (daemon)**: draws the borders. It registers the Mach bootstrap service
  `git.felix.borders` and runs a CFRunLoop forever.
- **Client**: started while a primary is registered. It forwards its argv to the
  primary over Mach IPC and exits 0.

There is no lock file, no PID file and no signal handling. The bootstrap name is
the only thing that keeps a single instance (§3.4).

Version: `MAJOR 1`, `MINOR 9`, `PATCH 0` (`src/main.c:20-22`). Supported OS per the
README/man page: macOS 14.0+. Some behavior depends on macOS 26 at **runtime**
(`__builtin_available(macOS 26.0, *)`), and one constant depends on the **SDK at
compile time** (§7.5, `BORDER_TSMW`).

Build (`makefile`): `clang -std=c99 -O3 -g` over `main.c parse.c mach.c hashtable.c
events.c windows.c border.c animation.c`, linked with `-framework AppKit
-framework CoreVideo -F/System/Library/PrivateFrameworks -framework SkyLight`. A
`debug` target (`-O0 -g -DDEBUG`, output `bin/debug`) enables `debug()` printing
to stdout (`src/misc/helpers.h:15-22`) and an event watcher for all SLS events
0..1999 (`src/events.c:129-133`). An `asan` target builds with ASan/UBSan and runs
the result. Release builds print nothing from `debug()`.

**BR-CLI-00 (asserts are live).** No target defines `NDEBUG`, so every `assert`
is active in release builds and aborts the daemon (SIGABRT, message on stderr)
when it fails:
- `border_get_settings`: must run on the main thread (`border.c:11`)
- `window_create`: `frame_region != NULL` and the new `wid != 0`
  (`window.h:235,263`), so a failed `SLSNewWindow` kills the daemon
- `animation_start`: no link and no context already set (`animation.c:11-12`)

mbar: treat these as hard errors or log and skip, but do not continue with a
zero wid.

---

## 2. Command line

### 2.1 Startup sequence (`src/main.c:171-237`)

**BR-CLI-01.** If `argc > 1` and `argv[1]` is exactly `--version` or `-v`, print
`borders-v1.9.0\n` to stdout and exit 0 (`main.c:172-176`). Only `argv[1]` is
checked. In any other position these strings are ordinary (invalid) arguments.

**BR-CLI-02.** If `argc > 1` and `argv[1]` is exactly `--help` or `-h`, print
`Refer to the man page for help: man borders\n` to stdout and exit 0
(`main.c:178-182`). Version is checked first.

**BR-CLI-03.** Otherwise, in this order:
1. Init the blacklist and whitelist tables (string keys, capacity 64)
   (`main.c:184-185`).
2. `g_settings.ax_focus = AXIsProcessTrusted()`. This is the silent trust check
   (`main.c:186`, `src/misc/ax.h:8-9`), so `ax_focus` defaults to on when the
   process (in practice its responsible parent, such as the terminal) has
   Accessibility permission. The silent path does **not** set the cached
   `g_ax_trust` flag (§6.3).
3. `update_mask = parse_settings(&g_settings, argc-1, argv+1)` (§2.3). This runs
   in **both** roles, so invalid-argument messages print in the client too.
4. Look up the bootstrap service `git.felix.borders` (§3.1).
   - Found and `update_mask != 0`: forward argv (§3.2), then exit 0
     (`main.c:190-192`).
   - Found and `update_mask == 0`: print to **stderr**
     `A borders instance is already running and no valid arguments where provided. To modify properties of the running instance provide them as arguments.\n`
     and exit 1 (EXIT_FAILURE) (`main.c:193-197`, `helpers.h:24-30`). The typo
     "where" is in the original.
   - Not found: continue as primary.
5. Primary only:
   1. `load_symbols()`: on macOS ≥ 26 at runtime, `dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_LAZY|RTLD_LOCAL)`
      and `dlsym("SLSWindowIteratorGetCornerRadii")`. Otherwise the symbol stays
      NULL (`main.c:162-169`).
   2. `g_pid = pid_for_task(mach_task_self())`, which is our own pid
      (`main.c:200`).
   3. Window table: u32 keys, capacity 1024 (`main.c:201`).
   4. `g_server_port = CGSGetConnectionPortById(SLSMainConnectionID())`, which
      is the WindowServer port used for the raw sub-level MIG call (§7.6).
      `CGSGetConnectionPortById` is not exported. It is found by scanning
      SkyLight's Mach-O `LC_SYMTAB` for `_CGSGetConnectionPortById` in the
      loaded image whose name is exactly
      `/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight`
      (`src/misc/connection.h:62-101`). **Quirk:** if the lookup fails, it calls
      a NULL function pointer and crashes.
   5. `events_register(main_cid)` (§5.1).
   6. `SLSGetEventPort(cid, &port)`. On success, wrap the port in a CFMachPort
      with `_CFMachPortSetOptions(port, 0x40)` and add it to the current run loop
      in default mode. Its callback drains the SLS event queue with
      `SLEventCreateNextEvent(cid)` until that returns NULL, releasing each event
      (`main.c:152-160, 205-225`). This keeps the connection's event queue from
      filling up. Notify-proc callbacks are delivered on main.
   7. `windows_add_existing_windows()` (§5.3).
   8. Start the Mach server (§3.3).
   9. If `update_mask == 0` (no **valid** argument was given, including the case
      of only invalid ones), run the config file (§4) (`main.c:230`).
   10. Register the yabai port (§10). This is always compiled in, because
       `src/misc/yabai.h:3` defines `_YABAI_INTEGRATION`.
   11. `CFRunLoopRun()` forever.

**BR-CLI-04.** **Quirk:** the primary does not determine the focused window at
startup. Every border starts inactive (`focused=false`) until the first event that
triggers focus detection (§6.2). Event 723 or 1508 usually fires quickly.

**BR-CLI-05.** The primary applies its own argv settings (step 3) before any
borders are created, so the first draw already uses them.

### 2.2 Settings and defaults (`src/main.c:31-46`, `src/border.h:31-53`)

| Field | Default | Set by |
|---|---|---|
| `enabled` | `true` (only meaningful on per-window overrides, §3.5) | internal |
| `apply_to` | `0` | `apply-to=` |
| `active_window` | solid `0xffe1e3e4` | `active_color=` |
| `inactive_window` | solid `0x00000000` (invisible) | `inactive_color=` |
| `background` | solid `0x00000000` | `background_color=` |
| `show_background` | `false` | derived from `background_color` |
| `border_width` | `4.0` (f32) | `width=` |
| `blur_radius` | `0` | **nothing**: the field exists but no parser sets it and it is never read |
| `border_style` | `'r'` (round) | `style=` |
| `hidpi` | `false` | `hidpi=` |
| `border_order` | `-1` (below) | `order=` |
| `ax_focus` | `AXIsProcessTrusted()` at startup | `ax_focus=` |
| `blacklist_enabled` / `blacklist` | `false` / empty | `blacklist=` |
| `whitelist_enabled` / `whitelist` | `false` / empty | `whitelist=` |
| `corner_mask` | zeroed | nothing (unused) |

Color styles (`border.h:23-29`): `SOLID{color}`, `GLOW{color}`,
`GRADIENT{direction, color1, color2}` with direction `TL_TO_BR=0` or `TR_TO_BL=1`
(`src/misc/drawing.h:4-8`). In C this is a union. `color` aliases
`gradient.direction`, not `color1` (see BR-PARSE-07).

### 2.3 Argument grammar (`src/parse.c:61-137`)

Each argument is matched against the rules below **in this order**. The first
match wins and there is no fallthrough after a match. `update_mask` is the OR of
the bits each matched rule sets:

| Bit | Name (`src/parse.h:4-10`) |
|---|---|
| `1<<0` | `ACTIVE` |
| `1<<1` | `INACTIVE` |
| `ACTIVE\|INACTIVE` | `ALL` |
| `1<<2` | `RECREATE_ALL` |
| `1<<3` | `SETTING` |

| # | Match | Effect | Mask |
|---|---|---|---|
| 1 | prefix `active_color` (no `=` required) | `parse_color(active_window, rest)`. Mask only on success | `ACTIVE` |
| 2 | prefix `inactive_color` | same for `inactive_window` | `INACTIVE` |
| 3 | prefix `background_color` | same for `background`. On success, `show_background = (background.color & 0xff000000) != 0` | `ALL` |
| 4 | prefix `blacklist=` | `blacklist_enabled = parse_list(blacklist, rest)` | `RECREATE_ALL` (always, even when the list is empty) |
| 5 | prefix `whitelist=` | same for `whitelist` | `RECREATE_ALL` (always) |
| 6 | `sscanf("width=%f") == 1` | `border_width = value` | `ALL` |
| 7 | `sscanf("order=%c") == 1` | first char `'a'` → `border_order = +1` (above). **Any other char** → `-1` (below) | `ALL` |
| 8 | `sscanf("style=%c") == 1` | `border_style = first char` | `ALL` |
| 9 | exactly `hidpi=on` | `hidpi = true` | `RECREATE_ALL` |
| 10 | exactly `hidpi=off` | `hidpi = false` | `RECREATE_ALL` |
| 11 | exactly `ax_focus=on` | `ax_focus = true` | `SETTING` |
| 12 | exactly `ax_focus=off` | `ax_focus = false` | `SETTING` |
| 13 | `sscanf("apply-to=%d") == 1` | `apply_to = (u32)value` | `SETTING` |
| 14 | anything else | stdout `[?] Borders: Invalid argument '<arg>'\n` | none |

**BR-PARSE-01 (prefix rules).** Rules 1–3 match on the bare key prefix. For
example `active_colorful=…` matches rule 1 with `rest = "ful=…"`, fails color
parsing, prints the color error (BR-PARSE-05), and does **not** fall through to
"Invalid argument". `inactive_color…` does not start with `active_color`, so
rules 1 and 2 never collide.

**BR-PARSE-02 (sscanf semantics).** All `%`-conversions follow C `sscanf`. mbar
must reproduce them:
- Trailing garbage is ignored: `width=5px` → 5.0, `apply-to=12abc` → 12,
  `order=above` → `'a'`.
- `%f` accepts leading whitespace, a sign, exponents, hex floats, `inf`, `nan`.
  So `width=-3` and `width=inf` are accepted with no range check.
- `%c` reads exactly one char, which may be whitespace (`style= ` → `' '`).
  `key=` with nothing after it gives `EOF` (≠1), so the next rule is tried and
  the argument ends at rule 14 ("Invalid argument").
- `%d` into a `uint32_t`: a negative value wraps (`apply-to=-1` → `0xffffffff`).
- `%x` accepts leading whitespace, an optional sign and an **optional second
  `0x` prefix** (`=0x0xff00ff00` parses). Upper/lower-case hex digits are fine.
  On overflow beyond 32 bits, Apple libc `strtoul` → `unsigned` truncation gives
  the low bits (implementation-defined). mbar: take the low 32 bits of the
  saturated `strtoul` result (`ULONG_MAX` → `0xffffffff`).

**BR-PARSE-03 (style).** Only the first char after `style=` matters:
- `'s'` → square
- `'u'` → round-uniform
- `'r'` → round

**Any other char** (such as `style=Round` → `'R'`, or `style=x`) is stored as-is
and behaves like round, because drawing only tests `== 's'` and `== 'u'`
(`border.c:84,103,118-120`). The man page documents `round`, `square` and
`uniform`.

**BR-PARSE-04 (order).** `order=` is undocumented. `order=a…` → above. Any other
first char → below (`order=b`, `order=x`). `apply-to=` (BR-IPC-07) is also
missing from the man page and README.

**BR-PARSE-05 (colors, `parse_color`, `parse.c:31-59`).** `token` is the text
after the key, **including** the leading `=`. Try in order:
1. `sscanf(token, "=0x%x")` → SOLID.
2. `sscanf(token, "=glow(0x%x)")` → GLOW. The closing `)` is not checked
   (`=glow(0xff00ff00` parses). The same holds for the closing `)` of both
   gradient forms below, and anything after the last `%x` is ignored.
3. `sscanf(token, "=gradient(top_left=0x%x,bottom_right=0x%x)") == 2` →
   GRADIENT with `TL_TO_BR`, `color1 = top_left`, `color2 = bottom_right`.
4. `sscanf(token, "=gradient(top_right=0x%x,bottom_left=0x%x)") == 2` →
   GRADIENT with `TR_TO_BL`, `color1 = top_right`, `color2 = bottom_left`.
5. Otherwise stdout `[?] Borders: Invalid color argument color<token>\n` (for
   example `active_color=red` → `[?] Borders: Invalid color argument color=red`)
   and fail.

Colors are `0xAARRGGBB`. There is no named-color or `#rrggbb` syntax. Gradient
keys must appear in exactly this order with no spaces (`gradient(bottom_right=…,top_left=…)`
is invalid).

**BR-PARSE-06.** **Quirk:** a failed gradient match can still write partially.
If step 3 matched only the first `%x` (for example
`gradient(top_left=0x11111111,bottom_left=…)`), `gradient.color1` is already
overwritten even though parsing fails and `stype` is unchanged. Because the C
struct is a union, this changes nothing visible for SOLID/GLOW (`color` aliases
`direction`). For an existing GRADIENT it silently changes `color1`, and the
change shows at the next redraw. mbar: optional, see Q-table.

**BR-PARSE-07 (background with gradient).** `background_color=gradient(…)`
parses successfully (mask `ALL`). `show_background` is then computed from the
`color` union member, which aliases `direction` (0 or 1), so it is **false** and
no background is drawn. `background_color=glow(0xAARRGGBB)` sets
`show_background` from the alpha and draws as a plain solid fill without glow
(§7.4). The man page says "only 0xAARRGGBB arguments supported".

**BR-PARSE-08 (lists, `parse_list`, `parse.c:12-29`).**
1. **Clear** the table first. Each `blacklist=` replaces the list; it never
   appends.
2. Split the value on `,` with `strsep`.
3. Skip empty entries.
4. Insert every other entry **verbatim**: no whitespace trimming, and matching
   is case-sensitive and exact. So `"Safari, kitty"` stores `" kitty"`.

The function returns whether at least one entry was added. That result becomes
`*_enabled`, so `blacklist=` or `blacklist=,,` disables the list. Entries are
matched against the **BSD process name** (§5.4), not the localized app name.

**BR-PARSE-09.** Several valid arguments may appear in one invocation. They are
processed left to right and later ones override earlier ones. The
`background_color` and `apply-to` effects are per-argument.

**BR-PARSE-10 (unsupported options).** Common keys from other border tools, such
as `blur_radius=`, `glow=`, `animate=` and `--config`, all print "Invalid
argument". Glow exists only as a color syntax.

---

## 3. IPC

### 3.1 Service lookup (`src/mach.c:5-22`)
**BR-IPC-01.** Use `task_get_special_port(TASK_BOOTSTRAP_PORT)`, then
`bootstrap_look_up(bs, "git.felix.borders", &port)`. Any failure means "no
server" (port 0).

### 3.2 Client message (`src/main.c:130-150`, `src/mach.c:25-52`)
**BR-IPC-02 (payload).** The payload is every `argv[1..argc-1]`, **including
invalid ones**, each followed by `\0`, and then one extra `\0`. The declared
length is `argc + Σ(strlen(argv[i]) + 1)`. That is `argc − 1` bytes more than
were written, and the trailing bytes are uninitialized stack memory.
**Quirk:** the receiver stops at the first empty string, so the garbage is
ignored. mbar may send exactly `Σ(len+1) + 1` bytes.

**Quirk (empty argument truncates the message).** An empty argv element (for
example `borders "" width=3`) is forwarded as an empty string. The client's own
parse prints `Invalid argument ''` but still forwards because `width=3` is valid.
The primary stops at that empty string and applies **nothing** after it. mbar:
either reproduce this or skip empty arguments on the client side (document it).

**BR-IPC-03 (Mach message).** `struct mach_message` (`src/mach.h:6-10`), 44 bytes
on 64-bit:
- `mach_msg_header_t` (24 bytes):
  - `msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND, 0, 0, MACH_MSGH_BITS_COMPLEX)`
  - `msgh_size = 44`
  - `msgh_remote_port = server`
  - local port null, so no reply is expected
- `mach_msg_size_t msgh_descriptor_count = 1`
- one `mach_msg_ool_descriptor_t`:
  - `address` = payload, `size` = declared length
  - `copy = MACH_MSG_VIRTUAL_COPY`, `deallocate = false`
  - `type = MACH_MSG_OOL_DESCRIPTOR`

Send with `mach_msg(MACH_SEND_MSG, size 44, timeout NONE)`. The send result is
ignored. The client exits 0 even when the send fails (for example a stale
service).

### 3.3 Server (`src/mach.c:81-130`)
**BR-IPC-04.** Allocate a receive right and set `mpl_qlimit = MACH_PORT_QLIMIT_LARGE`
(1024). Insert a send right and call `bootstrap_register(bs, "git.felix.borders", port)`.
It is deprecated, but used anyway. Wrap the port in a CFMachPort source on the
**main** run loop, default mode. If any step fails, `mach_server_begin` returns
false. That result is ignored, so the primary keeps running without IPC.
**Quirk.**

**BR-IPC-05.** The callback calls `handler(descriptor.address, descriptor.size)`
and then `mach_msg_destroy(header)`, which frees the OOL region. The handler
ignores `len`. Unlike the yabai port (BR-YAB-02), there is **no** check of the
message size, the complex bit or the descriptor count, so any sender on the
service name can make the daemon read through an arbitrary pointer. mbar:
validate size, `MACH_MSGH_BITS_COMPLEX`, descriptor type and NUL termination
within `descriptor.size`.

### 3.4 Single instance
**BR-IPC-06.** Single-instance behavior comes only from the bootstrap name. The
check and the registration are not atomic. Two primaries started together can
both pass the lookup. The second `bootstrap_register` then fails silently, and
that process draws borders without IPC. **Quirk.** There is no lock file.

### 3.5 Applying a message (`message_handler`, `src/main.c:70-128`)
**BR-IPC-07.** On receipt:
1. `settings = g_settings` (copied by value).
2. For each NUL-separated string, in order: `mask |= parse_settings(&settings, 1, &str)`.
   Invalid arguments print to the **primary's** stdout as well as the client's.
3. **Per-window path:** if `settings.apply_to > 0`:
   - Look up the border whose **target window id** equals `apply_to`.
   - If found: `border.setting_override = settings` (the full current global
     settings plus this message), set `override.enabled = true` and
     `needs_redraw = true`, then `border_update`.
   - Found or not, **return**. The global settings are not touched, `mask` is
     ignored, and there is no recreate or bulk update.
4. **Global path:** otherwise:
   1. `g_settings = settings`.
   2. For every border with `override.enabled`, re-parse the **same message**
      into its override. This prints invalid-argument messages again, once per
      overridden border.
   3. If that re-parse produced a non-zero mask **and** the global mask has
      neither any `ALL` bit nor `RECREATE_ALL`, set `needs_redraw` and call
      `border_update` on that border. The test is literally
      `!((mask & ALL) || (mask & RECREATE_ALL))`, and `mask & ALL` is non-zero
      for `ACTIVE`-only or `INACTIVE`-only masks too. So the per-override update
      fires only for a `SETTING`-only global mask (`ax_focus=` or `apply-to=0`).
      Otherwise the bulk step below covers it.
5. Bulk action by priority:
   - `RECREATE_ALL` → recreate all borders (§5.6)
   - else `ALL` → redraw all
   - else `ACTIVE` → redraw focused borders
   - else `INACTIVE` → redraw unfocused borders
   - `SETTING` alone → nothing visible (the new `ax_focus` applies at the next
     focus detection)

   Each "redraw" sets `needs_redraw = true` and calls `border_update(border)`
   (`src/windows.c:114-160`).

**BR-IPC-08 (overrides).** An override holds a whole copy of the settings. Later
global messages are re-parsed into it, so a global change also changes
overridden windows for the keys that message contains. Keys set only through
`apply-to` keep their per-window value. Overrides are lost when the border is
destroyed or recreated (window closed, `hidpi`/`blacklist`/`whitelist` change).
**Quirk.**

**BR-IPC-09 (apply-to quirks).**
- `apply-to=<wid>` combined with `hidpi=…`, `blacklist=…` or `whitelist=…`
  stores the value in the override but does not recreate anything. Hidpi only
  takes effect when the border window is created, so it usually has no effect.
  Exception: if the border has no window yet (target on a hidden space), the
  later lazy creation (BR-DRW-04 step 6) does use the override's `hidpi`. Lists
  are only read from `g_settings` (§5.4).
- `ax_focus` in an override is ignored, because focus detection reads
  `g_settings.ax_focus`.
- `apply-to=0` takes the global path.
- **Quirk (sticky apply-to):** if the **primary** was started with
  `apply-to=N`, then `g_settings.apply_to = N`. From then on every message
  without its own `apply-to=` copies `apply_to = N` and is handled as a
  per-window message for window N, so no global change goes through. The only
  way out is a message that contains `apply-to=0`: it takes the global path and
  stores `apply_to = 0` in `g_settings`, together with the other keys of that
  message.
- **Memory-safety quirk:** the C struct copy shares the hash-table bucket
  pointers. `blacklist=`/`whitelist=` in any message frees the table that
  overrides (and, on the `apply-to` path, `g_settings`) still point to, which is
  undefined behavior. On the global path, the re-parse into each override then
  calls `table_clear` on the already-freed buckets (a double free), so the
  daemon will likely crash when a list is changed while any override exists. mbar:
  give each override its own list copy, or don't store lists in overrides. Not
  reproduced.

---

## 4. Config file (`src/misc/helpers.h:49-78`)

**BR-CFG-01.** The config file runs only in the primary, and only when the
primary's own argv contained no valid argument (`update_mask == 0`). It runs
after the Mach server has started, so `borders …` calls inside it reach the
primary as clients.

**BR-CFG-02 (lookup).**
1. If `$HOME` is unset, skip silently.
2. Try `"$HOME/.config/borders/bordersrc"`, then `"$HOME/.bordersrc"`.
3. A candidate "exists" if `stat` succeeds and `!(st_mode & S_IFDIR)`. This is a
   bitwise test, so block devices and sockets, which share that bit, also count
   as missing.
4. `$XDG_CONFIG_HOME` is **not** consulted.
5. If neither exists, nothing happens (in debug builds the message is
   `No config file found...`).

**BR-CFG-03 (exec bit).** If `S_IXUSR` is not set, `chmod(path, mode | S_IXUSR)`.
On failure, stdout `[!] Failed to make config at '<path>' executable...\n`, and
do not run the file.

**BR-CFG-04 (exec).**
1. `signal(SIGCHLD, SIG_IGN)` and `signal(SIGPIPE, SIG_IGN)` in the primary. The
   change is permanent, and children are reaped automatically.
2. `fork()`. The parent returns immediately and does not wait.
3. In the child: `alarm(60)`, then `execvp("/usr/bin/env", ["/usr/bin/env","sh","-c",<path>])`,
   then `exit(<execvp result>)`.

So the file is run by `sh -c` with the path as a **command string**. **Quirk:**
paths with spaces or shell metacharacters break, and the shebang is honored
because sh executes the file. The `alarm(60)` survives `exec`, so the
interpreter gets `SIGALRM` and is killed after 60 s unless it handles the
signal. The child inherits the primary's stdout/stderr.

---

## 5. Window tracking

### 5.1 Registered SkyLight events (`src/events.h`, `src/events.c:110-134`)

All events use `context = (void*)(intptr_t)SLSMainConnectionID()`. The callback
signature is `void (*)(uint32_t event, void* data, size_t len, void* context)`.

| Event | Name in source | Payload (as read) | Handler → action |
|---|---|---|---|
| 723 | `EVENT_WINDOW_UPDATE` | `u32 wid` | if not own: focus detection after **50 ms** |
| 804 | `EVENT_WINDOW_CLOSE` | `u32 wid` | if not own: `windows_window_destroy(wid, sid=0)` (unconditional) |
| 806 | `EVENT_WINDOW_MOVE` | `u32 wid` | if not own: `border_move` (§7.7) |
| 807 | `EVENT_WINDOW_RESIZE` | `u32 wid` | if not own: `border_update` |
| 808 | `EVENT_WINDOW_REORDER` | `u32 wid` | if not own: `border_update`, then focus detection after **10 ms** |
| 811 | `EVENT_WINDOW_LEVEL` | `u32 wid` | if not own: `border_update` |
| 815 | `EVENT_WINDOW_UNHIDE` | `u32 wid` | if not own: `border_unhide` |
| 816 | `EVENT_WINDOW_HIDE` | `u32 wid` | if not own: `border_hide` |
| 1322 | `EVENT_WINDOW_TITLE` | `u32 wid` | if not own: focus detection after **50 ms** |
| 1325 | `EVENT_WINDOW_CREATE` | `{u64 sid; u32 wid}` | §5.5 |
| 1326 | `EVENT_WINDOW_DESTROY` | `{u64 sid; u32 wid}` | §5.5 |
| 1401 | `EVENT_SPACE_CHANGE` | ignored | after **20 ms**: `windows_draw_borders_on_current_spaces` (§5.7) |
| 1508 | `EVENT_FRONT_CHANGE` | ignored | after **50 ms**: focus detection |

For 806, 807, 808, 811, 815 and 816 the per-window action runs only if a border
exists for `wid` (`table_find`). Otherwise the event does nothing, except that
808 still schedules the 10 ms focus detection.

**BR-EV-01 (own windows).** "Own" means `SLSGetWindowOwner(main_cid, wid)` →
`SLSConnectionGetPID` → pid == `g_pid` (`events.c:30-36`). Border windows belong
to per-border connections (§7.1) of the same process, so they count as own. The
check costs two SLS calls per event.

**BR-EV-02 (delays).** "After N ms" is
`DELAY_ASYNC_EXEC_ON_MAIN_THREAD` (`helpers.h:6-13`):
1. `dispatch_async` to the LOW-priority global queue.
2. `usleep(N*1000)`.
3. `dispatch_async` to main.

There is **no coalescing**. Every event schedules its own delayed run, so a burst
of 723 events gives a burst of focus detections. The `event_buffer` struct
(`border.h:55-59`) is unused; only `yabai.h:115,118` writes it.

**BR-EV-03 (per-window notification subscription).** After every
`windows_window_create` that reaches a suitable window (even when it reuses an
existing border), after every destroy, after recreate-all's clear, and after the
initial scan,
`SLSRequestNotificationsForWindows(main_cid, list, count)` is called with
**all** keys of the window table (`windows.c:221-238`). SkyLight delivers events
804–816 (and probably 723/1322) only for windows in that list. That is
inference from yabai and SketchyBar practice, not from this source. **Quirk:** the
list is a 1024-entry stack array with no bounds check, so more than 1024 borders
overflows it. mbar: no limit.

**BR-EV-04.** There is no handler for display reconfiguration, sleep/wake,
minimize, app hide/unhide or app launch/terminate. Those cases are covered only
indirectly, through the events above and the `SLSWindowIsOrderedIn` check in
`border_update` (§7.2). When a window is minimized or its app is hidden, the
window is ordered out. Any later update of that window hides its border, and
SkyLight 816/1326 presumably fire too (Open Question 2).

### 5.2 Window suitability (`src/misc/window.h:13-26`)

**BR-WIN-01.** For a SkyLight window-query iterator, a window is *suitable* iff
all of these hold:
- `parent_wid == 0`
- `(attributes & 0x2) != 0 || (tags & (1<<58)) != 0` (`0x400000000000000`)
- `!(tags & (1<<7))` (ATTACHED)
- `!(tags & (1<<18))` (IGNORES_CYCLE)
- `(tags & (1<<0))` (DOCUMENT) `|| ((tags & (1<<1))` (FLOATING) `&& (tags & (1<<31)))` (MODAL)

Window level is **not** filtered, so suitable windows at any level get borders,
and the border copies the level. There is no size or alpha filter. Menus,
popovers, tooltips and sheets are excluded by the parent and tag rules. The
"sticky" tag `1<<11` doesn't affect suitability.

### 5.3 Initial scan (`windows_add_existing_windows`, `src/windows.c:310-376`)

**BR-WIN-02.**
1. `SLSCopyManagedDisplaySpaces(cid)`. For each display dictionary, read
   `"Spaces"`, and for each space read `"id64"`. Collect all space ids of all
   displays (fullscreen spaces included).
2. `SLSCopyWindowsWithOptionsAndTags(cid, owner=0, spaces, options=0x2, &set_tags=1, &clear_tags=0)`.
3. Run `SLSWindowQueryWindows(cid, list, 0)` → `SLSWindowQueryResultCopyWindows`
   and iterate.
4. For each suitable window, call `windows_window_create(wid, window_space_id(wid))`.
5. Call `SLSRequestNotificationsForWindows` once more.

Borders on non-visible spaces get a border object but **no window** yet (§7.2
step 3). The window is created lazily when their space becomes visible.

### 5.4 Creating a border (`windows_window_create`, `src/windows.c:31-92`)

**BR-WIN-03.** Steps:
1. Get the owner: `SLSGetWindowOwner(main_cid, wid)` → `SLSConnectionGetPID` → `pid`.
2. Get the name: `proc_name(pid, static buf, PROC_PIDPATHINFO_MAXSIZE)`. This is
   the BSD short name (`pbsi_name`, at most 32 chars, else `p_comm`, 16 chars),
   **not** the bundle display name. Example: `Safari`, `kitty`, `Code Helper`.
   **Quirk:** the buffer is `static` and its return value is ignored. If
   `proc_name` fails, the previous window's name is reused.
3. Reject when `pid == g_pid` or the app is not allowed by the **global**
   lists:
   - whitelist enabled and name not in whitelist → reject
   - else blacklist enabled and name in blacklist → reject
4. Query the single window. If the iterator count > 0, it advances, and the
   window is suitable:
   1. If no border exists for `wid`, `border_create()` (§7.1), insert it, and
      `created = true`. An existing border is reused and `created` stays
      false.
   2. Corner radius:
      - If `SLSWindowIteratorGetCornerRadii` was resolved (macOS 26+), take
        element 0 of the returned CFArray as SInt32. Release the array.
      - If the radius is ≤ 0, use **9**.
      - `border.radius = r` and `border.inner_radius = r + 1`.
   3. `target_wid = wid`, `sid = <sid argument>` (this overwrites any existing
      value).
   4. `border_update(border)` runs synchronously.
   5. `windows_update_notifications()`.
5. Return `created`.

### 5.5 Create / destroy events (`window_spawn_handler`, `events.c:38-56`)

**BR-WIN-04.** Ignore the event if `wid == 0`, `sid == 0` or the window is own.
- **1325:** if `windows_window_create(wid, sid)` returns true (a new border),
  run focus detection **immediately**, with no delay.
- **1326:** `windows_window_destroy(wid, sid)`, then **always** run focus
  detection immediately.

**BR-WIN-05 (`windows_window_destroy`, `windows.c:210-219`).** Destroy only if
a border exists and one of these holds:
- `border.sid == sid`
- `border.sticky`
- `sid == 0` (event 804)

On destroy: remove the border from the table, `border_destroy` (§7.8), then
`windows_update_notifications`. **Quirk:** the parameter is `uint32_t sid`, so a
64-bit space id is truncated before the comparison. Real space ids are small, so
this is harmless. mbar: compare u64.
Rationale (inference): SkyLight sends 1326/1325 when a window leaves or enters a
space. The sid check keeps a "destroyed on space A" event from removing a border
that has already been re-homed to space B.

### 5.6 Recreate all (`windows.c:94-112`)
**BR-WIN-06.** On `RECREATE_ALL`:
1. Destroy every border.
2. Clear the table.
3. `SLSRequestNotificationsForWindows` with an empty list.
4. Run `windows_add_existing_windows`.

**Quirk:** focus is not re-determined, so every border is inactive until the
next focus event. Overrides are lost.

### 5.7 Space change (`windows_draw_borders_on_current_spaces`, `windows.c:257-308`)
**BR-WIN-07.** 20 ms after SLS 1401 (the comment says some native-fullscreen
windows update their space id late):
1. Take the **current** space of every managed display
   (`SLSCopyManagedDisplays` + `SLSManagedDisplayGetCurrentSpace`).
2. List windows with the same call as in §5.3.
3. For each suitable window:
   - a border exists → `border_update`
   - no border → `windows_window_create(wid, window_space_id(wid))`

Nothing is hidden explicitly. Border windows live in their target's space, so
WindowServer hides them together with it. No focus detection follows.
**Quirk:** unlike `is_space_visible`, this path does not check
`SLSCopyManagedDisplays` for NULL (`CFArrayGetCount(NULL)` crashes). mbar:
treat NULL as "no displays".

### 5.8 `window_space_id` (`src/misc/window.h:98-132`)
**BR-WIN-08.**
1. `SLSCopySpacesForWindows(cid, 0x7, [wid])`. If the result is non-empty,
   return element 0.
2. If that gives 0, use `SLSCopyManagedDisplayForWindow(cid, wid)` →
   `SLSManagedDisplayGetCurrentSpace`.
3. Otherwise return 0.

### 5.9 Space visibility (`src/misc/space.h:32-47`)
**BR-WIN-09.** `is_space_visible(sid)` is true iff `sid` equals the current
space of some managed display. If `SLSCopyManagedDisplays` returns NULL, the
result is false.

---

## 6. Focus detection

### 6.1 Applying focus (`windows_window_focus`, `windows.c:167-193`)
**BR-FOC-01.** For a front window id `F`, go over every border:
- focused and `target_wid != F` → `focused = false`, `needs_redraw`,
  `border_update`
- not focused and `target_wid == F` → `focused = true`, `needs_redraw`,
  `border_update`

The function returns whether some border has `target_wid == F`.

### 6.2 `windows_determine_and_focus_active_window` (`windows.c:240-255`)
**BR-FOC-02.**
1. `F = g_settings.ax_focus ? ax_get_front_window() : get_front_window()`.
2. Apply §6.1.
3. If no border matched and `F != 0`, try
   `windows_window_create(F, window_space_id(F))`. If that **newly** created a
   border, apply §6.1 again.

If `F == 0`, or `F` is unsuitable or filtered, every border ends up unfocused.

**Triggers:**
- SLS 1508, 1322, 723: 50 ms delay
- SLS 808: 10 ms delay
- 1325 that created a border: immediate
- 1326: immediate

### 6.3 Front window, SkyLight path (`get_front_window`, `window.h:49-96`)
**BR-FOC-03.**
1. `active_sid = get_active_space_id()` (`space.h:4-30`):
   - If `CGGetActiveDisplayList` reports exactly 1 display, take its UUID
     (`CGDisplayCreateUUIDFromDisplayID` → `CFUUIDCreateString`). If the second
     call doesn't return 1 display, print stdout
     `[!] ERROR (id): No active display detected!\n` and return 0.
   - Otherwise use `SLSCopyActiveMenuBarDisplayIdentifier(cid)`.
   - Then `SLSManagedDisplayGetCurrentSpace(cid, uuid)`.
2. `_SLPSGetFrontProcess(&psn)` → `SLSGetConnectionIDForPSN(cid, &psn, &target_cid)`.
3. `SLSCopyWindowsWithOptionsAndTags(cid, owner=target_cid, [active_sid], 0x2, &1, &0)`.
4. Query, iterate, and return the **first** suitable window. The list is in
   SkyLight's order, which is front-to-back. If none is found, return 0.

### 6.4 Front window, AX path (`ax_get_front_window`, `src/misc/ax.h:19-40`)
**BR-FOC-04.**
1. Trust check: if the cached `g_ax_trust` is false, call
   `AXIsProcessTrustedWithOptions({kAXTrustedCheckOptionPrompt: true})`. This
   may show the system prompt. Cache the result.
2. If still untrusted: stderr
   `In order to use 'ax_focus=on', the process must be trusted with accessibility permissions.\n`
   and **the primary exits with status 1**. **Quirk:** sending `ax_focus=on` to
   an untrusted daemon kills it at the next focus event.
3. Get the front process:
   `_SLPSGetFrontProcess` → `SLSGetConnectionIDForPSN` → `SLSConnectionGetPID`
   → `AXUIElementCreateApplication(pid)`.
4. Read `kAXFocusedWindowAttribute`. If there is none, return 0.
5. Otherwise return `_AXUIElementGetWindow(window, &wid)`.

There is no space filter on this path.

**BR-FOC-05.** The AX path is slower but follows the app's own idea of its key
window. The man page says this helps with tools that change window properties,
such as yabai.

---

## 7. Border windows and drawing

### 7.1 Border object (`border.c:273-304`, `border.h:61-91`)
**BR-DRW-01.**
- Each tracked window gets a `struct border` with a **recursive** pthread mutex.
- It gets its **own SkyLight connection**: `SLSNewConnection(0, &cid)`. If that
  yields 0, the main connection is used instead.
- The border window is created lazily, on the first `border_update` that gets
  past the visibility checks.
- Proxy borders (§10) share their parent's cid.

**BR-DRW-02 (window creation, `window_create`, `src/misc/window.h:228-289`; `border_create_window`, `border.c:161-174`).**
- The frame region is `CGSNewRegionWithRect(&frame)`, where `frame` has origin
  (0,0) and the border-window size.
- **Normal border:** `SLSNewWindow(cid, kCGBackingStoreBuffered=2, -9999, -9999, region, &wid)`.
- **Proxy (unmanaged) border:** `SLSNewWindowWithOpaqueShapeAndContext(cid, 2, region, CGRegionCreateEmptyRegion(), options = 13 | (1<<18), &tags, -9999, -9999, tag_size=64, &wid, NULL)`,
  then `SLSSetWindowAlpha(cid, wid, 0.0)`.
- Then, in both cases:
  - `SLSSetWindowResolution(cid, wid, hidpi ? 2.0 : 1.0)`. **This is the only
    place hidpi is applied**, which is why changing `hidpi` recreates every
    border.
  - `SLSSetWindowTags(cid, wid, &((1<<1)|(1<<9)), 64)`.
  - `SLSClearWindowTags(cid, wid, &0, 64)`.
  - `SLSSetWindowOpacity(cid, wid, false)`, so the window is non-opaque.
  - `SLSWindowSetShadowProperties(wid, {"com.apple.WindowShadowDensity": CFIndex 0})`,
    so there is no shadow.
- Then:
  - `context = SLWindowContextCreate(cid, wid, NULL)`.
  - `CGContextSetInterpolationQuality(context, kCGInterpolationNone)`.
  - If `border.sid == 0`, set `sid = window_space_id(target)`.
  - `SLSMoveWindowsToManagedSpace(cid, [wid], (uint32_t)sid)`. **Quirk:** this
    truncates to u32. It is the **only** place the border is put on a space.
    It doesn't happen again after the target changes space (Open Question 3).

### 7.2 `border_update` (`border.c:176-258, 338-363`)
**BR-DRW-03.** `border_update(border, try_async)` locks the mutex and runs
`border_update_internal(border, border_get_settings(border))` **synchronously**.
`border_get_settings` returns the override if `override.enabled`, else
`g_settings`, and asserts the main thread. The `try_async` thread-spawning path
after `return` is dead code. **Quirk:** mbar keeps updates synchronous on main.

**BR-DRW-04 (`border_update_internal`).** Steps, in this order:
1. If `external_proxy_wid != 0` (yabai animation in progress), return.
2. **Bounds** (`border_calculate_bounds`, `border.c:33-56`):
   1. Get the window frame `R`. For a proxy, use the stored `target_bounds`.
      Otherwise call `SLSGetWindowBounds(cid, target_wid, &R)`. These are global
      CG coordinates: origin at the top-left of the main display, y down.
   2. Store `target_bounds = R`.
   3. `too_small = (R.w − 2 < 2·inner_radius) || (R.h − 2 < 2·inner_radius)`
      (`CGRectInset(R,1,1)`). If true, `border_hide` and return. This also
      protects `CGPathCreateWithRoundedRect` from corner radii that are too
      large.
   4. Set `o = w + P`. The C code computes it as an **f32**
      (`float border_offset = -w - 8.0`). `border_move` computes the same
      offset in f64, so the two can differ in the last float bit.
   5. Border frame: `F = CGRectInset(R, −o, −o)`.
   6. `origin = F.origin = (R.x − o, R.y − o)`.
   7. `frame = (0, 0, R.w + 2o, R.h + 2o)`.
   8. `drawing_bounds = (o, o, R.w, R.h)`, the window rect in border-local
      coordinates.
3. Read the target's tags (`window_tags`, a one-window query). Set
   `sticky = tags & (1<<11)`. If not sticky and `!is_space_visible(sid)`,
   **return without hiding**. The border window is on that space, so it is
   already invisible.
4. `SLSWindowIsOrderedIn(cid, target_wid, &shown)`. If not shown and not a
   proxy, `border_hide` and return. This is how minimized and hidden windows
   lose their border.
5. `level = SLSWindowIteratorGetLevel(...)` for the target (0 if the query
   fails). `sub_level = window_sub_level(target)` (§7.6).
6. If `wid == 0`, create the window (§7.1) with `frame` and `settings.hidpi`.
   `unmanaged` = `is_proxy`. This sets `border.frame = frame` and
   `needs_redraw = true`.
7. **Reshape**, only if `frame != border.frame` (since origins are always 0,
   this compares sizes):
   1. `SLSDisableUpdate(cid)`.
   2. `SLSWindowFreezeWithOptions(cid, wid, NULL)`.
   3. `SLSSetWindowShape(cid, wid, origin.x, origin.y, region(frame))`.
   4. `needs_redraw = true`, `border.frame = frame`.
8. If `needs_redraw`, draw (§7.4). Drawing ends with `SLSWindowThaw`, which is
   called even if the window wasn't frozen.
9. One transaction:
   1. `t = SLSTransactionCreate(cid)`. If it returns NULL, **return without
      re-enabling updates**. **Quirk:** if step 7 disabled updates, they stay
      disabled. mbar: always re-enable.
   2. `SLSTransactionMoveWindowWithGroup(t, wid, origin)`.
   3. If not a proxy: `SLSTransactionSetWindowTransform(t, wid, 0, 0, {a=1,b=0,c=0,d=1,tx=−origin.x,ty=−origin.y})`.
      SkyLight window transforms map screen to window space, so this resets any
      transform that a yabai proxy animation left behind.
   4. `SLSTransactionSetWindowLevel(t, wid, level)`.
   5. `SLSTransactionSetWindowSubLevel(t, wid, sub_level)`.
   6. `SLSTransactionOrderWindow(t, wid, settings.border_order (+1 above / −1 below), target_wid)`.
   7. `SLSTransactionCommit(t, 0)`, then release.
10. Tags:
    - `set = (1<<1)|(1<<9)`. If sticky, add `1<<11`.
    - `clear = 0`. If sticky, use `clear = 1<<45`.
    - `SLSSetWindowTags(cid, wid, &set, 0x40)` and
      `SLSClearWindowTags(cid, wid, &clear, 0x40)`.

    Sticky targets get sticky borders that show on every space.
11. If step 7 ran, `SLSReenableUpdate(cid)`.

### 7.3 Geometry summary
**BR-DRW-05.** Border window size is `(W + 2(w+8)) × (H + 2(w+8))`, placed at
`(x − w − 8, y − w − 8)`. The 8 pt padding leaves room for the glow (blur 10).
At hidpi=off the backing scale is 1.0, even on Retina displays.

### 7.4 Drawing (`border_draw`, `border.c:58-159`; helpers in `src/misc/drawing.h`)
**BR-DRW-06.** Inputs:
- `frame` = (0,0,FW,FH)
- `D` = `drawing_bounds`
- `r` = `border.radius`
- `ri` = `border.inner_radius` = r+1
- `cs` = `focused ? active_window : inactive_window`

Color conversion: `a=(c>>24&255)/255, r=(c>>16&255)/255, g=(c>>8&255)/255, b=(c&255)/255`
(`drawing.h:10-15`).

1. `CGContextSaveGState`, then `needs_redraw = false`.
2. Paint setup:
   - **SOLID/GLOW:** set the RGB fill **and** stroke color to `cs.color`. For
     GLOW also call `CGContextSetShadowWithColor(ctx, CGSizeZero, blur=10.0, CGColorCreateGenericRGB(r,g,b, 1.0))`.
     The shadow alpha is forced to 1.
   - **GRADIENT:** build a CGGradient from two `CGColorCreateSRGB(r,g,b,a)` colors
     `[color1, color2]` with NULL locations, so evenly spaced. The direction is
     in unit coordinates scaled by `(FW, FH)`:
     - `TL_TO_BR`: start `(0, FH)`, end `(FW, 0)`
     - `TR_TO_BL`: start `(FW, FH)`, end `(0, 0)`

     The context is unflipped (y up), so `(0, FH)` is top-left. The gradient
     spans the **whole border window**, not the target rect.
3. `CGContextSetLineWidth(ctx, w)`, then `CGContextClearRect(ctx, frame)`.
4. **Inner clip path `C`:**
   - If `style=='s' && order==above && w >= BORDER_TSMW`: `path_rect = CGRectInset(D, 3.27, 3.27)` and `C = rect(path_rect)`. The border then overlaps the window's own rounded corners, which gives a truly square look.
   - Else: `path_rect = D` and `C = roundedRect(CGRectInset(D,1,1), ri, ri)`.
5. Clip: add `rect(frame)` plus `C` to one path and `CGContextEOClip`. Painting
   is allowed only **outside** `C`.
6. **Square (`'s'`):**
   - SOLID/GLOW: **fill** `CGRectInset(path_rect, −w/2, −w/2)`.
   - GRADIENT: add the same rect, `CGContextClip`, then
     `CGContextDrawLinearGradient(start, end, options=0)`.

   The visible band runs from the clip edge out to `w/2` beyond `path_rect`.
7. **Round (everything else):**
   - `cr = (style=='u') ? 9.0 : r`.
   - If `'u'` (uniform): first **fill** `roundedRect(path_rect, 9, 9)` with the
     current fill color. This fills the gap between the window's real corner
     (radius `ri`, which the clip excludes) and a fixed radius 9, so the band
     looks the same width everywhere. **Quirk:** with a GRADIENT color no fill
     color was ever set, so the context default (opaque black) fills those
     corner gaps. **Quirk:** `too_small` only guarantees `R.w, R.h ≥ 2r + 4`.
     With `'u'` and a reported radius `r < 7`, a window narrower than 18 pt
     passes that check but violates `CGPathCreateWithRoundedRect`'s
     `2·radius ≤ width` precondition for the fixed radius 9 (CoreGraphics
     asserts). mbar: clamp the radius to `min(w, h)/2`.
   - SOLID/GLOW: **stroke** `roundedRect(path_rect, cr, cr)` with width `w`.
     The stroke is centered on the window edge. Only the outer half, plus a
     1 pt sliver inside the edge, survives the clip.
   - GRADIENT: add the same rounded rect, then
     `CGContextReplacePathWithStrokedPath`, `CGContextClip`, and draw the linear
     gradient.
8. Release the gradient.
9. **Background:** if `show_background && border_order != +1` (that is, only
   when below):
   1. `RestoreGState` + `SaveGState`. This drops the clip and the shadow.
   2. Set fill to `background.color` and stroke to 0.
   3. Fill `C`, the inset-1 rounded window shape, behind the window. GLOW
      backgrounds are drawn as plain fills. GRADIENT backgrounds never get here
      (BR-PARSE-07).
10. Release `C`, `CGContextFlush`, `RestoreGState`.
11. `SLSFlushWindowContentRegion(cid, wid, NULL)`, then
    `SLSWindowThaw(cid, wid)`.

**BR-DRW-07.** Inactive default `0x00000000`: inactive borders are drawn fully
transparent. The windows still exist and get ordered.

### 7.5 Magic constants (`src/border.h:9-21`)
| Constant | Value | Use |
|---|---|---|
| `BORDER_ORDER_ABOVE` / `BELOW` | `+1` / `−1` | `SLSTransactionOrderWindow` order; hide uses `0` (order out) |
| `BORDER_STYLE_ROUND` / `ROUND_UNIFORM` / `SQUARE` | `'r'` / `'u'` / `'s'` | style chars |
| `BORDER_PADDING` | `8.0` | extra outset beyond `w` |
| `BORDER_TSMN` | `3.27` | inset for the "truly square" mode |
| `BORDER_TSMW` | `52.0` if built with the **macOS 26 SDK** (`__MAC_OS_X_VERSION_MAX_ALLOWED >= 260000`), else `8.0` | minimum width for truly-square mode. mbar: decide at runtime (macOS ≥ 26 → 52), see Open Question 5 |
| default corner radius | `9` (when radius ≤ 0 or no `SLSWindowIteratorGetCornerRadii`) | `windows.c:75` |
| uniform corner radius | `9.0` | `border.c:118` |
| glow blur | `10.0` | `drawing.h:37` |
| `inner_radius` | `radius + 1` | `windows.c:78` |

### 7.6 Sub-level via raw MIG (`window_sub_level`, `src/misc/window.h:134-196`)
**BR-DRW-08.** `SLSGetWindowSubLevel` is declared but not used. Instead,
JankyBorders sends a MIG request straight to the WindowServer connection port
`g_server_port` (§2.1):

- **Message ids:**
  - request `0x73c3`, reply `0x7427`
  - on macOS ≥ 26 at runtime: request `0x76e3`, reply `0x7747`
- **Layout:** packed to 2 bytes.
  - `mach_msg_header_t` (24 bytes)
  - `NDR_record_t` (8 bytes, `= NDR_record`)
  - `int32 wid` (send size **36**)
  - receive buffer **48** bytes; `sub_level` is read at **byte offset 36**
- **Header:** `msgh_bits = COPY_SEND | (MAKE_SEND_ONCE << 8)` = `0x1513`,
  `msgh_local_port = mig_get_special_reply_port()`.
- **Call:** `mach_msg(SEND|SEND_SYNC_OVERRIDE|SEND_PROPAGATE_QOS|RCV|RCV_SYNC_WAIT, 36, 48, reply_port, TIMEOUT_NONE)`.
- **On a send/receive error:** stdout `SubLevel: Error receiving message.\n`,
  `mig_dealloc_special_reply_port`, and the result is 0.
- **On a wrong reply id:** stdout `SubLevel: Invalid message received\n`,
  `mach_msg_destroy`, and the result is 0.

This runs on every `border_update_internal` that gets past the visibility checks.

### 7.7 Move (`border_move`, `border.c:306-336`)
**BR-DRW-09.** On SLS 806:
1. If `external_proxy_wid` is set, return.
2. Capture the settings pointer on main.
3. `dispatch_async` to the **HIGH-priority global queue**, which is not main.
   There, under the mutex:
   1. `SLSGetWindowBounds(target)`.
   2. `origin = bounds.origin − (w + 8)`.
   3. Transaction `MoveWindowWithGroup(wid, origin)`, commit.
   4. Store `target_bounds` and `origin`.

There is no redraw, size check, level, order or visibility handling. A pure
move during a drag translates the existing bitmap. If `wid == 0`, the
transaction targets window 0, which is harmless. Resize (807) is a full
synchronous update with reshape and redraw on every event.

### 7.8 Hide / unhide / destroy (`border.c:292-304, 365-403`)
**BR-DRW-10.**
- **`border_hide`:** if `wid`, run a transaction
  `OrderWindow(wid, 0, target_wid)`, which orders the border out.
- **`border_unhide`:** return if any of these hold:
  - `too_small`
  - `external_proxy_wid`
  - not sticky and `!is_space_visible(sid)`

  Otherwise, if `wid`, run a transaction
  `OrderWindow(wid, settings.border_order, target_wid)`. There is no redraw and
  no reposition.
- **`border_destroy`:**
  1. `border_hide`.
  2. `dispatch_async(main)`:
     1. `CGContextRelease`, `SLSReleaseWindow`.
     2. Recursively destroy the proxy.
     3. `animation_stop`.
     4. If not a proxy and `cid != main`, `SLSReleaseConnection(cid)`.
     5. Free.

### 7.9 Threads and batching
**BR-DRW-11.** There is **no** CFRunLoop observer, frame throttle or update
coalescing. Batching works like this:
- each update's move, transform, level, sub-level and order go into **one**
  SLS transaction, committed asynchronously (`Commit(t, 0)`)
- reshape and redraw are bracketed by `SLSDisableUpdate`/`SLSReenableUpdate` and
  window freeze/thaw, so a resize appears atomically

Threads touching borders:
- main (everything)
- the HIGH global queue (moves)
- a CVDisplayLink thread (the yabai `track_transform`, §10)

A per-border recursive mutex serializes them.

---

## 8. Animation (`src/animation.c`, `src/animation.h`)

**BR-ANI-01.** "Animation" is only a CVDisplayLink wrapper used by the yabai
integration. Borders themselves have no animated transitions, color fades or
easing.
- `animation_start(proc, ctx)`:
  1. Assert there is no link and no context.
  2. `CVDisplayLinkCreateWithActiveCGDisplays`.
  3. `frame_time = 1e6 · timeValue / timeScale` (µs) from the nominal refresh
     period.
  4. Set the output callback `proc` with userinfo `&animation`, then start the
     link.
- `animation_stop`:
  1. Stop and release the link.
  2. `free(context)`.

---

## 9. Hash tables (`src/hashtable.c`)

**BR-TBL-01.** These are implementation details with no observable effect,
except that iteration order (bucket order) decides the order of per-border
updates and of the notification list.
- Windows table: hash = the u32 wid.
- List tables: djb2 hash. Note that `char c` makes high-bit bytes
  sign-extend.
- Load factor 0.75, then the capacity doubles.

mbar may use any map.

---

## 10. yabai integration (`src/misc/yabai.h`)

**BR-YAB-01 (port).** Steps:
1. Allocate a receive right with `mpl_qlimit = 1`.
2. `bootstrap_register(..., "git.felix.jbevent")`. On any failure, silently
   skip. Unlike §3.3, **no** `mach_port_insert_right(MAKE_SEND)` is done first,
   so the port is registered while the task holds only the receive right.
   mbar: insert a send right before registering, as in §3.3.
3. Add a CFMachPort source to the main run loop.

yabai (when its window animations are on) creates screenshot "proxy" windows,
animates those, and tells borders about them through this port.

**BR-YAB-02 (message).** The message is accepted only if the CFMachPort message
size is exactly `sizeof(struct mach_message)` (44 bytes, same layout as
BR-IPC-03) **and** `descriptor.size == 4104`. The OOL payload is:
```c
struct { uint32_t event; uint32_t count; uint32_t proxy_wid[512]; uint32_t real_wid[512]; };
```
- `event == 1325`: `proxy_begin(proxy_wid[i], real_wid[i])` for `i < count`.
- `event == 1326`: `proxy_end(proxy_wid[i], real_wid[i])` for `i < count`.

**Quirk:** `count` is not clamped to 512 (reads out of bounds). mbar: clamp.
Events other than 1325/1326 are ignored. `mach_msg_destroy` runs whenever the
outer message size is 44, even if `descriptor.size` did not match. If the outer
size is not 44, the message is **not** destroyed, so any OOL memory or port
rights in it leak. mbar: always destroy.

**BR-YAB-03 (`proxy_begin(wid, real)`).** Return if either id is 0. Then:
1. Find the border for `real`. If there is none, return.
2. Under its mutex, set `border.external_proxy_wid = wid`. This freezes all
   normal updates, moves and unhides for that border.
3. If the border has no proxy yet:
   1. Allocate `proxy`, `border_init` with the **parent's cid**.
   2. Create its window **unmanaged**, with `frame = CGRectNull` and hidpi off.
      It is created with alpha 0 (§7.1).
   3. Copy `target_bounds`, `frame`, `focused`, `target_wid`, `sid`, `radius`
      and `inner_radius` from the border.

   **Quirk:** step 2 runs before step 3, so inside `border_create_window` the
   proxy still has `sid = 0` and `target_wid = 0`. Its space is computed as
   `window_space_id(cid, 0)` (in practice 0), and `SLSMoveWindowsToManagedSpace`
   is called with that value. The proxy window is never put on the target's
   space explicitly. mbar: copy the fields first, then create the window.
4. Snapshot `{proxy, border_wid, external_proxy_wid, settings = effective settings of border}`.
5. `dispatch_async(main)` → `begin_proc`:
   1. `proxy_frame = SLSGetWindowBounds(external_proxy_wid)`.
   2. `initial_transform`:
      - `a = target_bounds.w / proxy_frame.w`
      - `d = target_bounds.h / proxy_frame.h`
      - `b = c = 0`
      - `tx = 0.5·(proxy.frame.w − target_bounds.w)`
      - `ty = 0.5·(proxy.frame.h − target_bounds.h)`

      Since the frame was copied from the border, `tx = ty = w + 8`.
   3. Stop any previous animation (this frees the old payload) and start a
      display link running `track_transform` with
      `{cid, border_wid, proxy_wid, target_wid = external_proxy_wid, initial_transform}`.
   4. If the proxy isn't flagged yet:
      1. Set `is_proxy = true` and `frame = CGRectNull`.
      2. `border_update_internal(proxy, settings)`. This takes geometry from
         `target_bounds`, reshapes and draws, moves to `origin`, sets **no**
         transform, takes level and sub-level from the real target, and orders
         relative to the real target.
   5. Transaction:
      - `OrderWindow(proxy.wid, settings.order, external_proxy_wid)`
      - `SetWindowAlpha(border_wid, 0)`
      - `SetWindowAlpha(proxy.wid, 1)`
      - commit

**BR-YAB-04 (`track_transform`, every display-link frame, display-link thread).**
1. `usleep(0.25 · frame_time µs)`.
2. `T = SLSGetWindowTransform(cid, external_proxy_wid)`. On error, skip the
   frame.
3. `B = CGAffineTransformConcat(T, initial_transform)`, which applies T first and
   then initial.
4. In one transaction, `SetWindowTransform(proxy_wid, 0, 0, B)` and
   `SetWindowTransform(border_wid, 0, 0, B)`, then commit.

**BR-YAB-05 (`proxy_end(wid, real)`).** Return if either id is 0. Then:
1. Find the border. Proceed only if it has a proxy **and**
   `external_proxy_wid == wid`.
2. Detach the proxy.
3. Transaction: `SetWindowAlpha(proxy.wid, 0)`, `SetWindowAlpha(border.wid, 1)`,
   commit.
4. `animation_stop(proxy)`, then `border_destroy(proxy)`. The connection is
   shared, so it is not released.
5. `dispatch_async(main)`:
   1. Under the mutex: `external_proxy_wid = 0`.
   2. `border_update_internal(border, settings snapshot)`. This resets the
      transform via step 9.3 of BR-DRW-04.

   `disable_coalescing` is set and cleared around this, but nothing reads it.

**Quirk (begin/end race).** `proxy_begin` defers `begin_proc` with
`dispatch_async(main)`, while `proxy_end` runs synchronously in the port
callback. If the end message is serviced before the deferred `begin_proc` runs,
the proxy is destroyed while `is_proxy` is still false. `border_destroy` then
releases the **parent's** connection (`cid != main`), and `begin_proc` later
uses the freed proxy. mbar: mark proxies as such at allocation and cancel a
pending begin on end.

---

## 11. Observable outputs summary

| Stream | Text | When |
|---|---|---|
| stdout | `borders-v1.9.0\n` | `-v`/`--version` as argv[1] |
| stdout | `Refer to the man page for help: man borders\n` | `-h`/`--help` as argv[1] |
| stdout | `[?] Borders: Invalid argument '<arg>'\n` | unknown arg (client and primary) |
| stdout | `[?] Borders: Invalid color argument color<rest>\n` | bad color value |
| stderr + exit 1 | `A borders instance is already running and no valid arguments where provided. To modify properties of the running instance provide them as arguments.\n` | server exists, no valid arg |
| stderr + exit 1 (daemon) | `In order to use 'ax_focus=on', the process must be trusted with accessibility permissions.\n` | AX focus without trust |
| stdout | `[!] Failed to make config at '<path>' executable...\n` | chmod failed |
| stdout | `[!] ERROR (id): No active display detected!\n` | `get_active_space_id` race |
| stdout | `SubLevel: Error receiving message.\n` / `SubLevel: Invalid message received\n` | MIG sub-level failure |

Exit codes: 0 for version, help and client forward. 1 for "already running" and
for the AX failure. A failed `assert` (BR-CLI-00) aborts with SIGABRT. A primary
that is not killed never exits.

Buffering: all `stdout` texts use `printf` with default stdio buffering, and
nothing calls `fflush`. When the primary's stdout is a file or pipe (for example
under `brew services`), its messages are block-buffered and may appear late or
never. The client flushes on `exit`/`return`. `stderr` texts are unbuffered.

---

## 12. Private API surface (summary)

The full table with C signatures is in [`borders-skylight.md`](borders-skylight.md).
These groups are used:
- **SkyLight:** connection, notify, window query and iterator, window
  create/shape/tags/resolution/opacity/shadow, transactions, managed
  displays/spaces, event port, `SLWindowContextCreate`.
- **Process:** `_SLPSGetFrontProcess` and `SLSGetConnectionIDForPSN`.
- **Hidden symbols:**
  - `CGSGetConnectionPortById` via symtab scan
  - `SLSWindowIteratorGetCornerRadii` via dlsym (macOS 26)
- **Raw MIG** sub-level request (§7.6).
- **Private AX:** `_AXUIElementGetWindow`.
- **Private CF:** `_CFMachPortSetOptions`.
- **Mach/bootstrap:** `bootstrap_register`, which is deprecated.

---

## 13. Quirk index

"Replicate?" is the recommendation for mbar.

| ID | Quirk | Where | Replicate? |
|---|---|---|---|
| BQ1 | No focus detection at startup or after recreate-all; every border is inactive until the first focus event | `main.c:227-235`, `windows.c:109-112` | yes (cheap); optionally run detection once (document as a deviation) |
| BQ2 | Prefix-matched color keys (`active_colorX=`) report a color error instead of "Invalid argument" | `parse.c:71-89` | yes |
| BQ3 | sscanf leniency: trailing garbage, `width=inf/-1`, `style=` with any char, `order=` with any non-`a` → below | `parse.c:102-131` | yes |
| BQ4 | A partially matched gradient overwrites `color1` | `parse.c:40-47` | optional |
| BQ5 | `background_color=gradient(...)` is accepted but never shown; glow background has no glow | `parse.c:87`, `border.c:143-153` | yes |
| BQ6 | List entries are not trimmed and are compared to the BSD process name | `parse.c:12-29`, `windows.c:39-42` | yes |
| BQ7 | `blacklist=`/`whitelist=` always recreate, even when the value is unchanged or empty | `parse.c:90-101` | yes |
| BQ8 | Client sends `argc−1` bytes of uninitialized padding | `main.c:130-150` | no (send exact) |
| BQ9 | Invalid args are reported by both client and primary; with overrides, the primary reports them once more per overridden border | `main.c:76,100` | yes for client+primary; optional per override |
| BQ10 | A primary started with `apply-to=N` turns every later message into a per-window message for N, until a message contains `apply-to=0` | `main.c:73,80` | optional (document) |
| BQ11 | `apply-to` + `hidpi`/lists/`ax_focus` has no effect; overrides are lost on recreate | `main.c:80-88` | yes |
| BQ12 | Table aliasing frees list buckets that overrides or `g_settings` still use (UB) | `main.c:73,83,100` | no |
| BQ13 | `ax_focus=on` on an untrusted daemon exits the daemon with status 1 | `ax.h:20-22` | yes (documented behavior), or deviate and log |
| BQ14 | `border_update(try_async)` is always synchronous (dead code) | `border.c:338-363` | yes (synchronous) |
| BQ15 | `SLSTransactionCreate` failure after `SLSDisableUpdate` leaves updates disabled | `border.c:208,223-224` | no |
| BQ16 | The border window is moved to its space only at creation (u32-truncated sid) | `border.c:171-172`, `window.h:218` | see Open Question 3 |
| BQ17 | Destroy compares a u32-truncated sid | `windows.c:210` | no (u64) |
| BQ18 | Notification list limited to 1024 windows (stack overflow) | `windows.c:223` | no |
| BQ19 | `proc_name` failure reuses the previous app name | `windows.c:39-40` | no (treat as empty name) |
| BQ20 | Uniform style + gradient fills corner gaps with opaque black | `border.c:120-125` | yes for exactness; or fill with color1 and document |
| BQ21 | Config is run via `sh -c <path>` (no quoting), with `alarm(60)` | `helpers.h:69-77` | partially: quote the path; keep the 60 s kill |
| BQ22 | Config runs when the primary got only invalid args | `main.c:230` | yes |
| BQ23 | `BORDER_TSMW` depends on the build SDK, not the runtime OS | `border.h:17-21` | no (runtime check) |
| BQ24 | yabai payload `count` not clamped | `yabai.h:207,213` | no (clamp to 512) |
| BQ25 | No coalescing of delayed focus detection | `events.c:73,81,98,105` | recommended: coalesce, but keep the 10/20/50 ms latency floor |
| BQ26 | Failure to register `git.felix.borders` is ignored; the primary runs without IPC | `main.c:229` | no (log a warning) |
| BQ27 | `CGSGetConnectionPortById` symbol lookup failure → NULL call → crash | `connection.h:98-101` | no (fall back to `SLSGetWindowSubLevel`, or sub-level 0) |
| BQ28 | An empty argv element ends the IPC message; later arguments are silently dropped by the primary | `main.c:75` | optional (document) |
| BQ29 | Borders IPC callback trusts the message layout without size or descriptor checks | `mach.c:54-59` | no (validate) |
| BQ30 | Asserts stay enabled in release builds; a failed `SLSNewWindow` aborts the daemon | `window.h:235,263`, `border.c:11` | no (log and skip) |
| BQ31 | Space-change path crashes if `SLSCopyManagedDisplays` returns NULL | `windows.c:260-261` | no |
| BQ32 | Uniform style uses radius 9 without a size guard (CG assertion for tiny windows when `r < 7`) | `border.c:118-125` | no (clamp) |
| BQ33 | jbevent port registered without a send right; non-44-byte messages are never destroyed | `yabai.h:224-242,193-221` | no |
| BQ34 | Proxy window created before its `sid`/`target_wid` are copied, so it is sent to space `window_space_id(0)` | `yabai.h:131-141` | no |
| BQ35 | yabai begin/end race can free the proxy under a pending `begin_proc` and release the parent's connection | `yabai.h:153-155,160-188` | no |
| BQ36 | stdout is never flushed in the primary | `parse.c:56,133` etc. | no (line-buffer or log) |

---

## 14. Open questions

1. **Exact trigger semantics of SLS events** 723, 808, 1322, 1401, 1508 and of
   815/816 on macOS 14–26. The names in `events.h` are the author's guesses
   (1322 is called "TITLE" but used for focus). This needs verification on real
   hardware when building `mbar-borders`.
2. **Minimize and app-hide.** Which events fire (816 vs 1326/1325, or only 808)?
   The border disappears through `SLSWindowIsOrderedIn` on the next update, but
   if no update arrives the border could stay visible. Verify.
3. **Moving a window between spaces** (yabai `--space`, Mission Control drag).
   If 1325(new sid) arrives before 1326(old sid), the border object survives with
   its window still on the old space (§7.1, BQ16). Does WindowServer migrate
   ordered-relative windows, or does a stale border show on the old space? Decide
   whether mbar re-sends `SLSMoveWindowsToManagedSpace` when `sid` changes.
4. **Whether `SLSCopyWindowsWithOptionsAndTags` writes back into
   `set_tags`/`clear_tags`.** They are passed as in/out pointers. mbar should pass
   fresh variables on every call, as the C code does.
5. **`BORDER_TSMW` on macOS 26 runtime.** Homebrew bottles are built with a
   recent SDK (52). Builds from source with an older SDK use 8. The runtime
   choice (macOS ≥ 26 → 52, else 8) matches the intent, which was the larger
   corner radii on macOS 26.
6. **Reply layout of the sub-level MIG call.** The code reads offset 36. The
   word at offset 32 is presumably a return code, which is ignored. Confirm on
   macOS 26 before trusting the ids `0x76e3`/`0x7747` on later releases.
7. **Ownership of hidden SkyLight symbols in the dyld shared cache.** The symtab
   scan for `_CGSGetConnectionPortById` relies on local symbols being present in
   the in-memory LINKEDIT. Confirm that this still works on macOS 26.x, or fall
   back.
