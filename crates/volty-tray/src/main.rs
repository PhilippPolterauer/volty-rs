#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod about;
#[cfg(windows)]
mod app;
#[cfg(windows)]
mod app_icon;
#[cfg(windows)]
mod capture;
mod config;
#[cfg(windows)]
mod dashboard;
#[cfg(windows)]
mod diagnose;
mod hid_cli;
#[cfg(windows)]
mod history;
#[cfg(windows)]
mod logging;
#[cfg(windows)]
mod native_ui;
#[cfg(windows)]
mod notify;
#[cfg(windows)]
mod provider;
#[cfg(windows)]
mod replay;
#[cfg(windows)]
mod report;
#[cfg(windows)]
mod state;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod update;
#[cfg(windows)]
mod update_progress;
#[cfg(windows)]
mod win32;

fn main() {
    #[cfg(windows)]
    if let Err(error) = app_icon::set_process_app_user_model_id() {
        show_error(&format!(
            "volty: could not set the Windows application identity: {error}"
        ));
        std::process::exit(2);
    }

    #[cfg(windows)]
    let cli_mode = matches!(
        std::env::args().nth(1).as_deref(),
        Some("hid" | "capture" | "history" | "dashboard")
    );
    #[cfg(windows)]
    let dashboard_window_mode = std::env::args()
        .skip(2)
        .any(|argument| argument == "--window" || argument == "--settings")
        && std::env::args().nth(1).as_deref() == Some("dashboard");

    #[cfg(windows)]
    if !cli_mode {
        if let Err(error) = attach_parent_console() {
            show_error(&format!(
                "volty: could not attach to the parent console: {error}"
            ));
        }
    }

    #[cfg(windows)]
    if cli_mode && !dashboard_window_mode {
        if let Err(error) = attach_cli_console() {
            show_error(&format!(
                "volty: could not initialize diagnostic console: {error}"
            ));
            std::process::exit(2);
        }
    }

    if let Err(error) = run() {
        let message = format!("volty: {error}");
        #[cfg(windows)]
        if cli_mode && !dashboard_window_mode {
            eprintln!("{message}");
        } else {
            show_error(&message);
        }
        #[cfg(not(windows))]
        eprintln!("{message}");
        std::process::exit(2);
    }
}

#[cfg(windows)]
fn attach_parent_console() -> std::io::Result<()> {
    use windows_sys::Win32::{
        Foundation::GetLastError,
        System::Console::{AttachConsole, ATTACH_PARENT_PROCESS},
    };

    // SAFETY: ATTACH_PARENT_PROCESS is a documented pseudo-process identifier.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0 {
        return Ok(());
    }

    // SAFETY: this reads the error immediately after AttachConsole failed.
    let error = unsafe { GetLastError() };
    if is_expected_parent_console_error(error) {
        return Ok(());
    }

    Err(std::io::Error::from_raw_os_error(error as i32))
}

#[cfg(windows)]
fn is_expected_parent_console_error(error: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER,
    };

    matches!(
        error,
        ERROR_ACCESS_DENIED | ERROR_INVALID_HANDLE | ERROR_INVALID_PARAMETER
    )
}

#[cfg(windows)]
fn show_error(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::MB_OK;

    crate::app_icon::show_message_box(message, "Volty", MB_OK);
}

