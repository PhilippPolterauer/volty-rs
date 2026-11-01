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
            GetWindowLongPtrW, PostQuitMessage, RegisterClassExW, SetWindowLongPtrW,
            SetWindowTextW, ShowWindow, TranslateMessage, BS_PUSHBUTTON, CREATESTRUCTW,
            GWLP_USERDATA, MSG, SW_SHOWNORMAL, WNDCLASSEXW, WS_CHILD, WS_TABSTOP, WS_VISIBLE,
        },
    },
};

use crate::app_icon::class_icons;

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
) -> io::Result<()> {
    if !is_terminated_wide(class_name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "window class name must have exactly one trailing NUL",
        ));
    }

    let (small_icon, large_icon) = class_icons(instance)?;
    let class = WNDCLASSEXW {
        cbSize: mem::size_of::<WNDCLASSEXW>()
            .try_into()
            .map_err(|_| io::Error::other("WNDCLASSEXW size exceeds u32"))?,
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: large_icon,
        hCursor: ptr::null_mut(),
        // SAFETY: COLOR_WINDOW is a valid system color index; the returned system brush is shared.
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        lpszMenuName: ptr::null(),
        lpszClassName: class_name.as_ptr(),
        hIconSm: small_icon,
    };
    // SAFETY: class and its NUL-terminated class name remain valid for this call.
    let registered = unsafe { RegisterClassExW(ptr::addr_of!(class)) };
    // SAFETY: GetLastError is read immediately after RegisterClassExW reports failure.
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

/// Runs the thread queue and retains the boxed state until the window cannot access it.
///
/// If both window destruction and state detachment fail, the state is intentionally leaked.
pub fn run_message_loop<T>(
    window: HWND,
    show_command: i32,
    mut state: Box<T>,
    set_create_complete: impl Fn(&mut T, bool),
) -> io::Result<()> {
    // SAFETY: window is the live HWND created by the caller on this thread.
    unsafe { ShowWindow(window, show_command) };
    // SAFETY: MSG is a plain Win32 output structure where every bit pattern is valid.
    let mut message: MSG = unsafe { mem::zeroed() };
    let message_error = loop {
        // SAFETY: GetMessageW writes into the valid stack slot; a null HWND requests the thread queue.
        match unsafe { GetMessageW(ptr::addr_of_mut!(message), ptr::null_mut(), 0, 0) } {
            -1 => break Some(io::Error::last_os_error()),
            0 => break None,
            // SAFETY: a positive GetMessageW result initialized message for these APIs.
            _ => unsafe {
                TranslateMessage(ptr::addr_of!(message));
                DispatchMessageW(ptr::addr_of!(message));
            },
        }
    };

    set_create_complete_on_message_error(&message_error, state.as_mut(), set_create_complete);
    let state_pointer = ptr::from_ref(state.as_ref()).cast_mut();
    match cleanup_message_loop_window(
        state,
        || window_state::<T>(window) == state_pointer,
        || destroy_window(window),
        || clear_window_state(window),
    ) {
        Ok(()) => match message_error {
            Some(error) => Err(error),
            None => Ok(()),
        },
        Err(mut cleanup_error) => {
            if let Some(state) = cleanup_error.state.take() {
                mem::forget(state);
            }
            match message_error {
                Some(error) => Err(io::Error::other(format!(
                    "{error}; {}",
                    cleanup_error.error
                ))),
                None => Err(cleanup_error.error),
            }
        }
    }
}

fn set_create_complete_on_message_error<T>(
    message_error: &Option<io::Error>,
    state: &mut T,
    set_create_complete: impl Fn(&mut T, bool),
) {
    if message_error.is_some() {
        set_create_complete(state, false);
    }
}

struct MessageLoopCleanupError<T> {
    error: io::Error,
    state: Option<Box<T>>,
}

fn cleanup_message_loop_window<T>(
    state: Box<T>,
    mut state_is_attached: impl FnMut() -> bool,
    destroy: impl FnOnce() -> io::Result<()>,
    clear_state: impl FnOnce() -> io::Result<()>,
) -> Result<(), MessageLoopCleanupError<T>> {
    if !state_is_attached() {
        return Ok(());
    }

    let Err(destroy_error) = destroy() else {
        return Ok(());
    };

    if !state_is_attached() {
        return Ok(());
    }

    match clear_state() {
        Ok(()) => Err(MessageLoopCleanupError {
            error: destroy_error,
            state: None,
        }),
        Err(clear_error) => Err(MessageLoopCleanupError {
            error: io::Error::other(format!(
                "{destroy_error}; could not detach window state: {clear_error}"
            )),
            state: Some(state),
        }),
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
    use super::{
        cleanup_message_loop_window, is_terminated_wide, set_create_complete_on_message_error,
        to_wide,
    };
    use std::{cell::Cell, io, rc::Rc};

    struct DropProbe(Rc<Cell<bool>>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    #[test]
    fn wide_strings_replace_embedded_nuls_and_append_one_terminator() {
        assert_eq!(to_wide("a\0b"), [b'a' as u16, 0xfffd, b'b' as u16, 0]);
        assert!(is_terminated_wide(&[b'a' as u16, 0]));
        assert!(!is_terminated_wide(&[]));
        assert!(!is_terminated_wide(&[b'a' as u16, 0, b'b' as u16, 0]));
    }

    #[test]
    fn create_complete_is_reset_only_after_a_message_error() {
        let mut create_complete = true;
        let no_error = None;
        set_create_complete_on_message_error(&no_error, &mut create_complete, |state, value| {
            *state = value;
        });
        assert!(create_complete);

        let message_error = Some(io::Error::other("GetMessageW failed"));
        set_create_complete_on_message_error(
            &message_error,
            &mut create_complete,
            |state, value| *state = value,
        );
        assert!(!create_complete);
    }

    #[test]
    fn failed_destroy_detaches_state_before_dropping_it() {
        let dropped = Rc::new(Cell::new(false));
        let detached = Rc::new(Cell::new(false));
        let detached_in_cleanup = Rc::clone(&detached);
        let result = cleanup_message_loop_window(
            Box::new(DropProbe(Rc::clone(&dropped))),
            || true,
            || Err(io::Error::other("destroy failed")),
            move || {
                detached_in_cleanup.set(true);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(detached.get());
        assert!(dropped.get());
    }

    #[test]
    fn already_destroyed_window_does_not_require_a_second_detach() {
        let dropped = Rc::new(Cell::new(false));
        let result = cleanup_message_loop_window(
            Box::new(DropProbe(Rc::clone(&dropped))),
            || false,
            || panic!("already-detached state should not destroy the window"),
            || panic!("state is already detached"),
        );

        assert!(result.is_ok());
        assert!(dropped.get());
    }

    #[test]
    fn failed_destroy_preserves_state_if_detaching_also_fails() {
        let dropped = Rc::new(Cell::new(false));
        let result = cleanup_message_loop_window(
            Box::new(DropProbe(Rc::clone(&dropped))),
            || true,
            || Err(io::Error::other("destroy failed")),
            || Err(io::Error::other("detach failed")),
        );

        let failure = result.expect_err("both cleanup attempts should fail");
        assert!(failure.state.is_some());
        assert!(!dropped.get());
    }
}
