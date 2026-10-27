use std::{ffi::c_void, io, ptr};

use windows_sys::Win32::{
    Foundation::{COLORREF, HINSTANCE, HWND, LRESULT, WPARAM},
    Graphics::Gdi::{
        CreateFontW, DeleteObject, GetSysColor, GetSysColorBrush, SetBkMode, SetTextColor,
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, COLOR_WINDOWTEXT, DEFAULT_CHARSET,
        DEFAULT_PITCH, FF_DONTCARE, HFONT, OUT_DEFAULT_PRECIS, TRANSPARENT,
    },
    UI::WindowsAndMessaging::{LoadImageW, SendMessageW, HICON, IMAGE_ICON, LR_SHARED, WM_SETFONT},
};

use crate::win32::to_wide;

const NORMAL_FONT_WEIGHT: i32 = 400;
const SEMIBOLD_FONT_WEIGHT: i32 = 600;

pub fn application_icon(instance: HINSTANCE, size: i32) -> io::Result<HICON> {
    // SAFETY: the integer resource ID is encoded as a non-dereferenced resource pointer.
    let icon = unsafe {
        LoadImageW(
            instance,
            ptr::without_provenance::<u16>(1),
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

pub struct FontSet {
    pub body: HFONT,
    pub body_small: HFONT,
    pub heading: HFONT,
    pub monospace: HFONT,
}

impl FontSet {
    pub fn new() -> io::Result<Self> {
        let body = create_font(-16, NORMAL_FONT_WEIGHT, "Segoe UI")?;
        let body_small = match create_font(-14, NORMAL_FONT_WEIGHT, "Segoe UI") {
            Ok(font) => font,
            Err(error) => {
                unsafe { DeleteObject(body) };
                return Err(error);
            }
        };
        let heading = match create_font(-27, SEMIBOLD_FONT_WEIGHT, "Segoe UI") {
            Ok(font) => font,
            Err(error) => {
                unsafe {
                    DeleteObject(body);
                    DeleteObject(body_small);
                }
                return Err(error);
            }
        };
        let monospace = match create_font(-15, NORMAL_FONT_WEIGHT, "Consolas") {
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
        unsafe { DeleteObject(self.body) };
        unsafe { DeleteObject(self.body_small) };
        unsafe { DeleteObject(self.heading) };
        unsafe { DeleteObject(self.monospace) };
    }
}

pub fn set_control_font(control: HWND, font: HFONT) {
    unsafe { SendMessageW(control, WM_SETFONT, font.expose_provenance(), 1) };
}

pub fn static_control_color(hdc: WPARAM) -> LRESULT {
    let hdc = ptr::with_exposed_provenance_mut::<c_void>(hdc);
    unsafe { SetBkMode(hdc, TRANSPARENT.cast_signed()) };
    unsafe { SetTextColor(hdc, system_text_color()) };
    unsafe { GetSysColorBrush(COLOR_WINDOW) }
        .expose_provenance()
        .cast_signed()
}

pub fn edit_control_color(hdc: WPARAM) -> LRESULT {
    let hdc = ptr::with_exposed_provenance_mut::<c_void>(hdc);
    unsafe { SetTextColor(hdc, system_text_color()) };
    unsafe { GetSysColorBrush(COLOR_WINDOW) }
        .expose_provenance()
        .cast_signed()
}

fn system_text_color() -> COLORREF {
    unsafe { GetSysColor(COLOR_WINDOWTEXT) }
}

fn create_font(height: i32, weight: i32, face_name: &'static str) -> io::Result<HFONT> {
    let face_name_wide = to_wide(face_name);
    let font = unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            u32::from(DEFAULT_CHARSET),
            u32::from(OUT_DEFAULT_PRECIS),
            u32::from(CLIP_DEFAULT_PRECIS),
            u32::from(CLEARTYPE_QUALITY),
            u32::from(DEFAULT_PITCH | FF_DONTCARE),
            face_name_wide.as_ptr(),
        )
    };
    if font.is_null() {
        Err(io::Error::other(format!(
            "could not create the {face_name} font"
        )))
    } else {
        Ok(font)
    }
}