#[cfg(windows)]
fn attach_cli_console() -> std::io::Result<()> {
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_ACCESS_DENIED, GENERIC_READ, GENERIC_WRITE, HANDLE,
            INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::Console::{
            AllocConsole, AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS,
            STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
        },
    };

    let is_valid_handle = |handle: HANDLE| !handle.is_null() && handle != INVALID_HANDLE_VALUE;
    // SAFETY: GetStdHandle returns the process's current stdout handle for validation.
    let stdout_handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    // SAFETY: GetStdHandle returns the process's current stderr handle for validation.
    let stderr_handle = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    if is_valid_handle(stdout_handle) && is_valid_handle(stderr_handle) {
        return Ok(());
    }

    // SAFETY: ATTACH_PARENT_PROCESS is a documented pseudo-process identifier.
    let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0;
    if !attached {
        // SAFETY: the error is read immediately after AttachConsole failed.
        let error = unsafe { GetLastError() };
        if error != ERROR_ACCESS_DENIED {
            // SAFETY: AllocConsole creates a console for the current process without pointer arguments.
            if unsafe { AllocConsole() } == 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }

    for (standard_handle, console_name) in [
        (STD_OUTPUT_HANDLE, "CONOUT$"),
        (STD_ERROR_HANDLE, "CONOUT$"),
    ] {
        // SAFETY: this reads the selected standard stream's current process handle.
        if is_valid_handle(unsafe { GetStdHandle(standard_handle) }) {
            continue;
        }

        let console_name = crate::win32::to_wide(console_name);
        // SAFETY: console_name is NUL-terminated and the optional security/template pointers are null.
        let handle = unsafe {
            CreateFileW(
                console_name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: handle is a valid file handle returned by CreateFileW for this standard stream.
        if unsafe { SetStdHandle(standard_handle, handle) } == 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: SetStdHandle failed, so this still-owned CreateFileW handle must be closed.
            if unsafe { CloseHandle(handle) } == 0 {
                eprintln!(
                    "Volty could not close a failed console handle: {}",
                    std::io::Error::last_os_error()
                );
            }
            return Err(error);
        }
    }

    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("hid") => return hid_cli::run(&args.collect::<Vec<_>>()).map_err(Into::into),
        #[cfg(windows)]
        Some("capture") => return capture::run_cli(&args.collect::<Vec<_>>()),
        #[cfg(windows)]
        Some("history") => return history::run_cli(&args.collect::<Vec<_>>()),
        #[cfg(windows)]
        Some("dashboard") => return run_dashboard_cli(&args.collect::<Vec<_>>()),
        #[cfg(windows)]
        Some("--apply-update") => return update::apply_update_cli(&args.collect::<Vec<_>>()),
        #[cfg(windows)]
        Some(update::FINISH_UPDATE_ARGUMENT) => {
            if let Err(error) = update::finish_update_cli(&args.collect::<Vec<_>>()) {
                show_error(&format!("volty: update cleanup failed: {error}"));
            }
        }
        _ => {}
    }

    #[cfg(windows)]
    {
        let _single_instance = SingleInstance::acquire()?;
        if !_single_instance.is_first() {
            return Ok(());
        }

        app::run()?;
    }

    #[cfg(not(windows))]
    eprintln!("volty-tray is a Windows application.");

    Ok(())
}

#[cfg(windows)]
fn run_dashboard_cli(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.iter().any(|argument| argument == "--settings") {
        let _shutdown_event = DashboardSettingsShutdownEvent::create()?;
        let activation_event = DashboardSettingsActivationEvent::create()?;
        let _single_instance =
            SingleInstance::acquire_named(dashboard::settings_instance_mutex_name())?;
        if !_single_instance.is_first() {
            activation_event.signal()?;
            return Ok(());
        }
        _shutdown_event.reset()?;
        return dashboard::run_cli(args);
    }

    if args.iter().any(|argument| argument == "--window") {
        let _shutdown_event = DashboardShutdownEvent::create()?;
        let activation_event = DashboardActivationEvent::create()?;
        let _single_instance = SingleInstance::acquire_named(DASHBOARD_INSTANCE_MUTEX)?;
        if !_single_instance.is_first() {
            activation_event.signal()?;
            return Ok(());
        }
        _shutdown_event.reset()?;
        return dashboard::run_cli(args);
    }

    dashboard::run_cli(args)
}

#[cfg(windows)]
const DASHBOARD_INSTANCE_MUTEX: &str = "Local\\VoltyDashboard";

#[cfg(windows)]
const DASHBOARD_ACTIVATION_EVENT: &str = "Local\\VoltyDashboardActivate";

#[cfg(windows)]
const DASHBOARD_SHUTDOWN_EVENT: &str = "Local\\VoltyDashboardShutdown";

#[cfg(windows)]
const DASHBOARD_SETTINGS_ACTIVATION_EVENT: &str = "Local\\VoltySettingsActivate";

#[cfg(windows)]
const DASHBOARD_SETTINGS_SHUTDOWN_EVENT: &str = "Local\\VoltySettingsShutdown";

