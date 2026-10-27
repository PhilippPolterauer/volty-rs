use std::{io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{
        GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
    },
    Graphics::Gdi::{GetSysColorBrush, COLOR_WINDOW},
    UI::{
        Shell::ShellExecuteW,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
            GetWindowLongPtrW, PostQuitMessage, RegisterClassW, SetWindowLongPtrW, ShowWindow,
            TranslateMessage, CREATESTRUCTW, GWLP_USERDATA, MSG, WNDCLASSW,
        },
    },
};

use crate::native_ui::application_icon;

pub struct ControlSpec<'a> {
    pub class: &'a [u16],
    pub text: &'a [u16],
    pub extended_style: u32,
    pub style: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub id: usize,
}

pub fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

pub fn register_window_class(
    class_name: &[u16],
    window_proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    instance: HINSTANCE,
    icon_size: i32,
) -> io::Result<()> {
    if class_name.last() != Some(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "window class name is not null terminated",
        ));
    }

    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: application_icon(instance, icon_size)?,
        hCursor: ptr::null_mut(),
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        lpszMenuName: ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    if unsafe { RegisterClassW(&class) } == 0
        && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
    {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

pub fn create_control(parent: HWND, instance: HINSTANCE, control: ControlSpec<'_>) -> HWND {
    if control.class.last() != Some(&0) || control.text.last() != Some(&0) {
        return ptr::null_mut();
    }

    unsafe {
        CreateWindowExW(
            control.extended_style,
            control.class.as_ptr(),
            control.text.as_ptr(),
            control.style,
            control.x,
            control.y,
            control.width,
            control.height,
            parent,
            control.id as *mut _,
            instance,
            ptr::null(),
        )
    }
}

pub unsafe fn attach_window_state(window: HWND, lparam: LPARAM) {
    // SAFETY: this helper is called only for WM_NCCREATE, whose lParam points to CREATESTRUCTW.
    let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
    unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize) };
}

pub fn window_state<T>(window: HWND) -> *mut T {
    unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as *mut T }
}

pub fn run_message_loop(
    window: HWND,
    show_command: i32,
    create_complete: &mut bool,
) -> io::Result<()> {
    unsafe { ShowWindow(window, show_command) };
    let mut message: MSG = unsafe { mem::zeroed() };
    loop {
        match unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) } {
            -1 => {
                let error = io::Error::last_os_error();
                *create_complete = false;
                unsafe { DestroyWindow(window) };
                return Err(error);
            }
            0 => return Ok(()),
            _ => unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            },
        }
    }
}

pub fn open_url(url: &str) -> io::Result<()> {
    let verb = to_wide("open");
    let target = to_wide(url);
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            ptr::null(),
            ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
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

pub fn default_window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

pub fn quit_after_creation(create_complete: bool) {
    if create_complete {
        unsafe { PostQuitMessage(0) };
    }
}
