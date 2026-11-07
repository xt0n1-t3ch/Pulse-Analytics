//! Linux desktop checks that must pass before Pulse builds a tray or a WebView.
//!
//! The pure decisions take their inputs as arguments so tests can cover them
//! without a display, a driver or an installed library.

use std::ffi::OsStr;

/// Environment variable that turns off WebKitGTK's DMA-BUF renderer.
pub const WEBKIT_DMABUF_VARIABLE: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

/// Present only while an NVIDIA kernel driver is loaded.
pub const NVIDIA_DRIVER_VERSION_FILE: &str = "/proc/driver/nvidia/version";

/// Libraries `libappindicator-sys` 0.9 tries, in its order (src/lib.rs:7-18).
/// It panics when none of them loads, which killed Pulse on window close.
pub const APPINDICATOR_LIBRARIES: [&str; 2] =
    ["libayatana-appindicator3.so.1", "libappindicator3.so.1"];

/// True when at least one appindicator library can be loaded.
pub fn appindicator_present(can_load: impl Fn(&str) -> bool) -> bool {
    APPINDICATOR_LIBRARIES.iter().any(|name| can_load(name))
}

/// WebKitGTK shows a blank window on some NVIDIA setups while its DMA-BUF
/// renderer is on. Respect a value the user already chose, including "0".
pub fn dmabuf_workaround_needed(current: Option<&OsStr>, nvidia_driver_present: bool) -> bool {
    current.is_none() && nvidia_driver_present
}

/// WebKitGTK flag for CSS `corner-shape`, which draws the UI's squircle corners.
/// WebView2 ships the property; WebKitGTK 2.54 has it behind this flag.
pub const CORNER_SHAPE_FEATURE: &str = "CSSCornerShapeEnabled";

/// Turns on CSS `corner-shape` for this WebView. Returns true when the flag
/// changed, so the caller reloads the page to apply it to the stylesheet.
#[cfg(target_os = "linux")]
pub fn enable_corner_shape(webview: &webkit2gtk::WebView) -> bool {
    use std::ffi::{CStr, c_char, c_void};
    use webkit2gtk::WebViewExt;
    use webkit2gtk::glib::translate::ToGlibPtr;

    // Stable WebKitGTK 2.42+ feature API (webkit/WebKitFeature.h, WebKitSettings.h).
    unsafe extern "C" {
        fn webkit_settings_get_all_features() -> *mut c_void;
        fn webkit_feature_list_get_length(list: *mut c_void) -> usize;
        fn webkit_feature_list_get(list: *mut c_void, index: usize) -> *mut c_void;
        fn webkit_feature_list_unref(list: *mut c_void);
        fn webkit_feature_get_identifier(feature: *mut c_void) -> *const c_char;
        fn webkit_settings_get_feature_enabled(settings: *mut c_void, feature: *mut c_void) -> i32;
        fn webkit_settings_set_feature_enabled(
            settings: *mut c_void,
            feature: *mut c_void,
            enabled: i32,
        );
    }

    let Some(settings) = webview.settings() else {
        return false;
    };
    let settings_ptr: *mut webkit2gtk::ffi::WebKitSettings = settings.to_glib_none().0;
    let settings_ptr = settings_ptr.cast::<c_void>();
    let mut changed = false;
    // SAFETY: the list is owned here and released once; every feature pointer
    // comes from that list and is used before the list is released. The
    // identifier is a NUL-terminated string owned by the feature.
    unsafe {
        let list = webkit_settings_get_all_features();
        if list.is_null() {
            return false;
        }
        for index in 0..webkit_feature_list_get_length(list) {
            let feature = webkit_feature_list_get(list, index);
            let identifier = webkit_feature_get_identifier(feature);
            if identifier.is_null()
                || CStr::from_ptr(identifier).to_bytes() != CORNER_SHAPE_FEATURE.as_bytes()
            {
                continue;
            }
            if webkit_settings_get_feature_enabled(settings_ptr, feature) == 0 {
                webkit_settings_set_feature_enabled(settings_ptr, feature, 1);
                changed = true;
            }
            break;
        }
        webkit_feature_list_unref(list);
    }
    changed
}

pub fn nvidia_driver_present() -> bool {
    std::path::Path::new(NVIDIA_DRIVER_VERSION_FILE).exists()
}

/// Whether a system tray can be built. Always true off Linux; on Linux it
/// needs an appindicator library. Probed once and logged once.
pub fn tray_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::sync::OnceLock;
        static SUPPORTED: OnceLock<bool> = OnceLock::new();
        *SUPPORTED.get_or_init(|| {
            let supported = appindicator_present(can_load_library);
            if !supported {
                tracing::warn!(
                    "no appindicator library found (libayatana-appindicator3.so.1 or \
                     libappindicator3.so.1); the tray is disabled and closing the window quits Pulse"
                );
            }
            supported
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

#[cfg(target_os = "linux")]
fn can_load_library(name: &str) -> bool {
    let Ok(name) = std::ffi::CString::new(name) else {
        return false;
    };
    // SAFETY: `name` is a NUL-terminated string that outlives the call and
    // RTLD_LAZY is a valid flag. A non-null handle comes from this `dlopen`
    // and is closed exactly once.
    unsafe {
        let handle = libc::dlopen(name.as_ptr(), libc::RTLD_LAZY);
        if handle.is_null() {
            return false;
        }
        libc::dlclose(handle);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn appindicator_is_present_when_only_the_second_library_loads() {
        assert!(appindicator_present(|name| name == "libappindicator3.so.1"));
    }

    #[test]
    fn appindicator_is_missing_when_no_library_loads() {
        assert!(!appindicator_present(|_| false));
    }

    #[test]
    fn appindicator_probe_asks_for_both_versioned_names_in_order() {
        let asked = RefCell::new(Vec::new());
        assert!(!appindicator_present(|name| {
            asked.borrow_mut().push(name.to_string());
            false
        }));
        assert_eq!(
            asked.into_inner(),
            ["libayatana-appindicator3.so.1", "libappindicator3.so.1"]
        );
    }

    #[test]
    fn appindicator_probe_stops_at_the_first_library_that_loads() {
        let asked = RefCell::new(0);
        assert!(appindicator_present(|_| {
            *asked.borrow_mut() += 1;
            true
        }));
        assert_eq!(asked.into_inner(), 1);
    }

    #[test]
    fn dmabuf_workaround_applies_on_nvidia_when_the_variable_is_unset() {
        assert!(dmabuf_workaround_needed(None, true));
    }

    #[test]
    fn dmabuf_workaround_keeps_a_user_value() {
        assert!(!dmabuf_workaround_needed(Some(OsStr::new("0")), true));
        assert!(!dmabuf_workaround_needed(Some(OsStr::new("1")), true));
    }

    #[test]
    fn dmabuf_workaround_is_skipped_without_an_nvidia_driver() {
        assert!(!dmabuf_workaround_needed(None, false));
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn dlopen_probe_finds_glibc_and_rejects_a_missing_library() {
        assert!(can_load_library("libc.so.6"));
        assert!(!can_load_library("libpulse-test-missing.so.9"));
    }
}
