use std::{io, ptr, sync::OnceLock};

use tao::window::Icon;
use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Shell::SetCurrentProcessExplicitAppUserModelID,
        WindowsAndMessaging::{
            GetSystemMetrics, LoadImageW, MessageBoxIndirectW, SendMessageW, HICON, ICON_BIG,
            ICON_SMALL, IMAGE_ICON, LR_SHARED, MB_USERICON, MSGBOXPARAMSW, SM_CXICON, SM_CXSMICON,
            SM_CYICON, SM_CYSMICON, WM_SETICON,
        },
    },
};

const APP_USER_MODEL_ID: &str = "PhilippPolterauer.Volty";
// build.rs uses winresource::set_icon, whose default application-icon ID is 1.
const ICON_RESOURCE_ID: usize = 1;
const FAVICON_PNG: &[u8] = include_bytes!("../assets/volty-favicon.png");

type IconPixels = Result<(Vec<u8>, u32, u32), String>;

static FAVICON_DATA_URI: OnceLock<String> = OnceLock::new();
static TAO_ICON_PIXELS: OnceLock<IconPixels> = OnceLock::new();

pub fn set_process_app_user_model_id() -> io::Result<()> {
    let app_user_model_id: Vec<u16> = APP_USER_MODEL_ID.encode_utf16().chain([0]).collect();
    // SAFETY: app_user_model_id is NUL-terminated and remains alive for this call.
    let result = unsafe { SetCurrentProcessExplicitAppUserModelID(app_user_model_id.as_ptr()) };
    if result < 0 {
        Err(io::Error::from_raw_os_error(result))
    } else {
        Ok(())
    }
}

pub fn load_application_icon(instance: HINSTANCE, size: i32) -> io::Result<HICON> {
    if size <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "application icon size must be positive",
        ));
    }
    // SAFETY: the integer resource ID is encoded as a non-dereferenced resource pointer.
    let icon = unsafe {
        LoadImageW(
            instance,
            ptr::without_provenance::<u16>(ICON_RESOURCE_ID),
            IMAGE_ICON,
            size,
            size,
            LR_SHARED,
        )
    };
    if icon.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(icon)
    }
}

pub fn class_icons(instance: HINSTANCE) -> io::Result<(HICON, HICON)> {
    // SAFETY: these are valid system metric indices and require no pointer arguments.
    let small = unsafe { GetSystemMetrics(SM_CXSMICON) };
    // SAFETY: these are valid system metric indices and require no pointer arguments.
    let small_height = unsafe { GetSystemMetrics(SM_CYSMICON) };
    // SAFETY: these are valid system metric indices and require no pointer arguments.
    let large = unsafe { GetSystemMetrics(SM_CXICON) };
    // SAFETY: these are valid system metric indices and require no pointer arguments.
    let large_height = unsafe { GetSystemMetrics(SM_CYICON) };
    if small <= 0 || small_height <= 0 || large <= 0 || large_height <= 0 {
        return Err(io::Error::last_os_error());
    }

    let small_icon = load_application_icon(instance, small.max(small_height))?;
    let large_icon = load_application_icon(instance, large.max(large_height))?;
    Ok((small_icon, large_icon))
}

pub fn apply_to_window(window: HWND) -> io::Result<()> {
    if window.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot set an icon on a null window handle",
        ));
    }
    // SAFETY: a null module name requests the current process executable's module handle.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    if instance.is_null() {
        return Err(io::Error::last_os_error());
    }
    let (small, large) = class_icons(instance)?;
    // SAFETY: window is a live HWND and both icons are shared handles loaded from this executable.
    unsafe {
        SendMessageW(
            window,
            WM_SETICON,
            ICON_SMALL as usize,
            small.expose_provenance().cast_signed(),
        );
        SendMessageW(
            window,
            WM_SETICON,
            ICON_BIG as usize,
            large.expose_provenance().cast_signed(),
        );
    }
    Ok(())
}

pub fn tao_window_icon() -> io::Result<Icon> {
    let (pixels, width, height) = TAO_ICON_PIXELS
        .get_or_init(|| {
            image::load_from_memory(FAVICON_PNG)
                .map(|image| {
                    let image = image.into_rgba8();
                    let (width, height) = image.dimensions();
                    (image.into_raw(), width, height)
                })
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| io::Error::other(error.clone()))?;
    Icon::from_rgba(pixels.clone(), *width, *height)
        .map_err(|error| io::Error::other(format!("could not create window icon: {error}")))
}

pub fn show_message_box(text: &str, title: &str, style: u32) -> i32 {
    let text = crate::win32::to_wide(text);
    let title = crate::win32::to_wide(title);
    // SAFETY: a null module name requests the current process executable's module handle.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    if instance.is_null() {
        return 0;
    }
    let parameters = MSGBOXPARAMSW {
        cbSize: std::mem::size_of::<MSGBOXPARAMSW>() as u32,
        hwndOwner: ptr::null_mut(),
        hInstance: instance,
        lpszText: text.as_ptr(),
        lpszCaption: title.as_ptr(),
        dwStyle: style | MB_USERICON,
        lpszIcon: ptr::without_provenance::<u16>(ICON_RESOURCE_ID),
        dwContextHelpId: 0,
        lpfnMsgBoxCallback: None,
        dwLanguageId: 0,
    };
    // SAFETY: parameters and both NUL-terminated strings remain live for the synchronous call.
    unsafe { MessageBoxIndirectW(&parameters) }
}

pub fn favicon_data_uri() -> &'static str {
    FAVICON_DATA_URI
        .get_or_init(|| format!("data:image/png;base64,{}", base64_encode(FAVICON_PNG)))
        .as_str()
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        encoded.push(ALPHABET[(first >> 2) as usize] as char);
        encoded.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(third & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_encoding_handles_partial_chunks() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
    }
}
