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
mod tray;

fn main() {
    if let Err(error) = run() {
        eprintln!("volty: {error}");
        std::process::exit(2);
    }
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
