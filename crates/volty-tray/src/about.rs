use std::{error::Error, io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    System::LibraryLoader::GetModuleHandleW,
    System::SystemServices::{SS_ETCHEDHORZ, SS_ICON},
    UI::WindowsAndMessaging::{
        CreateWindowExW, GetCursorPos, SendMessageW, STM_SETICON, SW_SHOWNORMAL, WM_CLOSE,
        WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY, WM_NCCREATE, WS_CAPTION, WS_OVERLAPPED,
        WS_SYSMENU, WS_VISIBLE,
    },
};

use crate::{
    native_ui::{application_icon, set_control_font, static_control_color, FontSet},
    win32,
};

const WINDOW_CLASS: &str = "VoltyAboutWindow";
const WINDOW_WIDTH: i32 = 500;
const WINDOW_HEIGHT: i32 = 312;
const ICON_SIZE: i32 = 128;
const ICON_X: i32 = 24;
const ICON_Y: i32 = 24;
const HEADING_X: i32 = 164;
const HEADING_Y: i32 = 22;
const HEADING_WIDTH: i32 = 300;
const HEADING_HEIGHT: i32 = 38;
const VERSION_X: i32 = 166;
const VERSION_Y: i32 = 65;
const VERSION_WIDTH: i32 = 300;
const VERSION_HEIGHT: i32 = 22;
const DESCRIPTION_X: i32 = 166;
const DESCRIPTION_Y: i32 = 98;
const DESCRIPTION_WIDTH: i32 = 310;
const DESCRIPTION_HEIGHT: i32 = 56;
const DIVIDER_X: i32 = 24;
const DIVIDER_Y: i32 = 188;
const DIVIDER_WIDTH: i32 = 436;
const DIVIDER_HEIGHT: i32 = 2;
const STATUS_X: i32 = 30;
const STATUS_Y: i32 = 158;
const STATUS_WIDTH: i32 = 436;
const STATUS_HEIGHT: i32 = 20;
const STAR_BUTTON_X: i32 = 24;
const STAR_BUTTON_WIDTH: i32 = 144;
const CLOSE_BUTTON_X: i32 = 384;
const CLOSE_BUTTON_WIDTH: i32 = 76;
const BUTTON_Y: i32 = 220;
const BUTTON_HEIGHT: i32 = 32;
const STAR_ON_GITHUB_ID: usize = 1001;
const CLOSE_ID: usize = 1002;
const GITHUB_URL: &str = "https://github.com/PhilippPolterauer/volty-rs";

struct AboutWindow {
    status: HWND,
    create_complete: bool,
    fonts: FontSet,
}

pub fn show_about_window() -> Result<(), Box<dyn Error>> {
    let class_name = win32::to_wide(WINDOW_CLASS);
    let title = win32::to_wide("About Volty");
    // SAFETY: a null module name requests the current process executable's module handle.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    if instance.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    win32::register_window_class(&class_name, window_proc, instance, 32)?;

    let (window_x, window_y) = centered_window_position(WINDOW_WIDTH, WINDOW_HEIGHT)?;
    let fonts = FontSet::new()?;
    let mut state = Box::new(AboutWindow {
        status: ptr::null_mut(),
        create_complete: false,
        fonts,
    });
    // SAFETY: class/title are NUL-terminated and live through creation; the Box-backed state pointer
    // remains valid for every synchronous creation callback and until the window is destroyed.
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            window_x,
            window_y,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::from_mut(state.as_mut()).cast(),
        )
    };
    if window.is_null() {
        return Err(io::Error::last_os_error().into());
    }

    if let Err(error) = create_controls(window, instance, &mut state) {
        if let Err(destroy_error) = win32::destroy_window(window) {
            if let Err(clear_error) = win32::clear_window_state(window) {
                eprintln!("Volty could not detach About-window state: {clear_error}");
            }
            eprintln!("Volty could not destroy the incomplete About window: {destroy_error}");
        }
        return Err(error);
    }
    state.create_complete = true;
    win32::run_message_loop(window, SW_SHOWNORMAL, &mut state.create_complete)?;
    Ok(())
}

