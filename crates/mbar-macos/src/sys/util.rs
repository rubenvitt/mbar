//! Small shared helpers: CoreFoundation conversions, OS version, lazily resolved private
//! symbols (SkyLight, DisplayServices, MediaRemote) and main-queue dispatch.

use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, ConcreteType, Type,
};
use objc2_foundation::NSProcessInfo;
use std::ffi::{c_void, CString};
use std::ptr::NonNull;
use std::sync::OnceLock;

/// `CFString` from a Rust string.
pub(crate) fn cfstr(s: &str) -> CFRetained<CFString> {
    CFString::from_str(s)
}

/// Wraps a pointer returned by a CF "Create"/"Copy" function (+1 retain count).
///
/// # Safety
/// `ptr` must be null or a valid CF object of type `T` that the caller owns.
pub(crate) unsafe fn owned<T: Type>(ptr: *const T) -> Option<CFRetained<T>> {
    // SAFETY: the caller guarantees ownership of a +1 reference of the right type.
    NonNull::new(ptr as *mut T).map(|p| unsafe { CFRetained::from_raw(p) })
}

/// Retains a pointer returned by a CF "Get" function (+0 retain count).
///
/// # Safety
/// `ptr` must be null or a valid CF object of type `T`.
pub(crate) unsafe fn borrowed<T: Type>(ptr: *const T) -> Option<CFRetained<T>> {
    // SAFETY: the caller guarantees a valid object; retaining it keeps it alive.
    NonNull::new(ptr as *mut T).map(|p| unsafe { CFRetained::retain(p) })
}

/// Value of `key` in a CF dictionary with string keys.
pub(crate) fn dict_get(dict: &CFDictionary, key: &str) -> Option<CFRetained<CFType>> {
    // SAFETY: every dictionary we read (window info, power source descriptions, SkyLight
    // display/space dictionaries) has CFString keys and CF object values; `get` only uses
    // CFEqual/CFHash on the key, which is valid for any CF type.
    let typed: &CFDictionary<CFString, CFType> = unsafe { dict.cast_unchecked() };
    typed.get(&cfstr(key))
}

/// Downcasts a CF object to a concrete type.
pub(crate) fn downcast<T: ConcreteType>(v: CFRetained<CFType>) -> Option<CFRetained<T>> {
    v.downcast::<T>().ok()
}

pub(crate) fn cf_string(v: &CFType) -> Option<String> {
    v.downcast_ref::<CFString>().map(|s| s.to_string())
}

pub(crate) fn cf_i64(v: &CFType) -> Option<i64> {
    if let Some(n) = v.downcast_ref::<CFNumber>() {
        return n.as_i64().or_else(|| n.as_f64().map(|f| f as i64));
    }
    v.downcast_ref::<CFBoolean>().map(|b| b.as_bool() as i64)
}

pub(crate) fn cf_f64(v: &CFType) -> Option<f64> {
    v.downcast_ref::<CFNumber>().and_then(|n| n.as_f64())
}

pub(crate) fn cf_bool(v: &CFType) -> Option<bool> {
    if let Some(b) = v.downcast_ref::<CFBoolean>() {
        return Some(b.as_bool());
    }
    cf_i64(v).map(|i| i != 0)
}

pub(crate) fn dict_string(dict: &CFDictionary, key: &str) -> Option<String> {
    dict_get(dict, key).and_then(|v| cf_string(&v))
}

pub(crate) fn dict_i64(dict: &CFDictionary, key: &str) -> Option<i64> {
    dict_get(dict, key).and_then(|v| cf_i64(&v))
}

pub(crate) fn dict_bool(dict: &CFDictionary, key: &str) -> Option<bool> {
    dict_get(dict, key).and_then(|v| cf_bool(&v))
}

pub(crate) fn dict_dict(dict: &CFDictionary, key: &str) -> Option<CFRetained<CFDictionary>> {
    dict_get(dict, key).and_then(downcast::<CFDictionary>)
}

pub(crate) fn dict_array(dict: &CFDictionary, key: &str) -> Option<CFRetained<CFArray>> {
    dict_get(dict, key).and_then(downcast::<CFArray>)
}

