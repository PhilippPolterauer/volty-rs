use std::{ffi::c_void, io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{
        GetLastError, SetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT,
        WPARAM,
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
    value
        .encode_utf16()
        .map(|unit| if unit == 0 { 0xfffd } else { unit })
        .chain(Some(0))
        .collect()
}

fn is_terminated_wide(value: &[u16]) -> bool {
    value.last() == Some(&0) && !value[..value.len() - 1].contains(&0)
}

pub fn register_window_class(
    class_name: &[u16],
    window_proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    instance: HINSTANCE,
    icon_size: i32,
) -> io::Result<()> {
    if !is_terminated_wide(class_name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "window class name must have exactly one trailing NUL",
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
        // SAFETY: COLOR_WINDOW is a valid system color index; the returned system brush is shared.
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        lpszMenuName: ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    // SAFETY: class and its NUL-terminated class name remain valid for this call.
    let registered = unsafe { RegisterClassW(ptr::addr_of!(class)) };
    // SAFETY: GetLastError is read immediately after RegisterClassW reports failure.
    if registered == 0 && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

pub fn create_control(
    parent: HWND,
    instance: HINSTANCE,
    control: ControlSpec<'_>,
) -> io::Result<HWND> {
    if !is_terminated_wide(control.class) || !is_terminated_wide(control.text) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "control class and text must have exactly one trailing NUL",
        ));
    }

    // SAFETY: both slices are NUL-terminated and live through the call; parent and instance are valid handles.
    let handle = unsafe {
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
    };
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
    create_control(
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
    create_control(
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

/// Stores the state pointer supplied during a window's creation.
///
/// # Safety
/// Must be called only for WM_NCCREATE with its valid CREATESTRUCTW lParam.
pub unsafe fn attach_window_state(window: HWND, lparam: LPARAM) -> bool {
    // SAFETY: WM_NCCREATE supplies a live CREATESTRUCTW in lParam; its state pointer remains
    // owned by the creator for this window's lifetime, and the HWND is being created now.
    unsafe {
        let create = &*ptr::with_exposed_provenance::<CREATESTRUCTW>(lparam.cast_unsigned());
        SetLastError(0);
        SetWindowLongPtrW(
            window,
            GWLP_USERDATA,
            create.lpCreateParams.expose_provenance().cast_signed(),
        );
        GetLastError() == 0
    }
}

pub fn window_state<T>(window: HWND) -> *mut T {
    ptr::with_exposed_provenance_mut::<T>(
        // SAFETY: callers query user data only on a live HWND created and owned by this module.
        unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) }.cast_unsigned(),
    )
}

pub fn destroy_window(window: HWND) -> io::Result<()> {
    // SAFETY: callers pass a live window owned by this GUI thread; destruction is synchronous.
    if unsafe { DestroyWindow(window) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn clear_window_state(window: HWND) -> io::Result<()> {
    // SAFETY: this live window was created by our code; GWLP_USERDATA is its integer state slot.
    unsafe {
        SetLastError(0);
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        if GetLastError() == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

pub fn set_window_text(window: HWND, text: &[u16]) -> io::Result<()> {
    if !is_terminated_wide(text) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "window text must have exactly one trailing NUL",
        ));
    }
    // SAFETY: text is NUL-terminated and lives for this call; callers provide a live HWND.
    if unsafe { SetWindowTextW(window, text.as_ptr()) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn run_message_loop(
    window: HWND,
    show_command: i32,
    create_complete: &mut bool,
) -> io::Result<()> {
    // SAFETY: window is the live HWND created by the caller on this thread.
    unsafe { ShowWindow(window, show_command) };
    // SAFETY: MSG is a plain Win32 output structure where every bit pattern is valid.
    let mut message: MSG = unsafe { mem::zeroed() };
    loop {
        // SAFETY: GetMessageW writes into the valid stack slot; a null HWND requests the thread queue.
        match unsafe { GetMessageW(ptr::addr_of_mut!(message), ptr::null_mut(), 0, 0) } {
            -1 => {
                let error = io::Error::last_os_error();
                *create_complete = false;
                if let Err(destroy_error) = destroy_window(window) {
                    if let Err(clear_error) = clear_window_state(window) {
                        eprintln!("Volty could not detach window state after destruction failed: {clear_error}");
                    }
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{error}; could not destroy window: {destroy_error}"),
                    ));
                }
                return Err(error);
            }
            0 => return Ok(()),
            // SAFETY: a positive GetMessageW result initialized message for these APIs.
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
    // SAFETY: verb and target are NUL-terminated buffers that remain live through ShellExecuteW.
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
    // SAFETY: these are the parameters supplied to this window's system callback.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

pub fn quit_after_creation(create_complete: bool) {
    if create_complete {
        // SAFETY: PostQuitMessage targets the current thread and takes no pointers.
        unsafe { PostQuitMessage(0) };
    }
}

#[cfg(test)]
mod tests {
    use super::{is_terminated_wide, to_wide};

    #[test]
    fn wide_strings_replace_embedded_nuls_and_append_one_terminator() {
        assert_eq!(to_wide("a\0b"), [b'a' as u16, 0xfffd, b'b' as u16, 0]);
        assert!(is_terminated_wide(&[b'a' as u16, 0]));
        assert!(!is_terminated_wide(&[]));
        assert!(!is_terminated_wide(&[b'a' as u16, 0, b'b' as u16, 0]));
    }
}
