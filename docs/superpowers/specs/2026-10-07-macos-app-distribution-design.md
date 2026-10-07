# mbar.app: DMG distribution, onboarding and auto-updates

Status: approved design (2026-10-07). Next step: implementation plan.

## Goal

mbar becomes a regular Mac app. A user downloads one DMG from GitHub Releases,
drags `mbar.app` to `/Applications`, opens it once, and from then on:

- the bar daemon runs and starts at login,
- `mbar` and `sketchybar` are on the `PATH` of every login shell,
- an existing SketchyBar setup has been taken over and removed,
- Accessibility / Screen Recording grants survive updates,
- new versions are offered automatically and install with one click.

Releases are produced by CI from GitHub Releases, signed with the Developer ID
certificate and notarized. Versions, tags and changelogs come from
release-please.

## Non-goals

- Mac App Store distribution (sandboxing rules out the SkyLight/AX use).
- A `.pkg` installer. Commands are set up on first launch instead.
- Publishing crates to crates.io.
- Automatic rewriting of user helpers that look up `git.felix.*` mach ports.
  They are detected and reported only.

## Decisions

| Topic | Decision | Rejected |
|---|---|---|
| Installer | DMG with drag-to-Applications; setup on first launch | `.pkg` (heavy, awkward with Sparkle) |
| `PATH` | `/etc/paths.d/mbar` → `<app>/Contents/Resources/bin`, one admin prompt | symlinks into `/usr/local/bin`, `~/.local/bin` (not on default `PATH`) |
| Daemon start | `SMAppService.agent(plistName:)` with the plist inside the bundle | plist written to `~/Library/LaunchAgents` (today's `make install-agent`) |
| Update check | the always-running daemon checks the appcast | UI resident at login (memory), custom Rust updater (reimplements Sparkle) |
| Update install | Sparkle 2 in `mbar-ui`, opened by the daemon in update mode | — |
| Releases | release-please (release PR → tag + GitHub Release) | semantic-release (every merge ships), release-plz (crates.io focus) |
| Minimum OS | macOS 13 (`SMAppService`) | — |

## Bundle layout

```
mbar.app/Contents/
  Info.plist                    CFBundleIdentifier dev.rubeen.mbar,
                                CFBundleExecutable mbar-ui, SUFeedURL, SUPublicEDKey,
                                LSMinimumSystemVersion 13.0
  MacOS/mbar-ui                 UI, Sparkle host, onboarding (main executable)
  MacOS/mbar                    daemon + client (unchanged binary)
  Resources/bin/mbar            -> ../../MacOS/mbar        (relative symlink)
  Resources/bin/sketchybar      -> ../../MacOS/mbar
  Resources/AppIcon.icns
  Library/LaunchAgents/dev.rubeen.mbar.plist
                                Label dev.rubeen.mbar, BundleProgram Contents/MacOS/mbar,
                                RunAtLoad, KeepAlive {SuccessfulExit=false},
                                ProcessType Interactive, log paths
  Frameworks/Sparkle.framework  pinned version, checksum-verified download
```

The daemon keeps its name `mbar`: the bar name is derived from `argv[0]`, and the
`sketchybar` symlink must keep resolving to bar `mbar` as documented in
`docs/INSTALL.md`.

The agent plist cannot contain the app's absolute path. When the daemon runs
from inside an app bundle it prepends `<bundle>/Contents/Resources/bin` to the
`PATH` its scripts get, followed by the existing default
(`/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin`).

## Components

### Daemon (`crates/mbar`, `crates/mbar-macos`)

New, small and testable units:

- **bundle context**: detects whether the running executable sits in
  `*.app/Contents/MacOS/`, reads the bundle's `CFBundleVersion`,
  `CFBundleShortVersionString` and `SUFeedURL` (honouring a
  `defaults write dev.rubeen.mbar SUFeedURL …` override). Outside a bundle all
  update logic is off.
- **script `PATH`**: prepends the bundle's `Resources/bin` (see above).
- **update checker**: first check 2 min after start, then every 24 h, only when
  running from a signed bundle and the `SUEnableAutomaticChecks` preference of
  `dev.rubeen.mbar` is not `false`. Fetches the appcast with `/usr/bin/curl`
  (no HTTP/TLS stack in the daemon), parses the highest `sparkle:version` whose
  `sparkle:minimumSystemVersion` the system satisfies, compares it with its own
  build number.
- **update state**: `~/Library/Application Support/mbar/update-state.json`
  holds the last offered version + time and the last version it restarted
  itself for.
- **offer**: when a newer version exists and it was not offered in the last
  24 h, the daemon opens the app with `--update` (`NSWorkspace` /
  `open -b dev.rubeen.mbar --args --update`).
- **self-restart fallback**: if the bundle on disk carries a higher
  `CFBundleVersion` than the running daemon (an update was installed but the UI
  did not restart it), the daemon asks launchd to restart it
  (`launchctl kickstart -k gui/<uid>/dev.rubeen.mbar`), at most once per version.

### UI (`crates/mbar-ui`)

- **launch modes**: normal (main window, Dock icon) and `--update` (no main
  window, accessory activation policy; quits when the update cycle ends).
- **Sparkle bridge**: loads `Sparkle.framework`, creates an
  `SPUStandardUpdaterController`. `--update` calls
  `checkForUpdatesInBackground` (respects "Skip This Version" and "Remind Me
  Later"); a delegate (`updater:didFinishUpdateCycleFor:error:`) terminates the
  app in update mode. "Check for Updates…" lives in the app menu and on the
  System page, plus the "Automatically check for updates" toggle
  (`SUEnableAutomaticChecks`).
- **daemon version sync**: on every launch, compare the running daemon's
  version (IPC) with the bundle's; on mismatch `launchctl kickstart -k`.
- **login item**: replaces today's plist writer in `crates/mbar-ui/src/system.rs`
  with `SMAppService` (`register`, `unregister`, `status`), including the
  `requiresApproval` state with a link to System Settings → Login Items.
- **onboarding** (below).

### Onboarding

Runs on first launch (no completed flag in the app's defaults) and can be
reopened from the System page. Every step shows its state (done / open /
failed with output), can be skipped and retried. Nothing is skipped silently.

1. **App location**: if the app runs from the DMG or is translocated, offer to
   move it to `/Applications` and relaunch.
2. **Clean up old installations** (shows the list before removing anything,
   never touches configs):
   - Homebrew SketchyBar: `brew services stop sketchybar`, then
     `brew uninstall sketchybar` after confirmation.
   - Old mbar installs: `~/.local/bin/{mbar,sketchybar,mbar-ui}` and the same
     names in `/usr/local/bin` when they point to an mbar binary outside this
     app, `~/Library/LaunchAgents/dev.rubeen.mbar.plist` (booted out first), the
     Homebrew formula `mbar` / `homebrew.mxcl.mbar` service.
   - Any other `sketchybar` that comes before the app's `bin` on a login
     shell's `PATH` is reported.
3. **Take over the SketchyBar config**: configs stay where they are (lookup
   order in `docs/INSTALL.md`). A SbarLua `sketchybarrc` without `init.lua` gets
   an `init.lua` with the shebang and `package.cpath` lines removed (as in
   `docs/MIGRATING.md`). Helpers that look up `git.felix.` are reported with a
   hint.
4. **Starter config**: no config found → write an example `init.lua` to
   `~/.config/mbar/`.
5. **Command line**: write `/etc/paths.d/mbar` (one admin prompt via
   `osascript … with administrator privileges`), then verify with a fresh login
   shell that `sketchybar` resolves into the app. A stale entry (app moved) is
   detected and fixed the same way.
6. **Login item + daemon**: `SMAppService` register, start, confirm via IPC.
7. **Permissions**: live status for Accessibility and Screen Recording with
   buttons that open the right Settings pane; restart the daemon after a grant.

## Update flow

1. CI publishes `mbar-<version>.dmg`, `mbar-<version>.zip` and `appcast.xml` on
   the GitHub Release. The feed URL is
   `https://github.com/rubenvitt/mbar/releases/latest/download/appcast.xml`.
2. The daemon finds a newer `sparkle:version` and opens the app with `--update`.
3. Sparkle shows its dialog (or nothing, if skipped / postponed); the app quits
   afterwards.
4. On "Install", Sparkle verifies the EdDSA signature and the Developer ID,
   replaces the bundle and relaunches the app.
5. The relaunched app sees a daemon version mismatch and restarts the daemon.
   Fallback: the daemon restarts itself on its next check.

## Build, signing, release

### Local

- `make app`: universal (`arm64` + `x86_64`, `lipo`) release builds of `mbar`
  and `mbar-ui`, bundle assembly by `packaging/macos/build-app.sh`,
  `Info.plist` from a template (`CFBundleShortVersionString` = workspace
  version, `CFBundleVersion` = monotonically increasing build number), signing
  inside-out with hardened runtime and timestamp (Sparkle's XPC services and
  Autoupdate, `mbar`, `mbar-ui`, the bundle). Identity from
  `MBAR_SIGN_IDENTITY`, ad-hoc (`-`) when unset.
- `make dmg`: DMG with an `/Applications` link, signed; notarized and stapled
  (`notarytool`, `stapler`) unless `NOTARIZE=0`. Also produces the update
  `.zip`.
- Cargo's global `target-dir` (as on the maintainer's machine) is respected:
  the script asks `cargo metadata` for the target directory instead of assuming
  `target/`. The same fix applies to `make install` / `make install-ui`.

### CI

- `ci.yml` (PRs and `main`): additionally runs `make app` with ad-hoc signing
  and checks it (`codesign --verify --deep --strict`, required `Info.plist`
  keys, relative symlinks, bundle structure). Adds a Conventional Commits
  check on PR titles / commits.
- `release.yml` (push to `main`):
  - job `release-please`: maintains the release PR (version in `Cargo.toml`
    files, `CHANGELOG.md`); after its merge it creates the tag `vX.Y.Z` and the
    GitHub Release.
  - job `build` (only when a release was created, same workflow because
    releases created with `GITHUB_TOKEN` trigger no other workflows): import the
    `.p12` into a temporary keychain, `make dmg`, notarize + staple, sign the
    `.zip` with Sparkle's `sign_update`, generate `appcast.xml` (release notes
    from the GitHub Release body), upload DMG, ZIP and appcast to the release.
- From now on commits follow Conventional Commits (`feat:`, `fix:`, `feat!:`).

### Credentials (set up by Claude during implementation)

| Secret | Source |
|---|---|
| `MACOS_CERT_P12`, `MACOS_CERT_PASSWORD` | export of the existing "Developer ID Application: Ruben Vitt (H95J852PKP)" identity from the login keychain |
| `ASC_KEY_ID`, `ASC_ISSUER_ID`, `ASC_KEY_P8` | App Store Connect API key (Developer role), created in the browser |
| `SPARKLE_ED_PRIVATE_KEY` | Sparkle `generate_keys`; private key also kept in the login keychain, public key goes into `Info.plist` |

Secrets are set with `gh secret set`. Private keys never enter the repository
or the conversation.

## Error handling

- Update check failures (offline, `curl` error, malformed appcast): warning in
  the log, retry at the next interval, never open the app.
- No restart loops: the daemon restarts only for a higher bundle version, once
  per version (`update-state.json`).
- Dev builds (no bundle, no `SUFeedURL`): no automatic checks.
- Onboarding failures (admin prompt cancelled, login item needs approval,
  `brew uninstall` fails): the step stays open with its output and a retry
  button.
- Background Sparkle errors are silent; manual checks show Sparkle's dialog.

## Testing

- Rust unit tests: appcast parsing and version comparison, offer rate limit,
  bundle detection and script `PATH`, `paths.d` content and stale detection,
  old-install detection against a temporary `HOME`, SbarLua `sketchybarrc` →
  `init.lua` conversion.
- CI bundle check on every PR (see above).
- Update test without a real release: `SUFeedURL` override plus a local HTTP
  server serving an appcast for build N+1; verify the dialog opens by itself,
  the update installs, the daemon restarts, grants are kept.
- End-to-end on the maintainer's Mac: install from the DMG and let onboarding
  remove the manual install (`~/.local/bin/{mbar,sketchybar,mbar-ui}`, the old
  agent plist) and Homebrew SketchyBar; then check AeroSpace triggers, popups,
  app menus (Accessibility) and the update path.

## Maintainer setup changes (dotfiles)

- `aerospace/aerospace.toml`: AeroSpace runs commands without a login shell and
  does not read `/etc/paths.d`, so the triggers call
  `/Applications/mbar.app/Contents/Resources/bin/sketchybar`.
- `r-tools/sbar`: `restart` no longer requires a plist in
  `~/Library/LaunchAgents`; it uses `launchctl kickstart -k` on
  `gui/<uid>/dev.rubeen.mbar`.
- `docs/INSTALL.md` gets a "Install the app" section before "Build from
  source"; the LaunchAgent and Homebrew sections stay for source builds.

## Risks to verify early

- Sparkle inside a GPUI app: GPUI owns `NSApplication`; the Sparkle controller
  and its delegate must be created on the main thread after GPUI's app launch.
- Universal build of `mbar-ui` (GPUI) for `x86_64`.
- TCC attribution: grants should show as "mbar" (the app) for the
  `SMAppService` agent and survive an update; verify with two signed builds.
- Accessory activation policy for `--update` with GPUI (no Dock icon flash).
