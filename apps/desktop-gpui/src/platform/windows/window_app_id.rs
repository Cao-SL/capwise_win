use std::ffi::c_void;

use windows_sys::{
    Win32::{
        Foundation::HWND,
        System::Com::{
            CoTaskMemAlloc,
            StructuredStorage::{PROPVARIANT, PropVariantClear},
        },
        UI::Shell::PropertiesSystem::{PROPERTYKEY, SHGetPropertyStoreForWindow},
    },
    core::GUID,
};

use super::jump_list::APP_USER_MODEL_ID;

// propsys.h: MIDL_INTERFACE("886d8eeb-8cf2-4446-8d02-cdba1dbdcf99").
const IID_IPROPERTY_STORE: GUID = GUID {
    data1: 0x886d_8eeb,
    data2: 0x8cf2,
    data3: 0x4446,
    data4: [0x8d, 0x02, 0xcd, 0xba, 0x1d, 0xbd, 0xcf, 0x99],
};

// propkey.h: System.AppUserModel.ID, VT_LPWSTR, pid 5.
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID {
        data1: 0x9f4c_2855,
        data2: 0x9f79,
        data3: 0x4b39,
        data4: [0xa8, 0xd0, 0xe1, 0xd4, 0x2d, 0xe1, 0xd5, 0xf3],
    },
    pid: 5,
};

const VT_LPWSTR: u16 = 31;

/// `IPropertyStoreVtbl` from propsys.h. Only Release, SetValue and Commit are
/// called; the remaining slots are declared so the offsets stay exact.
#[repr(C)]
struct PropertyStoreVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
    get_at: unsafe extern "system" fn(*mut c_void, u32, *mut PROPERTYKEY) -> i32,
    get_value: unsafe extern "system" fn(*mut c_void, *const PROPERTYKEY, *mut PROPVARIANT) -> i32,
    set_value:
        unsafe extern "system" fn(*mut c_void, *const PROPERTYKEY, *const PROPVARIANT) -> i32,
    commit: unsafe extern "system" fn(*mut c_void) -> i32,
}

struct PropertyStore(*mut c_void);

impl PropertyStore {
    fn vtbl(&self) -> &PropertyStoreVtbl {
        unsafe { &**(self.0 as *const *const PropertyStoreVtbl) }
    }

    fn for_window(hwnd: HWND) -> Option<Self> {
        let mut store: *mut c_void = std::ptr::null_mut();
        let status = unsafe { SHGetPropertyStoreForWindow(hwnd, &IID_IPROPERTY_STORE, &mut store) };
        if status != 0 || store.is_null() {
            tracing::debug!(status, "window property store is unavailable");
            return None;
        }
        Some(Self(store))
    }

    fn set_app_id(&self, app_id: &[u16]) {
        let bytes = std::mem::size_of_val(app_id);
        let buffer = unsafe { CoTaskMemAlloc(bytes) } as *mut u16;
        if buffer.is_null() {
            return;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(app_id.as_ptr(), buffer, app_id.len());

            let mut variant: PROPVARIANT = std::mem::zeroed();
            {
                let header = &mut variant.Anonymous.Anonymous;
                header.vt = VT_LPWSTR;
                header.Anonymous.pwszVal = buffer;
            }

            let vtbl = self.vtbl();
            let status = (vtbl.set_value)(self.0, &PKEY_APP_USER_MODEL_ID, &variant);
            if status == 0 {
                let commit = (vtbl.commit)(self.0);
                if commit != 0 {
                    tracing::debug!(commit, "could not commit the window AppUserModelID");
                }
            } else {
                tracing::debug!(status, "could not set System.AppUserModel.ID");
            }
            // Keep the CoTaskMem buffer alive until Commit has written the
            // value, then release it through the variant that owns it.
            PropVariantClear(&mut variant);
        }
    }
}

impl Drop for PropertyStore {
    fn drop(&mut self) {
        unsafe { (self.vtbl().release)(self.0) };
    }
}

/// A shortcut without `System.AppUserModel.ID` gets an auto-generated id, so
/// its taskbar button would not match the process's explicit AUMID and the
/// jump list would stay empty. Tagging the window keeps both on the same id.
pub fn install(hwnd: HWND) {
    let app_id: Vec<u16> = APP_USER_MODEL_ID
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    if let Some(store) = PropertyStore::for_window(hwnd) {
        store.set_app_id(&app_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_vtbl_reaches_set_value_at_slot_six() {
        assert_eq!(
            std::mem::offset_of!(PropertyStoreVtbl, set_value),
            6 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            std::mem::offset_of!(PropertyStoreVtbl, commit),
            7 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            std::mem::size_of::<PropertyStoreVtbl>(),
            8 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn the_property_key_and_iid_match_the_windows_sdk() {
        assert_eq!(IID_IPROPERTY_STORE.data1, 0x886d_8eeb);
        assert_eq!(IID_IPROPERTY_STORE.data2, 0x8cf2);
        assert_eq!(IID_IPROPERTY_STORE.data3, 0x4446);
        assert_eq!(PKEY_APP_USER_MODEL_ID.fmtid.data1, 0x9f4c_2855);
        assert_eq!(PKEY_APP_USER_MODEL_ID.fmtid.data2, 0x9f79);
        assert_eq!(PKEY_APP_USER_MODEL_ID.fmtid.data3, 0x4b39);
        assert_eq!(PKEY_APP_USER_MODEL_ID.pid, 5);
        assert_eq!(VT_LPWSTR, 31);
    }

    #[test]
    fn a_null_window_is_a_no_op() {
        install(std::ptr::null_mut());
    }

    #[test]
    fn a_cotaskmem_string_variant_round_trips_and_clears() {
        let app_id: Vec<u16> = APP_USER_MODEL_ID
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let bytes = std::mem::size_of_val(app_id.as_slice());
        let buffer = unsafe { CoTaskMemAlloc(bytes) } as *mut u16;
        assert!(!buffer.is_null());
        unsafe {
            std::ptr::copy_nonoverlapping(app_id.as_ptr(), buffer, app_id.len());
            let mut variant: PROPVARIANT = std::mem::zeroed();
            {
                let header = &mut variant.Anonymous.Anonymous;
                header.vt = VT_LPWSTR;
                header.Anonymous.pwszVal = buffer;

                let restored = std::slice::from_raw_parts(
                    header.Anonymous.pwszVal,
                    APP_USER_MODEL_ID.len() + 1,
                );
                assert_eq!(restored, app_id.as_slice());
                assert_eq!(header.vt, VT_LPWSTR);
            }
            assert_eq!(PropVariantClear(&mut variant), 0);
            assert_eq!(variant.Anonymous.Anonymous.vt, 0);
        }
    }
}
