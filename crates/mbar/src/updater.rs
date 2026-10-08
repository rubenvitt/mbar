//! Daemon-side update check for mbar.app
//! (`docs/superpowers/specs/2026-10-07-macos-app-distribution-design.md`, "Update
//! flow"): read the appcast, then open the app's Sparkle dialog, or restart the daemon
//! when a newer bundle is already installed. The decision is pure (`mbar_app::update`);
//! this module only talks to the system through [`System`].

use std::path::{Path, PathBuf};

use mbar_app::appcast::{best_item, parse_appcast};
use mbar_app::bundle::AppBundle;
use mbar_app::update::{decide, record, Action, UpdateState};

/// First check this long after the daemon starts.
const FIRST_CHECK_SECS: u64 = 120;
/// Then one check per this interval (wall-clock time).
const CHECK_INTERVAL_SECS: u64 = 86_400;
/// How often the thread wakes to see whether the next check is due. `thread::sleep`
/// does not advance while a Mac sleeps, so one 24 h sleep could stretch over days.
const WAKE_SECS: u64 = 3_600;

/// Everything the check needs from the outside world (a fake in the tests, the
/// `curl`/`defaults`/`open`/`launchctl` runner on macOS).
pub trait System {
    /// The feed body, or `None` when offline / not found (HTTP errors included).
    fn fetch(&self, url: &str) -> Option<String>;
    fn os_version(&self) -> String;
    /// `defaults read dev.rubeen.mbar SUEnableAutomaticChecks`; `None` when unset.
    fn auto_checks(&self) -> Option<bool>;
    /// `defaults read dev.rubeen.mbar SUFeedURL`.
    fn feed_override(&self) -> Option<String>;
    fn ui_running(&self) -> bool;
    /// Ask the running UI to show the Sparkle dialog (distributed notification).
    fn notify_ui(&self);
    /// Launch the UI straight into the Sparkle dialog.
    fn open_ui_update(&self);
    /// Replace the running daemon with the binary on disk (normally does not return).
    fn restart_self(&self);
    /// Unix seconds.
    fn now(&self) -> u64;
    /// The state file's contents; empty when missing or unreadable.
    fn load_state(&self) -> String;
    /// Persist the state file. An error means the restart-once guard is not on disk,
    /// so the check must not restart the daemon.
    fn save_state(&self, json: &str) -> Result<(), String>;
    /// `CFBundleVersion` of the bundle currently on disk.
    fn bundle_build(&self) -> Option<u64>;
}

/// One update check. Returns what was done ([`Action::Nothing`] when a restart was
/// skipped because its guard could not be saved).
pub fn check_once(sys: &dyn System, bundle: &AppBundle, own_build: u64) -> Action {
    let mut state = UpdateState::from_json(&sys.load_state());
    let on_disk = sys.bundle_build().unwrap_or(own_build);
    let auto = sys.auto_checks().unwrap_or(bundle.auto_checks_default);
    // A pending restart for a newer bundle on disk is handled without the network (and
    // even with checks off). After that restart failed, the appcast is read again so a
    // release newer than the bundle on disk is still offered.
    let restart_pending = on_disk > own_build && state.restarted_for_build != Some(on_disk);
    let latest = if auto && !restart_pending {
        sys.feed_override()
            .or_else(|| bundle.feed_url.clone())
            .and_then(|url| sys.fetch(&url))
            .map(|xml| parse_appcast(&xml))
    } else {
        None
    };
    let os = sys.os_version();
    let best = latest.as_deref().and_then(|items| best_item(items, &os));
    let now = sys.now();
    let action = decide(own_build, on_disk, best, &state, now);
    match &action {
        Action::Nothing => {}
        Action::Offer { .. } => {
            if sys.ui_running() {
                sys.notify_ui();
            } else {
                sys.open_ui_update();
            }
            record(&mut state, &action, now);
            // Worst case without the file: the offer repeats at the next check.
            if let Err(e) = sys.save_state(&state.to_json()) {
                log::warn!("update check: cannot save state: {e}");
            }
        }
        Action::RestartSelf { build } => {
            // The guard must be on disk before the restart kills this process, or a
            // restart that keeps running the old binary would loop.
            record(&mut state, &action, now);
            if let Err(e) = sys.save_state(&state.to_json()) {
                log::error!(
                    "update check: cannot save state ({e}); not restarting for build {build}"
                );
                return Action::Nothing;
            }
            sys.restart_self();
        }
    }
    action
}