/// Elements of an untyped CF array (each retained).
pub(crate) fn array_items(array: &CFArray) -> Vec<CFRetained<CFType>> {
    // SAFETY: CF arrays returned by the system APIs we use hold CF objects only.
    let typed: &CFArray<CFType> = unsafe { array.cast_unchecked() };
    typed.to_vec()
}

/// `(major, minor, patch)` of the running macOS.
pub fn os_version() -> (i64, i64, i64) {
    static V: OnceLock<(i64, i64, i64)> = OnceLock::new();
    *V.get_or_init(|| {
        let v = NSProcessInfo::processInfo().operatingSystemVersion();
        (v.majorVersion as i64, v.minorVersion as i64, v.patchVersion as i64)
    })
}

/// `os_version() >= (major, minor)`.
pub fn os_at_least(major: i64, minor: i64) -> bool {
    let (ma, mi, _) = os_version();
    (ma, mi) >= (major, minor)
}

/// Runs `f` asynchronously on the main GCD queue.
pub(crate) fn on_main<F: FnOnce() + Send + 'static>(f: F) {
    dispatch2::DispatchQueue::main().exec_async(f);
}

/// A loaded dynamic library (never unloaded).
pub(crate) struct Library(*mut c_void);
// SAFETY: a dlopen handle is a process-global token, usable from any thread.
unsafe impl Send for Library {}
// SAFETY: see above; dlsym is thread safe.
unsafe impl Sync for Library {}

impl Library {
    pub(crate) fn open(path: &str) -> Option<Library> {
        let c = CString::new(path).ok()?;
        // SAFETY: valid NUL-terminated path; RTLD_LAZY|RTLD_LOCAL is a valid flag set.
        let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if h.is_null() {
            log::debug!("dlopen({path}) failed");
            None
        } else {
            Some(Library(h))
        }
    }

    pub(crate) fn symbol(&self, name: &str) -> Option<NonNull<c_void>> {
        let c = CString::new(name).ok()?;
        // SAFETY: `self.0` is a live handle and `c` a valid C string.
        NonNull::new(unsafe { libc::dlsym(self.0, c.as_ptr()) })
    }
}

pub(crate) const SKYLIGHT: &str = "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
pub(crate) const DISPLAY_SERVICES: &str =
    "/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices";
pub(crate) const MEDIA_REMOTE: &str =
    "/System/Library/PrivateFrameworks/MediaRemote.framework/MediaRemote";
pub(crate) const COLOR_SYNC: &str = "/System/Library/Frameworks/ColorSync.framework/ColorSync";

/// Cached `dlopen` of a framework binary.
pub(crate) fn library(path: &'static str) -> Option<&'static Library> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static LIBS: OnceLock<Mutex<HashMap<&'static str, Option<&'static Library>>>> = OnceLock::new();
    let mut map = LIBS.get_or_init(Default::default).lock().ok()?;
    *map.entry(path)
        .or_insert_with(|| Library::open(path).map(|l| &*Box::leak(Box::new(l))))
}

/// Declares lazily resolved private functions. Each becomes
/// `pub(crate) fn name() -> Option<unsafe extern "C" fn(..) -> R>`; `None` when the symbol
/// does not exist on this macOS version (callers degrade gracefully).
macro_rules! private_fns {
    ($lib:expr => $( fn $name:ident ( $($arg:ty),* $(,)? ) -> $ret:ty ; )* ) => {
        $(
            #[allow(non_snake_case, dead_code)]
            pub(crate) fn $name() -> Option<unsafe extern "C" fn($($arg),*) -> $ret> {
                static SYM: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
                let addr = (*SYM.get_or_init(|| {
                    $crate::sys::util::library($lib)
                        .and_then(|l| l.symbol(stringify!($name)))
                        .map(|p| p.as_ptr() as usize)
                }))?;
                // SAFETY: the symbol was resolved from the framework that exports it and the
                // declared signature matches the one used by SketchyBar/yabai for it.
                Some(unsafe {
                    std::mem::transmute::<usize, unsafe extern "C" fn($($arg),*) -> $ret>(addr)
                })
            }
        )*
    };
}
pub(crate) use private_fns;
