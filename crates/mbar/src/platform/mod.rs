//! Platform selection.
//!
//! A platform runs the daemon's main loop around a [`crate::driver::Driver`]:
//! it provides `Resources`, posts events from background threads, calls
//! `Driver::handle_event` / `Driver::poll`, presents frames and executes
//! `PlatformRequest`s.
//!
//! * [`headless::HeadlessPlatform`]: no windows, `HeadlessResources` (monospace metrics,
//!   one 1920×1080 display). The only platform on non-macOS, and `--headless` on macOS.
//! * macOS (`macos.rs`, `cfg(target_os = "macos")`): integrated separately; see the TODO
//!   in [`platform_main`].

pub mod headless;

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
        // TODO(macos): `mod macos;` with `macos::MacPlatform` (NSApplication run loop,
        // mbar-macos Resources, Metal windows from `FrameOutput`, mach server feeding
        // `Event::Request` with `Responder::Callback`, `PlatformRequest` execution).
        // Until it lands the daemon runs headless on macOS too.
        log::warn!("macOS platform not integrated yet; running headless");
        headless::HeadlessPlatform.run(setup)
    };
    #[cfg(not(target_os = "macos"))]
    let code = headless::HeadlessPlatform.run(setup);

    let _ = std::fs::remove_file(&socket);
    std::process::exit(code)
}