fn centered_window_position(width: i32, height: i32) -> io::Result<(i32, i32)> {
    let mut cursor = POINT { x: 0, y: 0 };
    // SAFETY: cursor is a writable, initialized POINT on this stack frame.
    if unsafe { GetCursorPos(ptr::addr_of_mut!(cursor)) } == 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: cursor is a valid POINT value; MonitorFromPoint does not retain its address.
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return Err(io::Error::other(
            "Windows did not return a monitor for the cursor position",
        ));
    }

    // SAFETY: MONITORINFO is a plain Win32 structure whose fields GetMonitorInfoW initializes.
    let mut info: MONITORINFO = unsafe { mem::zeroed() };
    info.cbSize = mem::size_of::<MONITORINFO>()
        .try_into()
        .map_err(|_| io::Error::other("MONITORINFO size exceeds u32"))?;
    // SAFETY: monitor is returned by MonitorFromPoint and info has its required cbSize.
    if unsafe { GetMonitorInfoW(monitor, ptr::addr_of_mut!(info)) } == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(center_in_work_area(&info.rcWork, width, height))
}

fn center_in_work_area(work_area: &RECT, width: i32, height: i32) -> (i32, i32) {
    (
        work_area.left + (work_area.right - work_area.left - width) / 2,
        work_area.top + (work_area.bottom - work_area.top - height) / 2,
    )
}

fn create_controls(
    window: HWND,
    instance: HINSTANCE,
    state: &mut AboutWindow,
) -> Result<(), Box<dyn Error>> {
    create_about_content(window, instance, state)?;
    create_about_buttons(window, instance, state)?;
    Ok(())
}

fn create_about_content(
    window: HWND,
    instance: HINSTANCE,
    state: &mut AboutWindow,
) -> Result<(), Box<dyn Error>> {
    create_about_icon(window, instance)?;
    create_about_text(window, instance, state)?;
    create_about_divider(window, instance)?;
    create_about_status(window, instance, state)?;
    Ok(())
}

fn create_about_icon(window: HWND, instance: HINSTANCE) -> io::Result<()> {
    let empty_text = win32::to_wide("");
    let icon = win32::create_static_control(
        window,
        instance,
        &empty_text,
        WS_VISIBLE | SS_ICON,
        win32::ControlRect {
            x: ICON_X,
            y: ICON_Y,
            width: ICON_SIZE,
            height: ICON_SIZE,
        },
    )?;
    // SAFETY: the icon control is valid and STM_SETICON accepts the shared application icon handle.
    unsafe {
        SendMessageW(
            icon,
            STM_SETICON,
            application_icon(instance, ICON_SIZE)?.expose_provenance(),
            0,
        );
    }
    Ok(())
}

fn create_about_text(window: HWND, instance: HINSTANCE, state: &AboutWindow) -> io::Result<()> {
    let heading = win32::to_wide("Volty");
    let version = win32::to_wide(&format!("VERSION {}", env!("CARGO_PKG_VERSION")));
    let description = win32::to_wide(
        "A low-overhead Windows system-tray battery monitor for supported HID devices, written in Rust.",
    );

    let control = win32::create_static_control(
        window,
        instance,
        &heading,
        WS_VISIBLE,
        win32::ControlRect {
            x: HEADING_X,
            y: HEADING_Y,
            width: HEADING_WIDTH,
            height: HEADING_HEIGHT,
        },
    )?;
    set_control_font(control, state.fonts.heading);

    let control = win32::create_static_control(
        window,
        instance,
        &version,
        WS_VISIBLE,
        win32::ControlRect {
            x: VERSION_X,
            y: VERSION_Y,
            width: VERSION_WIDTH,
            height: VERSION_HEIGHT,
        },
    )?;
    set_control_font(control, state.fonts.body_small);

    let control = win32::create_static_control(
        window,
        instance,
        &description,
        WS_VISIBLE,
        win32::ControlRect {
            x: DESCRIPTION_X,
            y: DESCRIPTION_Y,
            width: DESCRIPTION_WIDTH,
            height: DESCRIPTION_HEIGHT,
        },
    )?;
    set_control_font(control, state.fonts.body_small);
    Ok(())
}

