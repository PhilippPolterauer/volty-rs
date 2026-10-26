use std::{error::Error, ffi::c_void, io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{
        GetLastError, GlobalFree, ERROR_CLASS_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND, LPARAM,
        LRESULT, WPARAM,
    },
    Graphics::Gdi::{GetSysColorBrush, COLOR_WINDOW},
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        LibraryLoader::GetModuleHandleW,
        Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
    },
    UI::{
        Shell::ShellExecuteW,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
            GetWindowLongPtrW, PostQuitMessage, RegisterClassW, SetWindowLongPtrW, SetWindowTextW,
            ShowWindow, TranslateMessage, BS_PUSHBUTTON, CREATESTRUCTW, CW_USEDEFAULT,
            ES_AUTOVSCROLL, ES_MULTILINE, ES_NOHIDESEL, ES_READONLY, GWLP_USERDATA, MSG,
            SW_SHOWNORMAL, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_NCCREATE, WNDCLASSW, WS_CHILD,
            WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
        },
    },
};

const WINDOW_CLASS: &str = "VoltyDiagnoseWindow";
const COPY_LOG_ID: usize = 1001;
const OPEN_ISSUE_ID: usize = 1002;
const CLOSE_ID: usize = 1003;
const CLIPBOARD_UNICODE_TEXT: u32 = 13;
const ISSUE_URL: &str = "https://github.com/PhilippPolterauer/volty-rs/issues/new?title=Volty%20diagnostics&body=Please%20describe%20what%20happened%2C%20then%20paste%20the%20diagnostic%20log%20from%20Volty%20Diagnose.";

struct DiagnoseWindow {
    report: Vec<u16>,
    status: HWND,
    create_complete: bool,
}

pub fn show_diagnose_window(report: &str) -> Result<(), Box<dyn Error>> {
    let class_name = to_wide(WINDOW_CLASS);
    let title = to_wide("Volty Diagnose");
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

    let mut state = Box::new(DiagnoseWindow {
        report: to_wide(report),
        status: ptr::null_mut(),
        create_complete: false,
    });
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            800,
            600,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            (&mut *state as *mut DiagnoseWindow).cast(),
        )
    };
    if window.is_null() {
        return Err(io::Error::last_os_error().into());
    }

    let controls_result = unsafe { create_controls(window, instance, &mut state) };
    if let Err(error) = controls_result {
        unsafe { DestroyWindow(window) };
        return Err(error);
    }
    state.create_complete = true;

    unsafe {
        ShowWindow(window, windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW);
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
    state: &mut DiagnoseWindow,
) -> Result<(), Box<dyn Error>> {
    let static_class = to_wide("STATIC");
    let edit_class = to_wide("EDIT");
    let button_class = to_wide("BUTTON");
    let log_label = to_wide("Diagnostic log — review before sharing");
    let status_text = to_wide("Nothing is uploaded automatically.");
    let copy_text = to_wide("Copy Log");
    let issue_text = to_wide("Open Issue");
    let close_text = to_wide("Close");

    if create_child(
        window,
        instance,
        &static_class,
        &log_label,
        0,
        WS_CHILD | WS_VISIBLE,
        12,
        10,
        760,
        24,
        0,
    )
    .is_null()
    {
        return Err(io::Error::last_os_error().into());
    }

    let edit_style = WS_CHILD
        | WS_VISIBLE
        | WS_VSCROLL
        | WS_TABSTOP
        | ES_MULTILINE as u32
        | ES_READONLY as u32
        | ES_AUTOVSCROLL as u32
        | ES_NOHIDESEL as u32;
    if create_child(
        window,
        instance,
        &edit_class,
        &state.report,
        WS_EX_CLIENTEDGE,
        edit_style,
        12,
        36,
        760,
        450,
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
        0,
        WS_CHILD | WS_VISIBLE,
        12,
        496,
        450,
        28,
        0,
    );
    if state.status.is_null() {
        return Err(io::Error::last_os_error().into());
    }

    for (id, text, x) in [
        (COPY_LOG_ID, &copy_text, 450),
        (OPEN_ISSUE_ID, &issue_text, 555),
        (CLOSE_ID, &close_text, 675),
    ] {
        let button = create_child(
            window,
            instance,
            &button_class,
            text,
            0,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            x,
            496,
            if id == OPEN_ISSUE_ID {
                110
            } else if id == CLOSE_ID {
                90
            } else {
                100
            },
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
    extended_style: u32,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: usize,
) -> HWND {
    CreateWindowExW(
        extended_style,
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
            let state_ptr = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut DiagnoseWindow;
            if state_ptr.is_null() {
                return DefWindowProcW(window, message, wparam, lparam);
            }
            let state = &mut *state_ptr;
            let status = match id {
                COPY_LOG_ID => match copy_to_clipboard(window, &state.report) {
                    Ok(()) => "Diagnostic log copied to the clipboard.".to_owned(),
                    Err(error) => format!("Could not copy diagnostic log: {error}"),
                },
                OPEN_ISSUE_ID => match open_issue() {
                    Ok(()) => {
                        "Issue draft opened. Copy the log and paste it into the draft.".to_owned()
                    }
                    Err(error) => format!("Could not open GitHub issue page: {error}"),
                },
                CLOSE_ID => {
                    DestroyWindow(window);
                    return 0;
                }
                _ => return DefWindowProcW(window, message, wparam, lparam),
            };
            let status = to_wide(&status);
            SetWindowTextW(state.status, status.as_ptr());
            0
        }
        WM_CLOSE => {
            DestroyWindow(window);
            0
        }
        WM_DESTROY => {
            let state_ptr = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut DiagnoseWindow;
            if !state_ptr.is_null() && (*state_ptr).create_complete {
                PostQuitMessage(0);
            }
            0
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

unsafe fn copy_to_clipboard(window: HWND, report: &[u16]) -> Result<(), io::Error> {
    let bytes = report.len() * mem::size_of::<u16>();
    let memory = GlobalAlloc(GMEM_MOVEABLE, bytes);
    if memory.is_null() {
        return Err(io::Error::last_os_error());
    }
    let destination = GlobalLock(memory) as *mut u16;
    if destination.is_null() {
        GlobalFree(memory);
        return Err(io::Error::last_os_error());
    }
    ptr::copy_nonoverlapping(report.as_ptr(), destination, report.len());
    GlobalUnlock(memory);

    if OpenClipboard(window) == 0 {
        GlobalFree(memory);
        return Err(io::Error::last_os_error());
    }

    let mut transferred = false;
    let mut error = None;
    if EmptyClipboard() == 0 {
        error = Some(io::Error::last_os_error());
    } else if SetClipboardData(CLIPBOARD_UNICODE_TEXT, memory as HANDLE).is_null() {
        error = Some(io::Error::last_os_error());
    } else {
        transferred = true;
    }
    let close_succeeded = CloseClipboard() != 0;

    if !transferred {
        GlobalFree(memory);
    }
    match error {
        Some(error) => Err(error),
        None if !close_succeeded => Err(io::Error::last_os_error()),
        None => Ok(()),
    }
}

fn open_issue() -> Result<(), io::Error> {
    let verb = to_wide("open");
    let target = to_wide(ISSUE_URL);
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
