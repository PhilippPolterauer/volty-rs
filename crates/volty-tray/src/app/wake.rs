//! Lets background threads wake the tray loop so it can block instead of polling.

use std::{
    io,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        OnceLock,
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, HWND, WAIT_FAILED, WAIT_OBJECT_0},
    System::Threading::{CreateEventW, OpenEventW, SetEvent, EVENT_MODIFY_STATE, INFINITE},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            MsgWaitForMultipleObjects, DBT_DEVNODES_CHANGED, PBT_APMRESUMEAUTOMATIC, QS_ALLINPUT,
            WM_DEVICECHANGE, WM_NCDESTROY, WM_POWERBROADCAST, WM_SETTINGCHANGE, WM_SYSCOLORCHANGE,
            WM_THEMECHANGED,
        },
    },
};

struct WakeEvent(HANDLE);

// SAFETY: an event handle is a process-wide kernel object that any thread may signal or wait on.
unsafe impl Send for WakeEvent {}
// SAFETY: see the `Send` impl; SetEvent and waiting are thread-safe operations.
unsafe impl Sync for WakeEvent {}

static WAKE_EVENT: OnceLock<WakeEvent> = OnceLock::new();
static CTRL_C_REQUESTED: AtomicBool = AtomicBool::new(false);
static SYSTEM_EVENTS: AtomicU8 = AtomicU8::new(0);
pub(super) const REDISCOVER: u8 = 1;
pub(super) const APPEARANCE: u8 = 2;
const PREFERENCES_CHANGED: &str = "Local\\VoltyMonitorPreferencesChanged";

pub(super) fn listen_to_system(window: HWND) -> io::Result<()> {
    // SAFETY: this is called by the owning tray thread and the callback has static lifetime.
    if unsafe { SetWindowSubclass(window, Some(system_message), 1, 0) } == 0 {
        return Err(io::Error::other(
            "Could not subscribe to tray system messages",
        ));
    }
    Ok(())
}

pub(super) fn take_system_events() -> u8 {
    SYSTEM_EVENTS.swap(0, Ordering::AcqRel)
}

unsafe extern "system" fn system_message(
    window: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    id: usize,
    _: usize,
) -> isize {
    let event = match message {
        WM_DEVICECHANGE if wparam == DBT_DEVNODES_CHANGED as usize => REDISCOVER,
        WM_POWERBROADCAST if wparam == PBT_APMRESUMEAUTOMATIC as usize => REDISCOVER,
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED => APPEARANCE,
        WM_NCDESTROY => {
            // SAFETY: this removes the callback from the owning thread during destruction.
            unsafe { RemoveWindowSubclass(window, Some(system_message), id) };
            0
        }
        _ => 0,
    };
    if event != 0 {
        SYSTEM_EVENTS.fetch_or(event, Ordering::Release);
        signal();
    }
    // SAFETY: forwards the unmodified window message.
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

pub(crate) fn notify_preferences() {
    let name = crate::win32::to_wide(PREFERENCES_CHANGED);
    // SAFETY: the terminated name remains valid and only signal access is requested.
    let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if !handle.is_null() {
        // SAFETY: handle belongs to this call and is valid for signaling.
        unsafe {
            SetEvent(handle);
            CloseHandle(handle);
        }
    }
}

pub(super) struct PreferencesChanged(HANDLE);

impl PreferencesChanged {
    pub(super) fn new() -> io::Result<Self> {
        let name = crate::win32::to_wide(PREFERENCES_CHANGED);
        // SAFETY: the name is terminated and this creates an auto-reset event.
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for PreferencesChanged {
    fn drop(&mut self) {
        // SAFETY: the handle belongs to this instance.
        unsafe { CloseHandle(self.0) };
    }
}

pub(super) struct ConsoleControlHandler;

impl Drop for ConsoleControlHandler {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

        // SAFETY: this unregisters the static callback installed by `install_console_control_handler`.
        if unsafe { SetConsoleCtrlHandler(Some(handle_console_control), 0) } == 0 {
            eprintln!(
                "Volty could not unregister its Ctrl+C handler: {}",
                io::Error::last_os_error()
            );
        }
    }
}

pub(super) fn install_console_control_handler() -> io::Result<ConsoleControlHandler> {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    CTRL_C_REQUESTED.store(false, Ordering::Release);
    // SAFETY: the callback is static and only updates atomics and signals the wake event.
    if unsafe { SetConsoleCtrlHandler(Some(handle_console_control), 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ConsoleControlHandler)
}

pub(super) fn take_ctrl_c_request() -> bool {
    CTRL_C_REQUESTED.swap(false, Ordering::AcqRel)
}

unsafe extern "system" fn handle_console_control(control: u32) -> i32 {
    use windows_sys::Win32::System::Console::CTRL_C_EVENT;

    if control != CTRL_C_EVENT {
        return 0;
    }
    CTRL_C_REQUESTED.store(true, Ordering::Release);
    signal();
    1
}

/// Creates the auto-reset event; must run before the first [`wait`].
pub(super) fn init() -> io::Result<()> {
    if WAKE_EVENT.get().is_some() {
        return Ok(());
    }
    // SAFETY: null attributes and name request an unnamed auto-reset, initially unsignaled event.
    let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let _ = WAKE_EVENT.set(WakeEvent(handle));
    Ok(())
}

/// Wakes the tray loop. A no-op before [`init`], so it is safe to call from any thread.
pub(crate) fn signal() {
    if let Some(event) = WAKE_EVENT.get() {
        // SAFETY: the handle was created by `init` and is never closed.
        unsafe { SetEvent(event.0) };
    }
}

/// Blocks until [`signal`] is called, a window message arrives, or `timeout` elapses.
pub(super) fn wait(
    timeout: Option<Duration>,
    preferences: Option<&PreferencesChanged>,
) -> io::Result<bool> {
    let Some(event) = WAKE_EVENT.get() else {
        return Err(io::Error::other("wake event was not initialized"));
    };
    let milliseconds = timeout.map_or(INFINITE, |timeout| {
        // Round up so a sub-millisecond remainder does not become a busy loop.
        let millis = timeout.as_millis().saturating_add(1);
        u32::try_from(millis).map_or(INFINITE - 1, |millis| millis.min(INFINITE - 1))
    });
    let handles = [
        event.0,
        preferences.map_or(std::ptr::null_mut(), |event| event.0),
    ];
    let count = if preferences.is_some() { 2 } else { 1 };
    // SAFETY: the first count handles are live, and the array stays in scope throughout the wait.
    let result =
        unsafe { MsgWaitForMultipleObjects(count, handles.as_ptr(), 0, milliseconds, QS_ALLINPUT) };
    if result == WAIT_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(count == 2 && result == WAIT_OBJECT_0 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Mutex, time::Instant};

    // The wake event is process-wide, so the tests must not overlap.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn signal_from_another_thread_wakes_an_infinite_wait() {
        let _guard = SERIAL.lock().unwrap();
        init().unwrap();
        let signaler = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(50));
            signal();
        });
        let started = Instant::now();
        wait(None, None).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        signaler.join().unwrap();
    }

    #[test]
    fn wait_returns_once_the_timeout_elapses() {
        let _guard = SERIAL.lock().unwrap();
        init().unwrap();
        // Other tests may signal the shared event, so tolerate spurious wakeups.
        let deadline = Instant::now() + Duration::from_millis(60);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            wait(Some(remaining), None).unwrap();
        }
    }
}
