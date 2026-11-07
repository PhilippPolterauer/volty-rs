use std::{
    error::Error,
    io,
    process::Command,
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{BST_CHECKED, BST_UNCHECKED, EM_LIMITTEXT},
        Input::KeyboardAndMouse::{EnableWindow, SetFocus},
        WindowsAndMessaging::{
            CreateWindowExW, GetClientRect, GetWindowTextLengthW, GetWindowTextW, MessageBoxW,
            MoveWindow, PostMessageW, SendMessageW, SetWindowTextW, ShowWindow, BM_GETCHECK,
            BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CBS_DROPDOWNLIST,
            CBS_HASSTRINGS, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, ES_AUTOHSCROLL, ES_NUMBER,
            IDCANCEL, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO, MINMAXINFO, SW_SHOWNORMAL,
            WM_APP, WM_CLOSE, WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY, WM_GETMINMAXINFO,
            WM_NCCREATE, WM_SETFOCUS, WM_SIZE, WS_CHILD, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW,
            WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
        },
    },
};

use crate::{
    config::DashboardSettingsUpdate,
    native_ui::{edit_control_color, set_control_font, static_control_color, FontSet},
    win32::{self, ControlSpec},
};

use super::{
    management::DashboardBackend,
    model::{DashboardInventoryDevice, DashboardSettings},
};

const WINDOW_CLASS: &str = "VoltyNativeSettingsWindow";
const WINDOW_TITLE: &str = "Volty Settings";
const WINDOW_WIDTH: i32 = 900;
const WINDOW_HEIGHT: i32 = 900;
const MIN_WINDOW_WIDTH: i32 = 820;
const MIN_WINDOW_HEIGHT: i32 = 880;
const MARGIN: i32 = 32;
const OVERVIEW_ID: usize = 1001;
const THRESHOLDS_ID: usize = 1002;
const COOLDOWN_ID: usize = 1003;
const NOTIFICATIONS_ID: usize = 1004;
const TEST_NOTIFICATION_ID: usize = 1005;
const ICON_STYLE_ID: usize = 1006;
const ICON_THEME_ID: usize = 1007;
const PERCENTAGE_OVERLAY_ID: usize = 1008;
const PINNED_DEVICE_ID: usize = 1009;
const STARTUP_ID: usize = 1010;
const SAVE_ID: usize = 1011;
const SETTINGS_ACTIVATE_MESSAGE: u32 = WM_APP + 1;
const SETTINGS_INSTANCE_MUTEX: &str = "Local\\VoltySettings";

#[derive(Clone, Debug, PartialEq, Eq)]
struct FormValues {
    thresholds: String,
    cooldown: String,
    notifications: bool,
    icon_style: usize,
    icon_theme: usize,
    percentage_overlay: bool,
    pinned_device: usize,
    start_on_boot: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlRole {
    Heading,
    Description,
    Overview,
    ConfigError,
    ReadOnly,
    BatteryHeading,
    BatteryDescription,
    ThresholdLabel,
    ThresholdHelp,
    ThresholdEdit,
    ThresholdError,
    CooldownLabel,
    CooldownHelp,
    CooldownEdit,
    CooldownError,
    Notifications,
    TestNotification,
    TestStatus,
    BatteryDivider,
    TrayHeading,
    TrayDescription,
    IconStyleLabel,
    IconStyleHelp,
    IconStyle,
    IconThemeLabel,
    IconThemeHelp,
    IconTheme,
    PercentageOverlay,
    PercentageHelp,
    PinnedDeviceLabel,
    PinnedDeviceHelp,
    PinnedDevice,
    TrayDivider,
    StartupHeading,
    StartupDescription,
    Startup,
    StartupError,
    Status,
    Save,
}

struct LayoutControl {
    handle: HWND,
    role: ControlRole,
}

struct SettingsWindow {
    backend: DashboardBackend,
    days: u32,
    database: Option<std::path::PathBuf>,
    settings: Option<DashboardSettings>,
    pin_values: Vec<Option<String>>,
    controls: Vec<LayoutControl>,
    fonts: FontSet,
    original: Option<FormValues>,
    activation_listener: Option<SettingsEventListener>,
    create_complete: bool,
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        if let Some(mut listener) = self.activation_listener.take() {
            listener.stop();
        }
    }
}

pub(super) fn open_settings_window(
    backend: DashboardBackend,
    days: u32,
    database: Option<std::path::PathBuf>,
) -> Result<(), Box<dyn Error>> {
    win32::enable_system_dark_menus();
    let data = backend.get_dashboard_data(days)?;
    let class_name = win32::to_wide(WINDOW_CLASS);
    let title = win32::to_wide(WINDOW_TITLE);
    // SAFETY: a null module name requests the current process executable's module handle.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    if instance.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    win32::register_window_class(&class_name, window_proc, instance)?;

    let fonts = FontSet::new()?;
    let mut state = Box::new(SettingsWindow {
        backend,
        days,
        database,
        settings: data.settings,
        pin_values: Vec::new(),
        controls: Vec::new(),
        fonts,
        original: None,
        activation_listener: None,
        create_complete: false,
    });

    // SAFETY: class/title are NUL-terminated and live through creation; the Box-backed state pointer
    // remains valid until the window is destroyed and the message loop exits.
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            windows_sys::Win32::UI::WindowsAndMessaging::CW_USEDEFAULT,
            windows_sys::Win32::UI::WindowsAndMessaging::CW_USEDEFAULT,
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

    if let Err(error) = create_controls(window, instance, &data.inventory, &mut state) {
        if let Err(destroy_error) = win32::destroy_window(window) {
            if let Err(clear_error) = win32::clear_window_state(window) {
                eprintln!("Volty could not detach Settings-window state: {clear_error}");
            }
            eprintln!("Volty could not destroy the incomplete Settings window: {destroy_error}");
        }
        return Err(error);
    }
    state.create_complete = true;
    win32::allow_dark_mode_for_current_thread_windows();
    state.activation_listener = Some(SettingsEventListener::start(window)?);
    win32::run_dialog_message_loop(window, SW_SHOWNORMAL, state, |state, value| {
        state.create_complete = value;
    })?;
    Ok(())
}