/// `~/Library/Application Support/mbar/update-state.json`: loading and saving for the
/// real runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateFile {
    pub path: PathBuf,
}

// Used by the macOS runner only (and the tests).
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
impl StateFile {
    pub fn in_home(home: &str) -> StateFile {
        StateFile {
            path: Path::new(home).join("Library/Application Support/mbar/update-state.json"),
        }
    }

    pub fn load(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap_or_default()
    }

    /// Creates the directory if missing and replaces the file atomically (a torn write
    /// would read back as the default state and lose the restart guard).
    pub fn save(&self, json: &str) -> Result<(), String> {
        let shown = self.path.display();
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        // Same directory (so the rename is atomic); per process, so daemons of several
        // bars never write into each other's temp file.
        let tmp = self
            .path
            .with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, json).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot write {}: {e}", tmp.display())
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot replace {shown}: {e}")
        })
    }
}

/// Whether the next check is due, `last` and `now` in Unix seconds. A clock that went
/// backwards counts as due rather than postponing checks until it catches up.
fn check_due(last: u64, now: u64) -> bool {
    now < last || now - last >= CHECK_INTERVAL_SECS
}

/// The system runner for this platform, or `None` where there is none.
fn platform_system(bundle: &AppBundle, state: StateFile) -> Option<Box<dyn System + Send>> {
    #[cfg(target_os = "macos")]
    return Some(Box::new(real::MacSystem {
        bundle_root: bundle.root.clone(),
        state,
    }));
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (bundle, state);
        None
    }
}

/// `curl`/`defaults`/`open`/`launchctl` runner.
#[cfg(target_os = "macos")]
mod real {
    use super::{StateFile, System};
    use mbar_app::{BUNDLE_ID, UPDATE_NOTIFICATION};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    pub struct MacSystem {
        pub bundle_root: PathBuf,
        pub state: StateFile,
    }

    /// Trimmed stdout of a successful run, `None` otherwise.
    fn output(cmd: &str, args: &[&str]) -> Option<String> {
        let out = Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
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
            output(
                "/usr/bin/defaults",
                &["read", BUNDLE_ID, "SUEnableAutomaticChecks"],
            )
            .map(|s| matches!(s.as_str(), "1" | "true" | "YES"))
        }
        fn feed_override(&self) -> Option<String> {
            output("/usr/bin/defaults", &["read", BUNDLE_ID, "SUFeedURL"]).filter(|s| !s.is_empty())
        }
        fn ui_running(&self) -> bool {
            mbar_macos::sys::apps::is_app_running(BUNDLE_ID, "mbar-ui")
        }
        fn notify_ui(&self) {
            mbar_macos::sys::apps::post_distributed(UPDATE_NOTIFICATION);
        }
        fn open_ui_update(&self) {
            // By path and `-n`: the daemon itself is a running process of this bundle,
            // so `open -b` could just activate it instead of launching the UI.
            if let Err(e) = Command::new("/usr/bin/open")
                .arg("-n")
                .arg("-a")
                .arg(&self.bundle_root)
                .args(["--args", "--update"])
                .status()
            {
                log::warn!("update check: cannot open mbar.app: {e}");
            }
        }
        fn restart_self(&self) {
            let target = format!("gui/{}/{BUNDLE_ID}", unsafe { libc::getuid() });
            log::warn!("update check: newer mbar.app on disk, restarting ({target})");
            if let Err(e) = Command::new("/bin/launchctl")
                .args(["kickstart", "-k", &target])
                .status()
            {
                log::warn!("update check: cannot restart: {e}");
            }
        }
        fn now(&self) -> u64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        }
        fn load_state(&self) -> String {
            self.state.load()
        }
        fn save_state(&self, json: &str) -> Result<(), String> {
            self.state.save(json)
        }
        fn bundle_build(&self) -> Option<u64> {
            mbar_app::bundle::read_bundle(&self.bundle_root).map(|b| b.build)
        }
    }
}

