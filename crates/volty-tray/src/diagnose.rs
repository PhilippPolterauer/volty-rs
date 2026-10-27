use std::{error::Error, io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{GlobalFree, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        LibraryLoader::GetModuleHandleW,
        Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
        SystemServices::SS_ETCHEDHORZ,
    },
    UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetClientRect, MoveWindow, SetWindowTextW, BS_PUSHBUTTON,
        CW_USEDEFAULT, ES_AUTOVSCROLL, ES_MULTILINE, ES_NOHIDESEL, ES_READONLY, MINMAXINFO,
        SW_SHOW, WM_CLOSE, WM_COMMAND, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY,
        WM_GETMINMAXINFO, WM_NCCREATE, WM_SIZE, WS_CHILD, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW,
        WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
    },
};

use crate::{
    native_ui::{edit_control_color, set_control_font, static_control_color, FontSet},
    win32::{self, ControlSpec},
};

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
    let class_name = win32::to_wide(WINDOW_CLASS);
    let title = win32::to_wide("Volty Diagnose");
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    if instance.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    win32::register_window_class(&class_name, window_proc, instance, 32)?;

    let fonts = FontSet::new()?;
    let mut state = Box::new(DiagnoseWindow {
        report: win32::to_wide(report),
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

    let controls_result = create_controls(window, instance, &mut state);
    if let Err(error) = controls_result {
        unsafe { DestroyWindow(window) };
        return Err(error);
    }
    state.create_complete = true;
    win32::run_message_loop(window, SW_SHOW, &mut state.create_complete)?;
    Ok(())
}

fn create_controls(
    window: HWND,
    instance: HINSTANCE,
    state: &mut DiagnoseWindow,
) -> Result<(), Box<dyn Error>> {
    let static_class = win32::to_wide("STATIC");
    let edit_class = win32::to_wide("EDIT");
    let button_class = win32::to_wide("BUTTON");
    let heading_text = win32::to_wide("Diagnostic report");
    let description_text = win32::to_wide("Review the report before sharing.");
    let status_text = win32::to_wide("Nothing is uploaded automatically.");
    let copy_text = win32::to_wide("Copy Log");
    let issue_text = win32::to_wide("Open Issue");
    let close_text = win32::to_wide("Close");

    let heading = win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &static_class,
            text: &heading_text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE,
            x: 24,
            y: 20,
            width: 820,
            height: 38,
            id: 0,
        },
    );
    if heading.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(heading, state.fonts.heading);

    let description = win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &static_class,
            text: &description_text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE,
            x: 26,
            y: 64,
            width: 820,
            height: 24,
            id: 0,
        },
    );
    if description.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(description, state.fonts.body);

    let empty_text = win32::to_wide("");
    if win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &static_class,
            text: &empty_text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE | SS_ETCHEDHORZ,
            x: 24,
            y: 96,
            width: 900,
            height: 2,
            id: 0,
        },
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
    state.report_view = win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &edit_class,
            text: &state.report,
            extended_style: WS_EX_CLIENTEDGE,
            style: edit_style,
            x: 24,
            y: 108,
            width: 900,
            height: 500,
            id: 0,
        },
    );
    if state.report_view.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    set_control_font(state.report_view, state.fonts.monospace);

    state.status = win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &static_class,
            text: &status_text,
            extended_style: 0,
            style: WS_CHILD | WS_VISIBLE,
            x: 24,
            y: 636,
            width: 420,
            height: 28,
            id: 0,
        },
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
        let button = win32::create_control(
            window,
            instance,
            ControlSpec {
                class: &button_class,
                text,
                extended_style: 0,
                style: WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
                x: 0,
                y: 0,
                width,
                height: 36,
                id,
            },
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
        WM_CTLCOLOREDIT => edit_control_color(wparam),
        WM_GETMINMAXINFO => {
            // SAFETY: WM_GETMINMAXINFO supplies a pointer to a writable MINMAXINFO structure.
            let limits = unsafe { &mut *(lparam as *mut MINMAXINFO) };
            limits.ptMinTrackSize.x = 720;
            limits.ptMinTrackSize.y = 560;
            0
        }
        WM_SIZE => {
            let state_ptr = win32::window_state::<DiagnoseWindow>(window);
            if !state_ptr.is_null() {
                // SAFETY: the window owns the boxed state until its message loop exits.
                let state = unsafe { &*state_ptr };
                if !state.report_view.is_null() {
                    if let Err(error) = layout_controls(window, state) {
                        let status =
                            win32::to_wide(&format!("Could not resize the report view: {error}"));
                        unsafe { SetWindowTextW(state.status, status.as_ptr()) };
                    }
                }
            }
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xffff;
            let state_ptr = win32::window_state::<DiagnoseWindow>(window);
            if state_ptr.is_null() {
                return win32::default_window_proc(window, message, wparam, lparam);
            }
            // SAFETY: the window owns the boxed state until its message loop exits.
            let state = unsafe { &mut *state_ptr };
            let status = match id {
                COPY_LOG_ID => match copy_to_clipboard(window, &state.report) {
                    Ok(()) => "Diagnostic log copied to the clipboard.".to_owned(),
                    Err(error) => format!("Could not copy diagnostic log: {error}"),
                },
                OPEN_ISSUE_ID => match win32::open_url(ISSUE_URL) {
                    Ok(()) => {
                        "Issue draft opened. Copy the log and paste it into the draft.".to_owned()
                    }
                    Err(error) => format!("Could not open GitHub issue page: {error}"),
                },
                CLOSE_ID => {
                    unsafe { DestroyWindow(window) };
                    return 0;
                }
                _ => return win32::default_window_proc(window, message, wparam, lparam),
            };
            let status = win32::to_wide(&status);
            unsafe { SetWindowTextW(state.status, status.as_ptr()) };
            0
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(window) };
            0
        }
        WM_DESTROY => {
            let state_ptr = win32::window_state::<DiagnoseWindow>(window);
            if !state_ptr.is_null() {
                // SAFETY: the window owns the boxed state until its message loop exits.
                win32::quit_after_creation((*state_ptr).create_complete);
            }
            0
        }
        _ => win32::default_window_proc(window, message, wparam, lparam),
    }
}

