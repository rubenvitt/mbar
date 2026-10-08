//! `SMAppService.agent(plistName: "dev.rubeen.mbar.plist")` (ServiceManagement, macOS 13).

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
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
    let Some(s) = service() else {
        return LoginItem::NotFound;
    };
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
        Err(unsafe { err.as_ref() }
            .map(|e| e.localizedDescription().to_string())
            .unwrap_or_default())
    }
}

pub fn register() -> Result<(), String> {
    call(true)
}
pub fn unregister() -> Result<(), String> {
    call(false)
}

pub fn open_settings() {
    if let Some(cls) = AnyClass::get(c"SMAppService") {
        let _: () = unsafe { msg_send![cls, openSystemSettingsLoginItems] };
    }
}
