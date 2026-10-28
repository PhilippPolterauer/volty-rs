use std::io;

use super::TrayError;

#[cfg(windows)]
use winreg::{
    enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE},
    RegKey,
};

#[cfg(windows)]
const BATTERY_RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
#[cfg(windows)]
const APP_RUN_VALUE: &str = "Volty";
#[cfg(windows)]
const APP_SETTINGS_KEY: &str = "Software\\Volty";
#[cfg(windows)]
const STARTUP_PROMPT_VALUE: &str = "StartupPrompted";

pub fn start_on_boot_enabled() -> Result<bool, TrayError> {
    #[cfg(windows)]
    {
        Ok(read_registry_value(
            BATTERY_RUN_KEY,
            "could not read Windows startup setting",
            |key| key.get_value::<String, _>(APP_RUN_VALUE),
        )?
        .is_some())
    }

    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

#[cfg(windows)]
pub(super) fn start_on_boot_for_first_run() -> Result<bool, TrayError> {
    let enabled = start_on_boot_enabled()?;
    if enabled || startup_prompt_shown()? {
        return Ok(enabled);
    }

    let text = crate::win32::to_wide(
        "Would you like Volty to start automatically when you sign in to Windows?\n\n\
         If you choose Yes, Windows will start Volty from this executable's current location. \
         Keep the file there. You can change this later from the tray menu.",
    );
    let title = crate::win32::to_wide("Start Volty with Windows?");
    // SAFETY: text and title are NUL-terminated buffers that remain live through MessageBoxW.
    let response = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_DEFBUTTON2
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONQUESTION
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_YESNO,
        )
    };
    if response == 0 {
        return Err(TrayError::new(format!(
            "could not show first-run startup prompt: {}",
            io::Error::last_os_error()
        )));
    }
    let enable_startup = response == windows_sys::Win32::UI::WindowsAndMessaging::IDYES;
    if enable_startup {
        set_start_on_boot_enabled(true)?;
    }
    set_startup_prompt_shown()?;
    Ok(enable_startup)
}

#[cfg(windows)]
fn startup_prompt_shown() -> Result<bool, TrayError> {
    Ok(read_registry_value(
        APP_SETTINGS_KEY,
        "could not read first-run startup preference",
        |key| key.get_value::<u32, _>(STARTUP_PROMPT_VALUE),
    )?
    .is_some_and(|value| value != 0))
}

#[cfg(windows)]
fn set_startup_prompt_shown() -> Result<(), TrayError> {
    let key = create_registry_key(
        APP_SETTINGS_KEY,
        "could not save first-run startup preference",
    )?;
    write_registry_value(&key, "could not save first-run startup preference", |key| {
        key.set_value(STARTUP_PROMPT_VALUE, &1u32)
    })
}

#[cfg(windows)]
fn clear_startup_prompt_shown() -> Result<(), TrayError> {
    delete_registry_value(
        APP_SETTINGS_KEY,
        STARTUP_PROMPT_VALUE,
        "could not reset first-run startup preference",
    )
}

pub fn confirm_reset(
    configuration_file: &std::path::Path,
    local_state_file: &std::path::Path,
) -> Result<bool, TrayError> {
    let text = crate::win32::to_wide(&format!(
        "This permanently deletes Volty's configuration and local device state, disables Windows startup, and resets the first-run startup prompt:\n\n\
         Configuration: {}\n\
         Local state: {}\n\n\
         Volty will reset and continue running with fresh defaults.\n\n\
         Continue?",
        configuration_file.display(),
        local_state_file.display()
    ));
    let title = crate::win32::to_wide("Reset Volty?");
    // SAFETY: text and title are NUL-terminated buffers that remain live through MessageBoxW.
    let response = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_DEFBUTTON2
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONWARNING
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_YESNO,
        )
    };
    match response {
        windows_sys::Win32::UI::WindowsAndMessaging::IDYES => Ok(true),
        windows_sys::Win32::UI::WindowsAndMessaging::IDNO => Ok(false),
        0 => Err(TrayError::new(format!(
            "could not show reset confirmation: {}",
            io::Error::last_os_error()
        ))),
        _ => Err(TrayError::new(format!(
            "reset confirmation returned unexpected response {response}"
        ))),
    }
}

/// Enables or disables Windows startup using the current executable's Run entry.
pub fn set_start_on_boot_enabled(enabled: bool) -> Result<(), TrayError> {
    #[cfg(windows)]
    {
        if enabled {
            let key = create_registry_key(BATTERY_RUN_KEY, "could not enable Windows startup")?;
            let executable = std::env::current_exe().map_err(|error| {
                TrayError::new(format!("could not locate executable for startup: {error}"))
            })?;
            write_registry_value(&key, "could not enable Windows startup", |key| {
                key.set_value(APP_RUN_VALUE, &format!("\"{}\"", executable.display()))
            })
        } else {
            delete_registry_value(
                BATTERY_RUN_KEY,
                APP_RUN_VALUE,
                "could not disable Windows startup",
            )
        }
    }

    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err(TrayError::new("Start on Boot is only supported on Windows"))
    }
}

#[cfg(windows)]
pub fn reset_startup_preferences() -> Result<(), TrayError> {
    let mut errors = Vec::new();
    if let Err(error) = set_start_on_boot_enabled(false) {
        errors.push(error.to_string());
    }
    if let Err(error) = clear_startup_prompt_shown() {
        errors.push(error.to_string());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(TrayError::new(format!(
            "could not reset Windows startup preferences: {}",
            errors.join("; ")
        )))
    }
}

#[cfg(windows)]
fn open_registry_key(path: &str, access: u32, context: &str) -> Result<Option<RegKey>, TrayError> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    match current_user.open_subkey_with_flags(path, access) {
        Ok(key) => Ok(Some(key)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(registry_error(context, error)),
    }
}

#[cfg(windows)]
fn read_registry_value<T>(
    path: &str,
    context: &str,
    read: impl FnOnce(&RegKey) -> io::Result<T>,
) -> Result<Option<T>, TrayError> {
    let Some(key) = open_registry_key(path, KEY_READ, context)? else {
        return Ok(None);
    };
    match read(&key) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(registry_error(context, error)),
    }
}

#[cfg(windows)]
fn create_registry_key(path: &str, context: &str) -> Result<RegKey, TrayError> {
    RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(path)
        .map(|(key, _)| key)
        .map_err(|error| registry_error(context, error))
}

#[cfg(windows)]
fn write_registry_value(
    key: &RegKey,
    context: &str,
    write: impl FnOnce(&RegKey) -> io::Result<()>,
) -> Result<(), TrayError> {
    write(key).map_err(|error| registry_error(context, error))
}

#[cfg(windows)]
fn delete_registry_value(path: &str, value_name: &str, context: &str) -> Result<(), TrayError> {
    let Some(key) = open_registry_key(path, KEY_SET_VALUE, context)? else {
        return Ok(());
    };
    match key.delete_value(value_name) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(registry_error(context, error)),
    }
}

#[cfg(windows)]
fn registry_error(context: &str, error: io::Error) -> TrayError {
    TrayError::new(format!("{context}: {error}"))
}
