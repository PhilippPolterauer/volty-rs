use std::{error::Error, io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{GlobalFree, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        LibraryLoader::GetModuleHandleW,
        Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
        SystemServices::SS_ETCHEDHORZ,
    },
    UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetClientRect, MoveWindow, CW_USEDEFAULT, ES_AUTOVSCROLL,
        ES_MULTILINE, ES_NOHIDESEL, ES_READONLY, MINMAXINFO, SW_SHOW, WM_CLOSE, WM_COMMAND,
        WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY, WM_GETMINMAXINFO, WM_NCCREATE, WM_SIZE,
        WS_CHILD, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
    },
};

use crate::{
    native_ui::{edit_control_color, set_control_font, static_control_color, FontSet},
    win32,
};

const WINDOW_CLASS: &str = "VoltyDiagnoseWindow";
const WINDOW_WIDTH: i32 = 960;
const WINDOW_HEIGHT: i32 = 720;
const MIN_WINDOW_WIDTH: i32 = 720;
const MIN_WINDOW_HEIGHT: i32 = 560;
const COPY_LOG_ID: usize = 1001;
const OPEN_ISSUE_ID: usize = 1002;
const CLOSE_ID: usize = 1003;
const CLIPBOARD_UNICODE_TEXT: u32 = 13;
const ISSUE_URL: &str = "https://github.com/PhilippPolterauer/volty-rs/issues/new?title=Volty%20diagnostics&body=Please%20describe%20what%20happened%2C%20then%20paste%20the%20diagnostic%20log%20from%20Volty%20Diagnose.";
const CONTENT_MARGIN: i32 = 24;
const HEADING_X: i32 = 24;
const HEADING_Y: i32 = 20;
const HEADING_WIDTH: i32 = 820;
const HEADING_HEIGHT: i32 = 38;
const DESCRIPTION_X: i32 = 26;
const DESCRIPTION_Y: i32 = 64;
const DESCRIPTION_WIDTH: i32 = 820;
const DESCRIPTION_HEIGHT: i32 = 24;
const DIVIDER_X: i32 = 24;
const DIVIDER_Y: i32 = 96;
const DIVIDER_WIDTH: i32 = 900;
const DIVIDER_HEIGHT: i32 = 2;
const REPORT_TOP: i32 = 108;
const REPORT_BOTTOM_GAP: i32 = 20;
const MIN_REPORT_HEIGHT: i32 = 100;
const FOOTER_BOTTOM_OFFSET: i32 = 58;
const FOOTER_STATUS_Y_OFFSET: i32 = 4;
const STATUS_HEIGHT: i32 = 28;
const BUTTON_HEIGHT: i32 = 36;
const COPY_BUTTON_WIDTH: i32 = 110;
const ISSUE_BUTTON_WIDTH: i32 = 126;
const CLOSE_BUTTON_WIDTH: i32 = 92;
const BUTTON_GAP: i32 = 12;
const MIN_STATUS_WIDTH: i32 = 100;
const INITIAL_REPORT_HEIGHT: i32 = 500;
const INITIAL_STATUS_Y: i32 = 636;
const INITIAL_STATUS_WIDTH: i32 = 420;
const BUTTON_INITIAL_X: i32 = 0;
const BUTTON_INITIAL_Y: i32 = 0;

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

type ControlGeometry = win32::ControlRect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiagnoseGeometry {
    report: ControlGeometry,
    status: ControlGeometry,
    copy_button: ControlGeometry,
    issue_button: ControlGeometry,
    close_button: ControlGeometry,
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
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::from_mut(state.as_mut()).cast(),
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
    win32::run_message_loop(window, SW_SHOW, &mut state.create_complete)?;
    Ok(())
}

fn create_controls(
    window: HWND,
    instance: HINSTANCE,
    state: &mut DiagnoseWindow,
) -> Result<(), Box<dyn Error>> {
    create_header(window, instance, state)?;
    create_report_view(window, instance, state)?;
    create_footer_status(window, instance, state)?;
    create_action_buttons(window, instance, state)?;
    layout_controls(window, state)?;
    Ok(())
}