fn create_controls(
    window: HWND,
    instance: windows_sys::Win32::Foundation::HINSTANCE,
    inventory: &[DashboardInventoryDevice],
    state: &mut SettingsWindow,
) -> Result<(), Box<dyn Error>> {
    let heading = add_control(
        window,
        instance,
        state,
        ControlRole::Heading,
        "STATIC",
        "Settings",
        WS_VISIBLE,
        0,
        state.fonts.heading,
    )?;
    let description = add_control(
        window,
        instance,
        state,
        ControlRole::Description,
        "STATIC",
        "Choose how Volty alerts you, starts with Windows, and appears in the tray.",
        WS_VISIBLE,
        0,
        state.fonts.body,
    )?;
    let overview = add_control(
        window,
        instance,
        state,
        ControlRole::Overview,
        "BUTTON",
        "Overview",
        WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
        OVERVIEW_ID,
        state.fonts.body,
    )?;
    let config_error = state
        .settings
        .as_ref()
        .and_then(|settings| settings.config_error.as_deref())
        .map_or_else(String::new, |error| {
            format!("Volty could not load config.toml. Fix it before saving. {error}")
        });
    let read_only_text = if state.settings.is_none() {
        "Settings are read-only when this window uses a separately selected database."
    } else {
        ""
    };
    let config = add_control(
        window,
        instance,
        state,
        ControlRole::ConfigError,
        "STATIC",
        &config_error,
        WS_VISIBLE,
        0,
        state.fonts.body_small,
    )?;
    let read_only = add_control(
        window,
        instance,
        state,
        ControlRole::ReadOnly,
        "STATIC",
        read_only_text,
        WS_VISIBLE,
        0,
        state.fonts.body_small,
    )?;

    add_label(
        window,
        instance,
        state,
        ControlRole::BatteryHeading,
        "Battery alerts",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::BatteryDescription,
        "Choose the levels that trigger notifications. Alerts are tracked per discharge cycle.",
        state.fonts.body_small,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::ThresholdLabel,
        "Alert at",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::ThresholdHelp,
        "Whole percentages, highest first, separated by commas (for example 20, 10, 5).",
        state.fonts.body_small,
    )?;
    let thresholds = state
        .settings
        .as_ref()
        .map_or_else(String::new, |settings| {
            settings
                .low_battery_thresholds
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        });
    let thresholds_control = add_control(
        window,
        instance,
        state,
        ControlRole::ThresholdEdit,
        "EDIT",
        &thresholds,
        WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
        THRESHOLDS_ID,
        state.fonts.body,
    )?;
    send_message(thresholds_control, EM_LIMITTEXT, 1024, 0);
    add_label(
        window,
        instance,
        state,
        ControlRole::ThresholdError,
        "",
        state.fonts.body_small,
    )?;

    add_label(
        window,
        instance,
        state,
        ControlRole::CooldownLabel,
        "Repeat interval",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::CooldownHelp,
        "Minutes before repeating below the lowest level. Set to 0 to never repeat.",
        state.fonts.body_small,
    )?;
    let cooldown = state
        .settings
        .as_ref()
        .map_or_else(String::new, |settings| {
            settings.low_battery_cooldown_minutes.to_string()
        });
    let cooldown_control = add_control(
        window,
        instance,
        state,
        ControlRole::CooldownEdit,
        "EDIT",
        &cooldown,
        WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32 | ES_NUMBER as u32,
        COOLDOWN_ID,
        state.fonts.body,
    )?;
    send_message(cooldown_control, EM_LIMITTEXT, 19, 0);
    add_label(
        window,
        instance,
        state,
        ControlRole::CooldownError,
        "",
        state.fonts.body_small,
    )?;
    let notifications = add_control(
        window,
        instance,
        state,
        ControlRole::Notifications,
        "BUTTON",
        "Send low-battery notifications",
        WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        NOTIFICATIONS_ID,
        state.fonts.body,
    )?;
    if state
        .settings
        .as_ref()
        .is_some_and(|settings| settings.notifications_enabled)
    {
        send_message(notifications, BM_SETCHECK, BST_CHECKED as usize, 0);
    }
    let test = add_control(
        window,
        instance,
        state,
        ControlRole::TestNotification,
        "BUTTON",
        "Send a test notification",
        WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
        TEST_NOTIFICATION_ID,
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::TestStatus,
        "",
        state.fonts.body_small,
    )?;
    let battery_divider = add_control(
        window,
        instance,
        state,
        ControlRole::BatteryDivider,
        "STATIC",
        "",
        WS_VISIBLE | windows_sys::Win32::System::SystemServices::SS_ETCHEDHORZ,
        0,
        state.fonts.body,
    )?;

    add_label(
        window,
        instance,
        state,
        ControlRole::TrayHeading,
        "Tray icon",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::TrayDescription,
        "Choose what the icon shows and which battery stays visible.",
        state.fonts.body_small,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::IconStyleLabel,
        "Icon contents",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::IconStyleHelp,
        "Battery level shows a fill gauge; Percentage only shows a number.",
        state.fonts.body_small,
    )?;
    let icon_style = create_combo(
        window,
        instance,
        state,
        ControlRole::IconStyle,
        ICON_STYLE_ID,
        &["Battery level", "Percentage only"],
    )?;
    set_combo_selection(
        icon_style,
        state.settings.as_ref().map_or(0, |settings| {
            usize::from(settings.tray_icon_style == "percentage")
        }),
    );

    add_label(
        window,
        instance,
        state,
        ControlRole::IconThemeLabel,
        "Icon theme",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::IconThemeHelp,
        "Match Windows follows the system appearance; Light and Dark stay fixed.",
        state.fonts.body_small,
    )?;
    let icon_theme = create_combo(
        window,
        instance,
        state,
        ControlRole::IconTheme,
        ICON_THEME_ID,
        &["Match Windows", "Light", "Dark"],
    )?;
    set_combo_selection(
        icon_theme,
        state
            .settings
            .as_ref()
            .map_or(0, |settings| match settings.tray_icon_theme.as_str() {
                "light" => 1,
                "dark" => 2,
                _ => 0,
            }),
    );

    let percentage_overlay = add_control(
        window,
        instance,
        state,
        ControlRole::PercentageOverlay,
        "BUTTON",
        "Show battery percentage inside the battery icon",
        WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        PERCENTAGE_OVERLAY_ID,
        state.fonts.body,
    )?;
    if state
        .settings
        .as_ref()
        .is_some_and(|settings| settings.show_percentage_in_battery_icon)
    {
        send_message(percentage_overlay, BM_SETCHECK, BST_CHECKED as usize, 0);
    }
    add_label(
        window,
        instance,
        state,
        ControlRole::PercentageHelp,
        "The number is drawn over the battery glyph; it has no effect with Percentage only.",
        state.fonts.body_small,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::PinnedDeviceLabel,
        "Battery shown in the tray",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::PinnedDeviceHelp,
        "Choose one device to pin, or show the lowest monitored battery.",
        state.fonts.body_small,
    )?;
    let pinned_device = create_combo(
        window,
        instance,
        state,
        ControlRole::PinnedDevice,
        PINNED_DEVICE_ID,
        &[],
    )?;
    state.pin_values.push(None);
    add_combo_string(pinned_device, "Lowest monitored battery")?;
    let mut selected_pin = 0;
    for device in inventory {
        if device.identity.is_empty() {
            continue;
        }
        let label = format!("{} — {}", device.name, device.identity);
        add_combo_string(pinned_device, &label)?;
        state.pin_values.push(Some(device.identity.clone()));
        if state
            .settings
            .as_ref()
            .is_some_and(|settings| settings.pinned_device.as_deref() == Some(&device.identity))
        {
            selected_pin = state.pin_values.len() - 1;
        }
    }
    if let Some(pinned) = state
        .settings
        .as_ref()
        .and_then(|settings| settings.pinned_device.as_deref())
        .filter(|pinned| {
            !state
                .pin_values
                .iter()
                .any(|value| value.as_deref() == Some(pinned))
        })
    {
        add_combo_string(pinned_device, &format!("Unavailable device — {pinned}"))?;
        state.pin_values.push(Some(pinned.to_owned()));
        selected_pin = state.pin_values.len() - 1;
    }
    set_combo_selection(pinned_device, selected_pin);

    let tray_divider = add_control(
        window,
        instance,
        state,
        ControlRole::TrayDivider,
        "STATIC",
        "",
        WS_VISIBLE | windows_sys::Win32::System::SystemServices::SS_ETCHEDHORZ,
        0,
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::StartupHeading,
        "Windows startup",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::StartupDescription,
        "Keep Volty running in the background when you sign in.",
        state.fonts.body_small,
    )?;
    let startup = add_control(
        window,
        instance,
        state,
        ControlRole::Startup,
        "BUTTON",
        "Start automatically when I sign in",
        WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX as u32,
        STARTUP_ID,
        state.fonts.body,
    )?;
    if state
        .settings
        .as_ref()
        .is_some_and(|settings| settings.start_on_boot == Some(true))
    {
        send_message(startup, BM_SETCHECK, BST_CHECKED as usize, 0);
    }
    let startup_error = state
        .settings
        .as_ref()
        .and_then(|settings| settings.startup_error.as_deref())
        .map_or_else(String::new, |error| {
            format!("Windows startup setting could not be read: {error}")
        });
    add_label(
        window,
        instance,
        state,
        ControlRole::StartupError,
        &startup_error,
        state.fonts.body_small,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::Status,
        "",
        state.fonts.body_small,
    )?;
    let save = add_control(
        window,
        instance,
        state,
        ControlRole::Save,
        "BUTTON",
        "Save changes",
        WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        SAVE_ID,
        state.fonts.body,
    )?;

    for control in [
        heading,
        description,
        overview,
        config,
        read_only,
        test,
        battery_divider,
        tray_divider,
        save,
    ] {
        if control.is_null() {
            return Err(io::Error::other("could not create a Settings control").into());
        }
    }
    state.original = current_form(state);
    refresh_controls(window, state);
    layout_controls(window, state);
    Ok(())
}

