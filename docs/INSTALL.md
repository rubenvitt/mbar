# Installing mbar

mbar runs on macOS. On Linux it only builds a headless daemon, which the tests
use. There are no prebuilt releases yet, so you build it from source.

CI uploads the macOS release binary as a workflow artifact (`mbar-macos-ARM64`)
on every run. It is unsigned and meant for testing. Artifacts do not keep file
modes, so after downloading run `chmod +x mbar` and
`xattr -d com.apple.quarantine mbar`.

## Requirements

- macOS on Apple silicon or Intel
- Xcode Command Line Tools (`xcode-select --install`). The full Xcode is not
  needed for mbar, because its Metal shaders are compiled at runtime. mbar-ui's
  GPUI build uses runtime shaders too.
- A current stable Rust toolchain ([rustup](https://rustup.rs)). The workspace
  declares 1.80 as its minimum. The management UI follows GPUI and may need a
  newer one.

Lua 5.4 is vendored and compiled into the binary. Nothing else needs to be
installed.

## Build from source

```sh
git clone https://github.com/rubenvitt/mbar.git
cd mbar
cargo build --release -p mbar     # or: make release
```

The binary is `target/release/mbar`. It is both the daemon (`mbar` with no
arguments, or with `--config <file>`) and the client (`mbar --set ...`).

### Management UI (optional)

`crates/mbar-ui` is a separate Cargo workspace, because GPUI is a large
dependency and the bar process itself should stay small:

```sh
cd crates/mbar-ui
cargo build --release             # or, from the repository root: make ui
```

The binary is `crates/mbar-ui/target/release/mbar-ui`. `mbar-ui --bar-name <name>`
selects a bar other than the default `mbar`. The UI is a plain executable, not an
`.app` bundle.

## Install

`make install` copies the binary from `make release` and creates the
`sketchybar` symlink. It does not build anything, so it can run under `sudo`:

```sh
make release
make install PREFIX=$HOME/.local     # -> ~/.local/bin/mbar, ~/.local/bin/sketchybar
sudo make install                    # PREFIX defaults to /usr/local
make ui && make install-ui PREFIX=$HOME/.local   # optional: mbar-ui
```

It also installs the LuaLS definitions to `$PREFIX/share/mbar/mbar.d.lua`.
To install by hand:

```sh
install -d ~/.local/bin
install -m 755 target/release/mbar ~/.local/bin/mbar
ln -sf mbar ~/.local/bin/sketchybar
```

Make sure the directory is on your `PATH`.

### The `sketchybar` symlink

SketchyBar plugins call `sketchybar --set ...`. With a `sketchybar -> mbar`
symlink on the `PATH`, those calls reach mbar unchanged. Invoked as `sketchybar`,
the binary talks to the default bar `mbar` and prints SketchyBar's own `--help`
and `--version` text. A daemon started through the symlink also runs as bar
`mbar`, but its scripts see `BAR_NAME=sketchybar`.

- Plugins run with the **daemon's** environment, so the symlink must be on the
  daemon's `PATH`. `make install-agent` sets the agent's `PATH` to the install
  directory followed by
  `/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin`.
- If SketchyBar is also installed (for example via Homebrew), whichever
  `sketchybar` comes first on the `PATH` wins. Uninstall SketchyBar, or make sure
  the symlink comes first. Skip the symlink with `make install SKETCHYBAR_LINK=0`.

Any other name for the binary (for example a `bottom -> mbar` symlink) starts an
independent bar instance with its own config directory (`~/.config/bottom/`),
lock file and socket.

## Configuration location

The daemon uses the first regular file it finds:

1. `--config <file>` / `-c <file>`
2. `$XDG_CONFIG_HOME/mbar/init.lua`, `mbarrc`, `sketchybarrc`
3. `~/.config/mbar/init.lua`, `mbarrc`, `sketchybarrc`
4. `$XDG_CONFIG_HOME/sketchybar/init.lua`, `sketchybarrc`
5. `~/.config/sketchybar/init.lua`, `sketchybarrc`
6. `~/.sketchybarrc`

`init.lua` wins over a shell config in the same directory. For another bar name,
steps 2 and 3 use that name instead of `mbar`, and steps 4 and 5 are skipped. As in SketchyBar, the daemon sets the
owner's execute bit on a shell config and runs it with `CONFIG_DIR` set to its
directory.

## Start at login (LaunchAgent)

```sh
make install-agent            # uses $(PREFIX)/bin/mbar; PREFIX defaults to /usr/local
make install-agent PREFIX=$HOME/.local
```

This fills in `packaging/dev.rubeen.mbar.plist`, writes it to
`~/Library/LaunchAgents/dev.rubeen.mbar.plist`, and loads it with
`launchctl bootstrap`. The agent:

- uses the label `dev.rubeen.mbar`, the same label as mbar.app's login item,
  so the two never start two daemons. The management UI no longer writes this
  file; mbar.app's setup boots it out and removes it in favour of the login item.
- starts at login (`RunAtLoad`) and restarts mbar if it crashes
  (`KeepAlive` → `SuccessfulExit=false`). `mbar --exit` exits with status 0, so
  it stays stopped.
- runs with `ProcessType=Interactive` so macOS does not throttle it.
- logs stdout and stderr to `~/Library/Logs/mbar.log`. Set `MBAR_LOG` to
  `error`, `warn` (the default), `info`, `debug` or `trace` in the plist's
  `EnvironmentVariables` for more detail.

To do the same by hand:

```sh
sed -e "s|@MBAR_BIN@|$HOME/.local/bin/mbar|g" \
    -e "s|@HOME@|$HOME|g" \
    -e "s|@PATH@|$HOME/.local/bin:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin|g" \
    packaging/dev.rubeen.mbar.plist > ~/Library/LaunchAgents/dev.rubeen.mbar.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.rubeen.mbar.plist
```

Day-to-day commands:

```sh
launchctl kickstart -k gui/$(id -u)/dev.rubeen.mbar   # restart
launchctl bootout gui/$(id -u)/dev.rubeen.mbar        # stop and unload
mbar --reload                                         # reload the config without restarting
tail -f ~/Library/Logs/mbar.log
```

Only one daemon per user and bar name can run. A second one exits with
`mbar: could not acquire lock-file... already running?`. Stop the agent before
running `mbar` in a terminal.

### Homebrew (draft)

`packaging/homebrew/mbar.rb` is a draft formula that builds from git `HEAD`.
Recent Homebrew versions only install formulae from a tap, so put it in a local
tap:

```sh
brew tap-new "$USER/local"
cp packaging/homebrew/mbar.rb "$(brew --repository "$USER/local")/Formula/"
brew install --HEAD "$USER/local/mbar"
brew services start mbar
```

The formula does not create the `sketchybar` symlink, because that would
conflict with the `sketchybar` formula. Its caveats show the command for it.

`brew services` uses its own launchd label (`homebrew.mxcl.mbar`). Use either
`brew services` or `make install-agent`, not both.

## Permissions

macOS grants privacy permissions to the **responsible process**. When launchd
starts mbar, that is the `mbar` binary itself. When you start mbar from a
terminal, it is the terminal app (Terminal, iTerm, ...), so a permission granted
to the terminal does not carry over to the LaunchAgent, and the reverse is also
true.

Open the settings with:

```sh
open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
open "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
```

or go to System Settings → Privacy & Security. To add the binary, click **+**,
press <kbd>⌘</kbd><kbd>⇧</kbd><kbd>G</kbd> and enter its path (for example
`~/.local/bin/mbar`; add the real file, not the `sketchybar` symlink). Then
restart mbar (`launchctl kickstart -k gui/$(id -u)/dev.rubeen.mbar`).

The binary has only an ad-hoc code signature. After you rebuild or reinstall it,
macOS may treat it as a different program and keep showing the permission as
missing. If that happens, remove the entry with **−** and add it again.

The management UI's System page shows the current state of both permissions and
links to these panes.

| Permission | Needed for | Without it |
|---|---|---|
| **Accessibility** | `app_menu` items, `--menu`, `--query menus`, the `menus_change` event | `app_menu` draws nothing and `--query menus` answers with an error. mbar does not show a prompt, so add it by hand. mbar picks up the grant at the next front-app switch. |
| **Screen Recording** | `alias` items (capturing menu extras) and `--query default_menu_items` | Aliases stay empty, and the query prints SketchyBar's "Screen Recording Permissions not given" error. mbar asks once per process when the first alias is added. Restart mbar after you grant it. |
| **Location Services** | The Wi-Fi SSID (`wifi_change` `INFO`, the `wifi` provider's `{ssid}`) on macOS 14 and later | See below. |

### Wi-Fi SSID

Since macOS 14, CoreWLAN hides the SSID from processes that are not authorized
for Location Services. mbar does not request Location access itself, and macOS
only lists programs in Location Services after they have asked. So the usual
"grant the permission" step is not available for mbar. Instead, mbar falls back
to `ipconfig getsummary <interface>`:

- On **macOS 14**, the fallback works without further setup.
- On **macOS 15 and later**, `ipconfig` prints `<redacted>` unless verbose mode
  was enabled once as root:

  ```sh
  sudo ipconfig setverbose 1
  ```

  If the SSID goes blank again after a macOS update, run the command again.

If neither method works, the SSID is empty, as it is in SketchyBar. The `wifi`
provider's `{rssi}` and the `wifi_change` event itself are not affected.

## Hiding the native menu bar

mbar can replace the menu bar. To hide the native one:

```sh
mbar --menubar hide      # also: show, toggle
```

or put it in the config with `mbar --bar hide_menubar=on` (Lua:
`mbar.bar({ hide_menubar = true })`). Both turn on macOS's "Automatically hide
and show the menu bar" setting (`_HIHideMenuBar`). When mbar exits normally, it
**restores** the value it found at startup.

To hide the menu bar permanently, independent of mbar, use System Settings →
Control Center → "Automatically hide and show the menu bar" → **Always**. On
older macOS versions the setting is under Desktop & Dock or General.

The native menu bar still slides in when the pointer touches the top edge of the
screen. On displays with a notch, the bar property `notch_width` (default 200)
reserves the gap around the notch for `q`/`e` items, as in SketchyBar.

## Uninstall

```sh
make uninstall PREFIX=$HOME/.local   # stops and removes the LaunchAgent and the installed files
```

For a system-wide install, remove the agent as your own user first, then the
files as root:

```sh
make uninstall-agent
sudo make uninstall
```

or by hand:

```sh
launchctl bootout gui/$(id -u)/dev.rubeen.mbar 2>/dev/null
rm -f ~/Library/LaunchAgents/dev.rubeen.mbar.plist
rm -f ~/.local/bin/mbar ~/.local/bin/sketchybar ~/.local/bin/mbar-ui   # only if sketchybar is the mbar symlink
rm -rf ~/.local/share/mbar
rm -f ~/Library/Logs/mbar.log
```

Then remove `mbar` from System Settings → Privacy & Security → Accessibility and
Screen Recording. If mbar crashed while it was hiding the menu bar, turn the
setting back off with `defaults write NSGlobalDomain _HIHideMenuBar -bool false`
and log out and back in, or switch it off in System Settings. Your config in
`~/.config/mbar/` is left in place.