fn create_header(window: HWND, instance: HINSTANCE, state: &DiagnoseWindow) -> io::Result<()> {
    let heading_text = win32::to_wide("Diagnostic report");
    let description_text = win32::to_wide("Review the report before sharing.");
    let heading = win32::create_static_control(
        window,
        instance,
        &heading_text,
        WS_VISIBLE,
        win32::ControlRect {
            x: HEADING_X,
            y: HEADING_Y,
            width: HEADING_WIDTH,
            height: HEADING_HEIGHT,
        },
    )?;
    set_control_font(heading, state.fonts.heading);

    let description = win32::create_static_control(
        window,
        instance,
        &description_text,
        WS_VISIBLE,
        win32::ControlRect {
            x: DESCRIPTION_X,
            y: DESCRIPTION_Y,
            width: DESCRIPTION_WIDTH,
            height: DESCRIPTION_HEIGHT,
        },
    )?;
    set_control_font(description, state.fonts.body);

    let empty_text = win32::to_wide("");
    win32::create_static_control(
        window,
        instance,
        &empty_text,
        WS_VISIBLE | SS_ETCHEDHORZ,
        win32::ControlRect {
            x: DIVIDER_X,
            y: DIVIDER_Y,
            width: DIVIDER_WIDTH,
            height: DIVIDER_HEIGHT,
        },
    )?;
    Ok(())
}

fn create_report_view(
    window: HWND,
    instance: HINSTANCE,
    state: &mut DiagnoseWindow,
) -> io::Result<()> {
    let edit_class = win32::to_wide("EDIT");
    let edit_style = WS_CHILD
        | WS_VISIBLE
        | WS_VSCROLL
        | WS_TABSTOP
        | ES_MULTILINE as u32
        | ES_READONLY as u32
        | ES_AUTOVSCROLL as u32
        | ES_NOHIDESEL as u32;
    state.report_view = win32::create_control_checked(
        window,
        instance,
        win32::ControlSpec {
            class: &edit_class,
            text: &state.report,
            extended_style: WS_EX_CLIENTEDGE,
            style: edit_style,
            x: CONTENT_MARGIN,
            y: REPORT_TOP,
            width: WINDOW_WIDTH - CONTENT_MARGIN * 2,
            height: INITIAL_REPORT_HEIGHT,
            id: 0,
        },
    )?;
    set_control_font(state.report_view, state.fonts.monospace);
    Ok(())
}

fn create_footer_status(
    window: HWND,
    instance: HINSTANCE,
    state: &mut DiagnoseWindow,
) -> io::Result<()> {
    let status_text = win32::to_wide("Nothing is uploaded automatically.");
    state.status = win32::create_static_control(
        window,
        instance,
        &status_text,
        WS_VISIBLE,
        win32::ControlRect {
            x: CONTENT_MARGIN,
            y: INITIAL_STATUS_Y,
            width: INITIAL_STATUS_WIDTH,
            height: STATUS_HEIGHT,
        },
    )?;
    set_control_font(state.status, state.fonts.body);
    Ok(())
}