/// Starts the check thread: first check after 120 s, then every 24 h. Only when the
/// daemon runs from a bundle with a feed URL and the platform has a runner. The thread
/// is detached and holds no locks between checks; the daemon's exit ends it.
pub fn spawn(bundle: AppBundle, home: &str) {
    if bundle.feed_url.is_none() || home.is_empty() {
        return;
    }
    let Some(own_build) = mbar_app::version::build_number(mbar_app::version::VERSION) else {
        return;
    };
    let Some(sys) = platform_system(&bundle, StateFile::in_home(home)) else {
        log::debug!("update check: no runner on this platform");
        return;
    };
    let started = std::thread::Builder::new()
        .name("mbar-update".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(FIRST_CHECK_SECS));
            loop {
                let action = check_once(&*sys, &bundle, own_build);
                log::info!("update check: {action:?}");
                let last = sys.now();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(WAKE_SECS));
                    if check_due(last, sys.now()) {
                        break;
                    }
                }
            }
        });
    if let Err(e) = started {
        log::warn!("update check: cannot start thread: {e}");
    }
}

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
        save_fails: bool,
        calls: RefCell<Vec<&'static str>>,
        fetched: RefCell<Vec<String>>,
        feed_override: Option<String>,
    }

    impl System for Fake {
        fn fetch(&self, url: &str) -> Option<String> {
            self.fetched.borrow_mut().push(url.into());
            self.feed.clone()
        }
        fn os_version(&self) -> String {
            "15.1".into()
        }
        fn auto_checks(&self) -> Option<bool> {
            self.auto
        }
        fn feed_override(&self) -> Option<String> {
            self.feed_override.clone()
        }
        fn ui_running(&self) -> bool {
            self.ui
        }
        fn notify_ui(&self) {
            self.calls.borrow_mut().push("notify")
        }
        fn open_ui_update(&self) {
            self.calls.borrow_mut().push("open")
        }
        fn restart_self(&self) {
            self.calls.borrow_mut().push("restart")
        }
        fn now(&self) -> u64 {
            1_000
        }
        fn load_state(&self) -> String {
            self.state.borrow().clone()
        }
        fn save_state(&self, json: &str) -> Result<(), String> {
            if self.save_fails {
                return Err("read-only".into());
            }
            *self.state.borrow_mut() = json.into();
            Ok(())
        }
        fn bundle_build(&self) -> Option<u64> {
            self.bundle_build
        }
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
        let f = Fake {
            feed: Some(feed(2000)),
            bundle_build: Some(1000),
            ..Default::default()
        };
        assert!(matches!(
            check_once(&f, &bundle(), 1000),
            Action::Offer { build: 2000, .. }
        ));
        assert_eq!(*f.calls.borrow(), ["open"]);
        assert!(f.state.borrow().contains("2000"));
    }

    /// Review Focus 3: `open --args` would be ignored by a running UI.
    #[test]
    fn offer_posts_notification_when_ui_running() {
        let f = Fake {
            feed: Some(feed(2000)),
            ui: true,
            bundle_build: Some(1000),
            ..Default::default()
        };
        check_once(&f, &bundle(), 1000);
        assert_eq!(*f.calls.borrow(), ["notify"]);
    }

    /// Review Focus 2: offline or 404 → no offer, no state change, retry next interval.
    #[test]
    fn fetch_failure_does_nothing() {
        let f = Fake {
            feed: None,
            bundle_build: Some(1000),
            ..Default::default()
        };
        assert_eq!(check_once(&f, &bundle(), 1000), Action::Nothing);
        assert!(f.calls.borrow().is_empty());
        assert!(f.state.borrow().is_empty());
        // The next check fetches again.
        check_once(&f, &bundle(), 1000);
        assert_eq!(f.fetched.borrow().len(), 2);
    }

    #[test]
    fn html_error_page_does_nothing() {
        let f = Fake {
            feed: Some("<html><body>Not Found</body></html>".into()),
            bundle_build: Some(1000),
            ..Default::default()
        };
        assert_eq!(check_once(&f, &bundle(), 1000), Action::Nothing);
        assert!(f.calls.borrow().is_empty());
    }

    #[test]
    fn disabled_checks_still_restart_after_update() {
        let f = Fake {
            feed: Some(feed(3000)),
            auto: Some(false),
            bundle_build: Some(2000),
            ..Default::default()
        };
        assert_eq!(
            check_once(&f, &bundle(), 1000),
            Action::RestartSelf { build: 2000 }
        );
        assert_eq!(*f.calls.borrow(), ["restart"]);
        assert!(
            f.fetched.borrow().is_empty(),
            "no network when checks are off"
        );
    }

    #[test]
    fn restart_only_once_per_build() {
        let f = Fake {
            bundle_build: Some(2000),
            ..Default::default()
        };
        assert_eq!(
            check_once(&f, &bundle(), 1000),
            Action::RestartSelf { build: 2000 }
        );
        // The restart failed (still the old binary): the saved guard stops a loop.
        assert_eq!(check_once(&f, &bundle(), 1000), Action::Nothing);
        assert_eq!(*f.calls.borrow(), ["restart"]);
    }

    /// Never restart without a persisted guard: a restart that keeps the old binary
    /// would otherwise restart again on every check.
    #[test]
    fn restart_skipped_when_state_cannot_be_saved() {
        let f = Fake {
            bundle_build: Some(2000),
            save_fails: true,
            ..Default::default()
        };
        assert_eq!(check_once(&f, &bundle(), 1000), Action::Nothing);
        assert!(f.calls.borrow().is_empty());
        assert!(f.state.borrow().is_empty());
    }

    #[test]
    fn offer_happens_even_when_state_cannot_be_saved() {
        let f = Fake {
            feed: Some(feed(2000)),
            bundle_build: Some(1000),
            save_fails: true,
            ..Default::default()
        };
        assert!(matches!(
            check_once(&f, &bundle(), 1000),
            Action::Offer { build: 2000, .. }
        ));
        assert_eq!(*f.calls.borrow(), ["open"]);
    }

    /// After a restart that kept the old binary, a release newer than the bundle on
    /// disk is still offered.
    #[test]
    fn newer_release_offered_after_failed_restart() {
        let f = Fake {
            feed: Some(feed(3000)),
            bundle_build: Some(2000),
            state: RefCell::new(r#"{"restarted_for_build":2000}"#.into()),
            ..Default::default()
        };
        assert!(matches!(
            check_once(&f, &bundle(), 1000),
            Action::Offer { build: 3000, .. }
        ));
        assert_eq!(*f.calls.borrow(), ["open"]);
        // What is already on disk is not offered.
        let f = Fake {
            feed: Some(feed(2000)),
            ..f
        };
        f.calls.borrow_mut().clear();
        *f.state.borrow_mut() = r#"{"restarted_for_build":2000}"#.into();
        assert_eq!(check_once(&f, &bundle(), 1000), Action::Nothing);
        assert!(f.calls.borrow().is_empty());
    }

    #[test]
    fn check_due_uses_wall_clock() {
        assert!(!check_due(1_000, 1_000));
        assert!(!check_due(1_000, 1_000 + CHECK_INTERVAL_SECS - 1));
        assert!(check_due(1_000, 1_000 + CHECK_INTERVAL_SECS));
        assert!(check_due(1_000, 500), "clock went backwards");
    }

    #[test]
    fn disabled_checks_never_fetch() {
        let f = Fake {
            feed: Some(feed(3000)),
            auto: Some(false),
            bundle_build: Some(1000),
            ..Default::default()
        };
        assert_eq!(check_once(&f, &bundle(), 1000), Action::Nothing);
        assert!(f.fetched.borrow().is_empty());
    }

    #[test]
    fn feed_override_wins() {
        let f = Fake {
            feed: Some(feed(1000)),
            bundle_build: Some(1000),
            feed_override: Some("http://127.0.0.1:8765/appcast.xml".into()),
            ..Default::default()
        };
        check_once(&f, &bundle(), 1000);
        assert_eq!(*f.fetched.borrow(), ["http://127.0.0.1:8765/appcast.xml"]);
    }

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mbar-updater-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn state_file_creates_its_directory() {
        let home = temp_home("mkdir");
        let file = StateFile::in_home(home.to_str().unwrap());
        assert_eq!(
            file.path,
            home.join("Library/Application Support/mbar/update-state.json")
        );
        assert_eq!(file.load(), "");
        file.save("{\"restarted_for_build\":2000}").unwrap();
        assert_eq!(file.load(), "{\"restarted_for_build\":2000}");
        file.save("{}").unwrap();
        assert_eq!(file.load(), "{}");
        // Only the state file is left behind (the temp file was renamed over it).
        let entries = std::fs::read_dir(file.path.parent().unwrap())
            .unwrap()
            .count();
        assert_eq!(entries, 1);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn state_file_reports_save_errors() {
        // `Library` is a regular file, so the state directory cannot be created (even
        // as root).
        let home = temp_home("blocked");
        std::fs::write(home.join("Library"), "").unwrap();
        let file = StateFile::in_home(home.to_str().unwrap());
        assert!(file.save("{}").is_err());
        assert_eq!(file.load(), "");
        let _ = std::fs::remove_dir_all(&home);
    }
}
