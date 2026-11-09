use std::{
    ffi::c_void,
    sync::{
        OnceLock,
        atomic::{AtomicIsize, Ordering},
    },
};

use windows_sys::Win32::Foundation::HWND;

/// COM GUID in the memory layout `CoCreateInstance` expects.
#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

// ShObjIdl_core.h: MIDL_INTERFACE("ea1afb91-9e28-4b86-90e9-9e9f8a5eefaf").
const IID_ITASKBAR_LIST3: Guid = Guid {
    data1: 0xea1a_fb91,
    data2: 0x9e28,
    data3: 0x4b86,
    data4: [0x90, 0xe9, 0x9e, 0x9f, 0x8a, 0x5e, 0xef, 0xaf],
};
// ShObjIdl_core.h: DECLSPEC_UUID("56FDF344-FD6D-11d0-958A-006097C9A090").
const CLSID_TASKBAR_LIST: Guid = Guid {
    data1: 0x56fd_f344,
    data2: 0xfd6d,
    data3: 0x11d0,
    data4: [0x95, 0x8a, 0x00, 0x60, 0x97, 0xc9, 0xa0, 0x90],
};

const CLSCTX_ALL: u32 = 23;
const TBPF_NOPROGRESS: u32 = 0;
const TBPF_NORMAL: u32 = 0x2;

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoCreateInstance(
        class: *const Guid,
        outer: *mut c_void,
        context: u32,
        interface: *const Guid,
        result: *mut *mut c_void,
    ) -> i32;
}

/// The first eleven slots of `ITaskbarList3Vtbl`, in the exact order declared
/// in the Windows SDK's ShObjIdl_core.h. Slots beyond `SetProgressState` are
/// never called, so they are omitted.
#[repr(C)]
struct TaskbarVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    hr_init: unsafe extern "system" fn(*mut c_void) -> i32,
    add_tab: unsafe extern "system" fn(*mut c_void, HWND) -> i32,
    delete_tab: unsafe extern "system" fn(*mut c_void, HWND) -> i32,
    activate_tab: unsafe extern "system" fn(*mut c_void, HWND) -> i32,
    set_active_alt: unsafe extern "system" fn(*mut c_void, HWND) -> i32,
    mark_fullscreen_window: unsafe extern "system" fn(*mut c_void, HWND, i32) -> i32,
    set_progress_value: unsafe extern "system" fn(*mut c_void, HWND, u64, u64) -> i32,
    set_progress_state: unsafe extern "system" fn(*mut c_void, HWND, u32) -> i32,
}

struct Taskbar(*mut c_void);

// The interface pointer is only used on the UI thread that created it; COM
// itself is apartment-aware and marshals as needed.
unsafe impl Send for Taskbar {}
unsafe impl Sync for Taskbar {}

impl Taskbar {
    fn vtbl(&self) -> &TaskbarVtbl {
        unsafe { &**(self.0 as *const *const TaskbarVtbl) }
    }

    fn acquire() -> Option<Self> {
        let mut instance: *mut c_void = std::ptr::null_mut();
        let status = unsafe {
            CoCreateInstance(
                &CLSID_TASKBAR_LIST,
                std::ptr::null_mut(),
                CLSCTX_ALL,
                &IID_ITASKBAR_LIST3,
                &mut instance,
            )
        };
        if status != 0 || instance.is_null() {
            tracing::debug!(status, "taskbar progress is unavailable");
            return None;
        }
        let taskbar = Self(instance);
        if unsafe { (taskbar.vtbl().hr_init)(instance) } != 0 {
            tracing::debug!("ITaskbarList::HrInit failed");
            return None;
        }
        Some(taskbar)
    }

    fn set(&self, hwnd: HWND, completed: u64, total: u64) {
        let vtbl = self.vtbl();
        unsafe {
            (vtbl.set_progress_state)(self.0, hwnd, TBPF_NORMAL);
            (vtbl.set_progress_value)(self.0, hwnd, completed, total);
        }
    }

    fn clear(&self, hwnd: HWND) {
        unsafe { (self.vtbl().set_progress_state)(self.0, hwnd, TBPF_NOPROGRESS) };
    }
}

impl Drop for Taskbar {
    fn drop(&mut self) {
        unsafe { (self.vtbl().release)(self.0) };
    }
}

static TASKBAR: OnceLock<Option<Taskbar>> = OnceLock::new();
static PROGRESS_WINDOW: AtomicIsize = AtomicIsize::new(0);

fn instance() -> Option<&'static Taskbar> {
    TASKBAR.get_or_init(Taskbar::acquire).as_ref()
}

/// Called once when an export starts, while the editor window is available.
pub fn remember_window(hwnd: HWND) {
    PROGRESS_WINDOW.store(hwnd as isize, Ordering::Release);
}

pub fn set_progress(completed: u64, total: u64) {
    let hwnd = PROGRESS_WINDOW.load(Ordering::Acquire) as HWND;
    if hwnd.is_null() {
        return;
    }
    if let Some(taskbar) = instance() {
        taskbar.set(hwnd, completed, total);
    }
}

pub fn clear() {
    let hwnd = PROGRESS_WINDOW.load(Ordering::Acquire) as HWND;
    if hwnd.is_null() {
        return;
    }
    if let Some(taskbar) = instance() {
        taskbar.clear(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_match_the_windows_sdk_declarations() {
        assert_eq!(IID_ITASKBAR_LIST3.data1, 0xea1a_fb91);
        assert_eq!(IID_ITASKBAR_LIST3.data2, 0x9e28);
        assert_eq!(IID_ITASKBAR_LIST3.data3, 0x4b86);
        assert_eq!(CLSID_TASKBAR_LIST.data1, 0x56fd_f344);
        assert_eq!(CLSID_TASKBAR_LIST.data2, 0xfd6d);
        assert_eq!(CLSID_TASKBAR_LIST.data3, 0x11d0);
    }

    #[test]
    fn the_vtbl_reaches_set_progress_value_at_slot_nine() {
        // All slots are pointer-sized function pointers; the SDK order is
        // IUnknown(3) + ITaskbarList(5) + ITaskbarList2(1) + our two methods.
        assert_eq!(
            std::mem::offset_of!(TaskbarVtbl, set_progress_value),
            9 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            std::mem::offset_of!(TaskbarVtbl, set_progress_state),
            10 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            std::mem::size_of::<TaskbarVtbl>(),
            11 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn progress_with_no_registered_window_is_a_no_op() {
        PROGRESS_WINDOW.store(0, Ordering::Release);
        set_progress(1, 10);
        clear();
    }
}
