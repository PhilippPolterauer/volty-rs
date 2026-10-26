use std::io;

use windows_sys::Win32::{
    Foundation::{COLORREF, HINSTANCE, HWND, LRESULT, WPARAM},
    Graphics::Gdi::{
        CreateFontW, DeleteObject, GetSysColor, GetSysColorBrush, SetBkMode, SetTextColor,
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, COLOR_WINDOWTEXT, DEFAULT_CHARSET,
        DEFAULT_PITCH, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, OUT_DEFAULT_PRECIS, TRANSPARENT,
    },
    UI::WindowsAndMessaging::{LoadImageW, SendMessageW, HICON, IMAGE_ICON, LR_SHARED, WM_SETFONT},
};

pub fn application_icon(instance: HINSTANCE, size: i32) -> io::Result<HICON> {
    let icon = unsafe {
        LoadImageW(
            instance,
            1usize as *const u16,
            IMAGE_ICON,
            size,
            size,
            LR_SHARED,
        )
    };
    if icon.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(icon as HICON)
    }
}

pub struct FontSet {
    pub body: windows_sys::Win32::Graphics::Gdi::HFONT,
    pub body_small: windows_sys::Win32::Graphics::Gdi::HFONT,
    pub heading: windows_sys::Win32::Graphics::Gdi::HFONT,
    pub monospace: windows_sys::Win32::Graphics::Gdi::HFONT,
}

impl FontSet {
    pub fn new() -> io::Result<Self> {
        let body = create_font(-16, FW_NORMAL, "Segoe UI")?;
        let body_small = match create_font(-14, FW_NORMAL, "Segoe UI") {
            Ok(font) => font,
            Err(error) => {
                unsafe { DeleteObject(body) };
                return Err(error);
            }
        };
        let heading = match create_font(-27, FW_SEMIBOLD, "Segoe UI") {
            Ok(font) => font,
            Err(error) => {
                unsafe {
                    DeleteObject(body);
                    DeleteObject(body_small);
                }
                return Err(error);
            }
        };
        let monospace = match create_font(-15, FW_NORMAL, "Consolas") {
            Ok(font) => font,
            Err(error) => {
                unsafe {
                    DeleteObject(body);
                    DeleteObject(body_small);
                    DeleteObject(heading);
                }
                return Err(error);
            }
        };

        Ok(Self {
            body,
            body_small,
            heading,
            monospace,
        })
    }
}

impl Drop for FontSet {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.body);
            DeleteObject(self.body_small);
            DeleteObject(self.heading);
            DeleteObject(self.monospace);
        }
    }
}

pub unsafe fn set_control_font(control: HWND, font: windows_sys::Win32::Graphics::Gdi::HFONT) {
    SendMessageW(control, WM_SETFONT, font as usize, 1);
}

pub unsafe fn static_control_color(hdc: WPARAM) -> LRESULT {
    let hdc = hdc as windows_sys::Win32::Graphics::Gdi::HDC;
    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, GetSysColor(COLOR_WINDOWTEXT) as COLORREF);
    GetSysColorBrush(COLOR_WINDOW) as LRESULT
}

pub unsafe fn edit_control_color(hdc: WPARAM) -> LRESULT {
    let hdc = hdc as windows_sys::Win32::Graphics::Gdi::HDC;
    SetTextColor(hdc, GetSysColor(COLOR_WINDOWTEXT) as COLORREF);
    GetSysColorBrush(COLOR_WINDOW) as LRESULT
}

fn create_font(
    height: i32,
    weight: u32,
    face_name: &str,
) -> io::Result<windows_sys::Win32::Graphics::Gdi::HFONT> {
    let face_label = face_name;
    let face_name: Vec<u16> = face_name.encode_utf16().chain(Some(0)).collect();
    let font = unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face_name.as_ptr(),
        )
    };
    if font.is_null() {
        Err(io::Error::other(format!(
            "could not create the {face_label} font"
        )))
    } else {
        Ok(font)
    }
}
