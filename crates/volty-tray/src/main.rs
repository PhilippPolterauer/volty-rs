#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod about;
#[cfg(windows)]
mod app;
mod config;
#[cfg(windows)]
mod diagnose;
mod hid_cli;
#[cfg(windows)]
mod logging;
#[cfg(windows)]
mod native_ui;
#[cfg(windows)]
mod notify;
#[cfg(windows)]
mod report;
#[cfg(windows)]
mod state;
#[cfg(windows)]
mod tray;

fn main() {
    #[cfg(windows)]
    let cli_mode = std::env::args().nth(1).as_deref() == Some("hid");

    #[cfg(windows)]
    if !cli_mode {
        if let Err(error) = attach_parent_console() {
            show_error(&format!(
                "volty: could not attach to the parent console: {error}"
            ));
        }
    }

    #[cfg(windows)]
    if cli_mode {
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
        if cli_mode {
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
        Foundation::{GetLastError, ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE},
        System::Console::{AttachConsole, ATTACH_PARENT_PROCESS},
    };

    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0 {
        return Ok(());
    }

    let error = unsafe { GetLastError() };
    if error == ERROR_ACCESS_DENIED || error == ERROR_INVALID_HANDLE {
        return Ok(());
    }

    Err(std::io::Error::from_raw_os_error(error as i32))
}

#[cfg(windows)]
fn show_error(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "Volty".encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_ICONERROR | MB_OK,
        );
    }
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
    if is_valid_handle(unsafe { GetStdHandle(STD_OUTPUT_HANDLE) })
        && is_valid_handle(unsafe { GetStdHandle(STD_ERROR_HANDLE) })
    {
        return Ok(());
    }

    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0
        && unsafe { GetLastError() } != ERROR_ACCESS_DENIED
        && unsafe { AllocConsole() } == 0
    {
        return Err(std::io::Error::last_os_error());
    }

    for (standard_handle, console_name) in [
        (STD_OUTPUT_HANDLE, "CONOUT$\0"),
        (STD_ERROR_HANDLE, "CONOUT$\0"),
    ] {
        if is_valid_handle(unsafe { GetStdHandle(standard_handle) }) {
            continue;
        }

        let console_name: Vec<u16> = console_name.encode_utf16().collect();
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
        if unsafe { SetStdHandle(standard_handle, handle) } == 0 {
            unsafe { CloseHandle(handle) };
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("hid") {
        return hid_cli::run(&args.collect::<Vec<_>>()).map_err(Into::into);
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
struct SingleInstance(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl SingleInstance {
    fn acquire() -> std::io::Result<Self> {
        Self::acquire_named("Local\\VoltyTray-LogitechBattery")
    }

    fn acquire_named(name: &str) -> std::io::Result<Self> {
        use windows_sys::Win32::Foundation::{GetLastError, SetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::CreateMutexW;

        let name: Vec<u16> = format!("{name}\0").encode_utf16().collect();
        unsafe { SetLastError(0) };
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }

        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
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
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::SingleInstance;
    use std::time::{SystemTime, UNIX_EPOCH};

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
