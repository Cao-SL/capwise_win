use std::io;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::WM_NCDESTROY;

/// Different from `session_end`'s 0x4341_5053 ("CAPS"); this one is "CAPP".
const SUBCLASS_ID: usize = 0x4341_5050;

const WM_POWERBROADCAST: u32 = 0x0218;
const PBT_APMSUSPEND: WPARAM = 0x0004;
const PBT_APMRESUMESUSPEND: WPARAM = 0x0007;
const PBT_APMRESUMEAUTOMATIC: WPARAM = 0x0012;

/// `true` when the machine is about to sleep, `false` after it wakes.
/// A plain fn pointer, like `session_end`'s, so the subclass owns no gpui state.
pub fn install(hwnd: HWND, on_power_event: fn(bool)) -> io::Result<()> {
    let callback = Box::into_raw(Box::new(on_power_event));
    if unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, callback as usize) } == 0 {
        drop(unsafe { Box::from_raw(callback) });
        return Err(io::Error::other(
            "could not install the power event subclass",
        ));
    }
    Ok(())
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    let callback = reference_data as *mut fn(bool);
    if message == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID) };
        drop(unsafe { Box::from_raw(callback) });
    } else if message == WM_POWERBROADCAST {
        match wparam {
            PBT_APMSUSPEND => unsafe { (*callback)(true) },
            PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC => unsafe { (*callback)(false) },
            _ => {}
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE, SendMessageW,
    };

    use super::*;

    static SUSPENDS: AtomicUsize = AtomicUsize::new(0);
    static RESUMES: AtomicUsize = AtomicUsize::new(0);

    fn record(suspending: bool) {
        if suspending {
            SUSPENDS.fetch_add(1, Ordering::SeqCst);
        } else {
            RESUMES.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn only_suspend_and_resume_broadcasts_reach_the_callback() {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null());
        install(hwnd, record).unwrap();
        SUSPENDS.store(0, Ordering::SeqCst);
        RESUMES.store(0, Ordering::SeqCst);

        unsafe {
            SendMessageW(hwnd, WM_POWERBROADCAST, 0x0005, 0);
            SendMessageW(hwnd, WM_POWERBROADCAST, PBT_APMSUSPEND, 0);
            SendMessageW(hwnd, WM_POWERBROADCAST, PBT_APMRESUMESUSPEND, 0);
            SendMessageW(hwnd, WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC, 0);
        }
        assert_eq!(SUSPENDS.load(Ordering::SeqCst), 1);
        assert_eq!(RESUMES.load(Ordering::SeqCst), 2);

        assert_ne!(unsafe { DestroyWindow(hwnd) }, 0);
    }
}
