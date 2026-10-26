use std::{error::Error, ffi::c_void, io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{
        GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
    },
    Graphics::Gdi::{GetSysColorBrush, COLOR_WINDOW},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Shell::ShellExecuteW,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
            GetWindowLongPtrW, PostQuitMessage, RegisterClassW, SetWindowLongPtrW, SetWindowTextW,
            ShowWindow, TranslateMessage, BS_PUSHBUTTON, CREATESTRUCTW, CW_USEDEFAULT,
            GWLP_USERDATA, MSG, SW_SHOWNORMAL, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_NCCREATE,
            WNDCLASSW, WS_CAPTION, WS_CHILD, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
        },
    },
};

const WINDOW_CLASS: &str = "VoltyAboutWindow";
const STAR_ON_GITHUB_ID: usize = 1001;
const CLOSE_ID: usize = 1002;
const GITHUB_URL: &str = "https://github.com/PhilippPolterauer/volty-rs";

struct AboutWindow {
    status: HWND,
    create_complete: bool,
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
        hIcon: ptr::null_mut(),
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

    let mut state = Box::new(AboutWindow {
        status: ptr::null_mut(),
        create_complete: false,
    });
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            390,
            195,
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

unsafe fn create_controls(
    window: HWND,
    instance: HINSTANCE,
    state: &mut AboutWindow,
) -> Result<(), Box<dyn Error>> {
    let static_class = to_wide("STATIC");
    let button_class = to_wide("BUTTON");
    let version = to_wide(&format!("Volty {}", env!("CARGO_PKG_VERSION")));
    let description = to_wide("Lightweight Logitech battery monitor");
    let status_text = to_wide("");
    let star_text = to_wide("Star on GitHub");
    let close_text = to_wide("Close");

    for (text, y) in [(&version, 18), (&description, 48)] {
        if create_child(
            window,
            instance,
            &static_class,
            text,
            WS_CHILD | WS_VISIBLE,
            16,
            y,
            340,
            24,
            0,
        )
        .is_null()
        {
            return Err(io::Error::last_os_error().into());
        }
    }

    state.status = create_child(
        window,
        instance,
        &static_class,
        &status_text,
        WS_CHILD | WS_VISIBLE,
        16,
        82,
        340,
        24,
        0,
    );
    if state.status.is_null() {
        return Err(io::Error::last_os_error().into());
    }

    for (id, text, x, width) in [
        (STAR_ON_GITHUB_ID, &star_text, 16, 145),
        (CLOSE_ID, &close_text, 276, 80),
    ] {
        let button = create_child(
            window,
            instance,
            &button_class,
            text,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            x,
            118,
            width,
            30,
            id,
        );
        if button.is_null() {
            return Err(io::Error::last_os_error().into());
        }
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
