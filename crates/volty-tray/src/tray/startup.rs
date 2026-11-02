use std::{io, path::Path};

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
        Ok(startup_command_from_registry()?.is_some())
    }

    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

#[cfg(windows)]
pub(super) fn start_on_boot_for_first_run() -> Result<bool, TrayError> {
    if let Some(configured_command) = startup_command_from_registry()? {
        let executable = std::env::current_exe().map_err(|error| {
            TrayError::new(format!("could not locate the running executable: {error}"))
        })?;
        if should_confirm_startup_command_change(
            &configured_command,
            &executable,
            is_update_restart(),
        ) && confirm_startup_executable_change(&configured_command, &executable)?
        {
            set_start_on_boot_enabled(true)?;
        }
        return Ok(true);
    }
    if startup_prompt_shown()? {
        return Ok(false);
    }

    let text = "Would you like Volty to start automatically when you sign in to Windows?\n\n\
         If you choose Yes, Windows will start Volty from this executable's current location. \
         Keep the file there. You can change this later from the tray menu.";
    let response = crate::app_icon::show_message_box(
        text,
        "Start Volty with Windows?",
        windows_sys::Win32::UI::WindowsAndMessaging::MB_DEFBUTTON2
            | windows_sys::Win32::UI::WindowsAndMessaging::MB_YESNO,
    );
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
fn startup_command_from_registry() -> Result<Option<String>, TrayError> {
    read_registry_value(
        BATTERY_RUN_KEY,
        "could not read Windows startup setting",
        |key| key.get_value::<String, _>(APP_RUN_VALUE),
    )
}

fn startup_command(executable: &Path) -> String {
    format!("\"{}\"", executable.display())
}

fn startup_command_matches(configured_command: &str, executable: &Path) -> bool {
    configured_command.eq_ignore_ascii_case(&startup_command(executable))
}

fn should_confirm_startup_command_change(
    configured_command: &str,
    executable: &Path,
    is_update_restart: bool,
) -> bool {
    !is_update_restart && !startup_command_matches(configured_command, executable)
}

#[cfg(windows)]
fn is_update_restart() -> bool {
    std::env::args().nth(1).as_deref() == Some(crate::update::FINISH_UPDATE_ARGUMENT)
}

#[cfg(windows)]
fn confirm_startup_executable_change(
    configured_command: &str,
    executable: &Path,
) -> Result<bool, TrayError> {
    let text = format!(
        "Windows is currently configured to start Volty from:\n\n{}\n\n\
         This Volty instance is running from:\n\n{}\n\n\
         Change the startup executable to this one?",
        configured_command.trim_matches('"'),
        executable.display()
    );
    let response = crate::app_icon::show_message_box(
        &text,
        "Change Volty Startup Executable?",
        windows_sys::Win32::UI::WindowsAndMessaging::MB_DEFBUTTON2
            | windows_sys::Win32::UI::WindowsAndMessaging::MB_YESNO,
    );
    match response {
        windows_sys::Win32::UI::WindowsAndMessaging::IDYES => Ok(true),
        windows_sys::Win32::UI::WindowsAndMessaging::IDNO => Ok(false),
        0 => Err(TrayError::new(format!(
            "could not show startup executable confirmation: {}",
            io::Error::last_os_error()
        ))),
        _ => Err(TrayError::new(format!(
            "startup executable confirmation returned unexpected response {response}"
        ))),
    }
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
    let text = format!(
        "This permanently deletes Volty's configuration and local device state, disables Windows startup, and resets the first-run startup prompt:\n\n\
         Configuration: {}\n\
         Local state: {}\n\n\
         Volty will reset and continue running with fresh defaults.\n\n\
         Continue?",
        configuration_file.display(),
        local_state_file.display()
    );
    let response = crate::app_icon::show_message_box(
        &text,
        "Reset Volty?",
        windows_sys::Win32::UI::WindowsAndMessaging::MB_DEFBUTTON2
            | windows_sys::Win32::UI::WindowsAndMessaging::MB_YESNO,
    );
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
                key.set_value(APP_RUN_VALUE, &startup_command(&executable))
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

#[cfg(test)]
mod tests {
    use super::{should_confirm_startup_command_change, startup_command, startup_command_matches};
    use std::path::Path;

    #[test]
    fn startup_command_quotes_paths_with_spaces() {
        assert_eq!(
            startup_command(Path::new(r"C:\Program Files\Volty\volty.exe")),
            r#""C:\Program Files\Volty\volty.exe""#
        );
    }

    #[test]
    fn startup_command_match_is_case_insensitive() {
        assert!(startup_command_matches(
            r#""c:\program files\volty\VOLTY.exe""#,
            Path::new(r"C:\Program Files\Volty\volty.exe")
        ));
        assert!(!startup_command_matches(
            r#""C:\Old\Volty\volty.exe""#,
            Path::new(r"C:\Program Files\Volty\volty.exe")
        ));
    }

    #[test]
    fn update_restart_does_not_repeat_a_declined_startup_path_prompt() {
        assert!(should_confirm_startup_command_change(
            r#""C:\Old\Volty\volty.exe""#,
            Path::new(r"C:\Program Files\Volty\volty.exe"),
            false
        ));
        assert!(!should_confirm_startup_command_change(
            r#""C:\Old\Volty\volty.exe""#,
            Path::new(r"C:\Program Files\Volty\volty.exe"),
            true
        ));
    }
}
