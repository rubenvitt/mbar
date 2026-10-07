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
| D14 | Freeze flag is a plain bool (nested freeze/unfreeze cancel) | Kept as-is (no crash), documented |

Extensions (new behaviour that does not exist in SketchyBar) are listed in `docs/EXTENSIONS.md`.