fn layout_controls(window: HWND, state: &DiagnoseWindow) -> Result<(), io::Error> {
    let mut client = unsafe { mem::zeroed() };
    if unsafe { GetClientRect(window, &mut client) } == 0 {
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
        if unsafe { MoveWindow(control, x, y, control_width, control_height, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }

    Ok(())
}

fn copy_to_clipboard(window: HWND, report: &[u16]) -> Result<(), io::Error> {
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, mem::size_of_val(report)) };
    if memory.is_null() {
        return Err(io::Error::last_os_error());
    }
    let destination = unsafe { GlobalLock(memory) as *mut u16 };
    if destination.is_null() {
        unsafe { GlobalFree(memory) };
        return Err(io::Error::last_os_error());
    }
    // SAFETY: GlobalLock returned a writable buffer at least as large as the report slice.
    unsafe { ptr::copy_nonoverlapping(report.as_ptr(), destination, report.len()) };
    unsafe { GlobalUnlock(memory) };

    if unsafe { OpenClipboard(window) } == 0 {
        unsafe { GlobalFree(memory) };
        return Err(io::Error::last_os_error());
    }

    let set_result = if unsafe { EmptyClipboard() } == 0
        || unsafe { SetClipboardData(CLIPBOARD_UNICODE_TEXT, memory as HANDLE) }.is_null()
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    let close_succeeded = unsafe { CloseClipboard() } != 0;

    if set_result.is_err() {
        unsafe { GlobalFree(memory) };
    }
    set_result?;
    if !close_succeeded {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}
