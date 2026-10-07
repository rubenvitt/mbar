# Installing mbar

mbar runs on macOS. The usual way to install it is the app, `mbar.app`,
from a DMG. It contains the bar daemon, the management UI and the `mbar` and
`sketchybar` commands, and it updates itself. You can also build mbar from
source (below). On Linux mbar only builds a headless daemon, which the tests
use.

## Install the app

1. Download `mbar-<version>.dmg` from the [latest release](https://github.com/rubenvitt/mbar/releases/latest).
2. Open it and drag **mbar** to **Applications**.
3. Open mbar. The setup page walks through:
   - moving the app to Applications, if you opened it from the DMG or
     Downloads,
   - removing Homebrew SketchyBar and older mbar installs (it lists everything first),
   - using your existing `~/.config/sketchybar` config (a SbarLua `sketchybarrc` gets an `init.lua`),
   - installing the `mbar` and `sketchybar` commands (`/etc/paths.d/mbar`, one admin prompt),
   - starting mbar at login (System Settings → General → Login Items shows "mbar"),
   - Accessibility and Screen Recording.

Updates: mbar checks for new versions once a day and opens the update dialog by itself.
Turn this off on the System page. Open terminals after setup see the new commands;
tools started without a login shell (AeroSpace, skhd, …) need the full path
`/Applications/mbar.app/Contents/Resources/bin/sketchybar`.

The app needs macOS 13 or later. More about it:

- Every setup step shows whether it is done, open or failed (with the
  command's output), and can be skipped and retried. Setup never deletes or
  edits your configs. It only creates an `init.lua`, either next to a SbarLua
  `sketchybarrc` or, when no config exists at all, a starter config in
  `~/.config/mbar/`. A `sketchybar` that is not an mbar symlink (for example a
  script of your own in `~/.local/bin`) is reported, never removed. Helpers
  that talk to SketchyBar's mach port (`git.felix.*`) are reported with a hint
  (see [`MIGRATING.md`](MIGRATING.md)).
- The System page reopens the setup at any time. If you move or rename the
  app after setup, `/etc/paths.d/mbar` points to the old place. The System page
  shows the entry as stale and fixes it with the same admin prompt.
- The daemon runs as the login item `dev.rubeen.mbar`, from
  `mbar.app/Contents/MacOS/mbar`. It logs to `~/Library/Logs/mbar.log`. Its
  scripts find `mbar` and `sketchybar` first on their `PATH`, followed by
  `/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin`.
- Update checks start two minutes after the daemon starts and repeat every
  24 hours. A new version is offered at most once a day; "Skip This Version"
  and "Remind Me Later" in the dialog are respected. "Check for Updates…" is in
  the app menu and on the System page. After an update the app restarts the
  daemon. Updates are signed (Developer ID and an EdDSA signature on the update
  archive) and checked before they are installed. The switch on the System page
  is the same as `defaults write dev.rubeen.mbar SUEnableAutomaticChecks -bool false`.
- The feed is
  `https://github.com/rubenvitt/mbar/releases/latest/download/appcast.xml`. To
  test against another feed, run `defaults write dev.rubeen.mbar SUFeedURL <url>`
  and `defaults delete dev.rubeen.mbar SUFeedURL` afterwards.

## Build from source

Building from source gives you a plain `mbar` binary, or with `make app` an
app bundle like the released one. This section and the LaunchAgent and
Homebrew sections below are for source builds; the app does all of this in its
setup.

CI uploads the macOS release binary as a workflow artifact (`mbar-macos-ARM64`)
on every run. It is unsigned and meant for testing. Artifacts do not keep file
modes, so after downloading run `chmod +x mbar` and
`xattr -d com.apple.quarantine mbar`.

### Requirements

- macOS on Apple silicon or Intel
- Xcode Command Line Tools (`xcode-select --install`). The full Xcode is not
  needed for mbar, because its Metal shaders are compiled at runtime. mbar-ui's
  GPUI build uses runtime shaders too.
- A current stable Rust toolchain ([rustup](https://rustup.rs)). The workspace
  declares 1.80 as its minimum. The management UI follows GPUI and may need a
  newer one.

Lua 5.4 is vendored and compiled into the binary. Nothing else needs to be
installed.

### Build

```sh
git clone https://github.com/rubenvitt/mbar.git
cd mbar
cargo build --release -p mbar     # or: make release
```

The binary is `target/release/mbar` (cargo's target directory: if you set
`CARGO_TARGET_DIR` or `build.target-dir` in `~/.cargo/config.toml`, it is
there instead; the Makefile's install targets look it up with `cargo metadata`).
It is both the daemon (`mbar` with no
arguments, or with `--config <file>`) and the client (`mbar --set ...`).

### Management UI (optional)

`crates/mbar-ui` is a separate Cargo workspace, because GPUI is a large
dependency and the bar process itself should stay small:

```sh
cd crates/mbar-ui
cargo build --release             # or, from the repository root: make ui
```

The binary is `crates/mbar-ui/target/release/mbar-ui` (or `release/mbar-ui`
in your configured target directory). `mbar-ui --bar-name <name>` selects a bar
other than the default `mbar`. Built this way the UI is a plain executable;
setup, auto-updates and the login item need the app bundle.

### Build the app bundle

```sh
make app          # -> dist/mbar.app
make verify-app   # structural checks of the bundle
```

`make app` builds universal (`arm64` + `x86_64`) release binaries of `mbar` and
`mbar-ui`, downloads the pinned Sparkle framework and assembles and signs
`dist/mbar.app`. Both Rust targets must be installed
(`rustup target add aarch64-apple-darwin x86_64-apple-darwin`), or build for the
host only with `MBAR_UNIVERSAL=0`. Without `MBAR_SIGN_IDENTITY` the bundle is
signed ad hoc, which is enough to run it on your own Mac. Once
`packaging/macos/sparkle.env` carries the project's public EdDSA key, a locally
built bundle checks the official feed like the released app, so it may offer to
update itself to the latest release; turn automatic checks off on the System page
to keep your build. With an empty key Sparkle does not start at all, so the bundle
never checks for updates.
`make dmg` (needs a Developer ID identity in `MBAR_SIGN_IDENTITY`, the Sparkle
EdDSA private key in the login keychain or in `SPARKLE_KEY_FILE`, and
notarization credentials unless `NOTARIZE=0`) builds the DMG, the update zip and
`appcast.xml` in `dist/`, as the release workflow does.

### Install

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
install -m 755 target/release/mbar ~/.local/bin/mbar   # or your configured target directory
ln -sf mbar ~/.local/bin/sketchybar
```

Make sure the directory is on your `PATH`.

#### The `sketchybar` symlink

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

This applies to the app and to source builds.

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

## Start at login (LaunchAgent, for source builds)

The app starts mbar at login by itself (its own login item). Use this
LaunchAgent only for a source build. Both use the label `dev.rubeen.mbar`, so
only one of them can be loaded: the app's setup boots out and removes
`~/Library/LaunchAgents/dev.rubeen.mbar.plist` when it finds one.

```sh
make install-agent            # uses $(PREFIX)/bin/mbar; PREFIX defaults to /usr/local
make install-agent PREFIX=$HOME/.local
```

This fills in `packaging/dev.rubeen.mbar.plist`, writes it to
`~/Library/LaunchAgents/dev.rubeen.mbar.plist`, and loads it with
`launchctl bootstrap`. The agent:

- uses the label `dev.rubeen.mbar`, the same label as the app's login item,
  so the two never start two daemons. The management UI no longer writes this
  file; use `make install-agent`.
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

### Homebrew (draft, for source builds)

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
`brew services` or `make install-agent`, not both. The app's setup stops the
`homebrew.mxcl.mbar` service and uninstalls the formula.

## Permissions

With the app, the setup page and the System page show the live state of
Accessibility and Screen Recording and open the right Settings pane. The setup
page restarts the daemon after a grant. The released app is signed with a
Developer ID, so the grants survive updates. The rest of this section matters
mostly for source builds.

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

A source build has only an ad-hoc code signature. After you rebuild or reinstall it,
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

### The app

1. On the System page, turn off "Launch at login" and quit mbar. If the bar
   is still running, stop it with `mbar --exit`.
2. Remove the command-line entry: `sudo rm /etc/paths.d/mbar`.
3. Drag `mbar.app` from Applications to the Trash.
4. Optionally remove its state and settings:

   ```sh
   rm -rf ~/Library/Application\ Support/mbar
   rm -f ~/Library/Logs/mbar.log
   defaults delete dev.rubeen.mbar
   ```

Then remove mbar from System Settings → Privacy & Security → Accessibility and
Screen Recording. Your config in `~/.config/mbar/` (or `~/.config/sketchybar/`)
is left in place.

### Source builds

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