fn add_label(
    window: HWND,
    instance: windows_sys::Win32::Foundation::HINSTANCE,
    state: &mut SettingsWindow,
    role: ControlRole,
    text: &str,
    font: windows_sys::Win32::Graphics::Gdi::HFONT,
) -> io::Result<HWND> {
    add_control(
        window, instance, state, role, "STATIC", text, WS_VISIBLE, 0, font,
    )
}

#[allow(clippy::too_many_arguments)]
fn add_control(
    window: HWND,
    instance: windows_sys::Win32::Foundation::HINSTANCE,
    state: &mut SettingsWindow,
    role: ControlRole,
    class_name: &str,
    text: &str,
    style: u32,
    id: usize,
    font: windows_sys::Win32::Graphics::Gdi::HFONT,
) -> io::Result<HWND> {
    let class = win32::to_wide(class_name);
    let text = win32::to_wide(text);
    let handle = win32::create_control(
        window,
        instance,
        ControlSpec {
            class: &class,
            text: &text,
            extended_style: u32::from(class_name == "EDIT") * WS_EX_CLIENTEDGE,
            style: WS_CHILD | WS_VISIBLE | style,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            id,
        },
    )?;
    set_control_font(handle, font);
    state.controls.push(LayoutControl { handle, role });
    Ok(handle)
}

