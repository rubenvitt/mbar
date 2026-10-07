//! Integration layer: adapts the gfx and sys building blocks to `mbar-core`.
//!
//! * [`convert`]: pure mappings (scene → draw list, system event → input, mouse, text
//!   metrics), unit tested.
//! * [`resources`]: [`MacResources`], the `mbar_core::platform::Resources` implementation.
//! * [`windows`]: [`WindowManager`], `FrameOutput` → bar/popup windows.
//! * [`services`]: [`Services`], the running system sources, stateful event translation
//!   and `PlatformRequest` execution.
//! * [`runloop`]: accessory `NSApplication`, main-queue waker, deadline timer, frame pacer.
//!
//! The binary (`crates/mbar/src/platform/macos.rs`) wires these around its `Driver`.

pub mod convert;
pub mod resources;
pub mod runloop;
pub mod services;
pub mod windows;

pub use convert::{scene_to_drawlist, sys_event_to_input, SceneLookup};
pub use resources::{MacResources, TextCache};
pub use runloop::{App, DeadlineTimer, FramePacer, Waker};
pub use services::{Services, Translated};
pub use windows::{ViewMouseSink, WindowManager};

/// Re-exported so the binary needs no direct objc2 dependency.
pub use objc2::MainThreadMarker;
