# mbar.app Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship mbar as a signed, notarized `mbar.app` in a DMG, with first-launch onboarding, `mbar`/`sketchybar` on the `PATH`, a login-item daemon, and daemon-triggered Sparkle auto-updates released by release-please.

**Architecture:** Pure, platform-independent logic (version numbers, bundle/Info.plist reading, appcast parsing, update decisions, config lookup, onboarding detection and command building) lives in a new crate `mbar-app` and in `mbar_ui_model`, unit-tested on any host. Thin macOS glue (curl/launchctl/open runners, SMAppService, Sparkle via objc2, activation policy, distributed notifications) sits on top. Shell scripts under `packaging/macos/` assemble, sign, notarize and package the bundle; GitHub Actions runs release-please and the signed build.

**Tech Stack:** Rust 1.80+ (root workspace), GPUI/gpui-kit 0.7 (`crates/mbar-ui`, own workspace), objc2 0.6 + objc2-foundation/app-kit 0.3, Sparkle 2, bash, `codesign`/`notarytool`/`stapler`/`hdiutil`, release-please-action v4.

**Spec:** `docs/superpowers/specs/2026-10-07-macos-app-distribution-design.md`

## Where each task can run

| Host | Tasks | Why |
|---|---|---|
| **Linux VM** | 2, 3, 4, 5, 10, 11, 17, 18 | Pure Rust logic, `cargo test` (root) and `cargo test --no-default-features` (mbar-ui), YAML/JSON/Markdown |
| **Linux VM for logic + tests, macOS for the glue** | 6, 12, 15 | The decision/command code and its tests run anywhere; the `#[cfg(target_os = "macos")]` part (launchctl, `open`, AppKit) needs a Mac to compile and run |
| **Linux VM to write, macOS to run** | 7, 9, 19, 20 | Scripts and workflows can be authored anywhere (Task 9's appcast test runs on Linux); they execute on macOS (locally or on a `macos-*` runner) |
| **macOS required** | 1, 8, 13, 14, 16, 21, 22, 23, 24 | `codesign`, `lipo`, `hdiutil`, notarization, Sparkle, SMAppService, AppKit/objc2, GPUI on macOS, keychain export, browser login to App Store Connect, end-to-end tests |

Every task header repeats its host as **Host:**. A Linux-VM executor skips tasks marked macOS and hands them back.

## Global Constraints

- Minimum OS for the app: macOS 13.0 (`LSMinimumSystemVersion 13.0`, `sparkle:minimumSystemVersion 13.0`).
- Bundle identifier and launchd label: `dev.rubeen.mbar`. Agent plist name inside the bundle: `dev.rubeen.mbar.plist`.
- The daemon binary stays named `mbar` (bar name comes from `argv[0]`); the UI binary is `mbar-ui` and is `CFBundleExecutable`.
- `Resources/bin/mbar` and `Resources/bin/sketchybar` are relative symlinks to `../../MacOS/mbar`.
- `/etc/paths.d/mbar` contains exactly one line: `<app>/Contents/Resources/bin`.
- Feed URL: `https://github.com/rubenvitt/mbar/releases/latest/download/appcast.xml`; overridable with `defaults write dev.rubeen.mbar SUFeedURL <url>`.
- `CFBundleVersion` / `sparkle:version` = `major*1_000_000 + minor*1_000 + patch` of the semver version; `CFBundleShortVersionString` / `sparkle:shortVersionString` = the semver string.
- Update check: first 120 s after daemon start, then every 24 h; at most one offer per version per 24 h; self-restart at most once per version. State file: `~/Library/Application Support/mbar/update-state.json`.
- Automatic checks are on unless `defaults read dev.rubeen.mbar SUEnableAutomaticChecks` is false; `Info.plist` ships `SUEnableAutomaticChecks = true` (so Sparkle never asks).
- Update checks run only when the daemon runs from an app bundle whose `Info.plist` has `SUFeedURL` (deliberate simplification of the spec's "signed bundle": a locally built ad-hoc bundle may also update to the official release).
- Releases: release-please, Conventional Commits; tag format `vX.Y.Z`; release assets `mbar-X.Y.Z.dmg`, `mbar-X.Y.Z.zip`, `appcast.xml`.
- Secrets: `MACOS_CERT_P12`, `MACOS_CERT_PASSWORD`, `ASC_KEY_ID`, `ASC_ISSUER_ID`, `ASC_KEY_P8`, `SPARKLE_ED_PRIVATE_KEY`. Signing identity: `Developer ID Application: Ruben Vitt (H95J852PKP)`, team `H95J852PKP`.
- Onboarding never deletes or edits user configs except creating `init.lua` (SbarLua conversion) or a starter `~/.config/mbar/init.lua` when none exists.
- Code, identifiers, commit messages in English; Conventional Commit prefixes from Task 18 on (`feat:`, `fix:`, `chore:`, `docs:`, `ci:`, `build:`, `refactor:`, `test:`).
- Commit trailers on every commit:
  `Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>` and `Claude-Session: https://claude.ai/code/session_019nCwi5V5UPggncxAAJ1wRu`.

## Review Focus

1. **App moved or renamed after setup** (e.g. `~/Applications/mbar.app`): `/etc/paths.d/mbar` goes stale and scripts lose `sketchybar`; expect the System page to show "stale" and offer the fix. Test: Task 11 `paths_d_state_detects_stale_entry`.
2. **Offline / GitHub returns 404 for `appcast.xml`** (window between release creation and asset upload): the daemon must not open the app, must not crash, must retry next interval. Test: Task 6 `fetch_failure_does_nothing`.
3. **The UI is already open when an update is found**: `open --args` would be ignored; expect the running UI to show the Sparkle dialog via the distributed notification. Test: Task 6 `offer_posts_notification_when_ui_running`.
4. **Existing `~/.local/bin/sketchybar` that is not an mbar symlink** (e.g. a user script): onboarding must report it but never delete it. Test: Task 11 `foreign_sketchybar_is_reported_not_removed`.
5. **Bundle version on disk newer than the daemon but the UI never relaunched** (update installed while UI quit early): the daemon restarts itself exactly once for that version, never loops. Test: Task 5 `restart_once_per_version`.

---

### Task 1: macOS feasibility spike (throwaway)

**Host:** macOS required (Sparkle, SMAppService, TCC, GPUI on AppKit).

**Files:**
- Create: `docs/superpowers/plans/2026-10-07-spike-notes.md` (kept)
- Everything else lives in `/tmp/mbar-spike/` and is deleted afterwards.

**Interfaces:**
- Produces: confirmed Sparkle version + selectors, confirmed `SMAppService` behaviour, confirmed universal GPUI build, notes that Tasks 13–16 follow.

- [ ] **Step 1: Pin Sparkle**

```bash
SPARKLE_VERSION=$(gh release view -R sparkle-project/Sparkle --json tagName -q .tagName)
mkdir -p /tmp/mbar-spike && cd /tmp/mbar-spike
curl -fsSLO "https://github.com/sparkle-project/Sparkle/releases/download/${SPARKLE_VERSION}/Sparkle-${SPARKLE_VERSION}.tar.xz"
shasum -a 256 "Sparkle-${SPARKLE_VERSION}.tar.xz"
mkdir sparkle && tar -xJf "Sparkle-${SPARKLE_VERSION}.tar.xz" -C sparkle
ls sparkle/Sparkle.framework/Versions/B sparkle/bin
```

Record version and SHA-256 in the notes file.

- [ ] **Step 2: Universal build check**

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cd ~/dev/personal/mbar && cargo build --release -p mbar --target x86_64-apple-darwin
cd crates/mbar-ui && cargo build --release --target x86_64-apple-darwin
```

Expected: both succeed. If `mbar-ui` fails for x86_64, note the error; Task 7 then builds `mbar-ui` arm64-only and the spec's risk list is updated.

- [ ] **Step 3: Hand-assemble a bundle**

```bash
cd /tmp/mbar-spike
T=$(cargo metadata --manifest-path ~/dev/personal/mbar/Cargo.toml --format-version 1 --no-deps | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')
A=mbar.app/Contents
mkdir -p $A/MacOS $A/Resources/bin $A/Library/LaunchAgents $A/Frameworks
cp $T/release/mbar $T/release/mbar-ui $A/MacOS/
ln -s ../../MacOS/mbar $A/Resources/bin/mbar; ln -s ../../MacOS/mbar $A/Resources/bin/sketchybar
cp -R sparkle/Sparkle.framework $A/Frameworks/
cat > $A/Info.plist <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.rubeen.mbar.spike</string>
<key>CFBundleExecutable</key><string>mbar-ui</string>
<key>CFBundleName</key><string>mbar</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.0.1</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>SUFeedURL</key><string>http://127.0.0.1:8765/appcast.xml</string>
<key>SUEnableAutomaticChecks</key><true/>
</dict></plist>
EOF
cat > $A/Library/LaunchAgents/dev.rubeen.mbar.spike.plist <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>dev.rubeen.mbar.spike</string>
<key>BundleProgram</key><string>Contents/MacOS/mbar</string>
<key>ProgramArguments</key><array><string>mbar</string><string>--headless</string></array>
<key>RunAtLoad</key><true/>
</dict></plist>
EOF
ID="Developer ID Application: Ruben Vitt (H95J852PKP)"
for f in $A/Frameworks/Sparkle.framework/Versions/B/XPCServices/*.xpc $A/Frameworks/Sparkle.framework/Versions/B/Autoupdate $A/Frameworks/Sparkle.framework/Versions/B/Updater.app $A/Frameworks/Sparkle.framework $A/MacOS/mbar $A/MacOS/mbar-ui mbar.app; do
  codesign -f -s "$ID" -o runtime --timestamp "$f"
done
codesign --verify --deep --strict --verbose=2 mbar.app
```

Expected: `valid on disk`, `satisfies its Designated Requirement`.

- [ ] **Step 4: SMAppService probe**

Write `/tmp/mbar-spike/sm.swift` and run it from inside the bundle context (copy the compiled helper to `$A/MacOS/smtest` and sign it the same way):

```swift
import ServiceManagement
let s = SMAppService.agent(plistName: "dev.rubeen.mbar.spike.plist")
print("before:", s.status.rawValue)
do { try s.register() } catch { print("register error:", error) }
print("after:", s.status.rawValue)   // 1 = enabled, 2 = requiresApproval
```

Run `mbar.app/Contents/MacOS/smtest`. Check System Settings → General → Login Items shows "mbar" and `launchctl print gui/$(id -u)/dev.rubeen.mbar.spike` shows a running process from `mbar.app/Contents/MacOS/mbar`. Then `s.unregister()` the same way.

- [ ] **Step 5: Sparkle in GPUI + accessory policy**

Temporarily add to `crates/mbar-ui/src/main.rs` (on a scratch branch, do not commit), inside `run(|cx| …)` before opening the window:

```rust
#[cfg(target_os = "macos")]
unsafe {
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{msg_send, sel};
    use objc2_foundation::{NSBundle, NSString};
    let path = NSBundle::mainBundle().privateFrameworksPath().unwrap();
    let fw = NSBundle::bundleWithPath(&path.stringByAppendingPathComponent(&NSString::from_str("Sparkle.framework"))).unwrap();
    assert!(fw.load());
    let cls = AnyClass::get(c"SPUStandardUpdaterController").unwrap();
    let ctl: *mut AnyObject = msg_send![cls, alloc];
    let ctl: *mut AnyObject = msg_send![ctl, initWithStartingUpdater: true, updaterDelegate: std::ptr::null::<AnyObject>(), userDriverDelegate: std::ptr::null::<AnyObject>()];
    let updater: *mut AnyObject = msg_send![ctl, updater];
    let _: () = msg_send![updater, checkForUpdatesInBackground];
    std::mem::forget(ctl);
}
```

Serve an appcast for version `2` with `python3 -m http.server 8765` (enclosure can point to any zip; the dialog appearing is what matters). Also test `NSApplication::sharedApplication(mtm).setActivationPolicy(NSApplicationActivationPolicy::Accessory)` before the window opens: no Dock icon flash, and whether the Sparkle alert comes to front (if not, note that `NSApp.activate()` is needed).

- [ ] **Step 6: TCC attribution**

With the agent from Step 4 running, grant Accessibility to "mbar" in System Settings, re-sign a rebuilt `mbar` with the same identity, `launchctl kickstart -k` the spike agent, verify `mbar --query menus` (via `$A/Resources/bin/mbar`, adjusting the bar name if the spike uses `--headless` only for IPC) still works without re-granting. Note the TCC entry name shown in Settings.

- [ ] **Step 7: Write notes, clean up**

Fill `docs/superpowers/plans/2026-10-07-spike-notes.md` with: Sparkle version + SHA-256, exact working selectors, whether `NSApp.activate()` was needed, x86_64 GPUI result, SMAppService status values seen, TCC result. Then:

```bash
rm -rf /tmp/mbar-spike
git restore crates/mbar-ui/src/main.rs
git add docs/superpowers/plans/2026-10-07-spike-notes.md
git commit -m "docs: spike notes for mbar.app packaging"
```

---

### Task 2: Version helpers and `version` in `--query stats`

**Host:** Linux VM.

**Files:**
- Create: `crates/mbar-app/Cargo.toml`, `crates/mbar-app/src/lib.rs`, `crates/mbar-app/src/version.rs`
- Modify: `Cargo.toml` (workspace members + dependency), `crates/mbar-core/src/query.rs` (stats fields + test)
- Modify: `docs/EXTENSIONS.md` (stats field list)

**Interfaces:**
- Produces: `mbar_app::version::{VERSION: &str, build_number(&str) -> Option<u64>, parse_semver(&str) -> Option<(u64,u64,u64)>}`; `--query stats` gains a trailing `"version": "<semver>"`.

- [ ] **Step 1: Create the crate**

`crates/mbar-app/Cargo.toml`:

```toml
[package]
name = "mbar-app"
description = "mbar app bundle logic: versions, Info.plist, appcast, update decisions, config lookup"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
serde_json = { workspace = true }
```

`crates/mbar-app/src/lib.rs`:

```rust
//! Platform-independent logic shared by the daemon (`mbar`) and the management app
//! (`mbar-ui`) for running from `mbar.app`: version numbers, `Info.plist` values,
//! appcast parsing, update decisions and config lookup. Everything here is pure and
//! unit-tested on any host; process spawning lives in the callers.

pub mod version;
```

In root `Cargo.toml` add `"crates/mbar-app"` to `members` and `mbar-app = { path = "crates/mbar-app" }` to `[workspace.dependencies]`.

- [ ] **Step 2: Write failing tests** in `crates/mbar-app/src/version.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_number_is_monotonic_encoding() {
        assert_eq!(build_number("0.1.0"), Some(1_000));
        assert_eq!(build_number("1.2.3"), Some(1_002_003));
        assert_eq!(build_number("0.10.0"), Some(10_000));
        assert!(build_number("0.9.999").unwrap() < build_number("0.10.0").unwrap());
    }

    #[test]
    fn build_number_rejects_garbage() {
        assert_eq!(build_number(""), None);
        assert_eq!(build_number("1.2"), None);
        assert_eq!(build_number("1.2.3-beta.1"), None);
        assert_eq!(build_number("1.1000.0"), None);
    }

    #[test]
    fn version_matches_cargo() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(build_number(VERSION).is_some());
    }
}
```

- [ ] **Step 3: Run, expect compile failure**

Run: `cargo test -p mbar-app`
Expected: FAIL (`build_number` not found).

- [ ] **Step 4: Implement** (top of `version.rs`):

```rust
//! Version numbers. `CFBundleVersion` and Sparkle's `sparkle:version` must grow with
//! every release, so they are derived from the semver version:
//! `major * 1_000_000 + minor * 1_000 + patch` (minor and patch below 1000).

/// The workspace version (all crates share it).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `"1.2.3"` → `(1, 2, 3)`. Pre-release and build suffixes are rejected.
pub fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.split('.');
    let mut next = || it.next()?.parse::<u64>().ok();
    let v = (next()?, next()?, next()?);
    if s.split('.').count() != 3 {
        return None;
    }
    Some(v)
}

/// Build number for a semver version, see the module docs.
pub fn build_number(version: &str) -> Option<u64> {
    let (major, minor, patch) = parse_semver(version)?;
    if minor >= 1_000 || patch >= 1_000 {
        return None;
    }
    Some(major * 1_000_000 + minor * 1_000 + patch)
}
```

Add `pub mod version;` (already in lib.rs).

- [ ] **Step 5: Run tests**

Run: `cargo test -p mbar-app`
Expected: 3 passed.

- [ ] **Step 6: Failing test for stats** — in `crates/mbar-core/src/query.rs` test module (find the existing stats test with `grep -n "stats_json" crates/mbar-core/src/query.rs`) add:

```rust
#[test]
fn stats_reports_version_last() {
    let out = stats_json(&Stats::default(), 0);
    let last = out.trim_end().trim_end_matches('}').trim_end().lines().last().unwrap();
    assert_eq!(last, format!("\t\"version\": \"{}\"", env!("CARGO_PKG_VERSION")));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
}
```

Run: `cargo test -p mbar-core stats_reports_version_last` → FAIL.

- [ ] **Step 7: Implement** — in `stats_fields` append after `("ipc_messages", …)`:

```rust
        ("version", format!("\"{}\"", env!("CARGO_PKG_VERSION"))),
```

(`mbar-core` shares the workspace version, so no dependency on `mbar-app` is needed.) Update existing stats tests that compare the full output (search `ipc_messages` in tests) to include the new last line. Add to `docs/EXTENSIONS.md` in the `--query stats` field list: ``| `version` | daemon version (semver), used by mbar.app to detect a stale daemon after an update |``.

- [ ] **Step 8: Run all tests and commit**

Run: `cargo test --workspace` → all pass.

```bash
git add Cargo.toml Cargo.lock crates/mbar-app crates/mbar-core/src/query.rs docs/EXTENSIONS.md
git commit -m "feat: add mbar-app crate with build numbers; report version in --query stats"
```

---

### Task 3: Move config lookup into `mbar-app`

**Host:** Linux VM.

**Files:**
- Create: `crates/mbar-app/src/config.rs`
- Modify: `crates/mbar-app/src/lib.rs`, `crates/mbar/Cargo.toml`, `crates/mbar/src/daemon.rs` (remove `find_config`, `config_candidates` and their tests; call `mbar_app::config::find_config`)

**Interfaces:**
- Produces: `mbar_app::config::{find_config(bar_name: &str, xdg: &str, home: &str) -> Option<PathBuf>, config_candidates(bar_name: &str, xdg: &str, home: &str) -> Vec<PathBuf>}`, behaviour identical to today's `daemon.rs`.

- [ ] **Step 1: Move code**

Create `crates/mbar-app/src/config.rs` containing exactly `find_config`, `config_candidates` and the two tests `candidates_order` and `lookup_prefers_init_lua` cut from `crates/mbar/src/daemon.rs` (lines starting at the doc comment `/// Config lookup` through the end of `config_candidates`, and the two test functions). Replace `mbar_ipc::DEFAULT_BAR_NAME` with a local constant:

```rust
/// Default bar name (same value as `mbar_ipc::DEFAULT_BAR_NAME`; duplicated so this
/// crate stays dependency-free).
const DEFAULT_BAR_NAME: &str = "mbar";
```

and add a test that guards the duplication from the daemon side (Step 3). Make both functions `pub`. Add `pub mod config;` to `lib.rs`.

- [ ] **Step 2: Wire the daemon**

`crates/mbar/Cargo.toml` `[dependencies]`: `mbar-app = { workspace = true }`.
In `daemon.rs` replace the call `.or_else(|| find_config(&bar_name, &xdg, &home))` with `.or_else(|| mbar_app::config::find_config(&bar_name, &xdg, &home))`. Keep `lock_errors` test in `daemon.rs`.

- [ ] **Step 3: Guard test** in `crates/mbar/src/daemon.rs` tests:

```rust
#[test]
fn shared_config_lookup_uses_the_ipc_default_bar() {
    let c = mbar_app::config::config_candidates(mbar_ipc::DEFAULT_BAR_NAME, "", "/h");
    assert!(c.iter().any(|p| p.ends_with(".config/sketchybar/init.lua")));
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: all pass (the moved tests now run under `mbar-app`).

- [ ] **Step 5: Commit**

```bash
git add crates/mbar-app crates/mbar/Cargo.toml crates/mbar/src/daemon.rs Cargo.lock
git commit -m "refactor: move config lookup into mbar-app for reuse by mbar-ui"
```

---

### Task 4: Bundle context and script `PATH`

**Host:** Linux VM (logic and tests); the wiring compiles on every host.

**Files:**
- Create: `crates/mbar-app/src/bundle.rs`
- Modify: `crates/mbar-app/src/lib.rs`, `crates/mbar/src/daemon.rs` (`run`)

**Interfaces:**
- Produces:
  - `pub struct AppBundle { pub root: PathBuf, pub short_version: String, pub build: u64, pub feed_url: Option<String>, pub auto_checks_default: bool }`
  - `pub fn bundle_root_from_exe(exe: &Path) -> Option<PathBuf>` (`…/X.app/Contents/MacOS/bin` → `…/X.app`)
  - `pub fn plist_string(xml: &str, key: &str) -> Option<String>`; `pub fn plist_bool(xml: &str, key: &str) -> Option<bool>`
  - `pub fn read_bundle(root: &Path) -> Option<AppBundle>` (reads `Contents/Info.plist`)
  - `impl AppBundle { pub fn bin_dir(&self) -> PathBuf; pub fn info_plist(&self) -> PathBuf }`
  - `pub fn script_path(bin_dir: &Path, current: &str) -> String`
  - `pub const DEFAULT_SCRIPT_PATH: &str = "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";`

- [ ] **Step 1: Write failing tests** (bottom of `bundle.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
	<key>CFBundleShortVersionString</key>
	<string>0.2.0</string>
	<key>CFBundleVersion</key>
	<string>2000</string>
	<key>SUFeedURL</key>
	<string>https://example.org/a&amp;b.xml</string>
	<key>SUEnableAutomaticChecks</key>
	<true/>
</dict></plist>"#;

    #[test]
    fn root_from_exe() {
        assert_eq!(
            bundle_root_from_exe(Path::new("/Applications/mbar.app/Contents/MacOS/mbar")),
            Some(PathBuf::from("/Applications/mbar.app"))
        );
        assert_eq!(bundle_root_from_exe(Path::new("/usr/local/bin/mbar")), None);
        assert_eq!(bundle_root_from_exe(Path::new("/x/Contents/MacOS/mbar")), None);
    }

    #[test]
    fn plist_values() {
        assert_eq!(plist_string(PLIST, "CFBundleVersion").as_deref(), Some("2000"));
        assert_eq!(
            plist_string(PLIST, "SUFeedURL").as_deref(),
            Some("https://example.org/a&b.xml")
        );
        assert_eq!(plist_string(PLIST, "Missing"), None);
        assert_eq!(plist_bool(PLIST, "SUEnableAutomaticChecks"), Some(true));
        assert_eq!(plist_bool(PLIST, "CFBundleVersion"), None);
    }

    #[test]
    fn read_bundle_from_disk() {
        let root = std::env::temp_dir().join(format!("mbar-bundle-{}.app", std::process::id()));
        std::fs::create_dir_all(root.join("Contents")).unwrap();
        std::fs::write(root.join("Contents/Info.plist"), PLIST).unwrap();
        let b = read_bundle(&root).unwrap();
        assert_eq!(b.short_version, "0.2.0");
        assert_eq!(b.build, 2000);
        assert_eq!(b.feed_url.as_deref(), Some("https://example.org/a&b.xml"));
        assert!(b.auto_checks_default);
        assert_eq!(b.bin_dir(), root.join("Contents/Resources/bin"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn script_path_prepends_once() {
        let bin = Path::new("/Applications/mbar.app/Contents/Resources/bin");
        assert_eq!(
            script_path(bin, "/usr/bin:/bin"),
            "/Applications/mbar.app/Contents/Resources/bin:/usr/bin:/bin"
        );
        assert_eq!(
            script_path(bin, "/Applications/mbar.app/Contents/Resources/bin:/usr/bin"),
            "/Applications/mbar.app/Contents/Resources/bin:/usr/bin"
        );
        assert_eq!(
            script_path(bin, ""),
            format!("/Applications/mbar.app/Contents/Resources/bin:{DEFAULT_SCRIPT_PATH}")
        );
    }
}
```

Run: `cargo test -p mbar-app bundle` → FAIL (missing items).

- [ ] **Step 2: Implement** (top of `bundle.rs`):

```rust
//! Running from `mbar.app`: locating the bundle from the executable path and reading
//! the few `Info.plist` values mbar needs. The plist is the XML file written by
//! `packaging/macos/build-app.sh`, so a small tag scanner is enough.

use std::path::{Path, PathBuf};

/// `PATH` used when the daemon's own `PATH` is empty (launchd gives agents only the
/// system directories); same list as the LaunchAgent template.
pub const DEFAULT_SCRIPT_PATH: &str =
    "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppBundle {
    pub root: PathBuf,
    pub short_version: String,
    pub build: u64,
    pub feed_url: Option<String>,
    /// `SUEnableAutomaticChecks` from `Info.plist` (a user default overrides it).
    pub auto_checks_default: bool,
}

impl AppBundle {
    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("Contents/Resources/bin")
    }

    pub fn info_plist(&self) -> PathBuf {
        self.root.join("Contents/Info.plist")
    }
}

/// `…/Name.app/Contents/MacOS/<exe>` → `…/Name.app`.
pub fn bundle_root_from_exe(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let root = contents.parent()?;
    let is_app = root.extension().and_then(|e| e.to_str()) == Some("app");
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && is_app)
        .then(|| root.to_path_buf())
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The text after `<key>key</key>` up to the next tag, if that tag is `<string>`.
pub fn plist_string(xml: &str, key: &str) -> Option<String> {
    let rest = &xml[xml.find(&format!("<key>{key}</key>"))? + key.len() + 11..];
    let rest = rest.trim_start().strip_prefix("<string>")?;
    Some(unescape(&rest[..rest.find("</string>")?]))
}

pub fn plist_bool(xml: &str, key: &str) -> Option<bool> {
    let rest = &xml[xml.find(&format!("<key>{key}</key>"))? + key.len() + 11..];
    let rest = rest.trim_start();
    if rest.starts_with("<true/>") {
        Some(true)
    } else if rest.starts_with("<false/>") {
        Some(false)
    } else {
        None
    }
}

pub fn read_bundle(root: &Path) -> Option<AppBundle> {
    let xml = std::fs::read_to_string(root.join("Contents/Info.plist")).ok()?;
    Some(AppBundle {
        root: root.to_path_buf(),
        short_version: plist_string(&xml, "CFBundleShortVersionString")?,
        build: plist_string(&xml, "CFBundleVersion")?.parse().ok()?,
        feed_url: plist_string(&xml, "SUFeedURL").filter(|s| !s.is_empty()),
        auto_checks_default: plist_bool(&xml, "SUEnableAutomaticChecks").unwrap_or(true),
    })
}

/// `bin_dir` first, then `current` (or [`DEFAULT_SCRIPT_PATH`] when empty), without
/// adding `bin_dir` twice.
pub fn script_path(bin_dir: &Path, current: &str) -> String {
    let bin = bin_dir.to_string_lossy();
    let base = if current.is_empty() { DEFAULT_SCRIPT_PATH } else { current };
    if base.split(':').next() == Some(&*bin) {
        return base.to_string();
    }
    format!("{bin}:{base}")
}
```

Add `pub mod bundle;` to `lib.rs`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p mbar-app bundle`
Expected: 4 passed.

- [ ] **Step 4: Wire into the daemon** — in `crates/mbar/src/daemon.rs` `run`, directly before `// \`BAR_NAME\` was set in \`main\`…` / `let base_env = …`:

```rust
    // Running from mbar.app: scripts (and Lua's io.popen) find `sketchybar`/`mbar` in the
    // bundle first, whatever PATH launchd gave the agent. Set before any thread exists.
    let bundle = std::env::current_exe()
        .ok()
        .and_then(|exe| mbar_app::bundle::bundle_root_from_exe(&exe))
        .and_then(|root| mbar_app::bundle::read_bundle(&root));
    if let Some(b) = &bundle {
        let current = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", mbar_app::bundle::script_path(&b.bin_dir(), &current));
    }
```

Keep `bundle` in scope; Task 6 passes it to the updater.

- [ ] **Step 5: Run all tests, commit**

Run: `cargo test --workspace` → pass.

```bash
git add crates/mbar-app crates/mbar/src/daemon.rs
git commit -m "feat: detect mbar.app bundle and put its bin directory first on the script PATH"
```

---

### Task 5: Appcast parsing and update decisions

**Host:** Linux VM.

**Files:**
- Create: `crates/mbar-app/src/appcast.rs`, `crates/mbar-app/src/update.rs`
- Modify: `crates/mbar-app/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub struct AppcastItem { pub build: u64, pub short_version: String, pub minimum_system: Option<String> }`
  - `pub fn parse_appcast(xml: &str) -> Vec<AppcastItem>`
  - `pub fn best_item(items: &[AppcastItem], os_version: &str) -> Option<&AppcastItem>`
  - `pub fn os_at_least(os: &str, min: &str) -> bool`
  - `#[derive(Default, …)] pub struct UpdateState { pub offered_build: Option<u64>, pub offered_at: Option<u64>, pub restarted_for_build: Option<u64> }` with `to_json(&self) -> String`, `from_json(&str) -> UpdateState` (invalid → default)
  - `pub enum Action { Nothing, Offer { build: u64, short_version: String }, RestartSelf { build: u64 } }`
  - `pub fn decide(own_build: u64, bundle_build: u64, latest: Option<&AppcastItem>, state: &UpdateState, now: u64) -> Action`
  - `pub fn record(state: &mut UpdateState, action: &Action, now: u64)`
  - `pub const OFFER_INTERVAL_SECS: u64 = 86_400;`

- [ ] **Step 1: Failing appcast tests** (`appcast.rs` bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
<channel><title>mbar</title>
<item><title>Version 0.3.0</title>
  <sparkle:version>3000</sparkle:version>
  <sparkle:shortVersionString>0.3.0</sparkle:shortVersionString>
  <sparkle:minimumSystemVersion>15.0</sparkle:minimumSystemVersion>
  <enclosure url="https://x/mbar-0.3.0.zip" length="1" type="application/octet-stream" sparkle:edSignature="s"/>
</item>
<item><title>Version 0.2.0</title>
  <sparkle:version>2000</sparkle:version>
  <sparkle:shortVersionString>0.2.0</sparkle:shortVersionString>
  <sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
  <enclosure url="https://x/mbar-0.2.0.zip" length="1" type="application/octet-stream" sparkle:edSignature="s"/>
</item>
</channel></rss>"#;

    #[test]
    fn parses_items() {
        let items = parse_appcast(FEED);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].build, 3000);
        assert_eq!(items[0].short_version, "0.3.0");
        assert_eq!(items[1].minimum_system.as_deref(), Some("13.0"));
    }

    #[test]
    fn best_item_respects_minimum_system() {
        let items = parse_appcast(FEED);
        assert_eq!(best_item(&items, "14.6.1").unwrap().build, 2000);
        assert_eq!(best_item(&items, "15.0").unwrap().build, 3000);
        assert!(best_item(&items, "12.7").is_none());
    }

    #[test]
    fn os_comparison() {
        assert!(os_at_least("26.0", "13.0"));
        assert!(os_at_least("13.0", "13"));
        assert!(!os_at_least("13.6", "14.0"));
        assert!(os_at_least("14.10", "14.9"));
    }

    #[test]
    fn garbage_is_empty() {
        assert!(parse_appcast("<html>404</html>").is_empty());
        assert!(parse_appcast("").is_empty());
    }
}
```

Run: `cargo test -p mbar-app appcast` → FAIL.

- [ ] **Step 2: Implement `appcast.rs`:**

```rust
//! Sparkle appcast (RSS) reading: the daemon only needs each item's build number,
//! display version and minimum macOS version.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppcastItem {
    pub build: u64,
    pub short_version: String,
    pub minimum_system: Option<String>,
}

fn element<'a>(item: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = item.find(&open)? + open.len();
    let end = item[start..].find(&format!("</{name}>"))? + start;
    Some(item[start..end].trim())
}

pub fn parse_appcast(xml: &str) -> Vec<AppcastItem> {
    xml.split("<item>")
        .skip(1)
        .filter_map(|chunk| {
            let item = &chunk[..chunk.find("</item>")?];
            Some(AppcastItem {
                build: element(item, "sparkle:version")?.parse().ok()?,
                short_version: element(item, "sparkle:shortVersionString")?.to_string(),
                minimum_system: element(item, "sparkle:minimumSystemVersion").map(str::to_string),
            })
        })
        .collect()
}

fn parts(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Numeric dotted-version comparison (`14.10 >= 14.9`, missing parts are 0).
pub fn os_at_least(os: &str, min: &str) -> bool {
    let (a, b) = (parts(os), parts(min));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    true
}

/// The newest item this macOS version can run.
pub fn best_item<'a>(items: &'a [AppcastItem], os_version: &str) -> Option<&'a AppcastItem> {
    items
        .iter()
        .filter(|i| i.minimum_system.as_deref().map_or(true, |m| os_at_least(os_version, m)))
        .max_by_key(|i| i.build)
}
```

Run: `cargo test -p mbar-app appcast` → 4 passed.

- [ ] **Step 3: Failing decision tests** (`update.rs` bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn item(build: u64) -> AppcastItem {
        AppcastItem { build, short_version: format!("{build}"), minimum_system: None }
    }

    #[test]
    fn offers_newer_version() {
        let s = UpdateState::default();
        assert_eq!(
            decide(1000, 1000, Some(&item(2000)), &s, 100),
            Action::Offer { build: 2000, short_version: "2000".into() }
        );
        assert_eq!(decide(2000, 2000, Some(&item(2000)), &s, 100), Action::Nothing);
        assert_eq!(decide(1000, 1000, None, &s, 100), Action::Nothing);
    }

    #[test]
    fn offer_rate_limited_per_version() {
        let mut s = UpdateState::default();
        let a = decide(1000, 1000, Some(&item(2000)), &s, 100);
        record(&mut s, &a, 100);
        assert_eq!(decide(1000, 1000, Some(&item(2000)), &s, 100 + 3600), Action::Nothing);
        assert!(matches!(
            decide(1000, 1000, Some(&item(2000)), &s, 100 + OFFER_INTERVAL_SECS),
            Action::Offer { .. }
        ));
        // A newer version is offered right away.
        assert!(matches!(
            decide(1000, 1000, Some(&item(3000)), &s, 200),
            Action::Offer { build: 3000, .. }
        ));
    }

    #[test]
    fn restart_once_per_version() {
        let mut s = UpdateState::default();
        let a = decide(1000, 2000, Some(&item(2000)), &s, 100);
        assert_eq!(a, Action::RestartSelf { build: 2000 });
        record(&mut s, &a, 100);
        // Still the old binary (restart failed): never loop.
        assert_eq!(decide(1000, 2000, Some(&item(2000)), &s, 200), Action::Nothing);
    }

    #[test]
    fn restart_wins_over_offer() {
        let s = UpdateState::default();
        assert_eq!(
            decide(1000, 2000, Some(&item(3000)), &s, 100),
            Action::RestartSelf { build: 2000 }
        );
    }

    #[test]
    fn state_json_roundtrip_and_garbage() {
        let s = UpdateState { offered_build: Some(2000), offered_at: Some(5), restarted_for_build: None };
        assert_eq!(UpdateState::from_json(&s.to_json()), s);
        assert_eq!(UpdateState::from_json("not json"), UpdateState::default());
    }
}
```

Run: `cargo test -p mbar-app update` → FAIL.

- [ ] **Step 4: Implement `update.rs`:**

```rust
//! What the daemon does after reading the appcast: offer the update (open the app),
//! restart itself (a newer bundle is installed on disk), or nothing. Pure; the caller
//! persists [`UpdateState`] in `~/Library/Application Support/mbar/update-state.json`.

use crate::appcast::AppcastItem;

/// At most one offer per version in this many seconds.
pub const OFFER_INTERVAL_SECS: u64 = 86_400;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateState {
    pub offered_build: Option<u64>,
    /// Unix seconds.
    pub offered_at: Option<u64>,
    pub restarted_for_build: Option<u64>,
}

impl UpdateState {
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "offered_build": self.offered_build,
            "offered_at": self.offered_at,
            "restarted_for_build": self.restarted_for_build,
        })
        .to_string()
    }

    pub fn from_json(s: &str) -> UpdateState {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(s) else {
            return UpdateState::default();
        };
        UpdateState {
            offered_build: v["offered_build"].as_u64(),
            offered_at: v["offered_at"].as_u64(),
            restarted_for_build: v["restarted_for_build"].as_u64(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Nothing,
    Offer { build: u64, short_version: String },
    RestartSelf { build: u64 },
}

pub fn decide(
    own_build: u64,
    bundle_build: u64,
    latest: Option<&AppcastItem>,
    state: &UpdateState,
    now: u64,
) -> Action {
    if bundle_build > own_build {
        if state.restarted_for_build == Some(bundle_build) {
            return Action::Nothing;
        }
        return Action::RestartSelf { build: bundle_build };
    }
    let Some(item) = latest.filter(|i| i.build > own_build) else {
        return Action::Nothing;
    };
    let recently = state.offered_build == Some(item.build)
        && state
            .offered_at
            .is_some_and(|t| now.saturating_sub(t) < OFFER_INTERVAL_SECS);
    if recently {
        return Action::Nothing;
    }
    Action::Offer { build: item.build, short_version: item.short_version.clone() }
}

pub fn record(state: &mut UpdateState, action: &Action, now: u64) {
    match action {
        Action::Offer { build, .. } => {
            state.offered_build = Some(*build);
            state.offered_at = Some(now);
        }
        Action::RestartSelf { build } => state.restarted_for_build = Some(*build),
        Action::Nothing => {}
    }
}
```

Add `pub mod appcast; pub mod update;` to `lib.rs`.

- [ ] **Step 5: Run and commit**

Run: `cargo test -p mbar-app` → all pass.

```bash
git add crates/mbar-app
git commit -m "feat: appcast parsing and update decisions for the daemon"
```

---

### Task 6: Daemon update checker

**Host:** Linux VM for the checker logic and its tests (fake runner); the real runner is `#[cfg(target_os = "macos")]` and is exercised in Task 23.

**Files:**
- Create: `crates/mbar/src/updater.rs`
- Modify: `crates/mbar/src/main.rs` (`mod updater;`), `crates/mbar/src/daemon.rs` (spawn), `crates/mbar-macos/src/sys/mod.rs` + new `crates/mbar-macos/src/sys/apps.rs` (running-app check and distributed notification)

**Interfaces:**
- Consumes: `mbar_app::{bundle::AppBundle, appcast::{parse_appcast, best_item}, update::{decide, record, Action, UpdateState}, version::{VERSION, build_number}}`.
- Produces:
  - `pub trait System { fn fetch(&self, url: &str) -> Option<String>; fn os_version(&self) -> String; fn auto_checks(&self) -> Option<bool>; fn feed_override(&self) -> Option<String>; fn ui_running(&self) -> bool; fn notify_ui(&self); fn open_ui_update(&self); fn restart_self(&self); fn now(&self) -> u64; fn load_state(&self) -> String; fn save_state(&self, json: &str); fn bundle_build(&self) -> Option<u64>; }`
  - `pub fn check_once(sys: &dyn System, bundle: &AppBundle, own_build: u64) -> Action`
  - `pub fn spawn(bundle: AppBundle, home: String)` (macOS only; no-op elsewhere)
  - `mbar_macos::sys::apps::{is_app_running(bundle_id: &str) -> bool, post_distributed(name: &str)}`
  - Distributed notification name: `dev.rubeen.mbar.checkForUpdates`.

- [ ] **Step 1: Failing tests** (`updater.rs` bottom) with a fake `System`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        feed: Option<String>,
        auto: Option<bool>,
        ui: bool,
        bundle_build: Option<u64>,
        state: RefCell<String>,
        calls: RefCell<Vec<&'static str>>,
        fetched: RefCell<Vec<String>>,
        feed_override: Option<String>,
    }

    impl System for Fake {
        fn fetch(&self, url: &str) -> Option<String> { self.fetched.borrow_mut().push(url.into()); self.feed.clone() }
        fn os_version(&self) -> String { "15.1".into() }
        fn auto_checks(&self) -> Option<bool> { self.auto }
        fn feed_override(&self) -> Option<String> { self.feed_override.clone() }
        fn ui_running(&self) -> bool { self.ui }
        fn notify_ui(&self) { self.calls.borrow_mut().push("notify") }
        fn open_ui_update(&self) { self.calls.borrow_mut().push("open") }
        fn restart_self(&self) { self.calls.borrow_mut().push("restart") }
        fn now(&self) -> u64 { 1_000 }
        fn load_state(&self) -> String { self.state.borrow().clone() }
        fn save_state(&self, json: &str) { *self.state.borrow_mut() = json.into() }
        fn bundle_build(&self) -> Option<u64> { self.bundle_build }
    }

    fn bundle() -> AppBundle {
        AppBundle {
            root: "/Applications/mbar.app".into(),
            short_version: "0.1.0".into(),
            build: 1000,
            feed_url: Some("https://feed/appcast.xml".into()),
            auto_checks_default: true,
        }
    }

    fn feed(build: u64) -> String {
        format!("<item><sparkle:version>{build}</sparkle:version><sparkle:shortVersionString>x</sparkle:shortVersionString></item>")
    }

    #[test]
    fn offer_opens_app_when_ui_not_running() {
        let f = Fake { feed: Some(feed(2000)), bundle_build: Some(1000), ..Default::default() };
        assert!(matches!(check_once(&f, &bundle(), 1000), Action::Offer { build: 2000, .. }));
        assert_eq!(*f.calls.borrow(), ["open"]);
        assert!(f.state.borrow().contains("2000"));
    }

    #[test]
    fn offer_posts_notification_when_ui_running() {
        let f = Fake { feed: Some(feed(2000)), ui: true, bundle_build: Some(1000), ..Default::default() };
        check_once(&f, &bundle(), 1000);
        assert_eq!(*f.calls.borrow(), ["notify"]);
    }

    #[test]
    fn fetch_failure_does_nothing() {
        let f = Fake { feed: None, bundle_build: Some(1000), ..Default::default() };
        assert_eq!(check_once(&f, &bundle(), 1000), Action::Nothing);
        assert!(f.calls.borrow().is_empty());
        assert!(f.state.borrow().is_empty());
    }

    #[test]
    fn disabled_checks_still_restart_after_update() {
        let f = Fake { feed: Some(feed(3000)), auto: Some(false), bundle_build: Some(2000), ..Default::default() };
        assert_eq!(check_once(&f, &bundle(), 1000), Action::RestartSelf { build: 2000 });
        assert_eq!(*f.calls.borrow(), ["restart"]);
        assert!(f.fetched.borrow().is_empty(), "no network when checks are off");
    }

    #[test]
    fn disabled_checks_never_fetch() {
        let f = Fake { feed: Some(feed(3000)), auto: Some(false), bundle_build: Some(1000), ..Default::default() };
        assert_eq!(check_once(&f, &bundle(), 1000), Action::Nothing);
        assert!(f.fetched.borrow().is_empty());
    }

    #[test]
    fn feed_override_wins() {
        let f = Fake { feed: Some(feed(1000)), bundle_build: Some(1000), feed_override: Some("http://127.0.0.1:8765/appcast.xml".into()), ..Default::default() };
        check_once(&f, &bundle(), 1000);
        assert_eq!(*f.fetched.borrow(), ["http://127.0.0.1:8765/appcast.xml"]);
    }
}
```

Run: `cargo test -p mbar updater` → FAIL.

- [ ] **Step 2: Implement the logic** (top of `updater.rs`):

```rust
//! Daemon-side update check for mbar.app (`docs/superpowers/specs/…-design.md`,
//! "Update flow"): read the appcast, then open the app's Sparkle dialog, or restart the
//! daemon when a newer bundle is already installed. The decision is pure
//! (`mbar_app::update`); this module only talks to the system through [`System`].

use mbar_app::appcast::{best_item, parse_appcast};
use mbar_app::bundle::AppBundle;
use mbar_app::update::{decide, record, Action, UpdateState};

pub const NOTIFICATION: &str = "dev.rubeen.mbar.checkForUpdates";
pub const BUNDLE_ID: &str = "dev.rubeen.mbar";

pub trait System {
    fn fetch(&self, url: &str) -> Option<String>;
    fn os_version(&self) -> String;
    /// `defaults read dev.rubeen.mbar SUEnableAutomaticChecks`; `None` when unset.
    fn auto_checks(&self) -> Option<bool>;
    /// `defaults read dev.rubeen.mbar SUFeedURL`.
    fn feed_override(&self) -> Option<String>;
    fn ui_running(&self) -> bool;
    fn notify_ui(&self);
    fn open_ui_update(&self);
    fn restart_self(&self);
    fn now(&self) -> u64;
    fn load_state(&self) -> String;
    fn save_state(&self, json: &str);
    /// `CFBundleVersion` of the bundle currently on disk.
    fn bundle_build(&self) -> Option<u64>;
}

pub fn check_once(sys: &dyn System, bundle: &AppBundle, own_build: u64) -> Action {
    let mut state = UpdateState::from_json(&sys.load_state());
    let on_disk = sys.bundle_build().unwrap_or(own_build);
    let auto = sys.auto_checks().unwrap_or(bundle.auto_checks_default);
    let latest = if auto && on_disk <= own_build {
        sys.feed_override()
            .or_else(|| bundle.feed_url.clone())
            .and_then(|url| sys.fetch(&url))
            .map(|xml| parse_appcast(&xml))
    } else {
        None
    };
    let os = sys.os_version();
    let best = latest.as_deref().and_then(|items| best_item(items, &os));
    let action = decide(own_build, on_disk, best, &state, sys.now());
    match &action {
        Action::Offer { .. } if sys.ui_running() => sys.notify_ui(),
        Action::Offer { .. } => sys.open_ui_update(),
        Action::RestartSelf { .. } => {}
        Action::Nothing => return action,
    }
    record(&mut state, &action, sys.now());
    sys.save_state(&state.to_json());
    if matches!(action, Action::RestartSelf { .. }) {
        sys.restart_self();
    }
    action
}
```

Note the order for `RestartSelf`: the state is saved before `restart_self` kills the process, so a failed restart cannot loop.

Run: `cargo test -p mbar updater` → 6 passed.

- [ ] **Step 3: macOS helpers in `mbar-macos`** — create `crates/mbar-macos/src/sys/apps.rs`:

```rust
//! Running-application lookup and distributed notifications for the update check.

use objc2_app_kit::NSRunningApplication;
use objc2_foundation::{NSDistributedNotificationCenter, NSString};

pub fn is_app_running(bundle_id: &str) -> bool {
    let id = NSString::from_str(bundle_id);
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&id).count() > 0
}

pub fn post_distributed(name: &str) {
    let center = NSDistributedNotificationCenter::defaultCenter();
    let name = NSString::from_str(name);
    unsafe {
        center.postNotificationName_object_userInfo_deliverImmediately(&name, None, None, true)
    };
}
```

Register it in `crates/mbar-macos/src/sys/mod.rs` with `pub mod apps;` (follow the file's existing `cfg` pattern). Check the exact objc2 method names with `cargo doc -p objc2-foundation --open` if the build complains; the selectors are `runningApplicationsWithBundleIdentifier:` and `postNotificationName:object:userInfo:deliverImmediately:`.

- [ ] **Step 4: Real `System` and thread** (append to `updater.rs`):

```rust
#[cfg(target_os = "macos")]
mod real {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    pub struct MacSystem {
        pub bundle_root: PathBuf,
        pub state_path: PathBuf,
    }

    fn output(cmd: &str, args: &[&str]) -> Option<String> {
        let out = Command::new(cmd).args(args).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    impl System for MacSystem {
        fn fetch(&self, url: &str) -> Option<String> {
            let body = output("/usr/bin/curl", &["-fsSL", "--max-time", "30", url]);
            if body.is_none() {
                log::warn!("update check: could not fetch {url}");
            }
            body
        }
        fn os_version(&self) -> String {
            output("/usr/bin/sw_vers", &["-productVersion"]).unwrap_or_default()
        }
        fn auto_checks(&self) -> Option<bool> {
            output("/usr/bin/defaults", &["read", BUNDLE_ID, "SUEnableAutomaticChecks"])
                .map(|s| matches!(s.as_str(), "1" | "true" | "YES"))
        }
        fn feed_override(&self) -> Option<String> {
            output("/usr/bin/defaults", &["read", BUNDLE_ID, "SUFeedURL"]).filter(|s| !s.is_empty())
        }
        fn ui_running(&self) -> bool {
            mbar_macos::sys::apps::is_app_running(BUNDLE_ID)
        }
        fn notify_ui(&self) {
            mbar_macos::sys::apps::post_distributed(NOTIFICATION);
        }
        fn open_ui_update(&self) {
            let _ = Command::new("/usr/bin/open")
                .args(["-b", BUNDLE_ID, "--args", "--update"])
                .status();
        }
        fn restart_self(&self) {
            let target = format!("gui/{}/{BUNDLE_ID}", unsafe { libc::getuid() });
            log::warn!("update check: newer mbar.app on disk, restarting ({target})");
            let _ = Command::new("/bin/launchctl").args(["kickstart", "-k", &target]).status();
        }
        fn now(&self) -> u64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        }
        fn load_state(&self) -> String {
            std::fs::read_to_string(&self.state_path).unwrap_or_default()
        }
        fn save_state(&self, json: &str) {
            if let Some(dir) = self.state_path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&self.state_path, json) {
                log::warn!("update check: cannot write {}: {e}", self.state_path.display());
            }
        }
        fn bundle_build(&self) -> Option<u64> {
            mbar_app::bundle::read_bundle(&self.bundle_root).map(|b| b.build)
        }
    }
}

/// Starts the check thread: first check after 120 s, then every 24 h. Only when the
/// daemon runs from a bundle with a feed URL.
pub fn spawn(bundle: AppBundle, home: String) {
    #[cfg(target_os = "macos")]
    {
        let Some(own_build) = mbar_app::version::build_number(mbar_app::version::VERSION) else {
            return;
        };
        if bundle.feed_url.is_none() || home.is_empty() {
            return;
        }
        let sys = real::MacSystem {
            bundle_root: bundle.root.clone(),
            state_path: std::path::Path::new(&home)
                .join("Library/Application Support/mbar/update-state.json"),
        };
        let _ = std::thread::Builder::new().name("mbar-update".into()).spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(120));
            loop {
                let action = check_once(&sys, &bundle, own_build);
                log::info!("update check: {action:?}");
                std::thread::sleep(std::time::Duration::from_secs(86_400));
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (bundle, home);
}
```

- [ ] **Step 5: Spawn from the daemon** — in `daemon.rs` `run`, just before `crate::platform::platform_main(setup)`:

```rust
    if let Some(b) = bundle {
        crate::updater::spawn(b, setup.driver.home.clone());
    }
```

(`DriverConfig.home` is a `String`; if the field is private, clone `home` before it moves into `DriverConfig` and pass that.) Add `mod updater;` to `main.rs`.

- [ ] **Step 6: Run tests on Linux, build on macOS**

Run (Linux VM): `cargo test --workspace` → pass; `cargo clippy --workspace --all-targets -- -D warnings` → clean.
Run (macOS, hand back if on Linux): `cargo build -p mbar` → compiles.

- [ ] **Step 7: Commit**

```bash
git add crates/mbar/src/updater.rs crates/mbar/src/main.rs crates/mbar/src/daemon.rs crates/mbar-macos/src/sys
git commit -m "feat: daemon checks the appcast and opens mbar.app's update dialog"
```

---

### Task 7: Makefile respects cargo's target directory

**Host:** Linux VM to write and test `make install`; macOS for `install-ui`.

**Files:**
- Modify: `Makefile`

**Interfaces:**
- Produces: `TARGET_DIR` / `UI_TARGET_DIR` Make variables used by `install`, `install-ui` and Task 8's targets.

- [ ] **Step 1: Reproduce**

```bash
mkdir -p /tmp/tdir && CARGO_TARGET_DIR=/tmp/tdir make release && make install PREFIX=/tmp/inst
```

Expected: FAIL "target/release/mbar not found".

- [ ] **Step 2: Implement** — below the variable block in `Makefile`:

```make
# cargo may build elsewhere (CARGO_TARGET_DIR, build.target-dir in ~/.cargo/config.toml).
TARGET_DIR    := $(shell $(CARGO) metadata --format-version 1 --no-deps 2>/dev/null | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
UI_TARGET_DIR := $(shell cd $(UI_DIR) && $(CARGO) metadata --format-version 1 --no-deps 2>/dev/null | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
```

Replace `target/release/mbar` in `install` with `$(TARGET_DIR)/release/mbar` (both the `test -x` and the `install` line) and `$(UI_DIR)/target/release/mbar-ui` in `install-ui` with `$(UI_TARGET_DIR)/release/mbar-ui`. Update the header comment `(target/release/mbar)` to `($(TARGET_DIR)/release/mbar)`.

- [ ] **Step 3: Verify**

```bash
CARGO_TARGET_DIR=/tmp/tdir make install PREFIX=/tmp/inst && ls -l /tmp/inst/bin
make release && make install PREFIX=/tmp/inst2 && ls -l /tmp/inst2/bin
rm -rf /tmp/tdir /tmp/inst /tmp/inst2
```

Expected: `mbar` and `sketchybar -> mbar` in both.

- [ ] **Step 4: Commit**

```bash
git add Makefile
git commit -m "fix: install targets find binaries in cargo's configured target directory"
```

---

### Task 8: `make app` — bundle assembly and signing

**Host:** macOS required (`lipo`, `codesign`, Sparkle framework).

**Files:**
- Create: `packaging/macos/Info.plist.in`, `packaging/macos/dev.rubeen.mbar.plist` (agent inside the bundle), `packaging/macos/sparkle.env`, `packaging/macos/build-app.sh`, `packaging/macos/verify-app.sh`, `packaging/macos/AppIcon.icns` (generated in Step 4)
- Modify: `Makefile` (targets `app`, `verify-app`), `.gitignore` (`/dist/`)

**Interfaces:**
- Consumes: `TARGET_DIR`, `UI_TARGET_DIR` (Task 7); Sparkle version/SHA from the spike notes (Task 1).
- Produces: `dist/mbar.app`; env vars `MBAR_SIGN_IDENTITY` (default `-`), `MBAR_UNIVERSAL` (default `1`); `packaging/macos/build-app.sh` prints the app path on success.

- [ ] **Step 1: Templates**

`packaging/macos/Info.plist.in`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>dev.rubeen.mbar</string>
	<key>CFBundleName</key>
	<string>mbar</string>
	<key>CFBundleDisplayName</key>
	<string>mbar</string>
	<key>CFBundleExecutable</key>
	<string>mbar-ui</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>@VERSION@</string>
	<key>CFBundleVersion</key>
	<string>@BUILD@</string>
	<key>LSMinimumSystemVersion</key>
	<string>13.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>SUFeedURL</key>
	<string>https://github.com/rubenvitt/mbar/releases/latest/download/appcast.xml</string>
	<key>SUPublicEDKey</key>
	<string>@SPARKLE_PUBLIC_KEY@</string>
	<key>SUEnableAutomaticChecks</key>
	<true/>
</dict>
</plist>
```

`packaging/macos/dev.rubeen.mbar.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>dev.rubeen.mbar</string>
	<key>BundleProgram</key>
	<string>Contents/MacOS/mbar</string>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>EnvironmentVariables</key>
	<dict>
		<key>PATH</key>
		<string>/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
	</dict>
	<key>StandardOutPath</key>
	<string>/tmp/mbar.log</string>
	<key>StandardErrorPath</key>
	<string>/tmp/mbar.log</string>
</dict>
</plist>
```

launchd does not expand `~` in agent plists and the bundle plist cannot know `$HOME`; the daemon therefore redirects its own log in Step 6 when running from a bundle (`/tmp/mbar.log` is only the launchd fallback).

`packaging/macos/sparkle.env` (values from the spike notes):

```bash
SPARKLE_VERSION=<version recorded in Task 1, e.g. 2.7.1>
SPARKLE_SHA256=<sha256 recorded in Task 1>
# Public EdDSA key (Task 21 generates it; until then build-app.sh writes an empty key
# and update checks fail signature validation, which is fine for local builds).
SPARKLE_PUBLIC_KEY=
```

(Fill the two values from `docs/superpowers/plans/2026-10-07-spike-notes.md`; they are known only after Task 1.)

- [ ] **Step 2: `packaging/macos/build-app.sh`**

```bash
#!/usr/bin/env bash
# Builds dist/mbar.app: universal mbar + mbar-ui, Sparkle, Info.plist, agent plist,
# command-line symlinks, then signs inside-out. MBAR_SIGN_IDENTITY=- (default) signs
# ad-hoc without hardened runtime; a Developer ID signs with runtime + timestamp.
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT=$PWD
source packaging/macos/sparkle.env

IDENTITY="${MBAR_SIGN_IDENTITY:--}"
UNIVERSAL="${MBAR_UNIVERSAL:-1}"
CARGO="${CARGO:-cargo}"
DIST="$ROOT/dist"
APP="$DIST/mbar.app"
C="$APP/Contents"

target_dir() { (cd "$1" && "$CARGO" metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p'); }
VERSION=$("$CARGO" metadata --format-version 1 --no-deps | sed -n 's/.*"name":"mbar","version":"\([^"]*\)".*/\1/p')
IFS=. read -r MAJ MIN PAT <<<"$VERSION"
BUILD=$((MAJ * 1000000 + MIN * 1000 + PAT))

if [ "$UNIVERSAL" = 1 ]; then TRIPLES=(aarch64-apple-darwin x86_64-apple-darwin); else TRIPLES=("$(rustc -vV | sed -n 's/^host: //p')"); fi
for t in "${TRIPLES[@]}"; do
  "$CARGO" build --release -p mbar --target "$t"
  (cd crates/mbar-ui && "$CARGO" build --release --target "$t")
done
TD=$(target_dir "$ROOT"); UTD=$(target_dir "$ROOT/crates/mbar-ui")

rm -rf "$APP"
mkdir -p "$C/MacOS" "$C/Resources/bin" "$C/Library/LaunchAgents" "$C/Frameworks"
lipo -create $(printf "$TD/%s/release/mbar " "${TRIPLES[@]}") -output "$C/MacOS/mbar"
lipo -create $(printf "$UTD/%s/release/mbar-ui " "${TRIPLES[@]}") -output "$C/MacOS/mbar-ui"
ln -s ../../MacOS/mbar "$C/Resources/bin/mbar"
ln -s ../../MacOS/mbar "$C/Resources/bin/sketchybar"
cp packaging/macos/AppIcon.icns "$C/Resources/AppIcon.icns"
cp lua/mbar.d.lua "$C/Resources/mbar.d.lua"
cp packaging/macos/dev.rubeen.mbar.plist "$C/Library/LaunchAgents/"
sed -e "s|@VERSION@|$VERSION|" -e "s|@BUILD@|$BUILD|" -e "s|@SPARKLE_PUBLIC_KEY@|$SPARKLE_PUBLIC_KEY|" \
  packaging/macos/Info.plist.in > "$C/Info.plist"
plutil -lint "$C/Info.plist" >/dev/null

# Sparkle (cached per version, checksum-verified).
CACHE="$DIST/.cache"; mkdir -p "$CACHE"
TARBALL="$CACHE/Sparkle-$SPARKLE_VERSION.tar.xz"
[ -f "$TARBALL" ] || curl -fsSL -o "$TARBALL" "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz"
echo "$SPARKLE_SHA256  $TARBALL" | shasum -a 256 -c - >/dev/null
rm -rf "$CACHE/sparkle" && mkdir -p "$CACHE/sparkle" && tar -xJf "$TARBALL" -C "$CACHE/sparkle"
ditto "$CACHE/sparkle/Sparkle.framework" "$C/Frameworks/Sparkle.framework"

sign() {
  if [ "$IDENTITY" = "-" ]; then codesign -f -s - "$@"
  else codesign -f -s "$IDENTITY" -o runtime --timestamp "$@"; fi
}
SP="$C/Frameworks/Sparkle.framework/Versions/B"
for x in "$SP"/XPCServices/*.xpc; do sign "$x"; done
sign "$SP/Autoupdate"
sign "$SP/Updater.app"
sign "$C/Frameworks/Sparkle.framework"
sign "$C/MacOS/mbar"
sign "$C/MacOS/mbar-ui"
sign "$APP"
echo "$APP"
```

`chmod +x packaging/macos/build-app.sh`.

- [ ] **Step 3: `packaging/macos/verify-app.sh`**

```bash
#!/usr/bin/env bash
# Structural checks for dist/mbar.app (CI runs this on every PR).
set -euo pipefail
APP="${1:-dist/mbar.app}"; C="$APP/Contents"; fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
check "codesign" "codesign --verify --deep --strict '$APP' 2>/dev/null"
for k in CFBundleIdentifier CFBundleExecutable CFBundleShortVersionString CFBundleVersion LSMinimumSystemVersion SUFeedURL SUPublicEDKey SUEnableAutomaticChecks; do
  check "Info.plist $k" "/usr/libexec/PlistBuddy -c 'Print :$k' '$C/Info.plist' >/dev/null 2>&1"
done
check "bundle id" "[ \"\$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' '$C/Info.plist')\" = dev.rubeen.mbar ]"
for n in mbar sketchybar; do
  check "bin/$n relative symlink" "[ \"\$(readlink '$C/Resources/bin/$n')\" = ../../MacOS/mbar ]"
done
check "daemon universal or native" "lipo -info '$C/MacOS/mbar' | grep -Eq 'arm64|x86_64'"
check "agent plist" "plutil -lint '$C/Library/LaunchAgents/dev.rubeen.mbar.plist' >/dev/null"
check "agent label" "[ \"\$(/usr/libexec/PlistBuddy -c 'Print :Label' '$C/Library/LaunchAgents/dev.rubeen.mbar.plist')\" = dev.rubeen.mbar ]"
check "Sparkle" "[ -d '$C/Frameworks/Sparkle.framework' ]"
check "mbar --version" "'$C/Resources/bin/mbar' --version | grep -q '^mbar-v'"
exit $fail
```

`chmod +x packaging/macos/verify-app.sh`.

- [ ] **Step 4: Icon** — a placeholder icon generated from an SF Symbol-free shape, so the bundle has one:

```bash
mkdir -p /tmp/icon.iconset
for s in 16 32 64 128 256 512; do
  sips -s format png -z $s $s /System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/GenericApplicationIcon.icns --out /tmp/icon.iconset/icon_${s}x${s}.png >/dev/null
  cp /tmp/icon.iconset/icon_${s}x${s}.png /tmp/icon.iconset/icon_$((s/2))x$((s/2))@2x.png 2>/dev/null || true
done
iconutil -c icns /tmp/icon.iconset -o packaging/macos/AppIcon.icns && rm -rf /tmp/icon.iconset
```

(A designed icon replaces this file later without code changes.)

- [ ] **Step 5: Makefile targets**

```make
app:
	packaging/macos/build-app.sh

verify-app:
	packaging/macos/verify-app.sh dist/mbar.app
```

Add `app verify-app` to `.PHONY`, add help lines (`app  build dist/mbar.app (MBAR_SIGN_IDENTITY, MBAR_UNIVERSAL)`, `verify-app  structural checks of dist/mbar.app`), and `/dist/` to `.gitignore`.

- [ ] **Step 6: Daemon log when running from the bundle** — in `crates/mbar/src/daemon.rs` `run`, after the bundle detection from Task 4:

```rust
    // The bundle's agent plist cannot name a per-user log path; redirect stdout/stderr
    // to ~/Library/Logs/mbar.log like `make install-agent` does.
    #[cfg(target_os = "macos")]
    if bundle.is_some() && std::env::var_os("MBAR_NO_LOG_REDIRECT").is_none() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            let path = std::path::Path::new(&home).join("Library/Logs/mbar.log");
            if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                use std::os::unix::io::AsRawFd;
                unsafe {
                    libc::dup2(f.as_raw_fd(), 1);
                    libc::dup2(f.as_raw_fd(), 2);
                }
            }
        }
    }
```

- [ ] **Step 7: Build and verify (ad-hoc and Developer ID)**

```bash
make app && make verify-app
MBAR_SIGN_IDENTITY="Developer ID Application: Ruben Vitt (H95J852PKP)" make app && make verify-app
spctl -a -vv dist/mbar.app   # "rejected (unnotarized)" is expected before Task 9
```

Expected: every line `ok`, exit 0.

- [ ] **Step 8: Commit**

```bash
git add packaging/macos Makefile .gitignore crates/mbar/src/daemon.rs
git commit -m "build: assemble and sign mbar.app with make app"
```

---

### Task 9: `make dmg` — notarization, update zip, appcast

**Host:** Linux VM can write `appcast.sh` and its test; DMG/notarization need macOS.

**Files:**
- Create: `packaging/macos/make-dmg.sh`, `packaging/macos/appcast.sh`, `packaging/macos/test-appcast.sh`
- Modify: `Makefile` (target `dmg`)

**Interfaces:**
- Consumes: `dist/mbar.app` (Task 8).
- Produces: `dist/mbar-<v>.dmg`, `dist/mbar-<v>.zip`, `dist/appcast.xml`. Env: `NOTARIZE` (1 default), `NOTARY_PROFILE` (local keychain profile, default `mbar-notary`) or `ASC_KEY_PATH`+`ASC_KEY_ID`+`ASC_ISSUER_ID` (CI), `SPARKLE_KEY_FILE` (optional; else `sign_update` uses the login keychain).
- `appcast.sh <version> <build> <zip-url> <signature-attrs> <notes-url>` prints the feed.

- [ ] **Step 1: Failing appcast test** — `packaging/macos/test-appcast.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
out=$(./appcast.sh 0.2.0 2000 https://x/mbar-0.2.0.zip 'sparkle:edSignature="SIG" length="42"' https://x/notes)
grep -q '<sparkle:version>2000</sparkle:version>' <<<"$out"
grep -q '<sparkle:shortVersionString>0.2.0</sparkle:shortVersionString>' <<<"$out"
grep -q '<sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>' <<<"$out"
grep -q 'url="https://x/mbar-0.2.0.zip" sparkle:edSignature="SIG" length="42"' <<<"$out"
command -v xmllint >/dev/null && xmllint --noout - <<<"$out"
echo ok
```

Run: `bash packaging/macos/test-appcast.sh` → FAIL (appcast.sh missing).

- [ ] **Step 2: `packaging/macos/appcast.sh`**

```bash
#!/usr/bin/env bash
# Prints a one-item Sparkle appcast. Args: version build zip-url signature-attrs notes-url
set -euo pipefail
v=$1 build=$2 url=$3 sig=$4 notes=$5
cat <<EOF
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>mbar</title>
    <item>
      <title>Version $v</title>
      <pubDate>$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
      <sparkle:version>$build</sparkle:version>
      <sparkle:shortVersionString>$v</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
      <sparkle:releaseNotesLink>$notes</sparkle:releaseNotesLink>
      <enclosure url="$url" $sig type="application/octet-stream"/>
    </item>
  </channel>
</rss>
EOF
```

`chmod +x` both scripts. Run the test → `ok`. Release notes are linked (GitHub release page) rather than embedded; Sparkle shows the page in its dialog.

- [ ] **Step 3: `packaging/macos/make-dmg.sh`**

```bash
#!/usr/bin/env bash
# dist/mbar.app → notarized app (stapled) → update zip + signature → DMG (notarized,
# stapled) → appcast.xml. NOTARIZE=0 skips Apple's service (local testing).
set -euo pipefail
cd "$(dirname "$0")/../.."
DIST=dist; APP=$DIST/mbar.app
VERSION=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")
BUILD=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$APP/Contents/Info.plist")
IDENTITY="${MBAR_SIGN_IDENTITY:?set MBAR_SIGN_IDENTITY to a Developer ID identity}"
ZIP=$DIST/mbar-$VERSION.zip; DMG=$DIST/mbar-$VERSION.dmg

notarize() {
  [ "${NOTARIZE:-1}" = 1 ] || return 0
  if [ -n "${ASC_KEY_PATH:-}" ]; then
    xcrun notarytool submit "$1" --key "$ASC_KEY_PATH" --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" --wait
  else
    xcrun notarytool submit "$1" --keychain-profile "${NOTARY_PROFILE:-mbar-notary}" --wait
  fi
}

# 1. Notarize the app (via a temporary zip) and staple it.
ditto -c -k --keepParent "$APP" "$DIST/notarize.zip"
notarize "$DIST/notarize.zip"; rm -f "$DIST/notarize.zip"
[ "${NOTARIZE:-1}" = 1 ] && xcrun stapler staple "$APP"

# 2. Update zip for Sparkle.
rm -f "$ZIP"; ditto -c -k --keepParent "$APP" "$ZIP"
SIGN_UPDATE="$DIST/.cache/sparkle/bin/sign_update"
if [ -n "${SPARKLE_KEY_FILE:-}" ]; then SIG=$("$SIGN_UPDATE" --ed-key-file "$SPARKLE_KEY_FILE" "$ZIP")
else SIG=$("$SIGN_UPDATE" "$ZIP"); fi

# 3. DMG with an /Applications link.
STAGE=$DIST/dmg-stage; rm -rf "$STAGE" "$DMG"; mkdir -p "$STAGE"
ditto "$APP" "$STAGE/mbar.app"; ln -s /Applications "$STAGE/Applications"
hdiutil create -volname mbar -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
codesign -f -s "$IDENTITY" --timestamp "$DMG"
notarize "$DMG"
[ "${NOTARIZE:-1}" = 1 ] && xcrun stapler staple "$DMG"

# 4. Appcast.
BASE="https://github.com/rubenvitt/mbar/releases/download/v$VERSION"
packaging/macos/appcast.sh "$VERSION" "$BUILD" "$BASE/mbar-$VERSION.zip" "$SIG" \
  "https://github.com/rubenvitt/mbar/releases/tag/v$VERSION" > "$DIST/appcast.xml"
echo "$DMG"; echo "$ZIP"; echo "$DIST/appcast.xml"
```

Makefile:

```make
dmg: app
	packaging/macos/make-dmg.sh
```

(add to `.PHONY` and help).

- [ ] **Step 4: Local dry run (macOS)**

```bash
MBAR_SIGN_IDENTITY="Developer ID Application: Ruben Vitt (H95J852PKP)" NOTARIZE=0 make dmg
hdiutil attach dist/mbar-*.dmg -nobrowse -mountpoint /tmp/mbar-dmg && ls /tmp/mbar-dmg && hdiutil detach /tmp/mbar-dmg
```

Expected: `mbar.app` and `Applications` in the volume, `dist/appcast.xml` present. (`sign_update` needs the Sparkle key from Task 21; before that, run with `SPARKLE_KEY_FILE` pointing at a throwaway key from `generate_keys -x /tmp/k`.) Full notarization is exercised in Task 21.

- [ ] **Step 5: Commit**

```bash
git add packaging/macos/make-dmg.sh packaging/macos/appcast.sh packaging/macos/test-appcast.sh Makefile
git commit -m "build: make dmg notarizes mbar.app and writes the Sparkle appcast"
```

---

### Task 10: `mbar-ui` launch modes

**Host:** Linux VM (`cargo test --no-default-features` in `crates/mbar-ui`).

**Files:**
- Modify: `crates/mbar-ui/src/model.rs` (`parse_cli_args`), `crates/mbar-ui/src/main.rs` (call site)

**Interfaces:**
- Produces: `pub struct CliArgs { pub bar_name: String, pub update: bool }`; `parse_cli_args(..) -> Result<CliArgs, String>`.

- [ ] **Step 1: Failing tests** — in `model.rs` tests (next to the existing `parse_cli_args` tests; adjust those to `.bar_name`):

```rust
#[test]
fn cli_update_flag() {
    let a = parse_cli_args(["--update".to_string()]).unwrap();
    assert!(a.update);
    assert_eq!(a.bar_name, "mbar");
    let a = parse_cli_args(Vec::<String>::new()).unwrap();
    assert!(!a.update);
    let a = parse_cli_args(["-psn_0_123".into(), "--update".into()]).unwrap();
    assert!(a.update);
}
```

Run: `cd crates/mbar-ui && cargo test --no-default-features cli_update_flag` → FAIL.

- [ ] **Step 2: Implement**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliArgs {
    pub bar_name: String,
    /// Opened by the daemon to show the Sparkle update dialog only.
    pub update: bool,
}
```

Change `parse_cli_args` to return `Result<CliArgs, String>`, add `else if arg == "--update" { update = true; }`, update the usage string to `usage: mbar-ui [--bar-name <name>] [--update]`, return `Ok(CliArgs { bar_name, update })`. In `main.rs`: `let args = match parse_cli_args(…) { Ok(a) => a, … }; let bar_name = args.bar_name.clone();`.

- [ ] **Step 3: Run tests and commit**

Run: `cargo test --no-default-features` → pass.

```bash
git add crates/mbar-ui/src/model.rs crates/mbar-ui/src/main.rs
git commit -m "feat: mbar-ui --update launch mode flag"
```

---

### Task 11: Onboarding model (detection and plans)

**Host:** Linux VM.

**Files:**
- Create: `crates/mbar-ui/src/onboarding.rs`
- Modify: `crates/mbar-ui/src/lib.rs` (`pub mod onboarding;`), `crates/mbar-ui/Cargo.toml` (`mbar-app = { path = "../mbar-app" }`)
- Create: `crates/mbar-ui/assets/starter-init.lua`

**Interfaces:**
- Consumes: `mbar_app::bundle::AppBundle`, `mbar_app::config::find_config`.
- Produces:
  - `pub enum Location { Applications, Translocated, DiskImage, Elsewhere }`, `pub fn location(bundle_root: &Path) -> Location`
  - `pub enum PathsD { Missing, Current, Stale(String) }`, `pub fn paths_d_state(current: Option<&str>, bin_dir: &Path) -> PathsD`, `pub fn paths_d_content(bin_dir: &Path) -> String`
  - `pub struct OldInstall { pub path: PathBuf, pub kind: OldKind, pub admin: bool }`, `pub enum OldKind { Binary, LaunchAgent, Foreign }`
  - `pub fn find_old_installs(home: &Path, usr_local_bin: &Path, bundle_root: Option<&Path>) -> Vec<OldInstall>`
  - `pub struct BrewState { pub sketchybar_installed: bool, pub sketchybar_running: bool, pub mbar_installed: bool }`, `pub fn parse_brew(formulas: &str, services: &str) -> BrewState`
  - `pub fn sbarlua_init_lua(sketchybarrc: &str) -> Option<String>`
  - `pub fn felix_helpers(config_dir: &Path) -> Vec<PathBuf>`
  - `pub fn needs_starter_config(home: &Path, xdg: &str) -> bool`, `pub const STARTER_INIT_LUA: &str`
  - `pub fn applescript_admin(shell: &str) -> String`, `pub fn shell_quote(s: &str) -> String`
  - `pub fn admin_shell(bin_dir: &Path, admin_removals: &[PathBuf]) -> String`

- [ ] **Step 1: Starter config** — `crates/mbar-ui/assets/starter-init.lua`:

```lua
-- mbar starter config. Docs: https://github.com/rubenvitt/mbar/blob/main/docs/LUA.md
local mbar = require("mbar")

mbar.bar({ height = 32, color = 0xe01e1e2e, padding_left = 8, padding_right = 8 })
mbar.default({
  icon = { font = "SF Pro:Semibold:14.0", color = 0xffcdd6f4 },
  label = { font = "SF Pro:Semibold:13.0", color = 0xffcdd6f4 },
  padding_left = 6,
  padding_right = 6,
})

mbar.add("item", "front_app", { position = "left", provider = { "front_app" } })
mbar.add("item", "clock", { position = "right", provider = { "clock", args = "%a %d.%m. %H:%M" } })
mbar.add("item", "battery", { position = "right", provider = { "battery", format = "{percent}%" } })
mbar.add("item", "cpu", { position = "right", provider = { "cpu", format = "CPU {percent}%" } })
```

- [ ] **Step 2: Failing tests** (`onboarding.rs` bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mbar-ob-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn location_kinds() {
        assert_eq!(location(Path::new("/Applications/mbar.app")), Location::Applications);
        assert_eq!(location(Path::new("/private/var/folders/x/AppTranslocation/ABC/d/mbar.app")), Location::Translocated);
        assert_eq!(location(Path::new("/Volumes/mbar/mbar.app")), Location::DiskImage);
        assert_eq!(location(Path::new("/Users/r/Downloads/mbar.app")), Location::Elsewhere);
    }

    #[test]
    fn paths_d_state_detects_stale_entry() {
        let bin = Path::new("/Applications/mbar.app/Contents/Resources/bin");
        assert_eq!(paths_d_state(None, bin), PathsD::Missing);
        assert_eq!(paths_d_state(Some("/Applications/mbar.app/Contents/Resources/bin\n"), bin), PathsD::Current);
        assert_eq!(
            paths_d_state(Some("/Users/r/Applications/mbar.app/Contents/Resources/bin\n"), bin),
            PathsD::Stale("/Users/r/Applications/mbar.app/Contents/Resources/bin".into())
        );
        assert_eq!(paths_d_content(bin), "/Applications/mbar.app/Contents/Resources/bin\n");
    }

    #[test]
    fn old_installs_found() {
        let home = tmp("home");
        let ulb = tmp("ulb");
        let lb = home.join(".local/bin");
        std::fs::create_dir_all(&lb).unwrap();
        std::fs::write(lb.join("mbar"), "bin").unwrap();
        symlink("mbar", lb.join("sketchybar")).unwrap();
        std::fs::write(lb.join("mbar-ui"), "bin").unwrap();
        let la = home.join("Library/LaunchAgents");
        std::fs::create_dir_all(&la).unwrap();
        std::fs::write(la.join("dev.rubeen.mbar.plist"), "<plist/>").unwrap();
        std::fs::write(ulb.join("mbar"), "bin").unwrap();

        let found = find_old_installs(&home, &ulb, Some(Path::new("/Applications/mbar.app")));
        let names: Vec<_> = found.iter().map(|o| (o.path.clone(), o.kind.clone(), o.admin)).collect();
        assert!(names.contains(&(lb.join("mbar"), OldKind::Binary, false)));
        assert!(names.contains(&(lb.join("sketchybar"), OldKind::Binary, false)));
        assert!(names.contains(&(lb.join("mbar-ui"), OldKind::Binary, false)));
        assert!(names.contains(&(la.join("dev.rubeen.mbar.plist"), OldKind::LaunchAgent, false)));
        assert!(names.contains(&(ulb.join("mbar"), OldKind::Binary, true)));
    }

    #[test]
    fn foreign_sketchybar_is_reported_not_removed() {
        let home = tmp("foreign");
        let ulb = tmp("foreign-ulb");
        let lb = home.join(".local/bin");
        std::fs::create_dir_all(&lb).unwrap();
        std::fs::write(lb.join("sketchybar"), "#!/bin/sh\necho mine\n").unwrap();
        let found = find_old_installs(&home, &ulb, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, OldKind::Foreign);
    }

    #[test]
    fn links_into_the_app_are_kept() {
        let home = tmp("keep");
        let ulb = tmp("keep-ulb");
        symlink("/Applications/mbar.app/Contents/MacOS/mbar", ulb.join("mbar")).unwrap();
        assert!(find_old_installs(&home, &ulb, Some(Path::new("/Applications/mbar.app"))).is_empty());
    }

    #[test]
    fn brew_parsing() {
        let s = parse_brew("lua\nsketchybar\njq\n", "Name Status User File\nsketchybar started r ~/Library/LaunchAgents/homebrew.mxcl.sketchybar.plist\n");
        assert_eq!(s, BrewState { sketchybar_installed: true, sketchybar_running: true, mbar_installed: false });
        let s = parse_brew("mbar\n", "sketchybar none\n");
        assert_eq!(s, BrewState { sketchybar_installed: false, sketchybar_running: false, mbar_installed: true });
    }

    #[test]
    fn sbarlua_conversion() {
        let rc = "#!/usr/bin/env lua\npackage.cpath = package.cpath .. \";/x/?.so\"\nsbar = require(\"sketchybar\")\nsbar.begin_config()\n";
        assert_eq!(sbarlua_init_lua(rc).unwrap(), "sbar = require(\"sketchybar\")\nsbar.begin_config()\n");
        assert_eq!(sbarlua_init_lua("#!/bin/bash\nsketchybar --bar height=30\n"), None);
    }

    #[test]
    fn felix_helper_detection() {
        let dir = tmp("felix");
        std::fs::create_dir_all(dir.join("helpers/x")).unwrap();
        std::fs::write(dir.join("helpers/x/sketchybar.h"), "snprintf(b, n, \"git.felix.%s\", name);").unwrap();
        std::fs::write(dir.join("init.lua"), "-- nothing").unwrap();
        assert_eq!(felix_helpers(&dir), vec![dir.join("helpers/x/sketchybar.h")]);
    }

    #[test]
    fn starter_needed_only_without_config() {
        let home = tmp("starter");
        assert!(needs_starter_config(&home, ""));
        std::fs::create_dir_all(home.join(".config/sketchybar")).unwrap();
        std::fs::write(home.join(".config/sketchybar/sketchybarrc"), "").unwrap();
        assert!(!needs_starter_config(&home, ""));
    }

    #[test]
    fn admin_script_quoting() {
        let s = admin_shell(
            Path::new("/Applications/mbar's.app/Contents/Resources/bin"),
            &[PathBuf::from("/usr/local/bin/mbar")],
        );
        assert_eq!(
            s,
            "printf '%s\\n' '/Applications/mbar'\\''s.app/Contents/Resources/bin' > /etc/paths.d/mbar && rm -f '/usr/local/bin/mbar'"
        );
        let a = applescript_admin("echo \"hi\" \\ there");
        assert_eq!(a, "do shell script \"echo \\\"hi\\\" \\\\ there\" with administrator privileges");
    }
}
```

Run: `cargo test --no-default-features onboarding` → FAIL.

- [ ] **Step 3: Implement** (top of `onboarding.rs`):

```rust
//! First-launch onboarding: what to clean up, what to convert, what to install. Pure
//! detection over the file system plus command/script builders; the GUI runs them.

use std::path::{Path, PathBuf};

pub const STARTER_INIT_LUA: &str = include_str!("../assets/starter-init.lua");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Applications,
    Translocated,
    DiskImage,
    Elsewhere,
}

pub fn location(bundle_root: &Path) -> Location {
    let s = bundle_root.to_string_lossy();
    if s.contains("/AppTranslocation/") {
        Location::Translocated
    } else if s.starts_with("/Volumes/") {
        Location::DiskImage
    } else if bundle_root.parent() == Some(Path::new("/Applications")) {
        Location::Applications
    } else {
        Location::Elsewhere
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathsD {
    Missing,
    Current,
    Stale(String),
}

pub fn paths_d_content(bin_dir: &Path) -> String {
    format!("{}\n", bin_dir.display())
}

pub fn paths_d_state(current: Option<&str>, bin_dir: &Path) -> PathsD {
    match current.map(str::trim) {
        None | Some("") => PathsD::Missing,
        Some(line) if Path::new(line) == bin_dir => PathsD::Current,
        Some(line) => PathsD::Stale(line.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OldKind {
    /// An mbar binary or `sketchybar -> mbar` link outside this app: removed.
    Binary,
    /// `~/Library/LaunchAgents/dev.rubeen.mbar.plist` from `make install-agent`: booted out, removed.
    LaunchAgent,
    /// Something else named `sketchybar` that shadows the app on the PATH: reported only.
    Foreign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldInstall {
    pub path: PathBuf,
    pub kind: OldKind,
    /// Needs the admin script (`/usr/local/bin`).
    pub admin: bool,
}

fn inside(path: &Path, root: Option<&Path>) -> bool {
    root.is_some_and(|r| path.starts_with(r))
}

/// Resolves a symlink chain (relative links against their directory); `None` when not a link.
fn link_target(p: &Path) -> Option<PathBuf> {
    let t = std::fs::read_link(p).ok()?;
    Some(if t.is_absolute() { t } else { p.parent()?.join(t) })
}

fn classify(path: &Path, bundle_root: Option<&Path>) -> Option<OldKind> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let target = link_target(path);
    if let Some(t) = &target {
        if inside(t, bundle_root) {
            return None;
        }
    }
    let name = path.file_name()?.to_str()?;
    let points_to_mbar = target
        .as_ref()
        .and_then(|t| t.file_name())
        .is_some_and(|n| n == "mbar" || n == "mbar-ui");
    match name {
        "mbar" | "mbar-ui" => Some(OldKind::Binary),
        "sketchybar" if points_to_mbar => Some(OldKind::Binary),
        "sketchybar" if meta.is_file() || target.is_some() => Some(OldKind::Foreign),
        _ => None,
    }
}

pub fn find_old_installs(home: &Path, usr_local_bin: &Path, bundle_root: Option<&Path>) -> Vec<OldInstall> {
    let mut out = Vec::new();
    for (dir, admin) in [(home.join(".local/bin"), false), (usr_local_bin.to_path_buf(), true)] {
        for name in ["mbar", "sketchybar", "mbar-ui"] {
            let p = dir.join(name);
            if let Some(kind) = classify(&p, bundle_root) {
                out.push(OldInstall { path: p, kind, admin });
            }
        }
    }
    let agent = home.join("Library/LaunchAgents/dev.rubeen.mbar.plist");
    if agent.exists() {
        out.push(OldInstall { path: agent, kind: OldKind::LaunchAgent, admin: false });
    }
    out
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrewState {
    pub sketchybar_installed: bool,
    pub sketchybar_running: bool,
    pub mbar_installed: bool,
}

/// `formulas` = `brew list --formula`, `services` = `brew services list`.
pub fn parse_brew(formulas: &str, services: &str) -> BrewState {
    let has = |n: &str| formulas.lines().any(|l| l.trim() == n);
    BrewState {
        sketchybar_installed: has("sketchybar"),
        sketchybar_running: services.lines().any(|l| {
            let mut f = l.split_whitespace();
            f.next() == Some("sketchybar") && f.next() == Some("started")
        }),
        mbar_installed: has("mbar"),
    }
}

/// SbarLua `sketchybarrc` (a `lua` shebang) → `init.lua` body without the shebang and
/// `package.cpath` lines (`docs/MIGRATING.md`).
pub fn sbarlua_init_lua(sketchybarrc: &str) -> Option<String> {
    let first = sketchybarrc.lines().next()?;
    if !(first.starts_with("#!") && first.contains("lua")) {
        return None;
    }
    let body: Vec<&str> = sketchybarrc
        .lines()
        .skip(1)
        .filter(|l| !l.contains("package.cpath"))
        .collect();
    Some(format!("{}\n", body.join("\n")))
}

/// Source files under the config dir that look up SketchyBar's mach service.
pub fn felix_helpers(config_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![config_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if matches!(p.extension().and_then(|x| x.to_str()), Some("c" | "h" | "m" | "swift" | "lua" | "sh"))
                && std::fs::read_to_string(&p).is_ok_and(|s| s.contains("git.felix."))
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn needs_starter_config(home: &Path, xdg: &str) -> bool {
    mbar_app::config::find_config("mbar", xdg, &home.to_string_lossy()).is_none()
}

pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// One root shell command: write `/etc/paths.d/mbar`, remove admin-owned old binaries.
pub fn admin_shell(bin_dir: &Path, admin_removals: &[PathBuf]) -> String {
    let mut cmd = format!(
        "printf '%s\\n' {} > /etc/paths.d/mbar",
        shell_quote(&bin_dir.to_string_lossy())
    );
    for p in admin_removals {
        cmd.push_str(&format!(" && rm -f {}", shell_quote(&p.to_string_lossy())));
    }
    cmd
}

/// AppleScript that runs `shell` as root after the standard admin prompt.
pub fn applescript_admin(shell: &str) -> String {
    let escaped = shell.replace('\\', "\\\\").replace('"', "\\\"");
    format!("do shell script \"{escaped}\" with administrator privileges")
}
```

(Use `.map_or(false, …)` instead of `is_some_and`/`is_ok_and` if the UI toolchain is older than 1.70; both exist since 1.70, so this is fine.)

- [ ] **Step 4: Run tests and commit**

Run: `cd crates/mbar-ui && cargo test --no-default-features` → pass.

```bash
git add crates/mbar-ui/src/onboarding.rs crates/mbar-ui/src/lib.rs crates/mbar-ui/Cargo.toml crates/mbar-ui/Cargo.lock crates/mbar-ui/assets/starter-init.lua
git commit -m "feat: onboarding detection and command builders for mbar.app"
```

---

### Task 12: Onboarding executor (commands)

**Host:** Linux VM for the command-list tests; execution on macOS (Task 16 drives it).

**Files:**
- Modify: `crates/mbar-ui/src/onboarding.rs` (append), `crates/mbar-ui/src/system.rs` (remove plist writer, add kickstart)

**Interfaces:**
- Consumes: Task 11 types.
- Produces:
  - `pub fn cleanup_commands(items: &[OldInstall], brew: &BrewState, brew_bin: Option<&Path>, uid: u32, remove_brew_sketchybar: bool) -> Vec<Vec<String>>` (user-level commands, in order)
  - `pub fn run_commands(cmds: &[Vec<String>]) -> Result<String, String>` (stops at first failure, returns combined output)
  - `pub fn run_admin(shell: &str) -> Result<(), String>` (`osascript -e`)
  - `pub fn find_brew() -> Option<PathBuf>`
  - `system::kickstart_daemon() -> io::Result<()>`, `system::current_uid() -> u32`; removed: `launch_agent_plist`, `launch_agent_program`, `install_launch_agent`, `remove_launch_agent`, `launch_agent_installed`, `LAUNCH_AGENT_PATH`.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn cleanup_command_order() {
    let items = vec![
        OldInstall { path: "/h/Library/LaunchAgents/dev.rubeen.mbar.plist".into(), kind: OldKind::LaunchAgent, admin: false },
        OldInstall { path: "/h/.local/bin/mbar".into(), kind: OldKind::Binary, admin: false },
        OldInstall { path: "/h/.local/bin/sketchybar".into(), kind: OldKind::Foreign, admin: false },
        OldInstall { path: "/usr/local/bin/mbar".into(), kind: OldKind::Binary, admin: true },
    ];
    let brew = BrewState { sketchybar_installed: true, sketchybar_running: true, mbar_installed: true };
    let cmds = cleanup_commands(&items, &brew, Some(Path::new("/opt/homebrew/bin/brew")), 501, true);
    let s: Vec<String> = cmds.iter().map(|c| c.join(" ")).collect();
    assert_eq!(s, [
        "/opt/homebrew/bin/brew services stop sketchybar",
        "/opt/homebrew/bin/brew uninstall sketchybar",
        "/opt/homebrew/bin/brew services stop mbar",
        "/opt/homebrew/bin/brew uninstall mbar",
        "/bin/launchctl bootout gui/501/dev.rubeen.mbar",
        "/bin/rm -f /h/Library/LaunchAgents/dev.rubeen.mbar.plist",
        "/bin/rm -f /h/.local/bin/mbar",
    ]);
    // Without consent the formula stays installed, only the service stops.
    let cmds = cleanup_commands(&[], &brew, Some(Path::new("/b/brew")), 501, false);
    assert_eq!(cmds[0].join(" "), "/b/brew services stop sketchybar");
    assert!(!cmds.iter().any(|c| c.join(" ") == "/b/brew uninstall sketchybar"));
}
```

`bootout` may fail when the agent is not loaded; `run_commands` therefore treats a failing `launchctl bootout` as success (assert in Step 3 code). Run → FAIL.

- [ ] **Step 2: Implement** (append to `onboarding.rs`):

```rust
use std::process::Command;

pub fn find_brew() -> Option<PathBuf> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

pub fn cleanup_commands(
    items: &[OldInstall],
    brew: &BrewState,
    brew_bin: Option<&Path>,
    uid: u32,
    remove_brew_sketchybar: bool,
) -> Vec<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut out = Vec::new();
    if let Some(b) = brew_bin.map(|p| p.to_string_lossy().into_owned()) {
        if brew.sketchybar_installed || brew.sketchybar_running {
            out.push(s(&[&b, "services", "stop", "sketchybar"]));
            if remove_brew_sketchybar && brew.sketchybar_installed {
                out.push(s(&[&b, "uninstall", "sketchybar"]));
            }
        }
        if brew.mbar_installed {
            out.push(s(&[&b, "services", "stop", "mbar"]));
            out.push(s(&[&b, "uninstall", "mbar"]));
        }
    }
    for i in items.iter().filter(|i| i.kind == OldKind::LaunchAgent) {
        out.push(s(&["/bin/launchctl", "bootout", &format!("gui/{uid}/dev.rubeen.mbar")]));
        out.push(s(&["/bin/rm", "-f", &i.path.to_string_lossy()]));
    }
    for i in items.iter().filter(|i| i.kind == OldKind::Binary && !i.admin) {
        out.push(s(&["/bin/rm", "-f", &i.path.to_string_lossy()]));
    }
    out
}

pub fn run_commands(cmds: &[Vec<String>]) -> Result<String, String> {
    let mut log = String::new();
    for c in cmds {
        let out = Command::new(&c[0]).args(&c[1..]).output().map_err(|e| format!("{}: {e}", c.join(" ")))?;
        log.push_str(&format!("$ {}\n{}{}", c.join(" "), String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
        let tolerated = c.get(1).map(String::as_str) == Some("bootout");
        if !out.status.success() && !tolerated {
            return Err(log);
        }
    }
    Ok(log)
}

pub fn run_admin(shell: &str) -> Result<(), String> {
    let out = Command::new("/usr/bin/osascript")
        .args(["-e", &applescript_admin(shell)])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
```

In `system.rs`: delete `LAUNCH_AGENT_PATH`, `xml_escape`, `launch_agent_program`, `launch_agent_plist`, `launch_agent_installed`, `install_launch_agent`, `remove_launch_agent` and their tests (`plist_contents`); keep `launch_agent_path` (onboarding uses it) and add:

```rust
/// Restarts the daemon through launchd (login item `dev.rubeen.mbar`).
pub fn kickstart_daemon() -> io::Result<()> {
    let uid = current_uid();
    let status = Command::new("/bin/launchctl")
        .args(["kickstart", "-k", &format!("gui/{uid}/{LAUNCH_AGENT_LABEL}")])
        .status()?;
    if status.success() { Ok(()) } else { Err(io::Error::other(format!("launchctl exited with {status}"))) }
}

pub fn current_uid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { getuid() }
}
```

`views/system.rs` still calls the removed functions; Task 16 replaces those calls. To keep the GUI build green in between, temporarily make `set_launch_agent` a no-op that pushes `Notification::info("Login item is managed by onboarding")` and render the switch as disabled.

- [ ] **Step 3: Run tests and commit**

Run: `cargo test --no-default-features` → pass. (macOS: `cargo build` → compiles.)

```bash
git add crates/mbar-ui/src
git commit -m "feat: onboarding cleanup commands; drop the hand-written LaunchAgent"
```

---

### Task 13: macOS glue in mbar-ui (login item, activation, app location, notifications)

**Host:** macOS required (ServiceManagement, AppKit).

**Files:**
- Create: `crates/mbar-ui/src/mac/mod.rs`, `crates/mbar-ui/src/mac/login_item.rs`, `crates/mbar-ui/src/mac/app.rs`
- Modify: `crates/mbar-ui/Cargo.toml` (macOS deps under the `gui` feature), `crates/mbar-ui/src/main.rs` (`#[cfg(target_os = "macos")] mod mac;`)

**Interfaces:**
- Produces:
  - `mac::login_item::{status() -> LoginItem, register() -> Result<(), String>, unregister() -> Result<(), String>, open_settings()}` with `pub enum LoginItem { NotRegistered, Enabled, RequiresApproval, NotFound }`
  - `mac::app::{bundle() -> Option<mbar_app::bundle::AppBundle>, set_accessory(bool), activate(), move_to_applications(src: &Path) -> Result<PathBuf, String>, relaunch(path: &Path) -> !, on_distributed(name: &str, f: impl Fn() + 'static)}`

- [ ] **Step 1: Dependencies** — `crates/mbar-ui/Cargo.toml`:

```toml
mbar-app = { path = "../mbar-app" }

[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
block2 = "0.6"
objc2-foundation = "0.3"
objc2-app-kit = "0.3"
```

(`mbar-app` is already added in Task 11.)

- [ ] **Step 2: `mac/login_item.rs`**

```rust
//! `SMAppService.agent(plistName: "dev.rubeen.mbar.plist")` (ServiceManagement, macOS 13).

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use objc2_foundation::{NSError, NSString};

#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItem {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
}

fn service() -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"SMAppService")?;
    let name = NSString::from_str("dev.rubeen.mbar.plist");
    unsafe { msg_send![cls, agentServiceWithPlistName: &*name] }
}

pub fn status() -> LoginItem {
    let Some(s) = service() else { return LoginItem::NotFound };
    let raw: isize = unsafe { msg_send![&*s, status] };
    match raw {
        1 => LoginItem::Enabled,
        2 => LoginItem::RequiresApproval,
        3 => LoginItem::NotFound,
        _ => LoginItem::NotRegistered,
    }
}

fn call(sel_register: bool) -> Result<(), String> {
    let s = service().ok_or("ServiceManagement unavailable")?;
    let mut err: *mut NSError = std::ptr::null_mut();
    let ok: bool = unsafe {
        if sel_register {
            msg_send![&*s, registerAndReturnError: &mut err]
        } else {
            msg_send![&*s, unregisterAndReturnError: &mut err]
        }
    };
    if ok {
        Ok(())
    } else {
        Err(unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string()).unwrap_or_default())
    }
}

pub fn register() -> Result<(), String> { call(true) }
pub fn unregister() -> Result<(), String> { call(false) }

pub fn open_settings() {
    if let Some(cls) = AnyClass::get(c"SMAppService") {
        let _: () = unsafe { msg_send![cls, openSystemSettingsLoginItems] };
    }
}
```

Use the selector spelling confirmed in the spike notes if it differs.

- [ ] **Step 3: `mac/app.rs`**

```rust
//! Bundle location, activation policy, relaunch and distributed notifications.

use std::path::{Path, PathBuf};
use std::process::Command;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSString};

pub fn bundle() -> Option<mbar_app::bundle::AppBundle> {
    let exe = std::env::current_exe().ok()?;
    let root = mbar_app::bundle::bundle_root_from_exe(&exe)?;
    mbar_app::bundle::read_bundle(&root)
}

/// No Dock icon / menu bar while only the Sparkle dialog is shown.
pub fn set_accessory(accessory: bool) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let policy = if accessory {
        NSApplicationActivationPolicy::Accessory
    } else {
        NSApplicationActivationPolicy::Regular
    };
    NSApplication::sharedApplication(mtm).setActivationPolicy(policy);
}

pub fn activate() {
    if let Some(mtm) = MainThreadMarker::new() {
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
}

/// Copies the running bundle to /Applications/mbar.app (replacing an older copy).
pub fn move_to_applications(src: &Path) -> Result<PathBuf, String> {
    let dst = PathBuf::from("/Applications/mbar.app");
    if dst.exists() {
        std::fs::remove_dir_all(&dst).map_err(|e| format!("remove {}: {e}", dst.display()))?;
    }
    let st = Command::new("/usr/bin/ditto").arg(src).arg(&dst).status().map_err(|e| e.to_string())?;
    if !st.success() {
        return Err(format!("ditto exited with {st}"));
    }
    Ok(dst)
}

pub fn relaunch(path: &Path) -> ! {
    let _ = Command::new("/usr/bin/open").arg("-n").arg(path).spawn();
    std::process::exit(0);
}

/// Calls `f` on the main thread whenever the distributed notification `name` arrives.
pub fn on_distributed(name: &str, f: impl Fn() + 'static) {
    let center = NSDistributedNotificationCenter::defaultCenter();
    let name = NSString::from_str(name);
    let block = RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| f());
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(&name), None, Some(&objc2_foundation::NSOperationQueue::mainQueue()), &block)
    };
    std::mem::forget(observer);
}
```

(Main-queue delivery runs on the main thread, i.e. GPUI's AppKit thread; the callback must only post into GPUI — see Task 14.)

- [ ] **Step 4: Build, manual check, commit**

Run: `cd crates/mbar-ui && cargo build && cargo clippy -- -D warnings`
Manual: from `dist/mbar.app` built with Task 8 (`make app` after this task), call nothing yet — this task is wired in Tasks 14–16.

```bash
git add crates/mbar-ui/Cargo.toml crates/mbar-ui/Cargo.lock crates/mbar-ui/src/mac crates/mbar-ui/src/main.rs
git commit -m "feat: macOS glue for mbar-ui: SMAppService, activation policy, relaunch"
```

---

### Task 14: Sparkle bridge and update mode

**Host:** macOS required.

**Files:**
- Create: `crates/mbar-ui/src/mac/sparkle.rs`
- Modify: `crates/mbar-ui/src/mac/mod.rs`, `crates/mbar-ui/src/main.rs`

**Interfaces:**
- Consumes: `mac::app::{set_accessory, activate, on_distributed}`, `CliArgs.update` (Task 10).
- Produces: `mac::sparkle::Updater` with `fn start(on_cycle_end: impl Fn() + 'static) -> Option<Updater>`, `fn check_in_background(&self)`, `fn check_now(&self)`, `fn auto_checks(&self) -> bool`, `fn set_auto_checks(&self, on: bool)`; GPUI action `CheckForUpdates` registered in `main.rs`.

- [ ] **Step 1: `mac/sparkle.rs`**

```rust
//! Sparkle 2 via the Objective-C runtime. Sparkle.framework is embedded in
//! Contents/Frameworks and loaded at runtime (no link-time dependency).

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_foundation::{NSBundle, NSError, NSString};

pub struct DelegateIvars {
    on_cycle_end: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: NSObject subclass; methods match SPUUpdaterDelegate /
    // SPUStandardUserDriverDelegate selectors (Sparkle 2).
    #[unsafe(super(NSObject))]
    #[name = "MbarSparkleDelegate"]
    #[ivars = DelegateIvars]
    struct SparkleDelegate;

    impl SparkleDelegate {
        #[unsafe(method(updater:didFinishUpdateCycleForUpdateCheck:error:))]
        fn did_finish(&self, _updater: &AnyObject, _check: isize, _error: Option<&NSError>) {
            (self.ivars().on_cycle_end)();
        }

        #[unsafe(method(supportsGentleScheduledUpdateReminders))]
        fn gentle(&self) -> bool {
            true
        }

        #[unsafe(method(standardUserDriverWillHandleShowingUpdate:forUpdate:state:))]
        fn will_show(&self, _handle: bool, _update: &AnyObject, _state: &AnyObject) {
            super::app::activate();
        }
    }
);

pub struct Updater {
    controller: Retained<AnyObject>,
    _delegate: Retained<SparkleDelegate>,
}

fn load_framework() -> bool {
    let Some(dir) = NSBundle::mainBundle().privateFrameworksPath() else { return false };
    let path = dir.stringByAppendingPathComponent(&NSString::from_str("Sparkle.framework"));
    NSBundle::bundleWithPath(&path).is_some_and(|b| b.load())
}

impl Updater {
    pub fn start(on_cycle_end: impl Fn() + 'static) -> Option<Updater> {
        if !load_framework() {
            log_warn("Sparkle.framework not found (not running from mbar.app)");
            return None;
        }
        let delegate = SparkleDelegate::alloc().set_ivars(DelegateIvars { on_cycle_end: Box::new(on_cycle_end) });
        let delegate: Retained<SparkleDelegate> = unsafe { msg_send![super(delegate), init] };
        let cls = AnyClass::get(c"SPUStandardUpdaterController")?;
        let controller: Retained<AnyObject> = unsafe {
            let alloc: *mut AnyObject = msg_send![cls, alloc];
            msg_send![alloc, initWithStartingUpdater: true, updaterDelegate: &*delegate, userDriverDelegate: &*delegate]
        };
        Some(Updater { controller, _delegate: delegate })
    }

    fn updater(&self) -> Retained<AnyObject> {
        unsafe { msg_send![&*self.controller, updater] }
    }

    pub fn check_in_background(&self) {
        let u = self.updater();
        let _: () = unsafe { msg_send![&*u, checkForUpdatesInBackground] };
    }

    pub fn check_now(&self) {
        let _: () = unsafe { msg_send![&*self.controller, checkForUpdates: std::ptr::null::<AnyObject>()] };
    }

    pub fn auto_checks(&self) -> bool {
        let u = self.updater();
        unsafe { msg_send![&*u, automaticallyChecksForUpdates] }
    }

    pub fn set_auto_checks(&self, on: bool) {
        let u = self.updater();
        let _: () = unsafe { msg_send![&*u, setAutomaticallyChecksForUpdates: on] };
    }
}

fn log_warn(msg: &str) {
    eprintln!("mbar-ui: {msg}");
}
```

Adjust to the exact `define_class!` syntax of objc2 0.6 (compare `crates/mbar-macos/src/platform/runloop.rs:149`, which uses the same macro) and to selectors confirmed in the spike. `ProtocolObject` import is unused unless the compiler asks for protocol conformance — remove if so.

- [ ] **Step 2: Wire `main.rs`**

```rust
actions!(mbar_ui, [Quit, CheckForUpdates]);
```

Inside `run(move |cx| { … })`, before opening the window:

```rust
            #[cfg(target_os = "macos")]
            let updater = {
                let update_mode = args.update;
                if update_mode {
                    mac::app::set_accessory(true);
                }
                let updater = std::rc::Rc::new(mac::sparkle::Updater::start(move || {
                    if update_mode {
                        std::process::exit(0);
                    }
                }));
                if let Some(u) = updater.as_ref() {
                    let u2 = updater.clone();
                    mac::app::on_distributed("dev.rubeen.mbar.checkForUpdates", move || {
                        if let Some(u) = u2.as_ref() { u.check_in_background() }
                    });
                    if update_mode {
                        u.check_in_background();
                    }
                }
                updater
            };
            #[cfg(target_os = "macos")]
            if args.update {
                // Only the Sparkle dialog; no main window. The delegate quits the app.
                if updater.is_none() {
                    std::process::exit(0);
                }
                return;
            }
            #[cfg(target_os = "macos")]
            {
                let u = updater.clone();
                cx.on_action(move |_: &CheckForUpdates, _cx: &mut App| {
                    if let Some(u) = u.as_ref() { u.check_now() }
                });
            }
```

Add an app menu with "Check for Updates…" bound to `CheckForUpdates` using gpui's `cx.set_menus(vec![Menu { name: "mbar".into(), items: vec![MenuItem::action("Check for Updates…", CheckForUpdates), MenuItem::separator(), MenuItem::action("Quit mbar", Quit)] }])` (check gpui-kit re-exports `Menu`/`MenuItem`; they come from gpui).

Pass `updater` to `AppView::new` (add a parameter `updater: Option<std::rc::Rc<Option<mac::sparkle::Updater>>>` behind `cfg(target_os = "macos")`, or store it in a GPUI global: `cx.set_global(UpdaterGlobal(updater))` with `struct UpdaterGlobal(std::rc::Rc<Option<mac::sparkle::Updater>>); impl Global for UpdaterGlobal {}`). Use the global; Task 16's System page reads it.

- [ ] **Step 3: Manual test (macOS)**

```bash
make app && open dist/mbar.app --args --update
```

With `defaults write dev.rubeen.mbar SUFeedURL http://127.0.0.1:8765/appcast.xml` and a feed for build `999999999` served by `python3 -m http.server 8765` from a temp dir: expect no Dock icon flash, Sparkle's update alert in front; "Remind Me Later" → app exits within a second. With a feed for the current build: app exits silently. Normal `open dist/mbar.app` shows the main window and the "Check for Updates…" menu item. Afterwards `defaults delete dev.rubeen.mbar SUFeedURL`.

- [ ] **Step 4: Commit**

```bash
git add crates/mbar-ui/src
git commit -m "feat: Sparkle updates in mbar-ui with a dialog-only --update mode"
```

---

### Task 15: Daemon version sync

**Host:** Linux VM for the comparison logic; macOS for the restart.

**Files:**
- Modify: `crates/mbar-ui/src/system.rs` (pure `daemon_version_from_stats`), `crates/mbar-ui/src/main.rs` (on launch)

**Interfaces:**
- Produces: `system::daemon_version_from_stats(json: &str) -> Option<String>`; `system::needs_daemon_restart(running: Option<&str>, bundle: &str) -> bool`.

- [ ] **Step 1: Failing tests** (`system.rs` tests):

```rust
#[test]
fn daemon_version_parsing() {
    assert_eq!(daemon_version_from_stats("{\n\t\"items\": 3,\n\t\"version\": \"0.2.0\"\n}\n").as_deref(), Some("0.2.0"));
    assert_eq!(daemon_version_from_stats("{\"items\": 3}"), None);
    assert!(needs_daemon_restart(Some("0.1.0"), "0.2.0"));
    assert!(!needs_daemon_restart(Some("0.2.0"), "0.2.0"));
    // An old daemon without the field is older than any bundle with this feature.
    assert!(needs_daemon_restart(None, "0.2.0"));
}
```

Run → FAIL.

- [ ] **Step 2: Implement**

```rust
pub fn daemon_version_from_stats(json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(json).ok()?["version"].as_str().map(str::to_string)
}

pub fn needs_daemon_restart(running: Option<&str>, bundle: &str) -> bool {
    running != Some(bundle)
}
```

- [ ] **Step 3: Wire on launch** (`main.rs`, macOS, after the updater block, both modes):

```rust
            #[cfg(target_os = "macos")]
            if let Some(bundle) = mac::app::bundle() {
                let client = Client::new(bar_name.clone());
                cx.background_executor().spawn(async move {
                    if let Ok(stats) = client.send_strs(&["--query", "stats"]) {
                        let running = mbar_ui_model::system::daemon_version_from_stats(&stats);
                        if mbar_ui_model::system::needs_daemon_restart(running.as_deref(), &bundle.short_version) {
                            let _ = mbar_ui_model::system::kickstart_daemon();
                        }
                    }
                }).detach();
            }
```

(Only when the daemon answers: a stopped daemon is not restarted here; onboarding/System page handle that.)

- [ ] **Step 4: Tests, build, commit**

Run: `cargo test --no-default-features` (Linux) → pass; `cargo build` (macOS).

```bash
git add crates/mbar-ui/src
git commit -m "feat: mbar-ui restarts a daemon older than the installed bundle"
```

---

### Task 16: Onboarding view and System page

**Host:** macOS required (GPUI app on macOS, SMAppService, admin prompt). The view compiles on Linux but cannot be exercised there.

**Files:**
- Create: `crates/mbar-ui/src/views/onboarding.rs`
- Modify: `crates/mbar-ui/src/views/mod.rs` (`Page::Setup`, first page when onboarding is incomplete), `crates/mbar-ui/src/views/system.rs` (login item via SMAppService, update section, "Run setup again")

**Interfaces:**
- Consumes: everything from Tasks 11–15.
- Produces: `views::onboarding::SetupView`; defaults key `dev.rubeen.mbar OnboardingCompleted` (bool) via `defaults write`.

- [ ] **Step 1: Step model inside the view**

```rust
//! First-launch setup: one row per step with state, action button and output.

use std::path::PathBuf;
use gpui_kit::component::{button::{Button, ButtonVariants as _}, h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::*;
use mbar_ui_model::onboarding as ob;

use super::{page_header, section, Shared};

#[derive(Clone, Debug, PartialEq)]
enum StepState { Pending, Running, Done(String), Failed(String), Skipped }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step { Location, Cleanup, TakeOver, Starter, CommandLine, LoginItem, Permissions }

impl Step {
    const ALL: [Step; 7] = [Step::Location, Step::Cleanup, Step::TakeOver, Step::Starter, Step::CommandLine, Step::LoginItem, Step::Permissions];
    fn title(self) -> &'static str {
        match self {
            Step::Location => "Move to Applications",
            Step::Cleanup => "Remove SketchyBar and old mbar installs",
            Step::TakeOver => "Use your SketchyBar config",
            Step::Starter => "Create a starter config",
            Step::CommandLine => "Install `mbar` and `sketchybar` commands",
            Step::LoginItem => "Start mbar at login",
            Step::Permissions => "Grant permissions",
        }
    }
}

pub struct SetupView {
    shared: Shared,
    states: Vec<(Step, StepState)>,
    plan: Option<SetupPlan>,
    remove_brew_sketchybar: bool,
}

/// Everything detected once when the page opens (background thread).
#[derive(Clone, Debug)]
struct SetupPlan {
    bundle_root: Option<PathBuf>,
    bin_dir: Option<PathBuf>,
    location: Option<ob::Location>,
    old: Vec<ob::OldInstall>,
    brew: ob::BrewState,
    paths_d: ob::PathsD,
    sbarlua: Option<(PathBuf, String)>,
    felix: Vec<PathBuf>,
    starter_needed: bool,
}
```

- [ ] **Step 2: Detection** (`impl SetupView`):

```rust
fn detect() -> SetupPlan {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let xdg = std::env::var("XDG_CONFIG_HOME").unwrap_or_default();
    let bundle = crate::mac::app::bundle();
    let root = bundle.as_ref().map(|b| b.root.clone());
    let bin = bundle.as_ref().map(|b| b.bin_dir());
    let brew = ob::find_brew().map(|b| {
        let out = |args: &[&str]| std::process::Command::new(&b).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        ob::parse_brew(&out(&["list", "--formula"]), &out(&["services", "list"]))
    }).unwrap_or_default();
    let sb = home.join(".config/sketchybar");
    let sbarlua = (!sb.join("init.lua").exists())
        .then(|| std::fs::read_to_string(sb.join("sketchybarrc")).ok())
        .flatten()
        .and_then(|rc| ob::sbarlua_init_lua(&rc))
        .map(|body| (sb.join("init.lua"), body));
    SetupPlan {
        location: root.as_deref().map(ob::location),
        old: ob::find_old_installs(&home, std::path::Path::new("/usr/local/bin"), root.as_deref()),
        paths_d: bin.as_deref().map(|b| ob::paths_d_state(std::fs::read_to_string("/etc/paths.d/mbar").ok().as_deref(), b)).unwrap_or(ob::PathsD::Missing),
        felix: ob::felix_helpers(&sb),
        starter_needed: ob::needs_starter_config(&home, &xdg),
        bundle_root: root,
        bin_dir: bin,
        brew,
        sbarlua,
    }
}
```

`SetupView::new` runs `detect` via `shared.spawn_blocking(cx, |_| Self::detect(), |this, plan, cx| { this.plan = Some(plan); this.init_states(); cx.notify(); None })`. `init_states` marks steps `Done("nothing to do")` when detection finds nothing (e.g. `Location::Applications`, empty `old` and no brew, `sbarlua == None`, `!starter_needed`, `PathsD::Current`, `login_item::status() == Enabled`).

- [ ] **Step 3: Actions** — one method per step, each runs on the background executor and sets `Running` → `Done(log)` / `Failed(log)`:

```rust
fn run_step(&mut self, step: Step, cx: &mut Context<Self>) {
    let Some(plan) = self.plan.clone() else { return };
    let remove_brew = self.remove_brew_sketchybar;
    self.set(step, StepState::Running, cx);
    self.shared.spawn_blocking(cx, move |_| -> Result<String, String> {
        match step {
            Step::Location => {
                let src = plan.bundle_root.ok_or("not running from mbar.app")?;
                let dst = crate::mac::app::move_to_applications(&src)?;
                crate::mac::app::relaunch(&dst)
            }
            Step::Cleanup => {
                let uid = mbar_ui_model::system::current_uid();
                let cmds = ob::cleanup_commands(&plan.old, &plan.brew, ob::find_brew().as_deref(), uid, remove_brew);
                let mut log = ob::run_commands(&cmds)?;
                let foreign: Vec<_> = plan.old.iter().filter(|o| o.kind == ob::OldKind::Foreign).map(|o| o.path.display().to_string()).collect();
                if !foreign.is_empty() {
                    log.push_str(&format!("\nLeft in place (not mbar): {}", foreign.join(", ")));
                }
                Ok(log)
            }
            Step::TakeOver => {
                let mut log = String::new();
                if let Some((path, body)) = &plan.sbarlua {
                    std::fs::write(path, body).map_err(|e| e.to_string())?;
                    log.push_str(&format!("Wrote {}\n", path.display()));
                }
                for f in &plan.felix {
                    log.push_str(&format!("Looks up SketchyBar's mach port (git.felix.*), change it to dev.rubeen.mbar: {}\n", f.display()));
                }
                Ok(log)
            }
            Step::Starter => {
                let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
                let dir = home.join(".config/mbar");
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                std::fs::write(dir.join("init.lua"), ob::STARTER_INIT_LUA).map_err(|e| e.to_string())?;
                Ok(format!("Wrote {}", dir.join("init.lua").display()))
            }
            Step::CommandLine => {
                let bin = plan.bin_dir.ok_or("not running from mbar.app")?;
                let admin: Vec<PathBuf> = plan.old.iter().filter(|o| o.admin && o.kind == ob::OldKind::Binary).map(|o| o.path.clone()).collect();
                ob::run_admin(&ob::admin_shell(&bin, &admin))?;
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
                let out = std::process::Command::new(&shell).args(["-lc", "command -v sketchybar"]).output().map_err(|e| e.to_string())?;
                let found = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if found.starts_with(&bin.to_string_lossy().to_string()) {
                    Ok(format!("sketchybar → {found}"))
                } else {
                    Err(format!("A new {shell} login shell finds sketchybar at '{found}'. Something earlier on your PATH shadows mbar."))
                }
            }
            Step::LoginItem => {
                crate::mac::login_item::register()?;
                match crate::mac::login_item::status() {
                    crate::mac::login_item::LoginItem::Enabled => Ok("Registered; mbar is running".into()),
                    crate::mac::login_item::LoginItem::RequiresApproval => {
                        crate::mac::login_item::open_settings();
                        Err("Allow mbar in System Settings → General → Login Items, then retry".into())
                    }
                    s => Err(format!("Login item status: {s:?}")),
                }
            }
            Step::Permissions => Ok(String::new()), // rendered live, see Step 4
        }
    }, move |this, result, cx| {
        this.set(step, match result { Ok(l) => StepState::Done(l), Err(e) => StepState::Failed(e) }, cx);
        if this.states.iter().all(|(_, s)| matches!(s, StepState::Done(_) | StepState::Skipped)) {
            let _ = std::process::Command::new("/usr/bin/defaults").args(["write", "dev.rubeen.mbar", "OnboardingCompleted", "-bool", "true"]).status();
        }
        None
    });
}
```

The Permissions row reuses `SystemView::permission_row` logic: move `permission_row` into `views/mod.rs` as a free function so both views use it; after a grant (status flips to Granted on the 2 s poll) call `sys::kickstart_daemon()` once.

- [ ] **Step 4: Render** — `page_header("Setup", "Get mbar running on this Mac", cx)`, then one `section(step.title(), cx)` per step containing: a status line (Pending/Running…/Done/Failed with output in a monospace `div().text_xs()`), a primary `Button::new(("run", i)).label("Run")` (disabled while `Running` or `Done`), a ghost `Button` "Skip" (`StepState::Skipped`). The Cleanup section lists `plan.old` paths and the brew state and has a `Switch` "Also uninstall Homebrew sketchybar" bound to `remove_brew_sketchybar` (default on). Order exactly `Step::ALL`.

- [ ] **Step 5: Navigation** — in `views/mod.rs`: add `Page::Setup` (label "Setup", icon `IconName::CircleCheck` or the closest existing icon), add it to `ALL` first; `AppView::new` starts on `Page::Setup` when `defaults read dev.rubeen.mbar OnboardingCompleted` is not `1`, otherwise `Page::Inspector`.

- [ ] **Step 6: System page** — replace the "Launch at login" section's switch with SMAppService:

```rust
let login_state = crate::mac::login_item::status();
let login = section("Launch at login", cx).child(setting_row(
    "Start mbar at login",
    match login_state {
        LoginItem::Enabled => "Registered as a login item (dev.rubeen.mbar)",
        LoginItem::RequiresApproval => "Waiting for approval in System Settings → Login Items",
        _ => "Not registered",
    },
    Switch::new("launch-at-login")
        .checked(login_state == LoginItem::Enabled)
        .on_click(cx.listener(|this, on: &bool, window, cx| this.set_login_item(*on, window, cx))),
    cx,
));
```

with `set_login_item` calling `register()`/`unregister()` and pushing a `Notification` with the error text. Add a section "Updates" with the current version (`mac::app::bundle().map(|b| b.short_version)`), a `Switch` "Automatically check for updates" bound to `Updater::auto_checks`/`set_auto_checks` (read the `UpdaterGlobal` from Task 14), and a `Button` "Check for Updates…" calling `check_now`. Add a `Button` "Run setup again" that switches the page to `Page::Setup` (emit an event or expose `AppView::show_setup`). Add a status row for `/etc/paths.d/mbar` showing `PathsD::Stale` with a "Fix" button running Step::CommandLine's admin action.

- [ ] **Step 7: Manual test on a clean user account (macOS)**

Create a test macOS user (System Settings → Users & Groups) or use a VM, install Homebrew SketchyBar there (`brew install FelixKratz/formulae/sketchybar && brew services start sketchybar`), copy `dist/mbar.app` to `~/Downloads`, open it: expect the Location step first; run every step; verify `sketchybar --query bar` from a new Terminal hits mbar, the login item shows "mbar", reboot/log out-in → bar comes back.

- [ ] **Step 8: Commit**

```bash
git add crates/mbar-ui/src
git commit -m "feat: first-launch setup and SMAppService login item in mbar-ui"
```

---

### Task 17: Docs for the app

**Host:** Linux VM.

**Files:**
- Modify: `docs/INSTALL.md`, `docs/MIGRATING.md`, `README.md`

- [ ] **Step 1: `docs/INSTALL.md`** — new first section before "Build from source":

```markdown
## Install the app

1. Download `mbar-<version>.dmg` from the [latest release](https://github.com/rubenvitt/mbar/releases/latest).
2. Open it and drag **mbar** to **Applications**.
3. Open mbar. The setup page walks through:
   - removing Homebrew SketchyBar and older mbar installs (it lists everything first),
   - using your existing `~/.config/sketchybar` config (a SbarLua `sketchybarrc` gets an `init.lua`),
   - installing the `mbar` and `sketchybar` commands (`/etc/paths.d/mbar`, one admin prompt),
   - starting mbar at login (System Settings → General → Login Items shows "mbar"),
   - Accessibility and Screen Recording.

Updates: mbar checks for new versions once a day and opens the update dialog by itself.
Turn this off on the System page. Open terminals after setup see the new commands;
tools started without a login shell (AeroSpace, skhd, …) need the full path
`/Applications/mbar.app/Contents/Resources/bin/sketchybar`.
```

Mark the "Start at login (LaunchAgent)" and Homebrew sections as "for source builds"; note that the app's setup removes `~/Library/LaunchAgents/dev.rubeen.mbar.plist` because both use the label `dev.rubeen.mbar`.

- [ ] **Step 2: `docs/MIGRATING.md`** — checklist step 1 becomes "Install `mbar.app` (`INSTALL.md`); its setup stops and removes SketchyBar for you."; keep the source-build path as an alternative.

- [ ] **Step 3: `README.md`** — install snippet points to the DMG first.

- [ ] **Step 4: Commit**

```bash
git add docs/INSTALL.md docs/MIGRATING.md README.md
git commit -m "docs: install mbar from the DMG"
```

---

### Task 18: release-please and Conventional Commits

**Host:** Linux VM.

**Files:**
- Create: `release-please-config.json`, `.release-please-manifest.json`, `.github/workflows/pr-title.yml`

**Interfaces:**
- Produces: release PRs bumping `Cargo.toml` `workspace.package.version`, `crates/mbar-ui/Cargo.toml` `package.version`, both `Cargo.lock` entries, `CHANGELOG.md`; tags `vX.Y.Z`.

- [ ] **Step 1: `release-please-config.json`**

```json
{
  "$schema": "https://raw.githubusercontent.com/googleapis/release-please/main/schemas/config.json",
  "release-type": "simple",
  "include-component-in-tag": false,
  "include-v-in-tag": true,
  "bump-minor-pre-major": true,
  "bump-patch-for-minor-pre-major": false,
  "packages": {
    ".": {
      "package-name": "mbar",
      "changelog-path": "CHANGELOG.md",
      "extra-files": [
        { "type": "toml", "path": "Cargo.toml", "jsonpath": "$.workspace.package.version" },
        { "type": "toml", "path": "crates/mbar-ui/Cargo.toml", "jsonpath": "$.package.version" },
        { "type": "toml", "path": "Cargo.lock", "jsonpath": "$.package[?(@.name.startsWith('mbar'))].version" },
        { "type": "toml", "path": "crates/mbar-ui/Cargo.lock", "jsonpath": "$.package[?(@.name.startsWith('mbar'))].version" }
      ]
    }
  }
}
```

`.release-please-manifest.json`:

```json
{ ".": "0.1.0" }
```

`release-type: simple` also writes `version.txt`; keep it (harmless) or add `"version-file": "version.txt"` explicitly. Check that no third-party crate name starts with `mbar` in either lock file: `grep -n '^name = "mbar' Cargo.lock crates/mbar-ui/Cargo.lock` must list only workspace crates.

- [ ] **Step 2: `.github/workflows/pr-title.yml`**

```yaml
name: PR title
on:
  pull_request:
    types: [opened, edited, synchronize, reopened]
permissions:
  pull-requests: read
jobs:
  conventional:
    runs-on: ubuntu-latest
    steps:
      - uses: amannn/action-semantic-pull-request@v5
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

PRs are squash-merged, so the PR title becomes the Conventional Commit on `main`. Set the repo default: `gh repo edit --enable-squash-merge --enable-merge-commit=false --enable-rebase-merge=false` (do this in Task 21 with the other repo settings).

- [ ] **Step 3: Validate JSON, commit**

```bash
python3 -m json.tool release-please-config.json >/dev/null && python3 -m json.tool .release-please-manifest.json >/dev/null
git add release-please-config.json .release-please-manifest.json .github/workflows/pr-title.yml
git commit -m "ci: release-please and Conventional Commit PR titles"
```

---

### Task 19: CI bundle check on every PR

**Host:** Linux VM to write; runs on `macos-latest`.

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add the job**

```yaml
  app-bundle:
    name: macOS (mbar.app, ad-hoc)
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-apple-darwin,x86_64-apple-darwin
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: |
            . -> target
            crates/mbar-ui -> target
      - run: bash packaging/macos/test-appcast.sh
      - run: make app
        env:
          MBAR_SIGN_IDENTITY: "-"
      - run: make verify-app
```

- [ ] **Step 2: Push the branch and watch**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: build and verify mbar.app on every PR"
git push -u origin feat/macos-app
gh pr create --draft --title "feat: mbar.app with DMG, onboarding and auto-updates" --body "Implements docs/superpowers/specs/2026-10-07-macos-app-distribution-design.md

🤖 Generated with [Claude Code](https://claude.com/claude-code)

https://claude.ai/code/session_019nCwi5V5UPggncxAAJ1wRu"
gh pr checks --watch
```

Expected: all jobs green (the PR title check included).

---

### Task 20: Release workflow

**Host:** Linux VM to write; runs on `macos-15`.

**Files:**
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Workflow**

```yaml
name: Release
on:
  push:
    branches: [main]
permissions:
  contents: write
  pull-requests: write
jobs:
  release-please:
    runs-on: ubuntu-latest
    outputs:
      created: ${{ steps.rp.outputs.release_created }}
      tag: ${{ steps.rp.outputs.tag_name }}
    steps:
      - id: rp
        uses: googleapis/release-please-action@v4
        with:
          config-file: release-please-config.json
          manifest-file: .release-please-manifest.json

  build:
    needs: release-please
    if: needs.release-please.outputs.created == 'true'
    runs-on: macos-15
    env:
      TAG: ${{ needs.release-please.outputs.tag }}
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ needs.release-please.outputs.tag }}
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-apple-darwin,x86_64-apple-darwin
      - name: Import signing certificate
        env:
          P12: ${{ secrets.MACOS_CERT_P12 }}
          P12_PASSWORD: ${{ secrets.MACOS_CERT_PASSWORD }}
        run: |
          KC=$RUNNER_TEMP/build.keychain-db; PW=$(openssl rand -hex 16)
          echo "$P12" | base64 --decode > $RUNNER_TEMP/cert.p12
          security create-keychain -p "$PW" "$KC"
          security set-keychain-settings -lut 21600 "$KC"
          security unlock-keychain -p "$PW" "$KC"
          security import $RUNNER_TEMP/cert.p12 -P "$P12_PASSWORD" -A -t cert -f pkcs12 -k "$KC"
          security set-key-partition-list -S apple-tool:,apple: -k "$PW" "$KC"
          security list-keychains -d user -s "$KC" $(security list-keychains -d user | tr -d '"')
          rm $RUNNER_TEMP/cert.p12
      - name: Write notary and Sparkle keys
        env:
          ASC_KEY_P8: ${{ secrets.ASC_KEY_P8 }}
          SPARKLE_KEY: ${{ secrets.SPARKLE_ED_PRIVATE_KEY }}
        run: |
          printf '%s' "$ASC_KEY_P8" > $RUNNER_TEMP/asc.p8
          printf '%s' "$SPARKLE_KEY" > $RUNNER_TEMP/sparkle.key
      - name: Build, sign, notarize
        env:
          MBAR_SIGN_IDENTITY: "Developer ID Application: Ruben Vitt (H95J852PKP)"
          ASC_KEY_PATH: ${{ runner.temp }}/asc.p8
          ASC_KEY_ID: ${{ secrets.ASC_KEY_ID }}
          ASC_ISSUER_ID: ${{ secrets.ASC_ISSUER_ID }}
          SPARKLE_KEY_FILE: ${{ runner.temp }}/sparkle.key
        run: make dmg && make verify-app
      - name: Upload release assets
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          V=${TAG#v}
          gh release upload "$TAG" dist/mbar-$V.dmg dist/mbar-$V.zip dist/appcast.xml --clobber
      - name: Clean up
        if: always()
        run: rm -f $RUNNER_TEMP/asc.p8 $RUNNER_TEMP/sparkle.key; security delete-keychain $RUNNER_TEMP/build.keychain-db || true
```

`make dmg` depends on `app`, which uses `MBAR_SIGN_IDENTITY` too. `SPARKLE_PUBLIC_KEY` comes from `packaging/macos/sparkle.env` (committed in Task 21).

- [ ] **Step 2: Lint and commit**

```bash
pip install --user yamllint >/dev/null 2>&1; yamllint -d relaxed .github/workflows/release.yml
git add .github/workflows/release.yml
git commit -m "ci: release workflow builds, notarizes and uploads mbar.app"
```

---

### Task 21: Credentials and repository settings

**Host:** macOS required (keychain export, `xcrun notarytool`, browser session for App Store Connect).

**Files:**
- Modify: `packaging/macos/sparkle.env` (public key)

Secrets never get written into the repo or printed into the conversation; files with private material live in `$TMPDIR/mbar-secrets` and are removed at the end.

- [ ] **Step 1: Developer ID `.p12`**

```bash
S=$(mktemp -d)/mbar-secrets; mkdir -p "$S"; chmod 700 "$S"
PW=$(openssl rand -base64 24)
security find-identity -v -p codesigning | grep "Developer ID Application: Ruben Vitt (H95J852PKP)"
security export -k login.keychain-db -t identities -f pkcs12 -P "$PW" -o "$S/devid-all.p12"
```

`security export -t identities` exports every identity in the keychain; to export only the Developer ID one, use Keychain Access (select "Developer ID Application: Ruben Vitt" + its private key → Export 2 items → `.p12`) and save to `$S/devid.p12` with the password `$PW`. macOS asks for the login password to allow the export — the user confirms that dialog. Then:

```bash
base64 -i "$S/devid.p12" | gh secret set MACOS_CERT_P12 -R rubenvitt/mbar
printf '%s' "$PW" | gh secret set MACOS_CERT_PASSWORD -R rubenvitt/mbar
```

- [ ] **Step 2: App Store Connect API key (browser)**

Using the Chrome DevTools MCP browser (the user is logged into appstoreconnect.apple.com; if not, ask them to log in — never type their Apple ID password):
1. Open `https://appstoreconnect.apple.com/access/integrations/api`.
2. "Team Keys" → "+" (or "Generate API Key" the first time), name `mbar notarization`, access `Developer`, Generate.
3. Note the **Key ID** and the **Issuer ID** shown above the table.
4. Download the `.p8` (one-time download) into `$S/` (set Chrome's download dir or move it from `~/Downloads`).

```bash
gh secret set ASC_KEY_ID -R rubenvitt/mbar --body "<Key ID from step 3>"
gh secret set ASC_ISSUER_ID -R rubenvitt/mbar --body "<Issuer ID from step 3>"
gh secret set ASC_KEY_P8 -R rubenvitt/mbar < "$S"/AuthKey_*.p8
xcrun notarytool store-credentials mbar-notary --key "$S"/AuthKey_*.p8 --key-id "<Key ID>" --issuer "<Issuer ID>"
```

(The angle-bracket values are read off the page in step 3; they are not secrets in the repo.)

- [ ] **Step 3: Sparkle EdDSA key**

```bash
GK=dist/.cache/sparkle/bin/generate_keys     # extracted by make app
"$GK"                                         # creates the key in the login keychain, prints the public key
"$GK" -x "$S/sparkle.key"                     # export private key
gh secret set SPARKLE_ED_PRIVATE_KEY -R rubenvitt/mbar < "$S/sparkle.key"
PUB=$("$GK" -p)
sed -i '' "s|^SPARKLE_PUBLIC_KEY=.*|SPARKLE_PUBLIC_KEY=$PUB|" packaging/macos/sparkle.env
```

- [ ] **Step 4: Repo settings**

```bash
gh repo edit rubenvitt/mbar --enable-squash-merge --enable-merge-commit=false --enable-rebase-merge=false --delete-branch-on-merge
gh secret list -R rubenvitt/mbar
```

Expected: the six secrets listed.

- [ ] **Step 5: Full local notarized build**

```bash
MBAR_SIGN_IDENTITY="Developer ID Application: Ruben Vitt (H95J852PKP)" make dmg
spctl -a -vv dist/mbar.app            # accepted, source=Notarized Developer ID
spctl -a -t open --context context:primary-signature -vv dist/mbar-*.dmg
xcrun stapler validate dist/mbar-*.dmg
```

- [ ] **Step 6: Clean up and commit**

```bash
rm -rf "$S"
git add packaging/macos/sparkle.env
git commit -m "build: Sparkle public key for update signatures"
```

---

### Task 22: Maintainer dotfiles

**Host:** macOS (paths on the maintainer's machine); edits are plain text.

**Files (in `~/.dotfiles`, a separate repo):**
- Modify: `aerospace/aerospace.toml` (two trigger lines + comment), `r-tools/sbar` (`do_restart`, `die` message), `r-tools/CLAUDE.md` (sbar paragraph)

- [ ] **Step 1: AeroSpace** — replace `$HOME/.local/bin/sketchybar` with `/Applications/mbar.app/Contents/Resources/bin/sketchybar` in both triggers; update the comment above to: "AeroSpace startet Befehle ohne Login-Shell und liest /etc/paths.d nicht, deshalb der volle Pfad in mbar.app."

- [ ] **Step 2: sbar** — `do_restart`:

```bash
do_restart() {
  if ! launchctl print "$SERVICE" >/dev/null 2>&1; then
    die "mbar ist nicht als Anmeldeobjekt registriert — mbar.app öffnen, Seite Setup"
  fi
  launchctl kickstart -k "$SERVICE"
  …(rest unchanged)
}
```

Remove the `PLIST` variable. In the header comment and `usage`, mention mbar.app instead of `make install-agent`. Update `r-tools/CLAUDE.md` accordingly (binary in `/Applications/mbar.app`, login item via the app).

- [ ] **Step 3: Commit in the dotfiles repo**

```bash
cd ~/.dotfiles
git add aerospace/aerospace.toml r-tools/sbar r-tools/CLAUDE.md
git commit -m "mbar: auf mbar.app umgestellt (AeroSpace-Pfad, sbar restart)"
```

(Do this right after Task 24's onboarding run, when `/Applications/mbar.app` exists.)

---

### Task 23: Update path end-to-end (local feed)

**Host:** macOS required.

- [ ] **Step 1: Build two versions**

```bash
git stash -u || true
MBAR_SIGN_IDENTITY="Developer ID Application: Ruben Vitt (H95J852PKP)" NOTARIZE=0 make dmg   # version N (Cargo.toml as is)
cp -R dist/mbar.app /tmp/mbar-N.app
sed -i '' 's/^version = "\(.*\)\.\(.*\)\.\(.*\)"/version = "\1.\2.99"/' Cargo.toml   # temporary N+1
MBAR_SIGN_IDENTITY="Developer ID Application: Ruben Vitt (H95J852PKP)" NOTARIZE=0 make dmg
mkdir -p /tmp/feed && cp dist/mbar-*.zip /tmp/feed/
sed "s|https://github.com/rubenvitt/mbar/releases/download/v[^/]*/|http://127.0.0.1:8765/|" dist/appcast.xml > /tmp/feed/appcast.xml
git checkout Cargo.toml Cargo.lock; git stash pop || true
```

- [ ] **Step 2: Install N and point it at the local feed**

```bash
rm -rf /Applications/mbar.app && cp -R /tmp/mbar-N.app /Applications/mbar.app
open /Applications/mbar.app          # finish setup if needed
defaults write dev.rubeen.mbar SUFeedURL http://127.0.0.1:8765/appcast.xml
(cd /tmp/feed && python3 -m http.server 8765) &
```

- [ ] **Step 3: Trigger the daemon check** — quit the UI, restart the daemon (`launchctl kickstart -k gui/$(id -u)/dev.rubeen.mbar`), wait 120 s (or temporarily build with the delay set to 5 s).

Expected, in order: Sparkle's dialog appears by itself (no main window, no Dock flash) → "Install Update" → app relaunches → within seconds `mbar --query stats | grep version` shows `X.Y.99` → Accessibility still granted (`mbar --query menus` works) → `~/Library/Application Support/mbar/update-state.json` has `offered_build`.

- [ ] **Step 4: UI-already-open variant** — reinstall N, open the main window, restart the daemon, wait: the dialog appears in the running app (distributed notification path).

- [ ] **Step 5: Offline variant** — stop the HTTP server, restart the daemon, wait 120 s: no dialog, `~/Library/Logs/mbar.log` has `update check: could not fetch`.

- [ ] **Step 6: Clean up**

```bash
kill %1; defaults delete dev.rubeen.mbar SUFeedURL; rm -rf /tmp/feed /tmp/mbar-N.app
```

Record the results in the PR description.

---

### Task 24: First real release and the maintainer's own migration

**Host:** macOS required.

- [ ] **Step 1: Merge** — mark the PR ready, wait for green checks, squash-merge with the title `feat: mbar.app with DMG, onboarding and auto-updates`.

- [ ] **Step 2: Release PR** — release-please opens "chore(main): release 0.2.0". Review `CHANGELOG.md` and the bumped versions (`Cargo.toml`, `crates/mbar-ui/Cargo.toml`, both lock files). Merge it.

- [ ] **Step 3: Watch the release build**

```bash
gh run watch -R rubenvitt/mbar $(gh run list -R rubenvitt/mbar -w Release -L 1 --json databaseId -q '.[0].databaseId')
gh release view v0.2.0 -R rubenvitt/mbar --json assets -q '.assets[].name'
curl -fsSL https://github.com/rubenvitt/mbar/releases/latest/download/appcast.xml | grep sparkle:version
```

Expected assets: `mbar-0.2.0.dmg`, `mbar-0.2.0.zip`, `appcast.xml`; `sparkle:version` `2000`.

- [ ] **Step 4: Install on the maintainer's Mac via the DMG** — download the DMG in the browser (so it gets the quarantine flag like for any user), open it, drag to Applications, open mbar. Run every setup step: it must remove `~/.local/bin/{mbar,sketchybar,mbar-ui}`, `~/Library/LaunchAgents/dev.rubeen.mbar.plist` and `brew uninstall sketchybar`; config `~/.config/sketchybar/init.lua` is used as is.

- [ ] **Step 5: Verify**

```bash
which -a sketchybar mbar                 # → /Applications/mbar.app/Contents/Resources/bin/… only (new terminal)
brew list --formula | grep -c sketchybar # → 0
ls ~/.local/bin/mbar ~/Library/LaunchAgents/dev.rubeen.mbar.plist 2>&1   # → No such file
mbar --query stats | grep version        # → 0.2.0
sbar status
```

Then Task 22 (dotfiles), and check: AeroSpace workspace switch updates the spaces item, popups open on click, app menus show (Accessibility granted to "mbar"), log out/in → bar starts.

- [ ] **Step 6: Second release for the real update path** — any small `fix:` merged later produces 0.2.1; on the maintainer's Mac the update dialog must appear within 24 h (or after `launchctl kickstart -k` + 120 s) and install cleanly.