#[cfg(windows)]
pub(crate) struct DashboardShutdownEvent(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl DashboardShutdownEvent {
    pub(crate) fn create() -> std::io::Result<Self> {
        use windows_sys::Win32::System::Threading::CreateEventW;

        let name = crate::win32::to_wide(DASHBOARD_SHUTDOWN_EVENT);
        // SAFETY: name is NUL-terminated and the null security attributes request default access.
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(handle))
    }

    pub(crate) fn signal(&self) -> std::io::Result<()> {
        use windows_sys::Win32::System::Threading::SetEvent;

        // SAFETY: this wrapper owns the valid event handle returned by CreateEventW.
        if unsafe { SetEvent(self.0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub(crate) fn reset(&self) -> std::io::Result<()> {
        use windows_sys::Win32::System::Threading::ResetEvent;

        // SAFETY: this wrapper owns the valid event handle returned by CreateEventW.
        if unsafe { ResetEvent(self.0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for DashboardShutdownEvent {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the valid handle returned by CreateEventW.
        if unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) } == 0 {
            eprintln!(
                "Volty could not close its dashboard shutdown event: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(windows)]
pub(crate) struct DashboardSettingsShutdownEvent(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl DashboardSettingsShutdownEvent {
    pub(crate) fn create() -> std::io::Result<Self> {
        use windows_sys::Win32::System::Threading::CreateEventW;

        let name = crate::win32::to_wide(DASHBOARD_SETTINGS_SHUTDOWN_EVENT);
        // SAFETY: name is NUL-terminated and the null security attributes request default access.
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(handle))
    }

    pub(crate) fn signal(&self) -> std::io::Result<()> {
        use windows_sys::Win32::System::Threading::SetEvent;

        // SAFETY: this wrapper owns the valid event handle returned by CreateEventW.
        if unsafe { SetEvent(self.0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub(crate) fn reset(&self) -> std::io::Result<()> {
        use windows_sys::Win32::System::Threading::ResetEvent;

        // SAFETY: this wrapper owns the valid event handle returned by CreateEventW.
        if unsafe { ResetEvent(self.0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for DashboardSettingsShutdownEvent {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the valid event handle returned by CreateEventW.
        if unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) } == 0 {
            eprintln!(
                "Volty could not close its Settings shutdown event: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(windows)]
struct DashboardSettingsActivationEvent(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl DashboardSettingsActivationEvent {
    fn create() -> std::io::Result<Self> {
        use windows_sys::Win32::System::Threading::CreateEventW;

        let name = crate::win32::to_wide(DASHBOARD_SETTINGS_ACTIVATION_EVENT);
        // SAFETY: name is NUL-terminated and the null security attributes request default access.
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(handle))
    }

    fn signal(&self) -> std::io::Result<()> {
        use windows_sys::Win32::System::Threading::SetEvent;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, SetForegroundWindow,
            ShowWindow, SW_RESTORE,
        };

        let title = crate::win32::to_wide("Volty Settings");
        // SAFETY: title is NUL-terminated; a null class name searches by title.
        let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if !window.is_null() {
            // SAFETY: window is the live HWND returned by FindWindowW.
            unsafe { ShowWindow(window, SW_RESTORE) };
            let mut process_id = 0;
            // SAFETY: window is the live HWND returned by FindWindowW.
            unsafe { GetWindowThreadProcessId(window, &mut process_id) };
            if process_id != 0 {
                // SAFETY: the target PID belongs to the Settings window found above.
                if unsafe { AllowSetForegroundWindow(process_id) } == 0 {
                    eprintln!("Windows did not grant foreground permission to Settings");
                }
            }
            // SAFETY: window is the live HWND returned by FindWindowW.
            if unsafe { SetForegroundWindow(window) } == 0 {
                eprintln!("Windows did not allow Settings to become the foreground window");
            }
        }

        // SAFETY: this wrapper owns the valid event handle returned by CreateEventW.
        if unsafe { SetEvent(self.0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for DashboardSettingsActivationEvent {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the valid event handle returned by CreateEventW.
        if unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) } == 0 {
            eprintln!(
                "Volty could not close its Settings activation event: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(windows)]
struct DashboardActivationEvent(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl DashboardActivationEvent {
    fn create() -> std::io::Result<Self> {
        use windows_sys::Win32::System::Threading::CreateEventW;

        let name = crate::win32::to_wide(DASHBOARD_ACTIVATION_EVENT);
        // SAFETY: name is NUL-terminated and the null security attributes request default access.
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(handle))
    }

    fn signal(&self) -> std::io::Result<()> {
        use windows_sys::Win32::System::Threading::SetEvent;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, SetForegroundWindow,
            ShowWindow, SW_RESTORE,
        };

        let title = crate::win32::to_wide("Volty Dashboard");
        // SAFETY: title is a NUL-terminated string; a null class name searches by title.
        let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if !window.is_null() {
            // SAFETY: window is the valid HWND returned by FindWindowW.
            unsafe { ShowWindow(window, SW_RESTORE) };
            let mut process_id = 0;
            // SAFETY: window is the valid HWND returned by FindWindowW.
            unsafe { GetWindowThreadProcessId(window, &mut process_id) };
            // SAFETY: process_id belongs to the dashboard process identified by its valid HWND.
            if process_id != 0 && unsafe { AllowSetForegroundWindow(process_id) } == 0 {
                eprintln!("Windows did not grant foreground permission to the existing dashboard");
            }
            // SAFETY: window is the valid HWND returned by FindWindowW.
            if unsafe { SetForegroundWindow(window) } == 0 {
                eprintln!("Windows did not allow the dashboard to become the foreground window");
            }
        }

        // SAFETY: this handle refers to the named auto-reset event shared with the dashboard.
        if unsafe { SetEvent(self.0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for DashboardActivationEvent {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the valid event handle returned by CreateEventW.
        if unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) } == 0 {
            eprintln!(
                "Volty could not close its dashboard activation event: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(windows)]
struct SingleInstance(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl SingleInstance {
    fn acquire() -> std::io::Result<Self> {
        Self::acquire_named("Local\\VoltyTray-LogitechBattery")
    }

    fn acquire_named(name: &str) -> std::io::Result<Self> {
        use windows_sys::Win32::Foundation::{GetLastError, SetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name = crate::win32::to_wide(name);
        // SAFETY: clearing the current thread's last error lets CreateMutexW's already-exists
        // indication be distinguished from errors left by prior APIs.
        unsafe { SetLastError(0) };
        // SAFETY: name is a NUL-terminated buffer that remains live through CreateMutexW.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }

        // SAFETY: CreateMutexW succeeded and last error is read before another Win32 call.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: this valid handle refers to the pre-existing mutex and is not retained.
            if unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            return Ok(Self(std::ptr::null_mut()));
        }

        Ok(Self(handle))
    }

    fn is_first(&self) -> bool {
        !self.0.is_null()
    }
}

#[cfg(windows)]
impl Drop for SingleInstance {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this guard uniquely owns the valid mutex handle returned by CreateMutexW.
            if unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) } == 0 {
                eprintln!(
                    "Volty could not close its single-instance mutex: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::{is_expected_parent_console_error, SingleInstance};
    use std::time::{SystemTime, UNIX_EPOCH};
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER,
    };

    #[test]
    fn missing_parent_console_errors_are_ignored() {
        assert!(is_expected_parent_console_error(ERROR_ACCESS_DENIED));
        assert!(is_expected_parent_console_error(ERROR_INVALID_HANDLE));
        assert!(is_expected_parent_console_error(ERROR_INVALID_PARAMETER));
        assert!(!is_expected_parent_console_error(ERROR_FILE_NOT_FOUND));
    }

    #[test]
    fn mutex_stays_exclusive_for_the_guard_lifetime() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos();
        let name = format!("Local\\VoltyTrayTest-{}-{suffix}", std::process::id());

        let first = SingleInstance::acquire_named(&name).expect("acquire first mutex");
        assert!(first.is_first());
        let second = SingleInstance::acquire_named(&name).expect("acquire second mutex");
        assert!(!second.is_first());

        drop(second);
        drop(first);

        let third = SingleInstance::acquire_named(&name).expect("reacquire released mutex");
        assert!(third.is_first());
    }
}
