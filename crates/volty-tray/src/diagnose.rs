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
        SystemServices::SS_ETCHEDHORZ,
    },
    UI::{
        Shell::ShellExecuteW,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
            GetMessageW, GetWindowLongPtrW, MoveWindow, PostQuitMessage, RegisterClassW,
            SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage, BS_PUSHBUTTON,
            CREATESTRUCTW, CW_USEDEFAULT, ES_AUTOVSCROLL, ES_MULTILINE, ES_NOHIDESEL, ES_READONLY,
            GWLP_USERDATA, MINMAXINFO, MSG, SW_SHOWNORMAL, WM_CLOSE, WM_COMMAND, WM_CTLCOLOREDIT,
            WM_CTLCOLORSTATIC, WM_DESTROY, WM_GETMINMAXINFO, WM_NCCREATE, WM_SIZE, WNDCLASSW,
            WS_CHILD, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
        },
    },
};

use crate::native_ui::{edit_control_color, set_control_font, static_control_color, FontSet};

const WINDOW_CLASS: &str = "VoltyDiagnoseWindow";
const COPY_LOG_ID: usize = 1001;
const OPEN_ISSUE_ID: usize = 1002;
const CLOSE_ID: usize = 1003;
const CLIPBOARD_UNICODE_TEXT: u32 = 13;
const ISSUE_URL: &str = "https://github.com/PhilippPolterauer/volty-rs/issues/new?title=Volty%20diagnostics&body=Please%20describe%20what%20happened%2C%20then%20paste%20the%20diagnostic%20log%20from%20Volty%20Diagnose.";

struct DiagnoseWindow {
    report: Vec<u16>,
    report_view: HWND,
    status: HWND,
    copy_button: HWND,
    issue_button: HWND,
    close_button: HWND,
    create_complete: bool,
    fonts: FontSet,
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

    let fonts = FontSet::new()?;
    let mut state = Box::new(DiagnoseWindow {
        report: to_wide(report),
        report_view: ptr::null_mut(),
        status: ptr::null_mut(),
        copy_button: ptr::null_mut(),
        issue_button: ptr::null_mut(),
        close_button: ptr::null_mut(),
        create_complete: false,
        fonts,
    });
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            960,
            720,
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
    let heading_text = to_wide("Diagnostic report");
    let description_text = to_wide("Review the report before sharing.");
    let status_text = to_wide("Nothing is uploaded automatically.");
    let copy_text = to_wide("Copy Log");
    let issue_text = to_wide("Open Issue");
    let close_text = to_wide("Close");

    let heading = create_child(
        window,
        instance,
        &static_class,
        &heading_text,
        0,
        WS_CHILD | WS_VISIBLE,
        24,
        20,
        820,
        38,
        0,
    );
    if heading.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(heading, state.fonts.heading);

    let description = create_child(
        window,
        instance,
        &static_class,
        &description_text,
        0,
        WS_CHILD | WS_VISIBLE,
        26,
        64,
        820,
        24,
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
        0,
        WS_CHILD | WS_VISIBLE | SS_ETCHEDHORZ,
        24,
        96,
        900,
        2,
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
    state.report_view = create_child(
        window,
        instance,
        &edit_class,
        &state.report,
        WS_EX_CLIENTEDGE,
        edit_style,
        24,
        108,
        900,
        500,
        0,
    );
    if state.report_view.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(state.report_view, state.fonts.monospace);

    state.status = create_child(
        window,
        instance,
        &static_class,
        &status_text,
        0,
        WS_CHILD | WS_VISIBLE,
        24,
        636,
        420,
        28,
        0,
    );
    if state.status.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(state.status, state.fonts.body);

    for (id, text) in [
        (COPY_LOG_ID, &copy_text),
        (OPEN_ISSUE_ID, &issue_text),
        (CLOSE_ID, &close_text),
    ] {
        let width = if id == OPEN_ISSUE_ID {
            126
        } else if id == CLOSE_ID {
            92
        } else {
            110
        };
        let button = create_child(
            window,
            instance,
            &button_class,
            text,
            0,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            0,
            0,
            width,
            36,
            id,
        );
        if button.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        set_control_font(button, state.fonts.body);
        match id {
            COPY_LOG_ID => state.copy_button = button,
            OPEN_ISSUE_ID => state.issue_button = button,
            CLOSE_ID => state.close_button = button,
            _ => unreachable!(),
        }
    }

    layout_controls(window, state)?;
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
        WM_CTLCOLORSTATIC => static_control_color(wparam),
        WM_CTLCOLOREDIT => edit_control_color(wparam),
        WM_GETMINMAXINFO => {
            let limits = &mut *(lparam as *mut MINMAXINFO);
            limits.ptMinTrackSize.x = 720;
            limits.ptMinTrackSize.y = 560;
            0
        }
        WM_SIZE => {
            let state_ptr = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut DiagnoseWindow;
            if !state_ptr.is_null() && !(*state_ptr).report_view.is_null() {
                if let Err(error) = layout_controls(window, &*state_ptr) {
                    let status = to_wide(&format!("Could not resize the report view: {error}"));
                    SetWindowTextW((*state_ptr).status, status.as_ptr());
                }
            }
            0
        }
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

unsafe fn layout_controls(window: HWND, state: &DiagnoseWindow) -> Result<(), io::Error> {
    let mut client = mem::zeroed();
    if GetClientRect(window, &mut client) == 0 {
        return Err(io::Error::last_os_error());
    }

    let width = client.right - client.left;
    let height = client.bottom - client.top;
    let margin = 24;
    let footer_y = height - 58;
    let report_y = 108;
    let report_height = (footer_y - report_y - 20).max(100);

    let close_width = 92;
    let issue_width = 126;
    let copy_width = 110;
    let gap = 12;
    let right = width - margin;
    let close_x = right - close_width;
    let issue_x = close_x - gap - issue_width;
    let copy_x = issue_x - gap - copy_width;
    let status_width = (copy_x - gap - margin).max(100);

    for (control, x, y, control_width, control_height) in [
        (
            state.report_view,
            margin,
            report_y,
            width - margin * 2,
            report_height,
        ),
        (state.status, margin, footer_y + 4, status_width, 28),
        (state.copy_button, copy_x, footer_y, copy_width, 36),
        (state.issue_button, issue_x, footer_y, issue_width, 36),
        (state.close_button, close_x, footer_y, close_width, 36),
    ] {
        if MoveWindow(control, x, y, control_width, control_height, 1) == 0 {
            return Err(io::Error::last_os_error());
        }
    }

    Ok(())
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
