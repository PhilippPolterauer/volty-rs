use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::{
        Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE},
        Gdi::{GetSysColor, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT},
    },
    UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        WindowsAndMessaging::{
            SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST,
        },
    },
};

pub(super) const SECTION_PADDING: i32 = 16;
pub(super) const SECTION_RADIUS: i32 = 8;
pub(super) const CONTROL_HEIGHT: i32 = 32;
pub(super) const GAP: i32 = 8;

pub(super) fn animation_seconds() -> f32 {
    let mut animate = 0_i32;
    // SAFETY: the action writes a Win32 BOOL into the provided storage.
    let ok = unsafe {
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut animate).cast(), 0)
    };
    if ok != 0 && animate != 0 {
        0.12
    } else {
        0.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Palette {
    pub canvas: [u8; 3],
    pub surface: [u8; 3],
    pub raised: [u8; 3],
    pub hover: [u8; 3],
    pub border: [u8; 3],
    pub text: [u8; 3],
    pub secondary: [u8; 3],
    pub accent: [u8; 3],
    pub on_accent: [u8; 3],
    pub destructive: [u8; 3],
}

impl Palette {
    pub(super) fn system_contrast() -> Option<Self> {
        let mut contrast = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            ..Default::default()
        };
        // SAFETY: the parameter points to a writable HIGHCONTRASTW of the documented size.
        let enabled = unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                contrast.cbSize,
                (&raw mut contrast).cast(),
                0,
            )
        } != 0
            && contrast.dwFlags & HCF_HIGHCONTRASTON != 0;
        if !enabled {
            return None;
        }
        // SAFETY: all four indices identify documented system colors.
        let [canvas, text, accent, on_accent] = unsafe {
            [
                COLOR_WINDOW,
                COLOR_WINDOWTEXT,
                COLOR_HIGHLIGHT,
                COLOR_HIGHLIGHTTEXT,
            ]
            .map(|index| GetSysColor(index).to_le_bytes()[..3].try_into().unwrap())
        };
        Some(Self {
            canvas,
            surface: canvas,
            raised: canvas,
            hover: canvas,
            border: text,
            text,
            secondary: text,
            accent,
            on_accent,
            destructive: text,
        })
    }

    pub(super) fn standard(dark: bool) -> Self {
        if dark {
            Self {
                canvas: [0x20; 3],
                surface: [0x27; 3],
                raised: [0x2D; 3],
                hover: [0x35; 3],
                border: [0x3D; 3],
                text: [0xF5; 3],
                secondary: [0xB4; 3],
                accent: [0x60, 0xCD, 0xFF],
                on_accent: [0x18; 3],
                destructive: [0xFF, 0x99, 0xA4],
            }
        } else {
            Self {
                canvas: [0xF3; 3],
                surface: [0xFF; 3],
                raised: [0xF8; 3],
                hover: [0xEC; 3],
                border: [0xD7; 3],
                text: [0x1A; 3],
                secondary: [0x58; 3],
                accent: [0x00, 0x63, 0x95],
                on_accent: [0xFF; 3],
                destructive: [0xA4, 0x26, 0x37],
            }
        }
    }
}

pub(super) fn apply_title_bar(window: HWND, dark: bool) {
    let dark = i32::from(dark);
    // SAFETY: callers supply a live HWND and the attribute takes a BOOL-sized buffer.
    unsafe {
        DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&raw const dark).cast(),
            std::mem::size_of_val(&dark) as u32,
        );
    }
}
