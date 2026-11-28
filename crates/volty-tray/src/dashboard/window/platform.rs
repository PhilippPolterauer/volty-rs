use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, TryLockError,
    },
    thread,
};

use eframe::egui::{self, Context};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE, HWND, SYSTEMTIME, WAIT_FAILED, WAIT_OBJECT_0},
    System::{
        Threading::{
            CreateEventW, OpenEventW, SetEvent, WaitForMultipleObjects, EVENT_MODIFY_STATE,
            INFINITE, SYNCHRONIZATION_SYNCHRONIZE,
        },
        Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTimeEx},
    },
    UI::Controls::Dialogs::{
        CommDlgExtendedError, GetSaveFileNameW, OFN_EXPLORER, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    },
};

use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, ptr};

use super::super::{
    design::{apply_title_bar, Palette},
    live::{self, LiveDevice},
};
use crate::{
    config::{AppConfig, AppTheme},
    win32::to_wide,
};

pub(super) fn hwnd(window: &impl HasWindowHandle) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}

#[derive(Default)]
pub(super) struct SystemChanges {
    pub(super) appearance: AtomicBool,
    pub(super) resume: AtomicBool,
}

pub(super) fn listen_to_system(window: HWND, ctx: Context) -> io::Result<Arc<SystemChanges>> {
    use windows_sys::Win32::UI::Shell::SetWindowSubclass;
    let changes = Arc::new(SystemChanges::default());
    let state = Box::into_raw(Box::new((ctx, Arc::clone(&changes))));
    // SAFETY: installation runs on the window's owning thread. WM_NCDESTROY reclaims state.
    if unsafe { SetWindowSubclass(window, Some(system_message), 1, state.expose_provenance()) } == 0
    {
        // SAFETY: failed installation leaves ownership with this function.
        drop(unsafe { Box::from_raw(state) });
        return Err(io::Error::other(
            "Could not subscribe to dashboard system messages",
        ));
    }
    Ok(changes)
}

unsafe extern "system" fn system_message(
    window: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    id: usize,
    data: usize,
) -> isize {
    use windows_sys::Win32::UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass},
        WindowsAndMessaging::{
            PBT_APMRESUMEAUTOMATIC, WM_NCDESTROY, WM_POWERBROADCAST, WM_SETTINGCHANGE,
            WM_SYSCOLORCHANGE, WM_THEMECHANGED,
        },
    };
    let state = ptr::with_exposed_provenance_mut::<(Context, Arc<SystemChanges>)>(data);
    match message {
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED | WM_POWERBROADCAST => {
            // SAFETY: the subclass owns this state until WM_NCDESTROY on this thread.
            let (ctx, changes) = unsafe { &*state };
            if message != WM_POWERBROADCAST {
                changes.appearance.store(true, Ordering::Release);
                ctx.request_repaint();
            } else if wparam == PBT_APMRESUMEAUTOMATIC as usize {
                changes.resume.store(true, Ordering::Release);
                ctx.request_repaint();
            }
        }
        WM_NCDESTROY => {
            // SAFETY: remove the installed callback before releasing its uniquely owned state.
            unsafe {
                RemoveWindowSubclass(window, Some(system_message), id);
                drop(Box::from_raw(state));
            }
        }
        _ => {}
    }
    // SAFETY: this forwards the unmodified message through the native subclass chain.
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

pub(super) fn update_title_bar(
    window: &impl HasWindowHandle,
    theme: egui::Theme,
    contrast: Option<Palette>,
) {
    if let Some(window) = hwnd(window) {
        apply_title_bar(window, contrast.is_none() && theme == egui::Theme::Dark);
    }
}

const DATA_CHANGED: &str = "Local\\VoltyDashboardDataChanged";
const LIVE_CHANGED: &str = "Local\\VoltyLiveDevicesChanged";
const APPEARANCE_CHANGED: &str = "Local\\VoltyDashboardAppearanceChanged";

pub(crate) fn notify_dashboard() {
    signal_event(DATA_CHANGED);
}

pub(crate) fn notify_live() {
    signal_event(LIVE_CHANGED);
}

pub(crate) fn notify_appearance() {
    signal_event(APPEARANCE_CHANGED);
}

fn signal_event(name: &str) {
    let name = to_wide(name);
    // SAFETY: the name is NUL-terminated and only signaling access is requested.
    let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if let Ok(event) = Event::from_handle(handle) {
        // SAFETY: the event is live and opened with modify access.
        unsafe { SetEvent(event.handle()) };
    }
}

pub(super) enum Signal {
    Activate,
    Shutdown,
    Appearance(AppTheme),
}

pub(super) struct LiveUpdate(pub(super) Option<Vec<LiveDevice>>);

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
    changed: Arc<AtomicBool>,
    live: Arc<Mutex<Option<LiveUpdate>>>,
}