fn create_about_divider(window: HWND, instance: HINSTANCE) -> io::Result<()> {
    let empty_text = win32::to_wide("");
    win32::create_static_control(
        window,
        instance,
        &empty_text,
        WS_VISIBLE | SS_ETCHEDHORZ,
        win32::ControlRect {
            x: DIVIDER_X,
            y: DIVIDER_Y,
            width: DIVIDER_WIDTH,
            height: DIVIDER_HEIGHT,
        },
    )?;
    Ok(())
}

fn create_about_status(
    window: HWND,
    instance: HINSTANCE,
    state: &mut AboutWindow,
) -> io::Result<()> {
    let status_text = win32::to_wide("");
    state.status = win32::create_static_control(
        window,
        instance,
        &status_text,
        WS_VISIBLE,
        win32::ControlRect {
            x: STATUS_X,
            y: STATUS_Y,
            width: STATUS_WIDTH,
            height: STATUS_HEIGHT,
        },
    )?;
    set_control_font(state.status, state.fonts.body_small);
    Ok(())
}

fn create_about_buttons(window: HWND, instance: HINSTANCE, state: &AboutWindow) -> io::Result<()> {
    let star_text = win32::to_wide("Star on GitHub");
    let close_text = win32::to_wide("Close");
    for (id, text, x, width) in [
        (
            STAR_ON_GITHUB_ID,
            &star_text,
            STAR_BUTTON_X,
            STAR_BUTTON_WIDTH,
        ),
        (CLOSE_ID, &close_text, CLOSE_BUTTON_X, CLOSE_BUTTON_WIDTH),
    ] {
        let button = win32::create_button_control(
            window,
            instance,
            text,
            id,
            win32::ControlRect {
                x,
                y: BUTTON_Y,
                width,
                height: BUTTON_HEIGHT,
            },
        )?;
        set_control_font(button, state.fonts.body_small);
    }
    Ok(())
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: WM_NCCREATE supplies this window's valid CREATESTRUCTW in lParam.
        if !unsafe { win32::attach_window_state(window, lparam) } {
            return 0;
        }
    }

    match message {
        WM_CTLCOLORSTATIC => static_control_color(wparam),
        WM_COMMAND => handle_command(window, message, wparam, lparam),
        WM_CLOSE => destroy_window(window),
        WM_DESTROY => handle_destroy(window),
        _ => win32::default_window_proc(window, message, wparam, lparam),
    }
}

fn handle_command(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match wparam & 0xffff {
        STAR_ON_GITHUB_ID => {
            let status = match win32::open_url(GITHUB_URL) {
                Ok(()) => "Opening GitHub...".to_owned(),
                Err(error) => format!("Could not open GitHub: {error}"),
            };
            set_status_text(window, &status);
            0
        }
        CLOSE_ID => destroy_window(window),
        _ => win32::default_window_proc(window, message, wparam, lparam),
    }
}

fn set_status_text(window: HWND, status: &str) {
    let state_ptr = win32::window_state::<AboutWindow>(window);
    if state_ptr.is_null() {
        return;
    }
    let text = win32::to_wide(status);
    // SAFETY: GWLP_USERDATA is the boxed state for this live window.
    let state = unsafe { &*state_ptr };
    if let Err(error) = win32::set_window_text(state.status, &text) {
        eprintln!("Volty could not update the About status: {error}");
    }
}

fn destroy_window(window: HWND) -> LRESULT {
    if let Err(error) = win32::destroy_window(window) {
        eprintln!("Volty could not close the About window: {error}");
    }
    0
}

fn handle_destroy(window: HWND) -> LRESULT {
    let state_ptr = win32::window_state::<AboutWindow>(window);
    if !state_ptr.is_null() {
        // SAFETY: GWLP_USERDATA remains the boxed state until the message loop exits.
        win32::quit_after_creation(unsafe { (*state_ptr).create_complete });
    }
    0
}

#[cfg(test)]
mod tests {
    use super::center_in_work_area;
    use windows_sys::Win32::Foundation::RECT;

    #[test]
    fn centers_window_in_work_area_with_negative_monitor_coordinates() {
        let work_area = RECT {
            left: -1920,
            top: 40,
            right: 0,
            bottom: 1040,
        };

        assert_eq!(center_in_work_area(&work_area, 500, 280), (-1210, 400));
    }
}