fn create_action_buttons(
    window: HWND,
    instance: HINSTANCE,
    state: &mut DiagnoseWindow,
) -> io::Result<()> {
    let copy_text = win32::to_wide("Copy Log");
    let issue_text = win32::to_wide("Open Issue");
    let close_text = win32::to_wide("Close");
    state.copy_button = win32::create_button_control(
        window,
        instance,
        &copy_text,
        COPY_LOG_ID,
        win32::ControlRect {
            x: BUTTON_INITIAL_X,
            y: BUTTON_INITIAL_Y,
            width: COPY_BUTTON_WIDTH,
            height: BUTTON_HEIGHT,
        },
    )?;
    state.issue_button = win32::create_button_control(
        window,
        instance,
        &issue_text,
        OPEN_ISSUE_ID,
        win32::ControlRect {
            x: BUTTON_INITIAL_X,
            y: BUTTON_INITIAL_Y,
            width: ISSUE_BUTTON_WIDTH,
            height: BUTTON_HEIGHT,
        },
    )?;
    state.close_button = win32::create_button_control(
        window,
        instance,
        &close_text,
        CLOSE_ID,
        win32::ControlRect {
            x: BUTTON_INITIAL_X,
            y: BUTTON_INITIAL_Y,
            width: CLOSE_BUTTON_WIDTH,
            height: BUTTON_HEIGHT,
        },
    )?;
    set_control_font(state.copy_button, state.fonts.body);
    set_control_font(state.issue_button, state.fonts.body);
    set_control_font(state.close_button, state.fonts.body);
    Ok(())
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: WM_NCCREATE supplies a CREATESTRUCTW in lParam for this window.
        unsafe { win32::attach_window_state(window, lparam) };
    }

    match message {
        WM_CTLCOLORSTATIC => static_control_color(wparam),
        WM_CTLCOLOREDIT => edit_control_color(wparam),
        WM_GETMINMAXINFO => handle_min_max_info(lparam),
        WM_SIZE => handle_resize(window),
        WM_COMMAND => handle_command(window, message, wparam, lparam),
        WM_CLOSE => destroy_window(window),
        WM_DESTROY => handle_destroy(window),
        _ => win32::default_window_proc(window, message, wparam, lparam),
    }
}

fn handle_min_max_info(lparam: LPARAM) -> LRESULT {
    // SAFETY: WM_GETMINMAXINFO provides a writable MINMAXINFO at lParam.
    let limits = unsafe { &mut *(lparam as *mut MINMAXINFO) };
    limits.ptMinTrackSize.x = MIN_WINDOW_WIDTH;
    limits.ptMinTrackSize.y = MIN_WINDOW_HEIGHT;
    0
}

fn handle_resize(window: HWND) -> LRESULT {
    let state_ptr = win32::window_state::<DiagnoseWindow>(window);
    if state_ptr.is_null() {
        return 0;
    }
    // SAFETY: GWLP_USERDATA is the boxed state for this live window.
    let state = unsafe { &*state_ptr };
    if !state.report_view.is_null() {
        if let Err(error) = layout_controls(window, state) {
            set_status_text(state, &format!("Could not resize the report view: {error}"));
        }
    }
    0
}

fn handle_command(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state_ptr = win32::window_state::<DiagnoseWindow>(window);
    if state_ptr.is_null() {
        return win32::default_window_proc(window, message, wparam, lparam);
    }
    // SAFETY: GWLP_USERDATA is the boxed state for this live window.
    let state = unsafe { &*state_ptr };
    let status = match wparam & 0xffff {
        COPY_LOG_ID => match copy_to_clipboard(window, &state.report) {
            Ok(()) => "Diagnostic log copied to the clipboard.".to_owned(),
            Err(error) => format!("Could not copy diagnostic log: {error}"),
        },
        OPEN_ISSUE_ID => match win32::open_url(ISSUE_URL) {
            Ok(()) => "Issue draft opened. Copy the log and paste it into the draft.".to_owned(),
            Err(error) => format!("Could not open GitHub issue page: {error}"),
        },
        CLOSE_ID => return destroy_window(window),
        _ => return win32::default_window_proc(window, message, wparam, lparam),
    };
    set_status_text(state, &status);
    0
}

fn set_status_text(state: &DiagnoseWindow, status: &str) {
    let text = win32::to_wide(status);
    win32::set_window_text(state.status, &text);
}

fn destroy_window(window: HWND) -> LRESULT {
    unsafe { DestroyWindow(window) };
    0
}

