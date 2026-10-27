use std::{error::Error, io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    System::LibraryLoader::GetModuleHandleW,
    System::SystemServices::{SS_ETCHEDHORZ, SS_ICON},
    UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetCursorPos, SendMessageW, SetWindowTextW, BS_PUSHBUTTON,
        STM_SETICON, SW_SHOWNORMAL, WM_CLOSE, WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY,
        WM_NCCREATE, WS_CAPTION, WS_CHILD, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
    },
};

use crate::{
    native_ui::{application_icon, set_control_font, static_control_color, FontSet},
    win32::{self, ControlSpec},
};

const WINDOW_CLASS: &str = "VoltyAboutWindow";
const WINDOW_WIDTH: i32 = 500;
const WINDOW_HEIGHT: i32 = 312;
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
            (&mut *state as *mut AboutWindow).cast(),
        )
    };
    if window.is_null() {
        return Err(io::Error::last_os_error().into());
    }

    if let Err(error) = create_controls(window, instance, &mut state) {
        unsafe { DestroyWindow(window) };
        return Err(error);
    }
    state.create_complete = true;
    win32::run_message_loop(window, SW_SHOWNORMAL, &mut state.create_complete)?;
    Ok(())
}

fn centered_window_position(width: i32, height: i32) -> io::Result<(i32, i32)> {
    let mut cursor = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut cursor) } == 0 {
        return Err(io::Error::last_os_error());
    }

    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: MONITORINFO is a plain Win32 structure whose fields are initialized by GetMonitorInfoW.
    let mut info: MONITORINFO = unsafe { mem::zeroed() };
    info.cbSize = mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(center_in_work_area(&info.rcWork, width, height))
}
fn center_in_work_area(
    work_area: &windows_sys::Win32::Foundation::RECT,
    width: i32,
    height: i32,
) -> (i32, i32) {
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
    let static_class = win32::to_wide("STATIC");
    let button_class = win32::to_wide("BUTTON");
    let heading_text = win32::to_wide("Volty");
    let version_text = win32::to_wide(&format!("VERSION {}", env!("CARGO_PKG_VERSION")));
    let description_text = win32::to_wide(
        "A low-overhead Windows system-tray battery monitor for supported HID devices, written in Rust.",
    );
    let status_text = win32::to_wide("");
    let empty_text = win32::to_wide("");
    let star_text = win32::to_wide("Star on GitHub");
    let close_text = win32::to_wide("Close");

    let icon = win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &static_class,
            text: &empty_text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE | SS_ICON,
            x: 24,
            y: 24,
            width: 128,
            height: 128,
            id: 0,
        },
    );
    if icon.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    unsafe {
        SendMessageW(
            icon,
            STM_SETICON,
            application_icon(instance, 128)? as usize,
            0,
        );
    }

    let heading = win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &static_class,
            text: &heading_text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE,
            x: 164,
            y: 22,
            width: 300,
            height: 38,
            id: 0,
        },
    );
    if heading.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(heading, state.fonts.heading);

    let version = win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &static_class,
            text: &version_text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE,
            x: 166,
            y: 65,
            width: 300,
            height: 22,
            id: 0,
        },
    );
    if version.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(version, state.fonts.body_small);

    let description = win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &static_class,
            text: &description_text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE,
            x: 166,
            y: 98,
            width: 310,
            height: 56,
            id: 0,
        },
    );
    if description.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(description, state.fonts.body_small);

    if win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &static_class,
            text: &empty_text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE | SS_ETCHEDHORZ,
            x: 24,
            y: 188,
            width: 436,
            height: 2,
            id: 0,
        },
    )
    .is_null()
    {
        return Err(io::Error::last_os_error().into());
    }

    state.status = win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &static_class,
            text: &status_text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE,
            x: 30,
            y: 158,
            width: 436,
            height: 20,
            id: 0,
        },
    );
    if state.status.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(state.status, state.fonts.body_small);

    for (id, text, x, width) in [
        (STAR_ON_GITHUB_ID, &star_text, 24, 144),
        (CLOSE_ID, &close_text, 384, 76),
    ] {
        let button = win32::create_control(
            window,
            instance,
            ControlSpec {
                class: &button_class,
                text,
                extended_style: 0,
                style: WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
                x,
                y: 220,
                width,
                height: 32,
                id,
            },
        );
        if button.is_null() {
            return Err(io::Error::last_os_error().into());
        }
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
        unsafe { win32::attach_window_state(window, lparam) };
    }

    match message {
        WM_CTLCOLORSTATIC => static_control_color(wparam),
        WM_COMMAND => {
            let id = wparam & 0xffff;
            match id {
                STAR_ON_GITHUB_ID => {
                    let status = match win32::open_url(GITHUB_URL) {
                        Ok(()) => "Opening GitHub...",
                        Err(error) => {
                            let message =
                                win32::to_wide(&format!("Could not open GitHub: {error}"));
                            let state_ptr = win32::window_state::<AboutWindow>(window);
                            if !state_ptr.is_null() {
                                // SAFETY: the window owns the boxed state until its message loop exits.
                                unsafe { SetWindowTextW((*state_ptr).status, message.as_ptr()) };
                            }
                            return 0;
                        }
                    };
                    let message = win32::to_wide(status);
                    let state_ptr = win32::window_state::<AboutWindow>(window);
                    if !state_ptr.is_null() {
                        // SAFETY: the window owns the boxed state until its message loop exits.
                        unsafe { SetWindowTextW((*state_ptr).status, message.as_ptr()) };
                    }
                    0
                }
                CLOSE_ID => {
                    unsafe { DestroyWindow(window) };
                    0
                }
                _ => win32::default_window_proc(window, message, wparam, lparam),
            }
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(window) };
            0
        }
        WM_DESTROY => {
            let state_ptr = win32::window_state::<AboutWindow>(window);
            if !state_ptr.is_null() {
                // SAFETY: the window owns the boxed state until its message loop exits.
                win32::quit_after_creation(unsafe { (*state_ptr).create_complete });
            }
            0
        }
        _ => win32::default_window_proc(window, message, wparam, lparam),
    }
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
