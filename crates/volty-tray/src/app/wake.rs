//! Lets background threads wake the tray loop so it can block instead of polling.

use std::{io, sync::OnceLock, time::Duration};
use windows_sys::Win32::{
    Foundation::{HANDLE, WAIT_FAILED},
    System::Threading::{CreateEventW, SetEvent, INFINITE},
    UI::WindowsAndMessaging::{MsgWaitForMultipleObjects, QS_ALLINPUT},
};

struct WakeEvent(HANDLE);

// SAFETY: an event handle is a process-wide kernel object that any thread may signal or wait on.
unsafe impl Send for WakeEvent {}
// SAFETY: see the `Send` impl; SetEvent and waiting are thread-safe operations.
unsafe impl Sync for WakeEvent {}

static WAKE_EVENT: OnceLock<WakeEvent> = OnceLock::new();

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
pub(super) fn wait(timeout: Option<Duration>) -> io::Result<()> {
    let Some(event) = WAKE_EVENT.get() else {
        return Err(io::Error::other("wake event was not initialized"));
    };
    let milliseconds = timeout.map_or(INFINITE, |timeout| {
        // Round up so a sub-millisecond remainder does not become a busy loop.
        let millis = timeout.as_millis().saturating_add(1);
        u32::try_from(millis).map_or(INFINITE - 1, |millis| millis.min(INFINITE - 1))
    });
    // SAFETY: one valid event handle is passed with a matching count.
    let result = unsafe { MsgWaitForMultipleObjects(1, &event.0, 0, milliseconds, QS_ALLINPUT) };
    if result == WAIT_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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
        wait(None).unwrap();
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
            wait(Some(remaining)).unwrap();
        }
    }
}
