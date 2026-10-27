use std::{ffi::c_void, io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{
        GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
    },
    Graphics::Gdi::{GetSysColorBrush, COLOR_WINDOW},
    UI::{
        Shell::ShellExecuteW,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
            GetWindowLongPtrW, PostQuitMessage, RegisterClassW, SetWindowLongPtrW, SetWindowTextW,
            ShowWindow, TranslateMessage, BS_PUSHBUTTON, CREATESTRUCTW, GWLP_USERDATA, MSG,
            SW_SHOWNORMAL, WNDCLASSW, WS_CHILD, WS_TABSTOP, WS_VISIBLE,
        },
    },
};

use crate::native_ui::application_icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy)]
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
    if unsafe { RegisterClassW(ptr::addr_of!(class)) } == 0
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
            ptr::without_provenance_mut::<c_void>(control.id),
            instance,
            ptr::null(),
        )
    }
}

pub fn create_control_checked(
    parent: HWND,
    instance: HINSTANCE,
    control: ControlSpec<'_>,
) -> io::Result<HWND> {
    let handle = create_control(parent, instance, control);
    if handle.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(handle)
    }
}

pub fn create_static_control(
    parent: HWND,
    instance: HINSTANCE,
    text: &[u16],
    style: u32,
    rect: ControlRect,
) -> io::Result<HWND> {
    let class = to_wide("STATIC");
    create_control_checked(
        parent,
        instance,
        ControlSpec {
            class: &class,
            text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE | style,
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            id: 0,
        },
    )
}

pub fn create_button_control(
    parent: HWND,
    instance: HINSTANCE,
    text: &[u16],
    id: usize,
    rect: ControlRect,
) -> io::Result<HWND> {
    let class = to_wide("BUTTON");
    create_control_checked(
        parent,
        instance,
        ControlSpec {
            class: &class,
            text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            id,
        },
    )
}

pub unsafe fn attach_window_state(window: HWND, lparam: LPARAM) {
    // SAFETY: this helper is called only for WM_NCCREATE, whose lParam points to CREATESTRUCTW.
    let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
    unsafe {
        SetWindowLongPtrW(
            window,
            GWLP_USERDATA,
            create.lpCreateParams.expose_provenance().cast_signed(),
        )
    };
}

pub fn window_state<T>(window: HWND) -> *mut T {
    ptr::with_exposed_provenance_mut::<T>(
        unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) }.cast_unsigned(),
    )
}

pub fn set_window_text(window: HWND, text: &[u16]) {
    unsafe { SetWindowTextW(window, text.as_ptr()) };
}

pub fn run_message_loop(
    window: HWND,
    show_command: i32,
    create_complete: &mut bool,
) -> io::Result<()> {
    unsafe { ShowWindow(window, show_command) };
    let mut message: MSG = unsafe { mem::zeroed() };
    loop {
        match unsafe { GetMessageW(ptr::addr_of_mut!(message), ptr::null_mut(), 0, 0) } {
            -1 => {
                let error = io::Error::last_os_error();
                *create_complete = false;
                unsafe { DestroyWindow(window) };
                return Err(error);
            }
            0 => return Ok(()),
            _ => unsafe {
                TranslateMessage(ptr::addr_of!(message));
                DispatchMessageW(ptr::addr_of!(message));
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
            SW_SHOWNORMAL,
        )
    }
    .expose_provenance()
    .cast_signed();
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
