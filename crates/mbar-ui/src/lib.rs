//! `mbar_ui_model`: the GPUI-independent part of `mbar-ui` (data model, IPC client,
//! system helpers). It builds without the `gui` feature so it is unit-tested on any host:
//! `cargo test --no-default-features`.

pub mod ipc;
pub mod model;
pub mod system;
