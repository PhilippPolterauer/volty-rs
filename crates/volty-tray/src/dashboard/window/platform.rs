use std::{
    io,
    sync::{mpsc, Arc},
    thread::{self, JoinHandle},
};

use eframe::egui::Context;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE, HWND, SYSTEMTIME, WAIT_FAILED, WAIT_OBJECT_0},
    System::{
        Threading::{
            CreateEventW, OpenEventW, SetEvent, WaitForMultipleObjects, INFINITE,
            SYNCHRONIZATION_SYNCHRONIZE,
        },
        Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTimeEx},
    },
    UI::Controls::Dialogs::{
        CommDlgExtendedError, GetSaveFileNameW, OFN_EXPLORER, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    },
};

use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, ptr};

use crate::win32::to_wide;

pub(super) enum Signal {
    Activate,
    Shutdown,
}

struct Event(HANDLE);

// SAFETY: Win32 event handles support signaling and waiting from any thread.
// The wrapper closes the handle only after its last owner drops it.
unsafe impl Send for Event {}
// SAFETY: shared access only exposes the handle to thread-safe Win32 event operations.
unsafe impl Sync for Event {}

impl Event {
    fn handle(&self) -> HANDLE {
        self.0
    }

    fn open(name: &str) -> io::Result<Self> {
        let name = to_wide(name);
        // SAFETY: the name is NUL-terminated and valid for the duration of the call.
        Self::from_handle(unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) })
    }

    fn from_handle(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the valid handle.
        unsafe { CloseHandle(self.0) };
    }
}

pub(super) struct WindowSignals {
    pub(super) receive: mpsc::Receiver<Signal>,
    stop: Arc<Event>,
    listener: Option<JoinHandle<()>>,
}

impl WindowSignals {
    pub(super) fn start(ctx: Context) -> io::Result<Self> {
        let activation = Event::open(crate::DASHBOARD_ACTIVATION_EVENT)?;
        let shutdown = Event::open(crate::DASHBOARD_SHUTDOWN_EVENT)?;
        // SAFETY: null pointers request default security attributes and an unnamed event.
        let stop = Arc::new(Event::from_handle(unsafe {
            CreateEventW(ptr::null(), 1, 0, ptr::null())
        })?);
        let thread_stop = Arc::clone(&stop);
        let (send, receive) = mpsc::channel();
        let listener = thread::Builder::new()
            .name("volty-dashboard-events".to_owned())
            .spawn(move || {
                let handles = [thread_stop.handle(), shutdown.handle(), activation.handle()];
                loop {
                    // SAFETY: the three event handles stay valid for the lifetime of this thread.
                    match unsafe {
                        WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, INFINITE)
                    } {
                        WAIT_OBJECT_0 => break,
                        result if result == WAIT_OBJECT_0 + 1 => {
                            let _ = send.send(Signal::Shutdown);
                            ctx.request_repaint();
                            break;
                        }
                        result if result == WAIT_OBJECT_0 + 2 => {
                            if send.send(Signal::Activate).is_err() {
                                break;
                            }
                            ctx.request_repaint();
                        }
                        WAIT_FAILED => {
                            eprintln!(
                                "Dashboard event wait failed: {}",
                                io::Error::last_os_error()
                            );
                            break;
                        }
                        result => {
                            eprintln!("Unexpected dashboard event wait result: {result}");
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            receive,
            stop,
            listener: Some(listener),
        })
    }
}

impl Drop for WindowSignals {
    fn drop(&mut self) {
        // SAFETY: stop remains valid until both the window and the listener release it.
        if unsafe { SetEvent(self.stop.0) } != 0 {
            if let Some(listener) = self.listener.take() {
                let _ = listener.join();
            }
        }
    }
}

pub(super) fn save_path(frame: &eframe::Frame) -> io::Result<Option<PathBuf>> {
    let handle = frame.window_handle().map_err(io::Error::other)?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err(io::Error::other("expected a Win32 dashboard window"));
    };
    let mut filename = [0_u16; 32768];
    let default = to_wide("volty-battery-history.csv");
    filename[..default.len()].copy_from_slice(&default);
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: handle.hwnd.get() as HWND,
        lpstrFile: filename.as_mut_ptr(),
        nMaxFile: filename.len() as u32,
        Flags: OFN_EXPLORER | OFN_NOCHANGEDIR | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    let filter: Vec<u16> = "CSV files\0*.csv\0\0".encode_utf16().collect();
    let extension = to_wide("csv");
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrDefExt = extension.as_ptr();
    // SAFETY: the owner is live and the dialog, buffers, and strings remain valid until it returns.
    if unsafe { GetSaveFileNameW(&mut dialog) } != 0 {
        let length = filename
            .iter()
            .position(|&ch| ch == 0)
            .unwrap_or(filename.len());
        return Ok(Some(OsString::from_wide(&filename[..length]).into()));
    }
    // SAFETY: this retrieves the status of the common dialog that just returned.
    match unsafe { CommDlgExtendedError() } {
        0 => Ok(None),
        code => Err(io::Error::other(format!(
            "Save dialog failed (0x{code:04X})"
        ))),
    }
}

pub(super) fn date(timestamp: i64) -> String {
    let ticks = timestamp
        .checked_add(11_644_473_600)
        .and_then(|seconds| u64::try_from(seconds).ok())
        .and_then(|seconds| seconds.checked_mul(10_000_000));
    let Some(ticks) = ticks else {
        return "—".to_owned();
    };
    let file_time = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    // SAFETY: both calls take initialized input and write to valid, separate output structures.
    let time = unsafe {
        if FileTimeToSystemTime(&file_time, &mut utc) == 0 {
            return "—".to_owned();
        }
        if SystemTimeToTzSpecificLocalTimeEx(ptr::null(), &utc, &mut local) != 0 {
            local
        } else {
            utc
        }
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute
    )
}
