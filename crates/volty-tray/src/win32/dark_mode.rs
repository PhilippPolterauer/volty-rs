use std::sync::OnceLock;

use windows_sys::core::BOOL;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM},
    System::{
        LibraryLoader::{GetProcAddress, LoadLibraryA},
        Threading::GetCurrentThreadId,
    },
    UI::WindowsAndMessaging::{EnumThreadWindows, SendMessageW, WM_THEMECHANGED},
};

type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
type AllowDarkModeForWindow = unsafe extern "system" fn(HWND, BOOL) -> BOOL;
type FlushMenuThemes = unsafe extern "system" fn();

#[derive(Clone, Copy)]
struct DarkModeApi {
    set_preferred_app_mode: SetPreferredAppMode,
    allow_dark_mode_for_window: Option<AllowDarkModeForWindow>,
    flush_menu_themes: Option<FlushMenuThemes>,
}

const SET_PREFERRED_APP_MODE_ORDINAL: u16 = 135;
const ALLOW_DARK_MODE_FOR_WINDOW_ORDINAL: u16 = 133;
const FLUSH_MENU_THEMES_ORDINAL: u16 = 136;
const PREFERRED_APP_MODE_ALLOW_DARK: i32 = 1;

static DARK_MODE_API: OnceLock<Option<DarkModeApi>> = OnceLock::new();

fn dark_mode_api() -> Option<DarkModeApi> {
    DARK_MODE_API
        .get_or_init(|| {
            // These Windows theme exports are undocumented, so resolve them by ordinal and
            // gracefully keep the native default theme when the OS does not provide them.
            // SAFETY: the C string is static and NUL-terminated.
            let module = unsafe { LoadLibraryA(c"uxtheme.dll".as_ptr().cast()) };
            if module.is_null() {
                return None;
            }

            // SAFETY: module is a loaded library and Windows encodes ordinal lookups as pointer values.
            let set_preferred_app_mode = unsafe {
                GetProcAddress(module, SET_PREFERRED_APP_MODE_ORDINAL as usize as *const u8)
            }?;
            // SAFETY: uxtheme ordinal 135 is SetPreferredAppMode with this signature.
            let set_preferred_app_mode = unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, SetPreferredAppMode>(
                    set_preferred_app_mode,
                )
            };
            // SAFETY: module is a loaded library and Windows encodes ordinal lookups as pointer values.
            let allow_dark_mode_for_window = unsafe {
                GetProcAddress(
                    module,
                    ALLOW_DARK_MODE_FOR_WINDOW_ORDINAL as usize as *const u8,
                )
            };
            // SAFETY: uxtheme ordinal 133 is AllowDarkModeForWindow with this signature.
            let allow_dark_mode_for_window = allow_dark_mode_for_window.map(|procedure| unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, AllowDarkModeForWindow>(
                    procedure,
                )
            });
            // SAFETY: module is a loaded library and Windows encodes ordinal lookups as pointer values.
            let flush_menu_themes =
                unsafe { GetProcAddress(module, FLUSH_MENU_THEMES_ORDINAL as usize as *const u8) };
            // SAFETY: uxtheme ordinal 136 is FlushMenuThemes with this signature.
            let flush_menu_themes = flush_menu_themes.map(|procedure| unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, FlushMenuThemes>(
                    procedure,
                )
            });

            Some(DarkModeApi {
                set_preferred_app_mode,
                allow_dark_mode_for_window,
                flush_menu_themes,
            })
        })
        .as_ref()
        .copied()
}

pub(crate) fn enable_system_dark_menus() {
    let Some(api) = dark_mode_api() else {
        eprintln!(
            "Windows dark-mode menu APIs are unavailable; using the default tray-menu theme."
        );
        return;
    };

    // AllowDark opts in while leaving the actual light/dark choice to Windows.
    // SAFETY: the function pointers came from the matching uxtheme ordinals above.
    unsafe {
        (api.set_preferred_app_mode)(PREFERRED_APP_MODE_ALLOW_DARK);
        if let Some(flush_menu_themes) = api.flush_menu_themes {
            flush_menu_themes();
        }
    }
}

pub(crate) fn allow_dark_mode_for_current_thread_windows() {
    let Some(api) = dark_mode_api() else {
        return;
    };
    if api.allow_dark_mode_for_window.is_none() {
        eprintln!(
            "Windows cannot enable dark mode for the tray window; using the default menu theme."
        );
        return;
    }

    // SAFETY: this enumerates live top-level windows owned by the current UI thread.
    if unsafe { EnumThreadWindows(GetCurrentThreadId(), Some(allow_dark_mode_for_window), 0) } == 0
    {
        eprintln!(
            "Could not apply the Windows dark-menu preference to tray windows: {}",
            std::io::Error::last_os_error()
        );
    }
}

unsafe extern "system" fn allow_dark_mode_for_window(window: HWND, _: LPARAM) -> BOOL {
    if let Some(api) = dark_mode_api() {
        if let Some(allow_dark_mode_for_window) = api.allow_dark_mode_for_window {
            // SAFETY: EnumThreadWindows supplies a live HWND and the resolved export has the
            // documented AllowDarkModeForWindow signature.
            unsafe {
                allow_dark_mode_for_window(window, 1);
                SendMessageW(window, WM_THEMECHANGED, 0, 0);
            }
        }
    }
    1
}