fn create_combo(
    window: HWND,
    instance: windows_sys::Win32::Foundation::HINSTANCE,
    state: &mut SettingsWindow,
    role: ControlRole,
    id: usize,
    values: &[&str],
) -> io::Result<HWND> {
    let handle = add_control(
        window,
        instance,
        state,
        role,
        "COMBOBOX",
        "",
        WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32 | CBS_HASSTRINGS as u32,
        id,
        state.fonts.body,
    )?;
    for value in values {
        add_combo_string(handle, value)?;
    }
    Ok(handle)
}

fn add_combo_string(combo: HWND, value: &str) -> io::Result<()> {
    let value = win32::to_wide(value);
    let result = send_message(
        combo,
        CB_ADDSTRING,
        0,
        value.as_ptr().expose_provenance().cast_signed(),
    );
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn set_combo_selection(combo: HWND, index: usize) {
    send_message(combo, CB_SETCURSEL, index, 0);
}

fn layout_controls(window: HWND, state: &SettingsWindow) {
    let mut client = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: client is writable and window is the live native Settings window.
    if unsafe { GetClientRect(window, ptr::addr_of_mut!(client)) } == 0 {
        eprintln!(
            "Volty could not read the Settings client area: {}",
            io::Error::last_os_error()
        );
        return;
    }
    let client_width = client.right - client.left;
    let width = client_width.min(1_000);
    let horizontal_offset = (client_width - width) / 2;
    let control_width = 250.min((width - MARGIN * 2) / 3).max(210);
    let control_x = width - MARGIN - control_width;
    let left_width = control_x - MARGIN * 2;
    let place = |role: ControlRole, x: i32, y: i32, w: i32, h: i32| {
        if let Some(control) = state.controls.iter().find(|control| control.role == role) {
            // SAFETY: the control is a live child of this Settings window.
            if unsafe {
                MoveWindow(
                    control.handle,
                    x + horizontal_offset,
                    y,
                    w.max(1),
                    h.max(1),
                    1,
                )
            } == 0
            {
                eprintln!(
                    "Volty could not lay out a Settings control: {}",
                    io::Error::last_os_error()
                );
            }
        }
    };

    place(ControlRole::Heading, MARGIN, 22, left_width, 38);
    place(ControlRole::Description, MARGIN, 63, width - MARGIN * 2, 24);
    place(ControlRole::Overview, width - MARGIN - 120, 26, 120, 34);
    place(ControlRole::ConfigError, MARGIN, 90, width - MARGIN * 2, 24);
    place(ControlRole::ReadOnly, MARGIN, 90, width - MARGIN * 2, 24);

    place(
        ControlRole::BatteryHeading,
        MARGIN,
        122,
        width - MARGIN * 2,
        22,
    );
    place(
        ControlRole::BatteryDescription,
        MARGIN,
        146,
        width - MARGIN * 2,
        20,
    );
    place(ControlRole::ThresholdLabel, MARGIN, 176, left_width, 20);
    place(ControlRole::ThresholdHelp, MARGIN, 198, left_width, 40);
    place(
        ControlRole::ThresholdEdit,
        control_x,
        174,
        control_width,
        30,
    );
    place(
        ControlRole::ThresholdError,
        control_x,
        207,
        control_width,
        34,
    );
    place(ControlRole::CooldownLabel, MARGIN, 251, left_width, 20);
    place(ControlRole::CooldownHelp, MARGIN, 273, left_width, 30);
    place(ControlRole::CooldownEdit, control_x, 249, control_width, 30);
    place(
        ControlRole::CooldownError,
        control_x,
        282,
        control_width,
        28,
    );
    place(ControlRole::Notifications, MARGIN, 314, left_width, 28);
    place(
        ControlRole::TestNotification,
        width - MARGIN - 220,
        310,
        220,
        34,
    );
    place(ControlRole::TestStatus, width - MARGIN - 220, 346, 220, 28);
    place(
        ControlRole::BatteryDivider,
        MARGIN,
        380,
        width - MARGIN * 2,
        2,
    );

    place(
        ControlRole::TrayHeading,
        MARGIN,
        399,
        width - MARGIN * 2,
        22,
    );
    place(
        ControlRole::TrayDescription,
        MARGIN,
        423,
        width - MARGIN * 2,
        20,
    );
    place(ControlRole::IconStyleLabel, MARGIN, 453, left_width, 20);
    place(ControlRole::IconStyleHelp, MARGIN, 475, left_width, 30);
    place(ControlRole::IconStyle, control_x, 451, control_width, 30);
    place(ControlRole::IconThemeLabel, MARGIN, 515, left_width, 20);
    place(ControlRole::IconThemeHelp, MARGIN, 537, left_width, 30);
    place(ControlRole::IconTheme, control_x, 513, control_width, 30);
    place(
        ControlRole::PercentageOverlay,
        MARGIN,
        560,
        width - MARGIN * 2,
        28,
    );
    place(
        ControlRole::PercentageHelp,
        MARGIN + 22,
        585,
        width - MARGIN * 2 - 22,
        26,
    );
    place(ControlRole::PinnedDeviceLabel, MARGIN, 617, left_width, 20);
    place(ControlRole::PinnedDeviceHelp, MARGIN, 639, left_width, 30);
    place(ControlRole::PinnedDevice, control_x, 615, control_width, 30);
    place(ControlRole::TrayDivider, MARGIN, 674, width - MARGIN * 2, 2);

    place(
        ControlRole::StartupHeading,
        MARGIN,
        693,
        width - MARGIN * 2,
        22,
    );
    place(
        ControlRole::StartupDescription,
        MARGIN,
        717,
        width - MARGIN * 2,
        20,
    );
    place(ControlRole::Startup, MARGIN, 743, width - MARGIN * 2, 28);
    place(
        ControlRole::StartupError,
        MARGIN,
        773,
        width - MARGIN * 2,
        22,
    );
    place(
        ControlRole::Status,
        MARGIN,
        804,
        width - MARGIN * 2 - 150,
        26,
    );
    place(ControlRole::Save, width - MARGIN - 140, 798, 140, 36);
}

fn control(state: &SettingsWindow, role: ControlRole) -> HWND {
    state
        .controls
        .iter()
        .find(|control| control.role == role)
        .map_or(ptr::null_mut(), |control| control.handle)
}

fn control_by_id(window: HWND, id: usize) -> HWND {
    // SAFETY: GetDlgItem returns a child control associated with this live window.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetDlgItem(window, id as i32) }
}

fn send_message(window: HWND, message: u32, wparam: usize, lparam: LPARAM) -> LRESULT {
    // SAFETY: callers pass a live child control and message-specific values.
    unsafe { SendMessageW(window, message, wparam, lparam) }
}

fn get_control_text(window: HWND) -> String {
    // SAFETY: window is a live edit control and the call only queries its text length.
    let length = unsafe { GetWindowTextLengthW(window) }.max(0) as usize;
    let mut buffer = vec![0_u16; length + 1];
    // SAFETY: buffer has room for length plus its NUL terminator and window is live.
    let copied = unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
}

fn check_control(window: HWND) -> bool {
    send_message(window, BM_GETCHECK, 0, 0) == BST_CHECKED as isize
}

fn current_form(state: &SettingsWindow) -> Option<FormValues> {
    let settings = state.settings.as_ref()?;
    let pinned_selection = send_message(
        control(state, ControlRole::PinnedDevice),
        CB_GETCURSEL,
        0,
        0,
    );
    let start_on_boot = settings
        .start_on_boot
        .map(|_| check_control(control(state, ControlRole::Startup)));
    Some(FormValues {
        thresholds: get_control_text(control(state, ControlRole::ThresholdEdit)),
        cooldown: get_control_text(control(state, ControlRole::CooldownEdit)),
        notifications: check_control(control(state, ControlRole::Notifications)),
        icon_style: send_message(control(state, ControlRole::IconStyle), CB_GETCURSEL, 0, 0).max(0)
            as usize,
        icon_theme: send_message(control(state, ControlRole::IconTheme), CB_GETCURSEL, 0, 0).max(0)
            as usize,
        percentage_overlay: check_control(control(state, ControlRole::PercentageOverlay)),
        pinned_device: pinned_selection.max(0) as usize,
        start_on_boot,
    })
}

fn validate_thresholds(value: &str) -> Result<Vec<u8>, &'static str> {
    let values = value
        .split(',')
        .map(str::trim)
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("Enter whole percentages separated by commas.");
            }
            let percentage = part
                .parse::<u8>()
                .map_err(|_| "Each percentage must be from 1 to 100.")?;
            if !(1..=100).contains(&percentage) {
                return Err("Each percentage must be from 1 to 100.");
            }
            Ok(percentage)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if values.is_empty() {
        Err("Enter at least one battery percentage.")
    } else {
        Ok(values)
    }
}

