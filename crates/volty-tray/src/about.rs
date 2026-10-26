use std::{error::Error, ffi::c_void, io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{
        GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
    },
    Graphics::Gdi::{
        GetMonitorInfoW, GetSysColorBrush, MonitorFromPoint, COLOR_WINDOW, MONITORINFO,
        MONITOR_DEFAULTTONEAREST,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        SystemServices::{SS_ETCHEDHORZ, SS_ICON},
    },
    UI::{
        Shell::ShellExecuteW,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
            GetMessageW, GetWindowLongPtrW, PostQuitMessage, RegisterClassW, SendMessageW,
            SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, BS_PUSHBUTTON,
            CREATESTRUCTW, GWLP_USERDATA, MSG, STM_SETICON, SW_SHOWNORMAL, WM_CLOSE, WM_COMMAND,
            WM_CTLCOLORSTATIC, WM_DESTROY, WM_NCCREATE, WNDCLASSW, WS_CAPTION, WS_CHILD,
            WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
        },
    },
};

use crate::native_ui::{application_icon, set_control_font, static_control_color, FontSet};

const WINDOW_CLASS: &str = "VoltyAboutWindow";
const WINDOW_WIDTH: i32 = 500;
const WINDOW_HEIGHT: i32 = 280;
const STAR_ON_GITHUB_ID: usize = 1001;
const CLOSE_ID: usize = 1002;
const GITHUB_URL: &str = "https://github.com/PhilippPolterauer/volty-rs";

struct AboutWindow {
    status: HWND,
    create_complete: bool,
    fonts: FontSet,
}

