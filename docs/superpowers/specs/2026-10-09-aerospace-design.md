# AeroSpace integration: design

Date: 2026-10-09. Status: accepted, being implemented.

mbar talks to a running [AeroSpace](https://github.com/nikitabobko/AeroSpace)
directly instead of being driven by `exec-on-workspace-change` shell chains.
AeroSpace stays a separate program that manages the windows; mbar subscribes to
its event stream and sends it commands over its documented socket. The
feasibility study (full port vs. integration) recommended this; a port would cost
24–36 person-weeks for little extra gain.

## What changes for users

Today (AeroSpace `docs/goodies.adoc`):

```toml
# aerospace.toml
exec-on-workspace-change = ['/bin/bash', '-c',
  'sketchybar --trigger aerospace_workspace_change FOCUSED_WORKSPACE=$AEROSPACE_FOCUSED_WORKSPACE']
```

```sh
# sketchybarrc
sketchybar --add event aerospace_workspace_change
for sid in $(aerospace list-workspaces --all); do
  sketchybar --add item space.$sid left \
             --subscribe space.$sid aerospace_workspace_change \
             --set space.$sid label="$sid" click_script="aerospace workspace $sid" \
                              script="$CONFIG_DIR/plugins/aerospace.sh $sid"
done
```

Each workspace switch forks bash, `sketchybar --trigger`, then one plugin script
per item (each forking `sketchybar --set`): N+2 processes, 10–50 ms.

With mbar:

- `aerospace_workspace_change` (and five more events) are **built-in**. They fire
  as soon as AeroSpace reports the change, with the same `FOCUSED_WORKSPACE`
  variable, so the sketchybarrc above keeps working unchanged. The
  `exec-on-workspace-change` line can (and should) be removed; if it stays, items
  get the event twice (harmless).
- Lua handlers run in-process: no process is started per switch.
- `mbar.aerospace.run({"workspace", "3"})` replaces `click_script="aerospace workspace 3"`
  without a fork.

## Events

| mbar event | AeroSpace event | Variables (besides `NAME`, `SENDER`, `INFO`) |
|---|---|---|
| `aerospace_workspace_change` | `focused-workspace-changed` | `FOCUSED_WORKSPACE`, `PREV_WORKSPACE` |
| `aerospace_focus_change` | `focus-changed` | `FOCUSED_WORKSPACE`, `WINDOW_ID` (empty on an empty workspace) |
| `aerospace_monitor_change` | `focused-monitor-changed` | `FOCUSED_WORKSPACE`, `MONITOR_ID` (1-based) |
| `aerospace_mode_change` | `mode-changed` | `MODE` |
| `aerospace_window_detected` | `window-detected` | `WINDOW_ID`, `WORKSPACE`, `APP_BUNDLE_ID`, `APP_NAME` |
| `aerospace_binding_triggered` | `binding-triggered` | `MODE`, `BINDING` |

`INFO` is the same data as a JSON object with lower-case keys
(`{"focused_workspace":"2","prev_workspace":"1"}`).

- The events are built-in: `--subscribe item aerospace_workspace_change` works
  without `--add event`. A config that still runs `--add event aerospace_workspace_change`
  (the SketchyBar recipe) is accepted silently and changes nothing; a manual
  `--trigger aerospace_workspace_change FOCUSED_WORKSPACE=…` keeps working as before.
- The connection starts lazily on first use: a subscription to any `aerospace_*`
  event, `provider=aerospace`, `--query aerospace`, or Lua `mbar.aerospace`.
  Users without AeroSpace never connect.
- On connect mbar asks for the initial state (AeroSpace sends the current
  workspace, focus, monitor and mode right away), so items are correct at startup
  without a separate query.

## Provider `provider=aerospace`

Sets the item's label (extension, like the other native providers):

| `provider.args` | label |
|---|---|
| (none) / `workspace` | focused workspace |
| `mode` | current binding mode (`main` …) |
| `monitor` | focused monitor id |

Highlighting the focused workspace item is a Lua or script handler on
`aerospace_workspace_change` (example in `docs/LUA.md`).

## `--query aerospace`

```json
{
	"connected": "on",
	"transport": "socket",
	"server_version": "0.20.0-Beta 33fa0643",
	"error": "",
	"focused_workspace": "2",
	"prev_workspace": "1",
	"mode": "main",
	"monitor": 1
}
```

An item named `aerospace` wins, as with `--query borders`. `transport` is
`socket`, `cli` or `none`.

## Lua

```lua
mbar.aerospace.run({ "workspace", "3" })                       -- fire and forget
mbar.aerospace.run({ "list-workspaces", "--all" }, function(r) -- r = { exit_code, stdout, stderr }
end)
mbar.aerospace.query({ "list-windows", "--all", "--json" }, function(list, err)
  -- stdout parsed as JSON (a Lua table); err set when the command or the parse failed
end)
mbar.aerospace.on("workspace_change", function(env) end)        -- = aerospace_workspace_change
```

Commands run on a worker thread; callbacks run on the daemon's Lua thread like
`mbar.exec` callbacks. Nothing blocks the bar when AeroSpace hangs (AX calls in
AeroSpace can block for seconds). `on` takes the event name with or without the
`aerospace_` prefix and registers an in-process handler (no item needed).

## Transport (`crates/mbar-aerospace`)

A platform-independent crate (std + serde_json + mbar-core types), testable on
Linux against a fake server.

- **Socket** (AeroSpace `docs/guide.adoc` "Socket protocol"):
  `/tmp/bobko.aerospace-$USER.sock`; handshake: client writes `u32 LE 1`, server
  answers its version; then frames `u32 LE length` + UTF-8 JSON.
  `ClientRequest {"args":[…],"stdin":"","windowId":null,"workspace":null}` →
  `ServerAnswer {"exitCode","stdout","stderr","serverVersionAndHash"}`.
  `subscribe --all` turns the connection into an unbounded stream of
  `ServerEvent` frames.
- **CLI fallback** for AeroSpace versions whose server predates this protocol
  (handshake answered with another version, closed, or not answered within 1 s):
  events from one long-running `aerospace subscribe --all` child (one JSON
  object per stdout line), commands as `aerospace <args>`. If the CLI has no
  `subscribe` either, the status says so and only manual `--trigger`s work.
  The `aerospace` binary is looked up on `PATH`, then `/opt/homebrew/bin`,
  `/usr/local/bin` and `/Applications/AeroSpace.app/Contents/Resources/bin`.
- **Reconnect**: when AeroSpace quits or restarts, retry with backoff (1 s,
  doubling, max 30 s) forever; every change is reported as an
  `AerospaceStatus`. `MBAR_AEROSPACE_SOCKET` and `MBAR_AEROSPACE_CLI` override
  the paths (tests).

Contract:

```rust
pub const PROTOCOL_VERSION: u32 = 1;
pub fn socket_path() -> PathBuf;                       // honours MBAR_AEROSPACE_SOCKET
pub struct Answer { pub exit_code: i32, pub stdout: String, pub stderr: String,
                    pub server_version: Option<String> }
pub enum Error { NotRunning(String), Protocol(String), Io(String) }
/// One command (socket, else CLI). Blocking: call it off the main thread.
pub fn run(args: &[String]) -> Result<Answer, Error>;
/// Background subscription with reconnect; dropping it stops the thread and the
/// CLI child.
pub struct Subscription { .. }
pub fn subscribe(
    on_event: impl Fn(mbar_core::aerospace::AerospaceEvent) + Send + 'static,
    on_status: impl Fn(mbar_core::aerospace::AerospaceStatus) + Send + 'static,
) -> Subscription;
```

## Core (`crates/mbar-core`)

- `aerospace.rs`: `AerospaceEvent` (+ JSON parsing, event names, env, INFO),
  `AerospaceStatus` (exists).
- Runtime:
  - `Input::Aerospace(ev)` → triggers `ev.event_name()` for its subscribers with
    `ev.env()` and `INFO = ev.info_json()`, exactly like other built-in events
    (scripts and Lua handlers); updates the stored state (focused workspace,
    previous workspace, mode, monitor) and `provider=aerospace` items.
  - `Input::AerospaceStatus(s)` → stored for `--query aerospace`.
  - First subscription to an `aerospace_*` event, first `provider=aerospace`,
    first `--query aerospace`, or a Lua request → one
    `Effect::Platform(PlatformRequest::StartAerospace)` (exists).
  - `--add event aerospace_*` is a silent no-op for the built-in names.
  - State survives `--reload` (the connection does too).

## Binary (`crates/mbar`)

- The driver handles `PlatformRequest::StartAerospace` itself (both platforms,
  like `SetHotload`): it starts one `mbar_aerospace::subscribe` and feeds its
  callbacks into the event queue as `Input::Aerospace` / `Input::AerospaceStatus`.
- Lua `mbar.aerospace.{run,query,on}` (crates/mbar-lua + driver): commands on a
  worker thread, results back as Lua callbacks.

## mbar.app

- System page "AeroSpace" section: status from `--query aerospace` (connected,
  transport, version, focused workspace, mode).
- Setup take-over step: detects `exec-on-workspace-change` lines in
  `aerospace.toml` that run `sketchybar --trigger aerospace_workspace_change` and
  advises removing them (mbar delivers the event itself).

## Work packages

| WP | Scope | Files |
|---|---|---|
| A | transport crate | `crates/mbar-aerospace/**`, workspace `Cargo.toml` member |
| B | core runtime | `crates/mbar-core/**` |
| C | binary + Lua (after A, B) | `crates/mbar/**`, `crates/mbar-lua/**`, `lua/mbar.d.lua` |
| D | mbar.app + docs | `crates/mbar-ui/**`, `docs/*.md`, `README.md` |

## Verification

Linux: fmt, clippy `-D warnings`, `cargo test --workspace` (fake AeroSpace server
for transport and an end-to-end daemon test), `crates/mbar-ui` tests.
macOS CI builds everything. Real AeroSpace is only exercised on a Mac.