fn validate_cooldown(value: &str) -> Result<u64, &'static str> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Enter a whole number of minutes.");
    }
    let minutes = value
        .parse::<u64>()
        .map_err(|_| "The repeat interval is too large.")?;
    if minutes > i64::MAX as u64 {
        return Err("The repeat interval is too large.");
    }
    Ok(minutes)
}

fn update_validation(window: HWND, state: &SettingsWindow) -> bool {
    let threshold = get_control_text(control(state, ControlRole::ThresholdEdit));
    let cooldown = get_control_text(control(state, ControlRole::CooldownEdit));
    let threshold_error = validate_thresholds(&threshold).err().unwrap_or("");
    let cooldown_error = validate_cooldown(&cooldown).err().unwrap_or("");
    set_control_text(control(state, ControlRole::ThresholdError), threshold_error);
    set_control_text(control(state, ControlRole::CooldownError), cooldown_error);
    let valid = threshold_error.is_empty() && cooldown_error.is_empty();
    let dirty = current_form(state).is_some_and(|form| state.original.as_ref() != Some(&form));
    set_control_text(
        control(state, ControlRole::Status),
        if dirty { "Unsaved changes" } else { "" },
    );
    let can_save = state.settings.is_some()
        && !state
            .settings
            .as_ref()
            .is_some_and(|settings| settings.config_error.is_some())
        && dirty
        && valid;
    // SAFETY: SAVE_ID resolves to the live Save button owned by the settings window.
    unsafe { EnableWindow(control_by_id(window, SAVE_ID), i32::from(can_save)) };
    valid
}

