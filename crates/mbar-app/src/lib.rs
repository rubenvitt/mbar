//! Platform-independent logic shared by the daemon (`mbar`) and the management app
//! (`mbar-ui`) for running from `mbar.app`: version numbers, `Info.plist` values,
//! appcast parsing, update decisions and config lookup. Everything here is pure and
//! unit-tested on any host; process spawning lives in the callers.

pub mod appcast;
pub mod bundle;
pub mod config;
pub mod update;
pub mod version;
