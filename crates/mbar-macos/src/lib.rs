//! macOS platform layer for mbar. See `docs/ARCHITECTURE.md` and `docs/MACOS.md`.
//!
//! The building blocks below expose crate-local APIs. `platform` (written last)
//! adapts them to `mbar-core`'s `Resources`/`Input`/`Effect`/`FrameOutput`.
#![cfg(target_os = "macos")]

// Graphics (owned by the graphics work package)
pub mod gfx;

// System integration (owned by the system work package)
pub mod sys;

// Integration with mbar-core (Resources, scene mapping, windows, services, run loop)
pub mod platform;
