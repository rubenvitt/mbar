# Deliberate deviations from SketchyBar and JankyBorders

mbar reproduces SketchyBar's behaviour including its quirks (see `docs/spec/`).
The exceptions below are cases where SketchyBar crashes, corrupts memory, leaks state,
or depends on undefined behaviour. Each one gets a defined, safe behaviour instead.
The JankyBorders deviations (B1…) are [below](#jankyborders-borders).

| # | SketchyBar | mbar |
|---|---|---|
| D1 | Subscribers of one event share an env object; variables leak between items; `vfork`+`setenv` leaks into the bar process | Every script run gets a fresh env: the daemon env + the item's persistent env + the event's vars |
| D2 | Slider click hit-test compares y-down point with y-up bounds | Correct hit test in one coordinate space |
| D3 | Hidden popup host with `popup.drawing=on` may leave the popup window visible | Popup is hidden while its host is not drawn |
| D4 | Nested popups are anchored one refresh late | Anchors are computed in the same layout pass |
| D5 | `--push` on a graph of width 0 divides by zero | Push is ignored |
| D6 | `--clone` of a graph shares the sample buffer | Buffer is deep-copied |
| D7 | `--clone` of a bracket corrupts the group | Clone of a bracket copies the members list |
| D8 | `--move x before x`, duplicate names in `--reorder`, bracket with a missing member | No-op / first occurrence wins / missing member skipped |
| D9 | `display=`/`space=` values ≥ 32 overflow the bitmask | Ignored (with an error response) |
| D10 | Negative layout values converted to unsigned: arm64 clamps, x86-64 wraps | Clamp to 0 (Apple-silicon behaviour) everywhere |
| D11 | `get_height` uses the background height of the previous layout pass | Uses the current pass |
| D12 | Animation durations count 60 Hz display-link frames | Duration `n` = `n/60` s of wall-clock time; frames are rendered at the display's refresh rate (smoother on 120 Hz) |
| D13 | `--query` prints strings without JSON escaping (backslashes, control chars) | Same layout and key order, but strings are properly JSON-escaped so the output always parses |
| D15 | Curves `bounce` and `overshoot` are accepted but behave as linear | Implemented as standard ease-out-bounce and ease-out-back (overshoot 1.70158) |
| D16 | `--trigger` and other events share one env object across subscribed items | Fresh env per item (see D1) |
| D17 | Client waits 100 ms for a reply; slow daemon → empty `--query` | Client waits up to 5 s |
| D18 | Removing an item leaves its animations pointing at freed memory | Animations of removed items are cancelled |
| D19 | After hotload, distributed notifications may be registered twice | Each notification is delivered once |
| D20 | Regex selectors with back-references (`\1`–`\9`) are matched by `regexec` without any limit; a pathological pattern such as `/^\(\(a*\)*\)*\1c$/` can stall the bar | Matching stops after 1,000,000 backtracking steps per item name and the selector fails like any other `regexec` error: `[!] Regex: Regex match failed 'out of memory'` (the `REG_ESPACE` text), empty selection |
| D21 | Every item is drawn into its own window, so an item window larger than its bar or popup window (a popup member with `width=` wider than the popup, a shadow overhang left of the bar) stays fully visible | Items are painted into their bar's / popup's window (one window per bar and popup, `docs/DESIGN-CORE.md`); the parts of an item window outside it are clipped. Hit testing still uses the full item windows |
| D22 | `--add graph` mallocs `width` (u32, unbounded) floats; a huge width fails the allocation or stalls the bar, and `--query` prints every sample | Width is clamped to 4096 samples/points (`MAX_GRAPH_WIDTH`) |
| D23 | Lock file `/tmp/<g_name>_<USER>.lock` in the world-writable `/tmp`; another local user can pre-create it and keep the bar from starting | Same `$TMPDIR`-independent base, but inside the private (0700, owner-checked) `/tmp/mbar-<uid>/` directory: `/tmp/mbar-<uid>/mbar_<USER>_<bar>.lock`. Error messages and exit codes unchanged |
| D24 | `SIGTERM`/`SIGINT`/`SIGHUP` take the default action: the daemon dies without cleanup (stale socket, `--menubar`/`hide_menubar` auto-hide left changed); scripts share the daemon's process group | The three signals shut the daemon down like `--exit` (mach helpers get `"k"`, menu-bar auto-hide restored, socket removed, exit code 0). Every script runs in its own process group; on exit (`--exit` or a signal) the groups that still have members get `SIGTERM`, so background children of scripts (`cmd &` in the config) end with the bar |
| D14 | Freeze flag is a plain bool (nested freeze/unfreeze cancel) | Kept as-is (no crash), documented: `--update` / `--trigger space_change` end a message's freeze with a refresh, so later events in the message see items added before as shown (events.md Q7) |

## JankyBorders (`borders`)

mbar draws JankyBorders-style window borders (reference: JankyBorders v1.9.0,
[`docs/spec/borders.md`](spec/borders.md)). It reproduces the argument grammar
and its quirks, except where JankyBorders crashes or has undefined behaviour,
and where a separate process makes no sense inside mbar. The quirk IDs (BQ…)
refer to the quirk index in `docs/spec/borders.md` §13.

| # | JankyBorders | mbar |
|---|---|---|
| B1 | Invalid arguments print `[?] Borders: Invalid argument '<arg>'` (or the color error) on the client's stdout and again on the daemon's stdout, once more per overridden border (BQ9) | The `borders` client prints the same `[?]` lines on stdout. The daemon does not print them; `mbar --borders` answers `[!] Borders: …` (stderr, exit 1, mbar's error convention). Valid keys in the same message still apply |
| B2 | `borders` without a running instance becomes the daemon | `borders` never starts a daemon. Without a running mbar it prints `borders: mbar is not running. mbar draws the window borders; start mbar.app.` on stderr and exits 1 |
| B3 | The client looks up `git.felix.borders` once; if it is missing, it becomes the daemon | The client retries for up to 5 s (window-manager startup lines can run before mbar's login item is up), then fails as in B2 |
| B4 | Mach service `git.felix.borders`; a failed registration is ignored and the daemon runs without IPC (BQ26) | Not registered. `borders` reaches mbar through the `borders -> mbar` link (`dev.rubeen.mbar`), like the `sketchybar` link |
| B5 | `bordersrc` runs once, only in a daemon started without valid arguments (BR-CFG-01) | The default bar `mbar` runs `bordersrc` after its main config, on every start and every `--reload`. Same lookup (`~/.config/borders/bordersrc`, then `~/.bordersrc`, no `$XDG_CONFIG_HOME`) |
| B6 | `bordersrc` runs as `sh -c <path>` with the path unquoted, so spaces and shell metacharacters break it (BQ21) | The path is quoted. The 60 s kill is kept |
| B7 | A daemon started with `apply-to=N` turns every later message without `apply-to=` into a message for window N, until one contains `apply-to=0` (BQ10, sticky apply-to) | Not sticky: `apply-to=` only affects the message it is in |
| B8 | Overrides share the global hash-table buckets; changing `blacklist=`/`whitelist=` while an override exists frees them and likely crashes (BQ12) | Every override keeps its own copy of the lists. A recreate (`hidpi=`, `blacklist=`, `whitelist=`) clears all overrides, as JankyBorders loses them when it recreates the borders (BQ11) |
| B9 | A failed gradient match can overwrite `color1` of an existing gradient (BQ4) | A color that fails to parse leaves the old color unchanged |
| B10 | No focus detection at startup or after a recreate; every border is inactive until the first focus event (BQ1) | Focus detection runs once after borders are configured and after every recreate, so the focused window is highlighted right away |
| B11 | `ax_focus=on` on a daemon without Accessibility permission prints an error and exits the daemon with status 1 at the next focus event (BQ13) | mbar logs a warning and falls back to the SkyLight focus path. The bar keeps running |
| B12 | Several focus checks (10/20/50 ms after an event) are scheduled per event, without coalescing (BQ25) | Delayed checks are coalesced. The latency floor (10/20/50 ms) is kept |
| B13 | The border window is moved to its target's space only when it is created (u32-truncated space id, BQ16, BQ17) | The space move is sent again when the target's space changes. Space ids are 64-bit |
| B14 | yabai integration: proxy windows during yabai's window animations (`git.felix.jbevent`, spec §10, BQ24, BQ33–BQ35) | Not implemented. With yabai animations the border follows the window only after the animation |
| B15 | Window sub-level through a raw MIG message and a symtab scan for `CGSGetConnectionPortById`; a failed lookup calls NULL (§7.6, BQ27) | `SLSGetWindowSubLevel`, or sub-level 0 when it is missing. Missing SkyLight symbols disable borders with one warning |
| B16 | `BORDER_TSMW` (corner radius offset) depends on the SDK the binary was built with (BQ23) | Chosen at runtime: macOS 26 or later uses the macOS 26 value |
| B17 | Asserts stay on in release builds; a failed `SLSNewWindow` aborts the daemon (BQ30); a failed `SLSTransactionCreate` leaves updates disabled (BQ15) | The window is skipped and the error logged; updates are always re-enabled |
| B18 | Space change crashes when `SLSCopyManagedDisplays` returns NULL (BQ31) | Treated as "no displays" |
| B19 | Window notifications are collected in a 1024-entry stack array (BQ18) | No limit |
| B20 | When `proc_name` fails, the previous window's app name is reused for black/whitelist matching (BQ19) | The name is empty |
| B21 | Uniform style uses radius 9 without a size guard; tiny windows trip a CoreGraphics assertion (BQ32) | The radius is clamped to the window size |
| B22 | The client sends `argc − 1` bytes of uninitialized padding; the daemon trusts the message layout (BQ8, BQ29) | mbar's IPC sends exactly the arguments and validates every message |
| B23 | The daemon's stdout is never flushed, so its messages may appear late or never under `brew services` (BQ36) | Diagnostics go to mbar's log |

Extensions (new behaviour that does not exist in SketchyBar or JankyBorders) are
listed in `docs/EXTENSIONS.md`.