fn refresh_controls(window: HWND, state: &SettingsWindow) {
    let editable = state
        .settings
        .as_ref()
        .is_some_and(|settings| settings.config_error.is_none());
    for role in [
        ControlRole::ThresholdEdit,
        ControlRole::CooldownEdit,
        ControlRole::Notifications,
        ControlRole::IconStyle,
        ControlRole::IconTheme,
        ControlRole::PercentageOverlay,
        ControlRole::PinnedDevice,
    ] {
        // SAFETY: each role corresponds to a live child control created by this window.
        unsafe { EnableWindow(control(state, role), i32::from(editable)) };
    }
    let startup_available = editable
        && state
            .settings
            .as_ref()
            .is_some_and(|settings| settings.start_on_boot.is_some());
    // SAFETY: STARTUP_ID resolves to the live Windows startup checkbox.
    unsafe {
        EnableWindow(
            control_by_id(window, STARTUP_ID),
            i32::from(startup_available),
        )
    };
    // SAFETY: the test action is a live child button; it is unavailable in read-only mode.
    unsafe {
        EnableWindow(
            control_by_id(window, TEST_NOTIFICATION_ID),
            i32::from(editable),
        )
    };
    let _ = update_validation(window, state);
}

fn set_control_text(window: HWND, text: &str) {
    let text = win32::to_wide(text);
    // SAFETY: text is NUL-terminated and lives through this synchronous update.
    if unsafe { SetWindowTextW(window, text.as_ptr()) } == 0 {
        eprintln!(
            "Volty could not update a Settings control: {}",
            io::Error::last_os_error()
        );
    }
}

fn handle_command(window: HWND, state: &mut SettingsWindow, wparam: WPARAM) -> LRESULT {
    let id = wparam & 0xffff;
    let notification = (wparam >> 16) & 0xffff;
    match id {
        OVERVIEW_ID => {
            let _ = open_overview(window, state);
            0
        }
        TEST_NOTIFICATION_ID => {
            let status = match state.backend.test_notification() {
                Ok(()) => "Test notification sent.".to_owned(),
                Err(error) => format!("Could not send test notification: {error}"),
            };
            set_control_text(control(state, ControlRole::TestStatus), &status);
            0
        }
        SAVE_ID => {
            save_settings(window, state);
            0
        }
        THRESHOLDS_ID | COOLDOWN_ID
            if notification == windows_sys::Win32::UI::WindowsAndMessaging::EN_CHANGE as usize =>
        {
            let _ = update_validation(window, state);
            0
        }
        NOTIFICATIONS_ID
        | ICON_STYLE_ID
        | ICON_THEME_ID
        | PERCENTAGE_OVERLAY_ID
        | PINNED_DEVICE_ID
        | STARTUP_ID => {
            let _ = update_validation(window, state);
            0
        }
        value if value == IDCANCEL as usize => {
            close_window(window, state);
            0
        }
        _ => 0,
    }
}

fn save_settings(window: HWND, state: &mut SettingsWindow) {
    if !update_validation(window, state) {
        return;
    }
    let Some(settings) = state.settings.as_ref() else {
        return;
    };
    let Some(form) = current_form(state) else {
        return;
    };
    let thresholds = match validate_thresholds(&form.thresholds) {
        Ok(thresholds) => thresholds,
        Err(error) => {
            set_control_text(control(state, ControlRole::Status), error);
            return;
        }
    };
    let cooldown = match validate_cooldown(&form.cooldown) {
        Ok(cooldown) => cooldown,
        Err(error) => {
            set_control_text(control(state, ControlRole::Status), error);
            return;
        }
    };
    let style = match form.icon_style {
        0 => "battery",
        1 => "percentage",
        _ => {
            set_control_text(
                control(state, ControlRole::Status),
                "Choose a valid tray icon style.",
            );
            return;
        }
    };
    let theme = match form.icon_theme {
        0 => "system",
        1 => "light",
        2 => "dark",
        _ => {
            set_control_text(
                control(state, ControlRole::Status),
                "Choose a valid tray icon theme.",
            );
            return;
        }
    };
    let update = DashboardSettingsUpdate {
        low_battery_thresholds: thresholds,
        low_battery_cooldown_minutes: cooldown,
        notifications_enabled: form.notifications,
        tray_icon_style: style.to_owned(),
        tray_icon_theme: theme.to_owned(),
        show_percentage_in_battery_icon: form.percentage_overlay,
        pinned_device: state.pin_values.get(form.pinned_device).cloned().flatten(),
        start_on_boot: settings
            .start_on_boot
            .map(|_| form.start_on_boot.unwrap_or(false)),
    };
    if let Err(error) = state.backend.update_settings(&update) {
        set_control_text(
            control(state, ControlRole::Status),
            &format!("Could not save settings: {error}"),
        );
        return;
    }

    match state.backend.get_dashboard_data(state.days) {
        Ok(data) => {
            state.settings = data.settings;
            if let Err(error) = refresh_pinned_values(state, &data.inventory) {
                set_control_text(
                    control(state, ControlRole::Status),
                    &format!("Settings saved, but the tray device list could not refresh: {error}"),
                );
                return;
            }
            apply_saved_settings(state);
            state.original = current_form(state);
            refresh_controls(window, state);
            set_control_text(
                control(state, ControlRole::Status),
                "Settings saved. Tray changes apply immediately; alerts use the next reading.",
            );
        }
        Err(error) => set_control_text(
            control(state, ControlRole::Status),
            &format!("Settings were saved, but could not be reloaded: {error}"),
        ),
    }
}

