//! Apply the original host's Windows 11 Desktop Acrylic on the native GPUI window.
//! Only ever called on this window's own UI thread. On older Windows keep GPUI's existing acrylic fallback.
use windows_sys::Win32::Foundation::HWND;
#[repr(C)]
struct Margins {
    i_left: i32,
    i_right: i32,
    i_top: i32,
    i_bottom: i32,
}
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmExtendFrameIntoClientArea(w: HWND, m: *const Margins) -> i32;
    fn DwmSetWindowAttribute(
        w: HWND,
        attribute: u32,
        v: *const std::ffi::c_void,
        bytes: u32,
    ) -> i32;
}
// GPUI 0.2 requests the legacy ACCENT_ENABLE_ACRYLICBLURBEHIND when it creates the HWND.
// That policy takes precedence over the Windows 11 transient backdrop. Clear it only after
// the new DWM attribute was accepted; otherwise keep the older system's fallback.
fn clear_legacy_accent(w: HWND) -> bool {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    #[repr(C)]
    struct Accent {
        i: i32,
        flags: i32,
        color: u32,
        animation: i32,
    }
    #[repr(C)]
    struct Data {
        attr: i32,
        data: *const std::ffi::c_void,
        size: usize,
    }
    type Set = unsafe extern "system" fn(HWND, *mut Data) -> i32;
    let user: Vec<_> = "user32.dll\0".encode_utf16().collect();
    let mut policy = Accent {
        i: 0,
        flags: 2,
        color: 0,
        animation: 0,
    };
    let mut data = Data {
        attr: 19,
        data: (&mut policy as *mut Accent).cast(),
        size: std::mem::size_of::<Accent>(),
    };
    unsafe {
        let h = GetModuleHandleW(user.as_ptr());
        if h.is_null() {
            return false;
        }
        let Some(p) = GetProcAddress(h, c"SetWindowCompositionAttribute".as_ptr().cast()) else {
            return false;
        };
        let set: Set = std::mem::transmute(p);
        set(w, &mut data) != 0
    }
}
pub fn apply(w: HWND, dark: bool) -> i32 {
    if w.is_null() {
        return -1;
    }
    fn set(w: HWND, attr: u32, v: i32) -> i32 {
        unsafe { DwmSetWindowAttribute(w, attr, (&v as *const i32).cast(), 4) }
    }
    let _ = set(w, 33, 2);
    let _ = set(w, 20, i32::from(dark));
    let r = set(w, 38, 3);
    if r >= 0 {
        if clear_legacy_accent(w) {
            let applied = set(w, 38, 3);
            if applied < 0 {
                return applied;
            }
        }
        unsafe {
            DwmExtendFrameIntoClientArea(
                w,
                &Margins {
                    i_left: -1,
                    i_right: -1,
                    i_top: -1,
                    i_bottom: -1,
                },
            )
        }
    } else {
        r
    }
}