impl WindowSignals {
    pub(super) fn start(ctx: Context) -> io::Result<Self> {
        let activation = Event::open(crate::DASHBOARD_ACTIVATION_EVENT)?;
        let shutdown = Event::open(crate::DASHBOARD_SHUTDOWN_EVENT)?;
        let named_event = |name| {
            let name = to_wide(name);
            // SAFETY: name is terminated and the remaining arguments select an auto-reset event.
            Event::from_handle(unsafe { CreateEventW(ptr::null(), 0, 0, name.as_ptr()) })
        };
        let changed_event = named_event(DATA_CHANGED)?;
        let live_event = named_event(LIVE_CHANGED)?;
        let appearance_event = named_event(APPEARANCE_CHANGED)?;
        // SAFETY: null pointers request default security attributes and an unnamed event.
        let stop = Arc::new(Event::from_handle(unsafe {
            CreateEventW(ptr::null(), 1, 0, ptr::null())
        })?);
        let thread_stop = Arc::clone(&stop);
        let changed = Arc::new(AtomicBool::new(false));
        let thread_changed = Arc::clone(&changed);
        let live = Arc::new(Mutex::new(None));
        let thread_live = Arc::clone(&live);
        let (send, receive) = mpsc::channel();
        thread::Builder::new()
            .name("volty-dashboard-events".to_owned())
            .spawn(move || {
                let update_live = || {
                    let state = live::read();
                    *thread_live.lock().unwrap() = Some(LiveUpdate(state));
                    ctx.request_repaint();
                };
                let update_appearance = || {
                    if let Ok(config) = AppConfig::load() {
                        let _ = send.send(Signal::Appearance(config.app_theme));
                        ctx.request_repaint();
                    }
                };
                update_live();
                update_appearance();
                let handles = [
                    thread_stop.handle(),
                    shutdown.handle(),
                    activation.handle(),
                    changed_event.handle(),
                    live_event.handle(),
                    appearance_event.handle(),
                ];
                loop {
                    // SAFETY: all event handles stay valid for the lifetime of this thread.
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
                            update_live();
                            update_appearance();
                        }
                        result if result == WAIT_OBJECT_0 + 3 => {
                            if !thread_changed.swap(true, Ordering::AcqRel) {
                                ctx.request_repaint();
                            }
                        }
                        result if result == WAIT_OBJECT_0 + 4 => update_live(),
                        result if result == WAIT_OBJECT_0 + 5 => update_appearance(),
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
            changed,
            live,
        })
    }

    pub(super) fn take_change(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    pub(super) fn take_live(&self, ctx: &Context) -> Option<LiveUpdate> {
        match self.live.try_lock() {
            Ok(mut live) => live.take(),
            Err(TryLockError::WouldBlock) => {
                ctx.request_repaint();
                None
            }
            Err(TryLockError::Poisoned(_)) => None,
        }
    }
}

impl Drop for WindowSignals {
    fn drop(&mut self) {
        // SAFETY: stop remains valid until both the window and the listener release it.
        unsafe { SetEvent(self.stop.0) };
    }
}

pub(super) fn save_path(frame: &eframe::Frame) -> io::Result<Option<PathBuf>> {
    let window =
        hwnd(frame).ok_or_else(|| io::Error::other("expected a Win32 dashboard window"))?;
    let mut filename = [0_u16; 32768];
    let default = to_wide("volty-battery-history.csv");
    filename[..default.len()].copy_from_slice(&default);
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: window,
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
    format_date(timestamp, false)
}

pub(super) fn exact_date(timestamp: i64) -> String {
    format_date(timestamp, true)
}

fn format_date(timestamp: i64, with_seconds: bool) -> String {
    let Some(time) = local_time(timestamp) else {
        return "—".to_owned();
    };
    let mut date = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute
    );
    if with_seconds {
        use std::fmt::Write;
        let _ = write!(date, ":{:02}", time.wSecond);
    }
    date
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub(super) fn summary_date(timestamp: i64) -> String {
    let Some(time) = local_time(timestamp) else {
        return "—".to_owned();
    };
    format!(
        "{} {}, {:02}:{:02}",
        time.wDay,
        MONTHS[usize::from(time.wMonth - 1)],
        time.wHour,
        time.wMinute
    )
}

pub(super) fn chart_date(timestamp: i64, short_range: bool) -> String {
    let Some(time) = local_time(timestamp) else {
        return "—".to_owned();
    };
    if short_range {
        return format!("{:02}:{:02}", time.wHour, time.wMinute);
    }
    format!("{} {}", time.wDay, MONTHS[usize::from(time.wMonth - 1)])
}

fn local_time(timestamp: i64) -> Option<SYSTEMTIME> {
    let ticks = timestamp
        .checked_add(11_644_473_600)
        .and_then(|seconds| u64::try_from(seconds).ok())
        .and_then(|seconds| seconds.checked_mul(10_000_000));
    let ticks = ticks?;
    let file_time = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    // SAFETY: both calls take initialized input and write to valid, separate output structures.
    let time = unsafe {
        if FileTimeToSystemTime(&file_time, &mut utc) == 0 {
            return None;
        }
        if SystemTimeToTzSpecificLocalTimeEx(ptr::null(), &utc, &mut local) != 0 {
            local
        } else {
            utc
        }
    };
    Some(time)
}