fn refresh_pinned_values(
    state: &mut SettingsWindow,
    inventory: &[DashboardInventoryDevice],
) -> io::Result<()> {
    let combo = control(state, ControlRole::PinnedDevice);
    send_message(
        combo,
        windows_sys::Win32::UI::WindowsAndMessaging::CB_RESETCONTENT,
        0,
        0,
    );
    state.pin_values.clear();
    state.pin_values.push(None);
    add_combo_string(combo, "Lowest monitored battery")?;
    let pinned = state
        .settings
        .as_ref()
        .and_then(|settings| settings.pinned_device.clone());
    let mut selected = 0;
    for device in inventory {
        if device.identity.is_empty() {
            continue;
        }
        add_combo_string(combo, &format!("{} — {}", device.name, device.identity))?;
        state.pin_values.push(Some(device.identity.clone()));
        if pinned.as_deref() == Some(&device.identity) {
            selected = state.pin_values.len() - 1;
        }
    }
    if let Some(pinned) = pinned.filter(|pinned| {
        !state
            .pin_values
            .iter()
            .any(|value| value.as_deref() == Some(pinned))
    }) {
        add_combo_string(combo, &format!("Unavailable device — {pinned}"))?;
        state.pin_values.push(Some(pinned));
        selected = state.pin_values.len() - 1;
    }
    set_combo_selection(combo, selected);
    Ok(())
}

fn apply_saved_settings(state: &SettingsWindow) {
    let Some(settings) = &state.settings else {
        return;
    };
    set_control_text(
        control(state, ControlRole::ThresholdEdit),
        &settings
            .low_battery_thresholds
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    );
    set_control_text(
        control(state, ControlRole::CooldownEdit),
        &settings.low_battery_cooldown_minutes.to_string(),
    );
    set_checkbox(
        control(state, ControlRole::Notifications),
        settings.notifications_enabled,
    );
    set_combo_selection(
        control(state, ControlRole::IconStyle),
        usize::from(settings.tray_icon_style == "percentage"),
    );
    set_combo_selection(
        control(state, ControlRole::IconTheme),
        match settings.tray_icon_theme.as_str() {
            "light" => 1,
            "dark" => 2,
            _ => 0,
        },
    );
    set_checkbox(
        control(state, ControlRole::PercentageOverlay),
        settings.show_percentage_in_battery_icon,
    );
    set_checkbox(
        control(state, ControlRole::Startup),
        settings.start_on_boot == Some(true),
    );
}

fn set_checkbox(window: HWND, checked: bool) {
    send_message(
        window,
        BM_SETCHECK,
        if checked {
            BST_CHECKED as usize
        } else {
            BST_UNCHECKED as usize
        },
        0,
    );
}

fn open_overview(window: HWND, state: &SettingsWindow) -> bool {
    if current_form(state).is_some_and(|form| state.original.as_ref() != Some(&form)) {
        let message = win32::to_wide("Discard unsaved Settings changes and open Overview?");
        let title = win32::to_wide(WINDOW_TITLE);
        // SAFETY: the live Settings window is a valid owner for this confirmation dialog.
        let response = unsafe {
            MessageBoxW(
                window,
                message.as_ptr(),
                title.as_ptr(),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
            )
        };
        if response != IDYES {
            return false;
        }
    }

    let result = (|| -> io::Result<()> {
        let executable = std::env::current_exe()?;
        let mut command = Command::new(executable);
        command.args(["dashboard", "--window", "--days"]);
        command.arg(state.days.to_string());
        if let Some(database) = &state.database {
            command.arg("--database").arg(database);
        }
        use std::os::windows::process::CommandExt;
        command
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
    })();
    match result {
        Ok(()) => {
            if let Err(error) = win32::destroy_window(window) {
                eprintln!("Volty could not close Settings after opening Overview: {error}");
            }
            true
        }
        Err(error) => {
            if let Some(control) = state
                .controls
                .iter()
                .find(|control| control.role == ControlRole::Status)
            {
                set_control_text(control.handle, &format!("Could not open Overview: {error}"));
            }
            false
        }
    }
}

fn close_window(window: HWND, state: &SettingsWindow) {
    if current_form(state).is_some_and(|form| state.original.as_ref() != Some(&form)) {
        let message = win32::to_wide("Discard unsaved Settings changes and close this window?");
        let title = win32::to_wide(WINDOW_TITLE);
        // SAFETY: the live Settings window is a valid owner for this confirmation dialog.
        let response = unsafe {
            MessageBoxW(
                window,
                message.as_ptr(),
                title.as_ptr(),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
            )
        };
        if response != IDYES {
            return;
        }
    }
    if let Err(error) = win32::destroy_window(window) {
        eprintln!("Volty could not close the Settings window: {error}");
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: WM_NCCREATE supplies this window's valid CREATESTRUCTW in lParam.
        if !unsafe { win32::attach_window_state(window, lparam) } {
            return 0;
        }
    }

    match message {
        WM_CTLCOLORSTATIC => static_control_color(wparam),
        windows_sys::Win32::UI::WindowsAndMessaging::WM_CTLCOLOREDIT => edit_control_color(wparam),
        WM_COMMAND => {
            let state = win32::window_state::<SettingsWindow>(window);
            if state.is_null() {
                0
            } else {
                // SAFETY: the attached Box remains alive until the message loop exits.
                handle_command(window, unsafe { &mut *state }, wparam)
            }
        }
        WM_SIZE => {
            let state = win32::window_state::<SettingsWindow>(window);
            if !state.is_null() {
                // SAFETY: the attached Box remains alive until the message loop exits.
                layout_controls(window, unsafe { &*state });
            }
            0
        }
        WM_SETFOCUS => {
            let state = win32::window_state::<SettingsWindow>(window);
            if !state.is_null() {
                // SAFETY: the attached state and its threshold control live with this window.
                unsafe { SetFocus(control(&*state, ControlRole::ThresholdEdit)) };
            }
            0
        }
        SETTINGS_ACTIVATE_MESSAGE => {
            // SAFETY: window is the live Settings window receiving a posted activation message.
            unsafe {
                ShowWindow(
                    window,
                    windows_sys::Win32::UI::WindowsAndMessaging::SW_RESTORE,
                );
                windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(window);
            }
            0
        }
        WM_GETMINMAXINFO => {
            if lparam != 0 {
                // SAFETY: WM_GETMINMAXINFO supplies a writable MINMAXINFO in lParam.
                let info = unsafe {
                    &mut *ptr::with_exposed_provenance_mut::<MINMAXINFO>(lparam.cast_unsigned())
                };
                info.ptMinTrackSize.x = MIN_WINDOW_WIDTH;
                info.ptMinTrackSize.y = MIN_WINDOW_HEIGHT;
            }
            0
        }
        WM_CLOSE => {
            let state = win32::window_state::<SettingsWindow>(window);
            if !state.is_null() {
                // SAFETY: the attached Box remains alive until the message loop exits.
                close_window(window, unsafe { &*state });
            } else {
                destroy_window(window);
            }
            0
        }
        WM_DESTROY => {
            let state = win32::window_state::<SettingsWindow>(window);
            if !state.is_null() {
                // SAFETY: the attached Box remains alive until the message loop exits.
                win32::quit_after_creation(unsafe { (*state).create_complete });
            }
            0
        }
        _ => win32::default_window_proc(window, message, wparam, lparam),
    }
}

