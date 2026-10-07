# Deliberate deviations from SketchyBar

mbar reproduces SketchyBar's behaviour including its quirks (see `docs/spec/`).
The exceptions below are cases where SketchyBar crashes, corrupts memory, leaks state,
or depends on undefined behaviour. Each one gets a defined, safe behaviour instead.

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

Extensions (new behaviour that does not exist in SketchyBar) are listed in `docs/EXTENSIONS.md`.
