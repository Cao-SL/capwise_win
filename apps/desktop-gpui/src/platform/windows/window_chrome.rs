use std::ffi::c_void;

use windows_sys::Win32::Foundation::HWND;

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: HWND, attribute: u32, value: *const c_void, size: u32) -> i32;
}

/// DWMWA_WINDOW_CORNER_PREFERENCE, Windows 11 only. Ignored on Windows 10.
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
/// DWMWA_SYSTEMBACKDROP_TYPE, Windows 11 22H2 only. Ignored on older builds.
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;

const DWMWCP_DEFAULT: u32 = 0;
const DWMWCP_ROUND: u32 = 2;

const DWMSBT_MAINWINDOW: u32 = 2;
const DWMSBT_TRANSIENTWINDOW: u32 = 3;

/// The shell paints a 16px rounded quad, but on Windows the OS window itself
/// stays square, so the four corners show an opaque block outside the quad.
/// Asking DWM for round corners makes the frame match the painted shell.
fn corner_preference(round: bool) -> u32 {
    if round { DWMWCP_ROUND } else { DWMWCP_DEFAULT }
}

/// Mica for the main window, Acrylic for transient panels. Both are a no-op
/// on Windows 10, where `DwmSetWindowAttribute` rejects the attribute.
fn backdrop_type(transient: bool) -> u32 {
    if transient {
        DWMSBT_TRANSIENTWINDOW
    } else {
        DWMSBT_MAINWINDOW
    }
}

fn set_attribute(hwnd: HWND, attribute: u32, value: u32, label: &'static str) {
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            std::ptr::addr_of!(value).cast(),
            std::mem::size_of_val(&value) as u32,
        )
    };
    if result < 0 {
        tracing::debug!(
            result,
            attribute,
            "DWM window chrome is not supported ({label})"
        );
    }
}

/// Install the Windows 11 look on one window. Safe to call on Windows 10:
/// unsupported attributes are rejected by DWM and left at their defaults.
pub fn install(hwnd: HWND, transient: bool) {
    set_attribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        corner_preference(true),
        "corner preference",
    );
    set_attribute(
        hwnd,
        DWMWA_SYSTEMBACKDROP_TYPE,
        backdrop_type(transient),
        "system backdrop",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_asks_for_round_corners_and_a_mica_or_acrylic_backdrop() {
        assert_eq!(corner_preference(true), DWMWCP_ROUND);
        assert_eq!(corner_preference(false), DWMWCP_DEFAULT);
        assert_eq!(backdrop_type(false), DWMSBT_MAINWINDOW);
        assert_eq!(backdrop_type(true), DWMSBT_TRANSIENTWINDOW);
        assert_ne!(backdrop_type(false), backdrop_type(true));
        assert_ne!(DWMSBT_MAINWINDOW, DWMSBT_TRANSIENTWINDOW);
    }

    #[test]
    fn dwm_attribute_ids_and_values_match_the_sdk() {
        assert_eq!(DWMWA_WINDOW_CORNER_PREFERENCE, 33);
        assert_eq!(DWMWA_SYSTEMBACKDROP_TYPE, 38);
        assert_eq!(DWMWCP_ROUND, 2);
        assert_eq!(DWMSBT_MAINWINDOW, 2);
        assert_eq!(DWMSBT_TRANSIENTWINDOW, 3);
    }
}