fn handle_destroy(window: HWND) -> LRESULT {
    let state_ptr = win32::window_state::<DiagnoseWindow>(window);
    if !state_ptr.is_null() {
        // SAFETY: GWLP_USERDATA remains the boxed state until the message loop exits.
        win32::quit_after_creation(unsafe { (*state_ptr).create_complete });
    }
    0
}

fn diagnose_geometry(width: i32, height: i32) -> DiagnoseGeometry {
    let footer_y = height - FOOTER_BOTTOM_OFFSET;
    let report_height = (footer_y - REPORT_TOP - REPORT_BOTTOM_GAP).max(MIN_REPORT_HEIGHT);
    let right = width - CONTENT_MARGIN;
    let close_x = right - CLOSE_BUTTON_WIDTH;
    let issue_x = close_x - BUTTON_GAP - ISSUE_BUTTON_WIDTH;
    let copy_x = issue_x - BUTTON_GAP - COPY_BUTTON_WIDTH;
    let status_width = (copy_x - BUTTON_GAP - CONTENT_MARGIN).max(MIN_STATUS_WIDTH);

    DiagnoseGeometry {
        report: ControlGeometry {
            x: CONTENT_MARGIN,
            y: REPORT_TOP,
            width: width - CONTENT_MARGIN * 2,
            height: report_height,
        },
        status: ControlGeometry {
            x: CONTENT_MARGIN,
            y: footer_y + FOOTER_STATUS_Y_OFFSET,
            width: status_width,
            height: STATUS_HEIGHT,
        },
        copy_button: ControlGeometry {
            x: copy_x,
            y: footer_y,
            width: COPY_BUTTON_WIDTH,
            height: BUTTON_HEIGHT,
        },
        issue_button: ControlGeometry {
            x: issue_x,
            y: footer_y,
            width: ISSUE_BUTTON_WIDTH,
            height: BUTTON_HEIGHT,
        },
        close_button: ControlGeometry {
            x: close_x,
            y: footer_y,
            width: CLOSE_BUTTON_WIDTH,
            height: BUTTON_HEIGHT,
        },
    }
}

fn layout_controls(window: HWND, state: &DiagnoseWindow) -> Result<(), io::Error> {
    let mut client = unsafe { mem::zeroed() };
    if unsafe { GetClientRect(window, ptr::addr_of_mut!(client)) } == 0 {
        return Err(io::Error::last_os_error());
    }

    let layout = diagnose_geometry(client.right - client.left, client.bottom - client.top);
    for (control, geometry) in [
        (state.report_view, layout.report),
        (state.status, layout.status),
        (state.copy_button, layout.copy_button),
        (state.issue_button, layout.issue_button),
        (state.close_button, layout.close_button),
    ] {
        if unsafe {
            MoveWindow(
                control,
                geometry.x,
                geometry.y,
                geometry.width,
                geometry.height,
                1,
            )
        } == 0
        {
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
    let destination = unsafe { GlobalLock(memory).cast::<u16>() };
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
        || unsafe { SetClipboardData(CLIPBOARD_UNICODE_TEXT, memory) }.is_null()
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

#[cfg(test)]
mod tests {
    use super::{diagnose_geometry, ControlGeometry};

    #[test]
    fn lays_out_controls_at_the_initial_client_size() {
        let layout = diagnose_geometry(960, 720);

        assert_eq!(
            layout.report,
            ControlGeometry {
                x: 24,
                y: 108,
                width: 912,
                height: 534,
            }
        );
        assert_eq!(
            layout.status,
            ControlGeometry {
                x: 24,
                y: 666,
                width: 548,
                height: 28,
            }
        );
        assert_eq!(layout.copy_button.x, 584);
        assert_eq!(layout.issue_button.x, 706);
        assert_eq!(layout.close_button.x, 844);
    }

    #[test]
    fn enforces_minimum_report_height_and_status_width() {
        let layout = diagnose_geometry(300, 100);

        assert_eq!(layout.report.height, 100);
        assert_eq!(layout.status.width, 100);
    }
}
