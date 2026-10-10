//! Platform-independent core of mbar. See `docs/ARCHITECTURE.md`, `docs/DESIGN-CORE.md`
//! and `docs/IMPLEMENTATION-PLAN.md` (work packages and ownership of every module).

#![allow(clippy::write_with_newline)]

pub mod aerospace;
pub mod animation;
pub mod bar;
pub mod borders;
pub mod color;
pub mod command;
pub mod components;
pub mod event;
pub mod geometry;
pub mod group;
pub mod item;
pub mod layout;
pub mod model;
pub mod platform;
pub mod popup;
pub mod privacy;
pub mod props;
pub mod provider;
pub mod query;
pub mod runtime;
pub mod scene;
pub mod script;
pub mod value;

pub use model::Model;
pub use runtime::{Runtime, RuntimeConfig};
