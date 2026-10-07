# mbar.app packaging spike — results (2026-10-07)

Throwaway spike for Task 1 of `2026-10-07-macos-app-distribution.md`, run on macOS 27.0.1 (arm64).
The hand-built bundle and scratch code are deleted; only these notes remain.

## Sparkle

- Version: **2.10.0** (latest GitHub release at the time).
- Archive: `https://github.com/sparkle-project/Sparkle/releases/download/2.10.0/Sparkle-2.10.0.tar.xz`
- SHA-256: `c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c`
- Layout matches the plan: `Sparkle.framework/Versions/B/{Autoupdate,Updater.app,XPCServices/{Downloader,Installer}.xpc}`, tools in `bin/{generate_appcast,generate_keys,sign_update,BinaryDelta}`.
- Inside-out signing loop from the plan (XPC services, Autoupdate, Updater.app, framework, binaries, app) with `-o runtime --timestamp` gives `valid on disk` / `satisfies its Designated Requirement`.

### Selectors that work (objc2 0.6, raw `msg_send!`)

- Load: `NSBundle::mainBundle().privateFrameworksPath()` + `Sparkle.framework`, `bundleWithPath`, `load` → `true`.
- `SPUStandardUpdaterController alloc` → `initWithStartingUpdater:updaterDelegate:userDriverDelegate:` (Rust `bool` is accepted for the first argument).
- `[controller updater]` → `SPUUpdater`.
- `[updater startUpdater:&error]` (when the controller is created with `false`) returns `BOOL` + `NSError`; useful for logging why Sparkle refuses to start.
- `[updater canCheckForUpdates]`, `automaticallyChecksForUpdates`, `feedURL` all respond.
- `[updater checkForUpdatesInBackground]` shows the standard "Software Update" window when an update is found.
- `[controller checkForUpdates:nil]` (interactive) works too.
- Both were sent via `performSelector:withObject:afterDelay:` a few seconds after start; calling directly right after init is ignored because `canCheckForUpdates` is still `false` then.

### Findings that change the plan

1. **`SUPublicEDKey` is mandatory.** Without it `startUpdater` fails with
   `SUSparkleErrorDomain Code=1 "For security reasons, updates need to be signed with an EdDSA key for mbar."`
   With `initWithStartingUpdater: true` this failure is **silent** (no alert, no log line, no feed request).
   Developer ID code signing alone is not accepted by 2.10.0. Task 8 must put `SUPublicEDKey` into `Info.plist`
   for every bundle that should update, including local test bundles (Task 23).
   Recommendation for Task 14: create the controller with `startingUpdater: false`, call `startUpdater:` explicitly and log the `NSError`.
2. An EdDSA key does not have to come from `generate_keys` for tests: an OpenSSL Ed25519 key works
   (`SUPublicEDKey` = base64 of the raw 32-byte public key; `sparkle:edSignature` = base64 of
   `openssl pkeyutl -sign -rawin` over the archive). Handy for Task 23 without touching the keychain.
3. Plain `http://127.0.0.1:8765/appcast.xml` works as feed (no ATS exception needed for loopback).
4. Clicking "Install Update" with a dummy archive (no app inside) downloads it, shows "Updating mbar" briefly and then closes all Sparkle windows without a visible error; the bundle stays unchanged. The real install path is covered by Task 23.

## Activation policy and focus

- `NSApplication::sharedApplication(mtm).setActivationPolicy(Accessory)` inside GPUI's `run` callback, before the window opens, returns `true`; no Dock icon.
- The Sparkle "Software Update" window is ordered above other apps' windows, but the app does **not** become frontmost (`lsappinfo front` still the previous app), so it has no keyboard focus.
  → Call `NSApp.activate()` (or `activateIgnoringOtherApps`) right before showing the update UI in Task 14.

## SMAppService

- Agent plist in `Contents/Library/LaunchAgents/`, `BundleProgram = Contents/MacOS/mbar`.
- Status before `register()`: `3` (`notFound`), after: `1` (`enabled`). No approval prompt, no `requiresApproval` on this machine.
- `launchctl print gui/$UID/<label>` shows `state = running`, `program identifier = Contents/MacOS/mbar`, the process' executable is the bundle's `Contents/MacOS/mbar`.
- `ProgramArguments[0]` is honoured as `argv[0]` (the spike used `mbarspike` to keep a separate bar name next to the live bar).
- `StandardErrorPath` with an absolute path works in the bundled plist (only `~` cannot be used; the daemon-side log redirect from the plan stays necessary).
- Background Task Management (`sfltool dumpbtm`) lists the app as "mbar", developer "Ruben Vitt", team `H95J852PKP`, and the agent as a child; the user gets the "background item added" notification.
- An existing hand-installed `~/Library/LaunchAgents/dev.rubeen.mbar.plist` also shows up as "mbar" — two "mbar" entries after migration unless onboarding removes the old agent (Task 11/24).

## Universal build

- `rustup target add x86_64-apple-darwin`, then `cargo build --release -p mbar --target x86_64-apple-darwin` and, in `crates/mbar-ui`, `cargo build --release --target x86_64-apple-darwin`: **both succeed** (only the known `block v0.1.6` future-incompat warning).
- `lipo -create` of the arm64 and x86_64 `mbar-ui` gives a fat binary (`x86_64 arm64`); the x86_64 slice runs under Rosetta (`--help`).
- So Task 7 can build both binaries universal; no arm64-only fallback needed.
- Mind the target directory: `~/.cargo/config.toml` points cargo to `~/.cache/cargo-target/` here, so per-target outputs are in `$T/x86_64-apple-darwin/release/` (Task 7).

## TCC (Accessibility)

- `--query menus` needs the windowed daemon: under `--headless` it always returns `[]`, with or without permission. The spike agent therefore ran windowed with a config that only does `mbar.bar({ hidden = "on" })`.
- Granting Accessibility to the **app bundle** (System Settings → Privacy & Security → Accessibility → "+" → `mbar.app`, shown as "mbar") is attributed to the agent process `Contents/MacOS/mbar` started via SMAppService: `--query menus` returns the menu titles.
- Rebuilt `mbar` with a different CDHash (`95fa24d5…` → `1978165f…`), re-signed with the same Developer ID, `launchctl kickstart -k`: menus still returned, **no re-grant needed**. The designated requirement is identity + team (`certificate leaf[subject.OU] = H95J852PKP`), not the hash.
- A fat (arm64 + x86_64) `mbar` signed the same way also kept the grant.
- `codesign` without `--identifier` gives the binaries the identifier `mbar` / `mbar-ui`; Task 8 should keep that stable (or pass `--identifier dev.rubeen.mbar.<name>` explicitly) because the designated requirement includes it.
- `tccutil reset Accessibility dev.rubeen.mbar.spike` fails with `OSStatus -10814` ("No such bundle identifier"), even after `lsregister -f`; the entry has to be removed by hand in System Settings. Onboarding should not rely on `tccutil` for cleanup.
- After `unregister()` the Background Task Management entries stay in `sfltool dumpbtm` (disabled); they are harmless and not removed by the app.
