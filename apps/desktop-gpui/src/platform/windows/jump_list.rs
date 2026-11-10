use std::{ffi::c_void, sync::OnceLock};

/// Stable AppUserModelID. Windows groups taskbar buttons, notifications and
/// the jump list by this id, so it must not change between releases.
pub const APP_USER_MODEL_ID: &str = "com.capwise.app";

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

// ShObjIdl_core.h: MIDL_INTERFACE("6332debf-87b5-4670-90c0-5e57b408a49e").
const IID_ICUSTOM_DESTINATION_LIST: Guid = Guid {
    data1: 0x6332_debf,
    data2: 0x87b5,
    data3: 0x4670,
    data4: [0x90, 0xc0, 0x5e, 0x57, 0xb4, 0x08, 0xa4, 0x9e],
};
// ShObjIdl_core.h: DECLSPEC_UUID("77f10cf0-3db5-4966-b520-b7c54fd35ed6").
const CLSID_DESTINATION_LIST: Guid = Guid {
    data1: 0x77f1_0cf0,
    data2: 0x3db5,
    data3: 0x4966,
    data4: [0xb5, 0x20, 0xb7, 0xc5, 0x4f, 0xd3, 0x5e, 0xd6],
};
const IID_NULL: Guid = Guid {
    data1: 0,
    data2: 0,
    data3: 0,
    data4: [0; 8],
};

const CLSCTX_ALL: u32 = 23;
const KDC_FREQUENT: u32 = 1;
const KDC_RECENT: u32 = 2;

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

#[link(name = "shell32")]
unsafe extern "system" {
    fn SetCurrentProcessExplicitAppUserModelID(app_id: *const u16) -> i32;
}

/// The first nine slots of `ICustomDestinationListVtbl`, in the exact order
/// in ShObjIdl_core.h. Later slots are never called.
#[repr(C)]
struct DestinationListVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    set_app_id: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    begin_list:
        unsafe extern "system" fn(*mut c_void, *mut u32, *const Guid, *mut *mut c_void) -> i32,
    append_category: unsafe extern "system" fn(*mut c_void, *const u16, *mut c_void) -> i32,
    append_known_category: unsafe extern "system" fn(*mut c_void, u32) -> i32,
    add_user_tasks: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    commit_list: unsafe extern "system" fn(*mut c_void) -> i32,
}

struct DestinationList(*mut c_void);

unsafe impl Send for DestinationList {}
unsafe impl Sync for DestinationList {}

impl DestinationList {
    fn vtbl(&self) -> &DestinationListVtbl {
        unsafe { &**(self.0 as *const *const DestinationListVtbl) }
    }

    fn acquire() -> Option<Self> {
        let mut instance: *mut c_void = std::ptr::null_mut();
        let status = unsafe {
            CoCreateInstance(
                &CLSID_DESTINATION_LIST,
                std::ptr::null_mut(),
                CLSCTX_ALL,
                &IID_ICUSTOM_DESTINATION_LIST,
                &mut instance,
            )
        };
        if status != 0 || instance.is_null() {
            tracing::debug!(status, "jump list is unavailable");
            return None;
        }
        Some(Self(instance))
    }

    fn publish(&self, app_id: &[u16]) {
        let vtbl = self.vtbl();
        let mut min_slots = 0u32;
        let mut removed: *mut c_void = std::ptr::null_mut();
        unsafe {
            if (vtbl.set_app_id)(self.0, app_id.as_ptr()) != 0 {
                return;
            }
            if (vtbl.begin_list)(self.0, &mut min_slots, &IID_NULL, &mut removed) != 0 {
                return;
            }
            // Windows owns these two categories and fills them from the .cap
            // file association; no IShellLink/IObjectArray plumbing is needed.
            let _ = (vtbl.append_known_category)(self.0, KDC_FREQUENT);
            let _ = (vtbl.append_known_category)(self.0, KDC_RECENT);
            let _ = (vtbl.commit_list)(self.0);
        }
    }
}

impl Drop for DestinationList {
    fn drop(&mut self) {
        unsafe { (self.vtbl().release)(self.0) };
    }
}

static DESTINATION_LIST: OnceLock<Option<DestinationList>> = OnceLock::new();

fn instance() -> Option<&'static DestinationList> {
    DESTINATION_LIST
        .get_or_init(DestinationList::acquire)
        .as_ref()
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Windows rejects a late AppUserModelID assignment once a window has been
/// associated with the taskbar, so this runs at the very top of `main`.
pub fn register_app_id() {
    let app_id = wide(APP_USER_MODEL_ID);
    unsafe {
        if SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr()) != 0 {
            tracing::debug!("could not set the explicit AppUserModelID");
        }
    }
}

/// Opens the jump list's recent/frequent categories. Every failure is a silent
/// no-op: an app without an installed shortcut or with shell integration
/// disabled simply has no jump list.
pub fn install() {
    let app_id = wide(APP_USER_MODEL_ID);
    if let Some(list) = instance() {
        list.publish(&app_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_match_the_windows_sdk_declarations() {
        assert_eq!(IID_ICUSTOM_DESTINATION_LIST.data1, 0x6332_debf);
        assert_eq!(IID_ICUSTOM_DESTINATION_LIST.data2, 0x87b5);
        assert_eq!(IID_ICUSTOM_DESTINATION_LIST.data3, 0x4670);
        assert_eq!(CLSID_DESTINATION_LIST.data1, 0x77f1_0cf0);
        assert_eq!(CLSID_DESTINATION_LIST.data2, 0x3db5);
        assert_eq!(CLSID_DESTINATION_LIST.data3, 0x4966);
        assert_eq!(IID_NULL.data1, 0);
    }

    #[test]
    fn the_vtbl_reaches_append_known_category_at_slot_six() {
        assert_eq!(
            std::mem::offset_of!(DestinationListVtbl, append_known_category),
            6 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            std::mem::offset_of!(DestinationListVtbl, commit_list),
            8 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            std::mem::size_of::<DestinationListVtbl>(),
            9 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn the_app_id_is_stable_null_terminated_and_non_empty() {
        assert_eq!(APP_USER_MODEL_ID, "com.capwise.app");
        let wide_id = wide(APP_USER_MODEL_ID);
        assert_eq!(wide_id.last(), Some(&0));
        assert_eq!(wide_id.len(), APP_USER_MODEL_ID.len() + 1);
    }

    #[test]
    fn known_categories_match_the_sdk_enum() {
        assert_eq!(KDC_FREQUENT, 1);
        assert_eq!(KDC_RECENT, 2);
    }
}