fn destroy_window(window: HWND) {
    if let Err(error) = win32::destroy_window(window) {
        eprintln!("Volty could not destroy the Settings window: {error}");
    }
}

struct EventHandle(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: named synchronization handles may be waited on from a worker thread.
unsafe impl Send for EventHandle {}

impl EventHandle {
    fn open(name: &str) -> io::Result<Self> {
        use windows_sys::Win32::System::Threading::{OpenEventW, SYNCHRONIZATION_SYNCHRONIZE};

        let name = win32::to_wide(name);
        // SAFETY: name is NUL-terminated and only synchronization access is requested.
        let handle = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns the event handle returned by OpenEventW.
        if unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) } == 0 {
            eprintln!(
                "Volty could not close a Settings event handle: {}",
                io::Error::last_os_error()
            );
        }
    }
}

struct SendableWindow(HWND);

// SAFETY: HWND values may be used by other threads for documented window messaging APIs.
unsafe impl Send for SendableWindow {}

impl SendableWindow {
    fn post_message(&self, message: u32) -> bool {
        // SAFETY: the owner keeps the Settings HWND alive until this event listener has stopped.
        unsafe { PostMessageW(self.0, message, 0, 0) != 0 }
    }
}

struct SettingsEventListener {
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl SettingsEventListener {
    fn start(window: HWND) -> io::Result<Self> {
        use windows_sys::Win32::System::Threading::WaitForMultipleObjects;

        let events = [
            EventHandle::open(crate::DASHBOARD_SETTINGS_ACTIVATION_EVENT)?,
            EventHandle::open(crate::DASHBOARD_SETTINGS_SHUTDOWN_EVENT)?,
        ];
        let window = SendableWindow(window);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker = thread::Builder::new()
            .name("volty-settings-events".to_owned())
            .spawn(move || {
                let handles = [events[0].0, events[1].0];
                while !worker_stopped.load(Ordering::Acquire) {
                    // SAFETY: both event handles are valid and remain owned for the worker lifetime.
                    let result = unsafe {
                        WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, 250)
                    };
                    match result {
                        WAIT_OBJECT_0 => {
                            // SAFETY: window remains live until this listener is stopped during teardown.
                            if !window.post_message(SETTINGS_ACTIVATE_MESSAGE) {
                                eprintln!(
                                    "Volty could not activate the Settings window: {}",
                                    io::Error::last_os_error()
                                );
                            }
                        }
                        value if value == WAIT_OBJECT_0 + 1 => {
                            // SAFETY: window remains live until its listener is stopped.
                            let _ = window.post_message(WM_CLOSE);
                            break;
                        }
                        WAIT_TIMEOUT => {}
                        WAIT_FAILED => {
                            eprintln!(
                                "Volty could not wait for Settings events: {}",
                                io::Error::last_os_error()
                            );
                            break;
                        }
                        value => {
                            eprintln!("Unexpected Settings event wait result: {value}");
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            stopped,
            worker: Some(worker),
        })
    }

    fn stop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                eprintln!("The Settings event listener panicked");
            }
        }
    }
}

impl Drop for SettingsEventListener {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) const fn instance_mutex_name() -> &'static str {
    SETTINGS_INSTANCE_MUTEX
}

#[cfg(test)]
mod tests {
    use super::{validate_cooldown, validate_thresholds};

    #[test]
    fn threshold_validation_accepts_multiple_whole_percentages() {
        assert_eq!(validate_thresholds("20, 10, 5"), Ok(vec![20, 10, 5]));
    }

    #[test]
    fn threshold_validation_rejects_empty_and_out_of_range_entries() {
        assert!(validate_thresholds("").is_err());
        assert!(validate_thresholds("20,,5").is_err());
        assert!(validate_thresholds("0, 5").is_err());
        assert!(validate_thresholds("101").is_err());
    }

    #[test]
    fn repeat_interval_accepts_zero_and_rejects_overflow() {
        assert_eq!(validate_cooldown("0"), Ok(0));
        assert!(validate_cooldown("-1").is_err());
        assert!(validate_cooldown("9223372036854775808").is_err());
    }
}