pub fn show_about_window() -> Result<(), Box<dyn Error>> {
    let class_name = to_wide(WINDOW_CLASS);
    let title = to_wide("About Volty");
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    if instance.is_null() {
        return Err(io::Error::last_os_error().into());
    }

    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: application_icon(instance, 32)?,
        hCursor: ptr::null_mut(),
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        lpszMenuName: ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    if unsafe { RegisterClassW(&class) } == 0
        && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
    {
        return Err(io::Error::last_os_error().into());
    }

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

    if let Err(error) = unsafe { create_controls(window, instance, &mut state) } {
        unsafe { DestroyWindow(window) };
        return Err(error);
    }
    state.create_complete = true;

    unsafe {
        ShowWindow(window, SW_SHOWNORMAL);
        let mut message: MSG = mem::zeroed();
        loop {
            match GetMessageW(&mut message, ptr::null_mut(), 0, 0) {
                -1 => {
                    let error = io::Error::last_os_error();
                    state.create_complete = false;
                    DestroyWindow(window);
                    return Err(error.into());
                }
                0 => break,
                _ => {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    }
    Ok(())
}

fn centered_window_position(width: i32, height: i32) -> io::Result<(i32, i32)> {
    unsafe {
        let mut cursor = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut cursor) == 0 {
            return Err(io::Error::last_os_error());
        }

        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return Err(io::Error::last_os_error());
        }

        let mut info: MONITORINFO = mem::zeroed();
        info.cbSize = mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(center_in_work_area(&info.rcWork, width, height))
    }
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

unsafe fn create_controls(
    window: HWND,
    instance: HINSTANCE,
    state: &mut AboutWindow,
) -> Result<(), Box<dyn Error>> {
    let static_class = to_wide("STATIC");
    let button_class = to_wide("BUTTON");
    let heading_text = to_wide("Volty");
    let version_text = to_wide(&format!("VERSION {}", env!("CARGO_PKG_VERSION")));
    let description_text = to_wide(
        "A low-overhead Windows system-tray battery monitor for supported HID and Bluetooth LE devices, written in Rust.",
    );
    let status_text = to_wide("");
    let empty_text = to_wide("");
    let star_text = to_wide("Star on GitHub");
    let close_text = to_wide("Close");

    let icon = create_child(
        window,
        instance,
        &static_class,
        &empty_text,
        WS_CHILD | WS_VISIBLE | SS_ICON,
        24,
        24,
        128,
        128,
        0,
    );
    if icon.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    SendMessageW(
        icon,
        STM_SETICON,
        application_icon(instance, 128)? as usize,
        0,
    );

    let heading = create_child(
        window,
        instance,
        &static_class,
        &heading_text,
        WS_CHILD | WS_VISIBLE,
        164,
        22,
        300,
        38,
        0,
    );
    if heading.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(heading, state.fonts.heading);

    let version = create_child(
        window,
        instance,
        &static_class,
        &version_text,
        WS_CHILD | WS_VISIBLE,
        166,
        65,
        300,
        22,
        0,
    );
    if version.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(version, state.fonts.body);

    let description = create_child(
        window,
        instance,
        &static_class,
        &description_text,
        WS_CHILD | WS_VISIBLE,
        166,
        98,
        300,
        48,
        0,
    );
    if description.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(description, state.fonts.body);

    let separator_text = to_wide("");
    if create_child(
        window,
        instance,
        &static_class,
        &separator_text,
        WS_CHILD | WS_VISIBLE | SS_ETCHEDHORZ,
        24,
        184,
        436,
        2,
        0,
    )
    .is_null()
    {
        return Err(io::Error::last_os_error().into());
    }

    state.status = create_child(
        window,
        instance,
        &static_class,
        &status_text,
        WS_CHILD | WS_VISIBLE,
        30,
        154,
        436,
        22,
        0,
    );
    if state.status.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(state.status, state.fonts.body);

    for (id, text, x, width) in [
        (STAR_ON_GITHUB_ID, &star_text, 24, 160),
        (CLOSE_ID, &close_text, 376, 84),
    ] {
        let button = create_child(
            window,
            instance,
            &button_class,
            text,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            x,
            205,
            width,
            36,
            id,
        );
        if button.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        set_control_font(button, state.fonts.body);
    }

    Ok(())
}

unsafe fn create_child(
    parent: HWND,
    instance: HINSTANCE,
    class: &[u16],
    text: &[u16],
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: usize,
) -> HWND {
    CreateWindowExW(
        0,
        class.as_ptr(),
        text.as_ptr(),
        style,
        x,
        y,
        width,
        height,
        parent,
        id as *mut c_void,
        instance,
        ptr::null(),
    )
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
    }

    match message {
        WM_CTLCOLORSTATIC => static_control_color(wparam),
        WM_COMMAND => {
            let id = (wparam & 0xffff) as usize;
            match id {
                STAR_ON_GITHUB_ID => {
                    let status = match open_github() {
                        Ok(()) => "Opening GitHub...",
                        Err(error) => {
                            let message = to_wide(&format!("Could not open GitHub: {error}"));
                            let state_ptr =
                                GetWindowLongPtrW(window, GWLP_USERDATA) as *mut AboutWindow;
                            if !state_ptr.is_null() {
                                SetWindowTextW((*state_ptr).status, message.as_ptr());
                            }
                            return 0;
                        }
                    };
                    let message = to_wide(status);
                    let state_ptr = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut AboutWindow;
                    if !state_ptr.is_null() {
                        SetWindowTextW((*state_ptr).status, message.as_ptr());
                    }
                    0
                }
                CLOSE_ID => {
                    DestroyWindow(window);
                    0
                }
                _ => DefWindowProcW(window, message, wparam, lparam),
            }
        }
        WM_CLOSE => {
            DestroyWindow(window);
            0
        }
        WM_DESTROY => {
            let state_ptr = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut AboutWindow;
            if !state_ptr.is_null() && (*state_ptr).create_complete {
                PostQuitMessage(0);
            }
            0
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

fn open_github() -> Result<(), io::Error> {
    let verb = to_wide("open");
    let target = to_wide(GITHUB_URL);
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        Err(io::Error::other(format!(
            "ShellExecuteW failed with code {result}"
        )))
    } else {
        Ok(())
    }
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
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
