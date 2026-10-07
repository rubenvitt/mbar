//! macOS glue for mbar.app: login item (SMAppService), activation policy, bundle
//! location, relaunch and distributed notifications.

pub mod app;
pub mod login_item;
pub mod sparkle;

/// The Sparkle updater of this process (`None` outside mbar.app or when Sparkle did not
/// start). Read by the System page.
pub struct UpdaterGlobal(pub std::rc::Rc<Option<sparkle::Updater>>);

impl gpui_kit::Global for UpdaterGlobal {}
