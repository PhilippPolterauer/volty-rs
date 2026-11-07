use std::{ffi::c_void, io, ptr};

use windows_sys::Win32::{
    Foundation::{COLORREF, HWND, LRESULT, WPARAM},
    Graphics::Gdi::{
        CreateFontW, DeleteObject, GetSysColor, GetSysColorBrush, SetBkMode, SetTextColor,
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, COLOR_WINDOWTEXT, DEFAULT_CHARSET,
        DEFAULT_PITCH, FF_DONTCARE, HFONT, OUT_DEFAULT_PRECIS, TRANSPARENT,
    },
    UI::WindowsAndMessaging::{SendMessageW, WM_SETFONT},
};

use crate::win32::to_wide;

const NORMAL_FONT_WEIGHT: i32 = 400;
const SEMIBOLD_FONT_WEIGHT: i32 = 600;

pub struct FontSet {
    pub body: HFONT,
    pub body_small: HFONT,
    pub heading: HFONT,
    pub monospace: HFONT,
}

impl FontSet {
    pub fn new() -> io::Result<Self> {
        Self::new_for_dpi(96)
    }

    pub fn new_for_dpi(dpi: u32) -> io::Result<Self> {
        let body = create_font(scale_font_height(-16, dpi), NORMAL_FONT_WEIGHT, "Segoe UI")?;
        let body_small =
            match create_font(scale_font_height(-14, dpi), NORMAL_FONT_WEIGHT, "Segoe UI") {
                Ok(font) => font,
                Err(error) => {
                    // SAFETY: this newly created font has not been selected into a device context.
                    delete_font(body);
                    return Err(error);
                }
            };
        let heading = match create_font(
            scale_font_height(-27, dpi),
            SEMIBOLD_FONT_WEIGHT,
            "Segoe UI",
        ) {
            Ok(font) => font,
            Err(error) => {
                delete_font(body);
                delete_font(body_small);
                return Err(error);
            }
        };
        let monospace =
            match create_font(scale_font_height(-15, dpi), NORMAL_FONT_WEIGHT, "Consolas") {
                Ok(font) => font,
                Err(error) => {
                    delete_font(body);
                    delete_font(body_small);
                    delete_font(heading);
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

fn scale_font_height(height: i32, dpi: u32) -> i32 {
    ((i64::from(height) * i64::from(dpi)) / 96) as i32
}

impl Drop for FontSet {
    fn drop(&mut self) {
        delete_font(self.body);
        delete_font(self.body_small);
        delete_font(self.heading);
        delete_font(self.monospace);
    }
}

fn delete_font(font: HFONT) {
    // SAFETY: this is a uniquely owned font not selected into a device context.
    if unsafe { DeleteObject(font) } == 0 {
        eprintln!(
            "Volty could not release a UI font: {}",
            io::Error::last_os_error()
        );
    }
}

pub fn set_control_font(control: HWND, font: HFONT) {
    // SAFETY: control is a live child HWND and font remains owned by its parent FontSet.
    unsafe { SendMessageW(control, WM_SETFONT, font.expose_provenance(), 1) };
}

pub fn static_control_color(hdc: WPARAM) -> LRESULT {
    let hdc = ptr::with_exposed_provenance_mut::<c_void>(hdc);
    // SAFETY: WM_CTLCOLORSTATIC supplies this live HDC; the returned system brush is shared.
    unsafe {
        SetBkMode(hdc, TRANSPARENT.cast_signed());
        SetTextColor(hdc, system_text_color());
        GetSysColorBrush(COLOR_WINDOW)
    }
    .expose_provenance()
    .cast_signed()
}

pub fn edit_control_color(hdc: WPARAM) -> LRESULT {
    let hdc = ptr::with_exposed_provenance_mut::<c_void>(hdc);
    // SAFETY: WM_CTLCOLOREDIT supplies this live HDC; the returned system brush is shared.
    unsafe {
        SetTextColor(hdc, system_text_color());
        GetSysColorBrush(COLOR_WINDOW)
    }
    .expose_provenance()
    .cast_signed()
}

fn system_text_color() -> COLORREF {
    // SAFETY: COLOR_WINDOWTEXT is a valid system color index.
    unsafe { GetSysColor(COLOR_WINDOWTEXT) }
}

fn create_font(height: i32, weight: i32, face_name: &'static str) -> io::Result<HFONT> {
    let face_name_wide = to_wide(face_name);
    // SAFETY: face_name_wide is NUL-terminated and remains alive for this synchronous call.
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
        let error = io::Error::last_os_error();
        Err(io::Error::new(
            error.kind(),
            format!("could not create the {face_name} font: {error}"),
        ))
    } else {
        Ok(font)
    }
}
