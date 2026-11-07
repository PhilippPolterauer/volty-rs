use crate::{config::AppTheme, win32};
use std::{io, ptr};

use windows_sys::Win32::{
    Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetStockObject,
        GetSysColor, RoundRect, SelectObject, SetBkColor, SetBkMode, SetDCPenColor, SetTextColor,
        StretchDIBits, BITMAPINFO, BI_RGB, COLOR_WINDOW, COLOR_WINDOWTEXT, DC_PEN, DIB_RGB_COLORS,
        HBRUSH, HDC, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
    },
    UI::WindowsAndMessaging::GetClientRect,
};

use super::{
    control, get_control_text, parse_hex_color, top_card_layout, ControlRole, SettingsWindow,
};

pub(super) struct Appearance {
    pub(super) dark: bool,
    high_contrast: bool,
    canvas: HBRUSH,
    surface: HBRUSH,
    canvas_color: COLORREF,
    surface_color: COLORREF,
    text: COLORREF,
    secondary: COLORREF,
    border: COLORREF,
    error: COLORREF,
}

impl Appearance {
    pub(super) fn new(preference: AppTheme) -> io::Result<Self> {
        use windows_sys::Win32::UI::{
            Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
            WindowsAndMessaging::{SystemParametersInfoW, SPI_GETHIGHCONTRAST},
        };
        let mut contrast = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            ..Default::default()
        };
        // SAFETY: contrast has the documented structure size and writable storage.
        unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                contrast.cbSize,
                ptr::from_mut(&mut contrast).cast(),
                0,
            );
        }
        let high_contrast = contrast.dwFlags & HCF_HIGHCONTRASTON != 0;
        let dark = !high_contrast
            && match preference {
                AppTheme::Dark => true,
                AppTheme::Light => false,
                AppTheme::System => {
                    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
                    match RegKey::predef(HKEY_CURRENT_USER).open_subkey(
                        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                    ) {
                        Ok(key) => match key.get_value::<u32, _>("AppsUseLightTheme") {
                            Ok(value) => value == 0,
                            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                            Err(error) => return Err(error),
                        },
                        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                        Err(error) => return Err(error),
                    }
                }
            };
        let (canvas_color, surface_color, text, secondary, border, error) = if high_contrast {
            // SAFETY: these are documented system color indices.
            unsafe {
                let bg = GetSysColor(COLOR_WINDOW);
                let fg = GetSysColor(COLOR_WINDOWTEXT);
                (bg, bg, fg, fg, fg, fg)
            }
        } else if dark {
            (
                0x00202020, 0x002b2b2b, 0x00f5f5f5, 0x00c2c2c2, 0x00404040, 0x009999ff,
            )
        } else {
            (
                0x00f3f3f3, 0x00ffffff, 0x001a1a1a, 0x005f5f5f, 0x00e2e2e2, 0x002b2bc4,
            )
        };
        // SAFETY: CreateSolidBrush accepts any COLORREF and returns an owned brush.
        let canvas = unsafe { CreateSolidBrush(canvas_color) };
        if canvas.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: as above; the two brushes are independently owned.
        let surface = unsafe { CreateSolidBrush(surface_color) };
        if surface.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: canvas is owned here and has not been selected into an HDC.
            unsafe { DeleteObject(canvas) };
            return Err(error);
        }
        Ok(Self {
            canvas,
            surface,
            canvas_color,
            surface_color,
            text,
            secondary,
            border,
            error,
            dark,
            high_contrast,
        })
    }

    pub(super) fn same_colors(&self, other: &Self) -> bool {
        self.dark == other.dark
            && self.high_contrast == other.high_contrast
            && self.canvas_color == other.canvas_color
            && self.text == other.text
    }

    pub(super) fn surface_rgb(&self) -> [u8; 3] {
        [
            self.surface_color as u8,
            (self.surface_color >> 8) as u8,
            (self.surface_color >> 16) as u8,
        ]
    }

    pub(super) fn apply_native_theme(&self, window: HWND, state: &SettingsWindow) {
        use windows_sys::Win32::Graphics::Gdi::{
            RedrawWindow, RDW_ALLCHILDREN, RDW_FRAME, RDW_INVALIDATE, RDW_NOERASE,
        };
        use windows_sys::Win32::{
            Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE},
            UI::Controls::SetWindowTheme,
        };
        win32::configure_app_dark_mode(self.dark);
        win32::set_window_dark_mode(window, self.dark);
        let dark = i32::from(self.dark);
        // SAFETY: window is live and the attribute expects a BOOL-sized buffer.
        unsafe {
            DwmSetWindowAttribute(
                window,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                ptr::from_ref(&dark).cast(),
                std::mem::size_of_val(&dark) as u32,
            );
        }
        for child in &state.controls {
            win32::set_window_dark_mode(child.handle, self.dark);
            let theme = win32::to_wide(if self.high_contrast {
                ""
            } else if self.dark {
                "DarkMode_Explorer"
            } else {
                "Explorer"
            });
            // SAFETY: the control is live and theme remains NUL-terminated through the call.
            unsafe {
                SetWindowTheme(
                    child.handle,
                    if self.high_contrast {
                        ptr::null()
                    } else {
                        theme.as_ptr()
                    },
                    ptr::null(),
                );
            }
        }
        for child in state.controls.iter().filter(|child| child.role.is_combo()) {
            use windows_sys::Win32::UI::Controls::{GetComboBoxInfo, SetWindowTheme, COMBOBOXINFO};
            let mut info = COMBOBOXINFO {
                cbSize: std::mem::size_of::<COMBOBOXINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: the combo and output structure are valid; this queries its native popup list.
            if unsafe { GetComboBoxInfo(child.handle, &mut info) } != 0 && !info.hwndList.is_null()
            {
                win32::set_window_dark_mode(info.hwndList, self.dark);
                let theme = win32::to_wide(if self.dark {
                    "DarkMode_Explorer"
                } else {
                    "Explorer"
                });
                // SAFETY: the combo owns this live list HWND and theme is NUL-terminated.
                unsafe {
                    SetWindowTheme(
                        info.hwndList,
                        if self.high_contrast {
                            ptr::null()
                        } else {
                            theme.as_ptr()
                        },
                        ptr::null(),
                    );
                }
            }
        }
        // SAFETY: invalidate the window and all child controls without changing input state.
        unsafe {
            RedrawWindow(
                window,
                ptr::null(),
                ptr::null_mut(),
                RDW_INVALIDATE | RDW_NOERASE | RDW_ALLCHILDREN | RDW_FRAME,
            );
        }
    }

    pub(super) fn erase(&self, _hdc: WPARAM, _window: HWND) -> LRESULT {
        1
    }

    pub(super) fn control_color(
        &self,
        hdc: WPARAM,
        child: LPARAM,
        state: &SettingsWindow,
    ) -> LRESULT {
        let role = state
            .controls
            .iter()
            .find(|c| c.handle.expose_provenance() == child as usize)
            .map(|c| c.role);
        let on_canvas = matches!(
            role,
            Some(
                ControlRole::Heading
                    | ControlRole::Description
                    | ControlRole::ConfigError
                    | ControlRole::ReadOnly
                    | ControlRole::Status
                    | ControlRole::Save
            )
        );
        let error = matches!(
            role,
            Some(
                ControlRole::ConfigError
                    | ControlRole::ThresholdError
                    | ControlRole::CooldownError
                    | ControlRole::ColorMapError
                    | ControlRole::StartupError
            )
        );
        let secondary = matches!(
            role,
            Some(
                ControlRole::Description
                    | ControlRole::BatteryDescription
                    | ControlRole::ThresholdHelp
                    | ControlRole::CooldownHelp
                    | ControlRole::TrayDescription
                    | ControlRole::AppThemeHelp
                    | ControlRole::IconStyleHelp
                    | ControlRole::IconThemeHelp
                    | ControlRole::PercentageHelp
                    | ControlRole::PinnedDeviceHelp
                    | ControlRole::ColorMapDescription
                    | ControlRole::RedCutoffLabel
                    | ControlRole::YellowCutoffLabel
                    | ControlRole::RedColorLabel
                    | ControlRole::YellowColorLabel
                    | ControlRole::GreenColorLabel
                    | ControlRole::PreviewLabel
                    | ControlRole::StartupDescription
                    | ControlRole::Status
            )
        );
        let brush = if on_canvas { self.canvas } else { self.surface };
        // SAFETY: color-control messages supply a live HDC for this child control.
        unsafe {
            let hdc = ptr::with_exposed_provenance_mut(hdc);
            SetBkMode(hdc, TRANSPARENT as i32);
            SetBkColor(
                hdc,
                if on_canvas {
                    self.canvas_color
                } else {
                    self.surface_color
                },
            );
            SetTextColor(
                hdc,
                if error {
                    self.error
                } else if secondary {
                    self.secondary
                } else {
                    self.text
                },
            );
        }
        brush.expose_provenance() as LRESULT
    }

    pub(super) fn draw_combo_item(
        &self,
        item: &windows_sys::Win32::UI::Controls::DRAWITEMSTRUCT,
        font: windows_sys::Win32::Graphics::Gdi::HFONT,
    ) {
        use windows_sys::Win32::{
            Graphics::Gdi::{
                DrawFocusRect, DrawTextW, GetSysColorBrush, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
                COLOR_HIGHLIGHTTEXT, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE,
                DT_VCENTER, HFONT, OPAQUE,
            },
            UI::{
                Controls::{
                    ODS_COMBOBOXEDIT, ODS_DISABLED, ODS_FOCUS, ODS_NOFOCUSRECT, ODS_SELECTED,
                },
                WindowsAndMessaging::{SendMessageW, CB_GETLBTEXT, CB_GETLBTEXTLEN},
            },
        };
        let selected = item.itemState & ODS_SELECTED != 0 && item.itemState & ODS_COMBOBOXEDIT == 0;
        // SAFETY: the system owns these brushes and supplies a live drawing HDC and rectangle.
        unsafe {
            let brush = if selected {
                GetSysColorBrush(COLOR_HIGHLIGHT)
            } else {
                self.surface
            };
            FillRect(item.hDC, &item.rcItem, brush);
            let text_color = if item.itemState & ODS_DISABLED != 0 {
                GetSysColor(COLOR_GRAYTEXT)
            } else if selected {
                GetSysColor(COLOR_HIGHLIGHTTEXT)
            } else {
                self.text
            };
            SetTextColor(item.hDC, text_color);
            SetBkMode(item.hDC, OPAQUE as i32);
            SetBkColor(
                item.hDC,
                if selected {
                    GetSysColor(COLOR_HIGHLIGHT)
                } else {
                    self.surface_color
                },
            );
            let old_font: HFONT = SelectObject(item.hDC, font);
            if item.itemID != u32::MAX {
                let length = SendMessageW(item.hwndItem, CB_GETLBTEXTLEN, item.itemID as usize, 0);
                if length >= 0 {
                    let mut text = vec![0_u16; length as usize + 1];
                    let copied = SendMessageW(
                        item.hwndItem,
                        CB_GETLBTEXT,
                        item.itemID as usize,
                        text.as_mut_ptr().expose_provenance() as isize,
                    );
                    if copied >= 0 {
                        let mut rect = item.rcItem;
                        rect.left += 6;
                        rect.right -= 6;
                        DrawTextW(
                            item.hDC,
                            text.as_ptr(),
                            copied as i32,
                            &mut rect,
                            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | DT_LEFT,
                        );
                    }
                }
            }
            if item.itemState & ODS_FOCUS != 0 && item.itemState & ODS_NOFOCUSRECT == 0 {
                DrawFocusRect(item.hDC, &item.rcItem);
            }
            SelectObject(item.hDC, old_font);
        }
    }

    pub(super) fn input_color(&self, hdc: WPARAM) -> LRESULT {
        // SAFETY: WM_CTLCOLOREDIT/WM_CTLCOLORLISTBOX supply a live device context.
        unsafe {
            let hdc = ptr::with_exposed_provenance_mut(hdc);
            SetBkMode(hdc, windows_sys::Win32::Graphics::Gdi::OPAQUE as i32);
            SetBkColor(hdc, self.surface_color);
            SetTextColor(hdc, self.text);
        }
        self.surface.expose_provenance() as LRESULT
    }

    pub(super) fn paint(&self, window: HWND, state: &SettingsWindow) {
        let mut paint = PAINTSTRUCT::default();
        let mut rect = RECT::default();
        // SAFETY: BeginPaint/EndPaint are paired for this window's WM_PAINT.
        unsafe {
            let hdc: HDC = BeginPaint(window, &mut paint);
            GetClientRect(window, &mut rect);
            FillRect(hdc, &rect, self.canvas);
            let width = (rect.right - rect.left).min(960);
            let offset = (rect.right - width) / 2;
            let old_brush = SelectObject(hdc, self.surface);
            let old_pen = SelectObject(hdc, GetStockObject(DC_PEN));
            SetDCPenColor(hdc, self.border);
            let (left_card_width, right_card_width, right_card_x) = top_card_layout(width);
            for (left, top, right, bottom) in [
                (32, 120, 32 + left_card_width, 390),
                (right_card_x, 120, right_card_x + right_card_width, 390),
                (32, 400, width - 32, 660),
                (32, 670, width - 32, 736),
            ] {
                RoundRect(hdc, offset + left, top, offset + right, bottom, 10, 10);
            }
            let inset = 56;
            let full_width = width - inset * 2;
            let gap = 16;
            let column_width = (full_width - gap * 2) / 3;
            for (role, column) in [
                (ControlRole::RedColor, 0),
                (ControlRole::YellowColor, 1),
                (ControlRole::GreenColor, 2),
            ] {
                let color = parse_hex_color(&get_control_text(control(state, role)))
                    .unwrap_or([110, 110, 110]);
                let brush = CreateSolidBrush(
                    u32::from(color[0]) | (u32::from(color[1]) << 8) | (u32::from(color[2]) << 16),
                );
                if !brush.is_null() {
                    let x = offset + inset + (column_width + gap) * column + 2;
                    let swatch = RECT {
                        left: x,
                        top: 564,
                        right: x + 22,
                        bottom: 586,
                    };
                    FillRect(hdc, &swatch, brush);
                    DeleteObject(brush);
                }
            }
            if state.preview_bgra.len() == 32 * 32 * 4 {
                let mut bitmap = BITMAPINFO::default();
                bitmap.bmiHeader.biSize = std::mem::size_of_val(&bitmap.bmiHeader) as u32;
                bitmap.bmiHeader.biWidth = 32;
                bitmap.bmiHeader.biHeight = -32;
                bitmap.bmiHeader.biPlanes = 1;
                bitmap.bmiHeader.biBitCount = 32;
                bitmap.bmiHeader.biCompression = BI_RGB;
                bitmap.bmiHeader.biSizeImage = state.preview_bgra.len() as u32;
                let preview_x = offset + width - 124;
                RoundRect(hdc, preview_x - 8, 590, preview_x + 72, 660, 8, 8);
                StretchDIBits(
                    hdc,
                    preview_x,
                    596,
                    64,
                    64,
                    0,
                    0,
                    32,
                    32,
                    state.preview_bgra.as_ptr().cast(),
                    &bitmap,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
            }
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old_brush);
            EndPaint(window, &paint);
        }
    }
}

impl Drop for Appearance {
    fn drop(&mut self) {
        // SAFETY: both brushes are owned and painting has restored the previous HDC objects.
        unsafe {
            DeleteObject(self.canvas);
            DeleteObject(self.surface);
        }
    }
}
