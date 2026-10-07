//! Platform selection.
//!
//! A platform runs the daemon's main loop around a [`crate::driver::Driver`]:
//! it provides `Resources`, posts events from background threads, calls
//! `Driver::handle_event` / `Driver::poll`, presents frames and executes
//! `PlatformRequest`s.
//!
//! * [`headless::HeadlessPlatform`]: no windows, `HeadlessResources` (monospace metrics,
//!   one 1920×1080 display). The only platform on non-macOS, and `--headless` on macOS.
//! * [`macos::MacPlatform`] (`cfg(target_os = "macos")`): `NSApplication` run loop, Metal
//!   windows, native system services and the mach server; the default on macOS unless
//!   `--headless`.

pub mod headless;
#[cfg(target_os = "macos")]
pub mod macos;

use crate::daemon::DaemonSetup;

/// A daemon main loop. Returns the process exit code.
pub trait Platform {
    fn run(self, setup: DaemonSetup) -> i32;
}

/// Runs the daemon on the platform selected by the options and exits the process.
pub fn platform_main(setup: DaemonSetup) -> ! {
    let socket = setup.socket_path.clone();

    #[cfg(target_os = "macos")]
    let code = if setup.headless {
        headless::HeadlessPlatform.run(setup)
    } else {
        macos::MacPlatform.run(setup)
    };
    #[cfg(not(target_os = "macos"))]
    let code = headless::HeadlessPlatform.run(setup);

    let _ = std::fs::remove_file(&socket);
    std::process::exit(code)
}
