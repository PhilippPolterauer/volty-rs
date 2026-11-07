use std::{
    error::Error,
    io, ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

use windows_sys::Win32::{
    Foundation::{
        COLORREF, HWND, LPARAM, LRESULT, RECT, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::Dialogs::{ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW},
        Controls::{
            InitCommonControlsEx, BST_CHECKED, BST_UNCHECKED, EM_LIMITTEXT, ICC_BAR_CLASSES,
            INITCOMMONCONTROLSEX, TBM_SETPOS, TBM_SETRANGEMAX, TBM_SETRANGEMIN,
        },
        Input::KeyboardAndMouse::EnableWindow,
        WindowsAndMessaging::{
            CreateWindowExW, GetClientRect, GetWindowTextLengthW, GetWindowTextW, MessageBoxW,
            PostMessageW, SendMessageW, SetWindowPos, SetWindowTextW, ShowWindow, BM_GETCHECK,
            BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CBS_DROPDOWNLIST,
            CBS_HASSTRINGS, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, ES_AUTOHSCROLL, ES_NUMBER,
            IDCANCEL, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO, MINMAXINFO, SWP_NOACTIVATE,
            SWP_NOREDRAW, SWP_NOZORDER, SW_SHOWNA, SW_SHOWNORMAL, WM_APP, WM_CLOSE, WM_COMMAND,
            WM_CTLCOLORSTATIC, WM_DESTROY, WM_GETMINMAXINFO, WM_HSCROLL, WM_NCCREATE, WM_SIZE,
            WS_CHILD, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
        },
    },
};

use crate::{
    config::{AppTheme, BatteryColorMap, DashboardSettingsUpdate},
    native_ui::{set_control_font, FontSet},
    win32::{self, ControlSpec},
};

use super::{
    management::DashboardBackend,
    model::{DashboardInventoryDevice, DashboardSettings},
};

mod appearance;
use appearance::Appearance;

const WINDOW_CLASS: &str = "VoltyNativeSettingsWindow";
const WINDOW_TITLE: &str = "Volty Settings";
const WINDOW_WIDTH: i32 = 940;
const WINDOW_HEIGHT: i32 = 820;
const MIN_WINDOW_WIDTH: i32 = 820;
const MIN_WINDOW_HEIGHT: i32 = 820;
const MARGIN: i32 = 32;
const TOP_CARD_GAP: i32 = 20;
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
const APP_THEME_ID: usize = 1012;
const RED_CUTOFF_ID: usize = 1013;
const YELLOW_CUTOFF_ID: usize = 1014;
const RED_COLOR_ID: usize = 1015;
const YELLOW_COLOR_ID: usize = 1016;
const GREEN_COLOR_ID: usize = 1017;
const PICK_RED_ID: usize = 1018;
const PICK_YELLOW_ID: usize = 1019;
const PICK_GREEN_ID: usize = 1020;
const PREVIEW_LEVEL_ID: usize = 1021;
const SETTINGS_ACTIVATE_MESSAGE: u32 = WM_APP + 1;
const SETTINGS_INSTANCE_MUTEX: &str = "Local\\VoltySettings";

#[derive(Clone, Debug, PartialEq, Eq)]
struct FormValues {
    app_theme: usize,
    red_cutoff: String,
    yellow_cutoff: String,
    red_color: String,
    yellow_color: String,
    green_color: String,
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
    AppTheme,
    AppThemeLabel,
    Heading,
    Description,
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
    BatteryDivider,
    TrayHeading,
    TrayDescription,
    AppThemeHelp,
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
    ColorMapHeading,
    ColorMapDescription,
    RedCutoffLabel,
    YellowCutoffLabel,
    RedCutoff,
    YellowCutoff,
    ColorMapError,
    RedColorLabel,
    YellowColorLabel,
    GreenColorLabel,
    RedColor,
    YellowColor,
    GreenColor,
    PickRed,
    PickYellow,
    PickGreen,
    PreviewLabel,
    PreviewLevel,
    PreviewValue,
    StartupHeading,
    StartupDescription,
    Startup,
    StartupError,
    Status,
    Save,
}

impl ControlRole {
    fn is_combo(self) -> bool {
        matches!(
            self,
            Self::AppTheme | Self::IconStyle | Self::IconTheme | Self::PinnedDevice
        )
    }
}

struct LayoutControl {
    handle: HWND,
    role: ControlRole,
}

struct SettingsWindow {
    backend: DashboardBackend,
    days: u32,
    settings: Option<DashboardSettings>,
    pin_values: Vec<Option<String>>,
    controls: Vec<LayoutControl>,
    fonts: FontSet,
    appearance: Appearance,
    original: Option<FormValues>,
    preview_bgra: Vec<u8>,
    custom_colors: [COLORREF; 16],
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
    let common_controls = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_BAR_CLASSES,
    };
    // SAFETY: the structure contains the requested trackbar class group and its exact size.
    if unsafe { InitCommonControlsEx(&common_controls) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    win32::register_window_class(&class_name, window_proc, instance)?;

    let fonts = FontSet::new()?;
    let preference = data
        .settings
        .as_ref()
        .and_then(|s| AppTheme::parse(&s.app_theme))
        .unwrap_or_default();
    let appearance = Appearance::new(preference)?;
    let mut state = Box::new(SettingsWindow {
        backend,
        days,
        settings: data.settings,
        pin_values: Vec::new(),
        controls: Vec::new(),
        fonts,
        appearance,
        original: None,
        preview_bgra: Vec::new(),
        custom_colors: [0; 16],
        activation_listener: None,
        create_complete: false,
    });

    let (window_width, window_height) = initial_window_size();
    // SAFETY: class/title are NUL-terminated and live through creation; the Box-backed state pointer
    // remains valid until the window is destroyed and the message loop exits.
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | windows_sys::Win32::UI::WindowsAndMessaging::WS_CLIPCHILDREN,
            windows_sys::Win32::UI::WindowsAndMessaging::CW_USEDEFAULT,
            windows_sys::Win32::UI::WindowsAndMessaging::CW_USEDEFAULT,
            window_width,
            window_height,
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
    win32::allow_dark_mode_for_current_thread_windows();
    state.appearance.apply_native_theme(window, &state);
    state.create_complete = true;
    for control in &state.controls {
        // SAFETY: each created child is live and the parent remains hidden until all are shown.
        unsafe { ShowWindow(control.handle, SW_SHOWNA) };
    }
    state.activation_listener = Some(SettingsEventListener::start(window)?);
    win32::run_dialog_message_loop(window, SW_SHOWNORMAL, state, |state, value| {
        state.create_complete = value;
    })?;
    Ok(())
}

fn initial_window_size() -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA};

    let mut work_area = windows_sys::Win32::Foundation::RECT::default();
    // SAFETY: SPI_GETWORKAREA writes one RECT into valid writable storage.
    if unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, ptr::from_mut(&mut work_area).cast(), 0) }
        == 0
    {
        return (WINDOW_WIDTH, WINDOW_HEIGHT);
    }
    let available_width = (work_area.right - work_area.left - 16).max(MIN_WINDOW_WIDTH);
    let available_height = (work_area.bottom - work_area.top - 4).max(MIN_WINDOW_HEIGHT);
    (
        WINDOW_WIDTH.min(available_width),
        WINDOW_HEIGHT.min(available_height),
    )
}

pub(super) fn top_card_layout(width: i32) -> (i32, i32, i32) {
    let available = width - MARGIN * 2 - TOP_CARD_GAP;
    let left_width = available * 55 / 100;
    let right_width = available - left_width;
    (left_width, right_width, MARGIN + left_width + TOP_CARD_GAP)
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
        "Make Volty work the way you do.",
        WS_VISIBLE,
        0,
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
        "Choose when to hear about a low battery.",
        state.fonts.body_small,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::ThresholdLabel,
        "Alert thresholds",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::ThresholdHelp,
        "Comma separated: 20, 10, 5.",
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
        "Minutes between alerts; 0 = off.",
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
        "Low-battery notifications",
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
        "Test notification",
        WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
        TEST_NOTIFICATION_ID,
        state.fonts.body,
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
        "Tray icon and app appearance.",
        state.fonts.body_small,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::AppThemeLabel,
        "App theme",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::AppThemeHelp,
        "System, Light, or Dark.",
        state.fonts.body_small,
    )?;
    let app_theme = create_combo(
        window,
        instance,
        state,
        ControlRole::AppTheme,
        APP_THEME_ID,
        &["System", "Light", "Dark"],
    )?;
    let preference = state
        .settings
        .as_ref()
        .map_or("system", |settings| settings.app_theme.as_str());
    set_combo_selection(
        app_theme,
        match preference {
            "light" => 1,
            "dark" => 2,
            _ => 0,
        },
    );
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
        "Gauge or percentage.",
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
        "Match Windows or set fixed.",
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
        "Show percentage inside the battery icon",
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
        "Applies when icon contents is set to Battery level.",
        state.fonts.body_small,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::PinnedDeviceLabel,
        "Tray battery",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::PinnedDeviceHelp,
        "Pick a device or the lowest.",
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

    let color_map = state
        .settings
        .as_ref()
        .map_or_else(BatteryColorMap::default, |settings| BatteryColorMap {
            red_max_percent: settings.battery_color_red_max_percent,
            yellow_max_percent: settings.battery_color_yellow_max_percent,
            red: parse_hex_color(&settings.battery_color_red).unwrap_or([210, 48, 48]),
            yellow: parse_hex_color(&settings.battery_color_yellow).unwrap_or([235, 190, 35]),
            green: parse_hex_color(&settings.battery_color_green).unwrap_or([45, 170, 75]),
        });
    add_label(
        window,
        instance,
        state,
        ControlRole::ColorMapHeading,
        "Battery color map",
        state.fonts.body,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::ColorMapDescription,
        "Set cutoffs, choose colors, and preview the tray icon.",
        state.fonts.body_small,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::RedCutoffLabel,
        "Red at or below (%)",
        state.fonts.body_small,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::YellowCutoffLabel,
        "Yellow through (%)",
        state.fonts.body_small,
    )?;
    let red_cutoff = add_control(
        window,
        instance,
        state,
        ControlRole::RedCutoff,
        "EDIT",
        &color_map.red_max_percent.to_string(),
        WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32 | ES_NUMBER as u32,
        RED_CUTOFF_ID,
        state.fonts.body,
    )?;
    send_message(red_cutoff, EM_LIMITTEXT, 3, 0);
    let yellow_cutoff = add_control(
        window,
        instance,
        state,
        ControlRole::YellowCutoff,
        "EDIT",
        &color_map.yellow_max_percent.to_string(),
        WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32 | ES_NUMBER as u32,
        YELLOW_CUTOFF_ID,
        state.fonts.body,
    )?;
    send_message(yellow_cutoff, EM_LIMITTEXT, 3, 0);
    add_label(
        window,
        instance,
        state,
        ControlRole::ColorMapError,
        "",
        state.fonts.body_small,
    )?;
    for (role, label_role, color, id) in [
        (
            ControlRole::RedColor,
            ControlRole::RedColorLabel,
            format!(
                "#{:02X}{:02X}{:02X}",
                color_map.red[0], color_map.red[1], color_map.red[2]
            ),
            RED_COLOR_ID,
        ),
        (
            ControlRole::YellowColor,
            ControlRole::YellowColorLabel,
            format!(
                "#{:02X}{:02X}{:02X}",
                color_map.yellow[0], color_map.yellow[1], color_map.yellow[2]
            ),
            YELLOW_COLOR_ID,
        ),
        (
            ControlRole::GreenColor,
            ControlRole::GreenColorLabel,
            format!(
                "#{:02X}{:02X}{:02X}",
                color_map.green[0], color_map.green[1], color_map.green[2]
            ),
            GREEN_COLOR_ID,
        ),
    ] {
        let label = match label_role {
            ControlRole::RedColorLabel => "Red",
            ControlRole::YellowColorLabel => "Yellow",
            _ => "Green",
        };
        add_label(
            window,
            instance,
            state,
            label_role,
            label,
            state.fonts.body_small,
        )?;
        let edit = add_control(
            window,
            instance,
            state,
            role,
            "EDIT",
            &color,
            WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            id,
            state.fonts.body,
        )?;
        send_message(edit, EM_LIMITTEXT, 7, 0);
    }
    for (role, id) in [
        (ControlRole::PickRed, PICK_RED_ID),
        (ControlRole::PickYellow, PICK_YELLOW_ID),
        (ControlRole::PickGreen, PICK_GREEN_ID),
    ] {
        add_control(
            window,
            instance,
            state,
            role,
            "BUTTON",
            "Pick…",
            WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            id,
            state.fonts.body_small,
        )?;
    }
    add_label(
        window,
        instance,
        state,
        ControlRole::PreviewLabel,
        "Live icon preview · drag to change battery level",
        state.fonts.body_small,
    )?;
    add_label(
        window,
        instance,
        state,
        ControlRole::PreviewValue,
        "65%",
        state.fonts.body,
    )?;
    let preview_level = add_control(
        window,
        instance,
        state,
        ControlRole::PreviewLevel,
        "msctls_trackbar32",
        "",
        WS_VISIBLE | WS_TABSTOP | windows_sys::Win32::UI::Controls::TBS_NOTICKS,
        PREVIEW_LEVEL_ID,
        state.fonts.body,
    )?;
    send_message(preview_level, TBM_SETRANGEMIN, 1, 0);
    send_message(preview_level, TBM_SETRANGEMAX, 1, 100);
    send_message(preview_level, TBM_SETPOS, 1, 65);

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
        ControlRole::StartupDescription,
        "Monitor batteries after you sign in.",
        state.fonts.body_small,
    )?;
    let startup = add_control(
        window,
        instance,
        state,
        ControlRole::Startup,
        "BUTTON",
        "Start with Windows",
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
    let startup_error_control = add_label(
        window,
        instance,
        state,
        ControlRole::StartupError,
        &startup_error,
        state.fonts.body_small,
    )?;
    if startup_error.is_empty() {
        // SAFETY: this is the unused error label created for the live settings window.
        unsafe {
            ShowWindow(
                startup_error_control,
                windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
            )
        };
    } else {
        // SAFETY: the startup error replaces the helper in its layout slot.
        unsafe {
            ShowWindow(
                control(state, ControlRole::StartupDescription),
                windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
            )
        };
    }
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
    refresh_preview_icon(state);
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
            style: WS_CHILD | (style & !WS_VISIBLE),
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            id,
        },
    )?;
    let font = if matches!(
        role,
        ControlRole::BatteryHeading
            | ControlRole::TrayHeading
            | ControlRole::ColorMapHeading
            | ControlRole::StartupHeading
    ) {
        state.fonts.section
    } else {
        font
    };
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
        WS_VISIBLE
            | WS_TABSTOP
            | WS_VSCROLL
            | CBS_DROPDOWNLIST as u32
            | CBS_HASSTRINGS as u32
            | windows_sys::Win32::UI::WindowsAndMessaging::CBS_OWNERDRAWFIXED as u32,
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
    let mut client = windows_sys::Win32::Foundation::RECT::default();
    // SAFETY: client is writable and window is live.
    if unsafe { GetClientRect(window, &mut client) } == 0 {
        return;
    }
    let width = client.right.min(960);
    let offset = (client.right - width) / 2;
    let inset = MARGIN + 24;
    let (left_card_width, right_card_width, right_card_x) = top_card_layout(width);
    let battery_input_width = 148;
    let battery_input_x = MARGIN + left_card_width - 24 - battery_input_width;
    let tray_combo_width = (right_card_width - 160).clamp(156, 180);
    let tray_combo_x = right_card_x + right_card_width - 24 - tray_combo_width;
    let full_width = width - inset * 2;
    let footer_y = client.bottom - 52;
    let cutoff_gap = 24;
    let cutoff_width = (full_width - cutoff_gap) / 2;
    let color_gap = 16;
    let color_width = (full_width - color_gap * 2) / 3;
    let preview_track_width = full_width - 160;
    let startup_checkbox_width = 192;
    let startup_checkbox_x = inset;
    let startup_description_x = inset + startup_checkbox_width + 12;
    let startup_description_width = full_width - startup_checkbox_width - 12;
    let startup_error_x = startup_description_x;
    let startup_error_width = startup_description_width;
    let rows = [
        (ControlRole::Heading, MARGIN, 24, width - 230, 36),
        (ControlRole::Description, MARGIN, 66, width - 340, 22),
        (ControlRole::ConfigError, MARGIN, 92, width - MARGIN * 2, 26),
        (ControlRole::ReadOnly, MARGIN, 92, width - MARGIN * 2, 26),
        (
            ControlRole::BatteryHeading,
            MARGIN + 24,
            132,
            left_card_width - 48,
            26,
        ),
        (
            ControlRole::BatteryDescription,
            MARGIN + 24,
            160,
            left_card_width - 48,
            20,
        ),
        (
            ControlRole::ThresholdLabel,
            MARGIN + 24,
            188,
            battery_input_x - inset - 12,
            22,
        ),
        (
            ControlRole::ThresholdHelp,
            MARGIN + 24,
            212,
            battery_input_x - inset - 12,
            32,
        ),
        (
            ControlRole::ThresholdEdit,
            battery_input_x,
            186,
            battery_input_width,
            30,
        ),
        (
            ControlRole::ThresholdError,
            battery_input_x,
            218,
            battery_input_width,
            24,
        ),
        (
            ControlRole::CooldownLabel,
            MARGIN + 24,
            248,
            battery_input_x - inset - 12,
            22,
        ),
        (
            ControlRole::CooldownHelp,
            MARGIN + 24,
            272,
            battery_input_x - inset - 12,
            32,
        ),
        (
            ControlRole::CooldownEdit,
            battery_input_x,
            246,
            battery_input_width,
            30,
        ),
        (
            ControlRole::CooldownError,
            battery_input_x,
            278,
            battery_input_width,
            24,
        ),
        (
            ControlRole::Notifications,
            MARGIN + 24,
            312,
            left_card_width - 192,
            26,
        ),
        (
            ControlRole::TestNotification,
            MARGIN + left_card_width - 24 - 128,
            348,
            128,
            32,
        ),
        (
            ControlRole::TrayHeading,
            right_card_x + 24,
            132,
            right_card_width - 48,
            26,
        ),
        (
            ControlRole::TrayDescription,
            right_card_x + 24,
            160,
            right_card_width - 48,
            20,
        ),
        (
            ControlRole::AppThemeLabel,
            right_card_x + 24,
            186,
            tray_combo_x - right_card_x - 36,
            22,
        ),
        (
            ControlRole::AppThemeHelp,
            right_card_x + 24,
            210,
            tray_combo_x - right_card_x - 36,
            20,
        ),
        (
            ControlRole::AppTheme,
            tray_combo_x,
            184,
            tray_combo_width,
            30,
        ),
        (
            ControlRole::IconStyleLabel,
            right_card_x + 24,
            224,
            tray_combo_x - right_card_x - 36,
            22,
        ),
        (
            ControlRole::IconStyleHelp,
            right_card_x + 24,
            254,
            tray_combo_x - right_card_x - 36,
            20,
        ),
        (
            ControlRole::IconStyle,
            tray_combo_x,
            222,
            tray_combo_width,
            30,
        ),
        (
            ControlRole::IconThemeLabel,
            right_card_x + 24,
            262,
            tray_combo_x - right_card_x - 36,
            22,
        ),
        (
            ControlRole::IconThemeHelp,
            right_card_x + 24,
            298,
            tray_combo_x - right_card_x - 36,
            20,
        ),
        (
            ControlRole::IconTheme,
            tray_combo_x,
            260,
            tray_combo_width,
            30,
        ),
        (
            ControlRole::PercentageOverlay,
            right_card_x + 24,
            300,
            right_card_width - 48,
            24,
        ),
        (
            ControlRole::PercentageHelp,
            right_card_x + 46,
            342,
            right_card_width - 70,
            20,
        ),
        (
            ControlRole::PinnedDeviceLabel,
            right_card_x + 24,
            330,
            right_card_width - 48,
            22,
        ),
        (
            ControlRole::PinnedDeviceHelp,
            right_card_x + 24,
            390,
            tray_combo_x - right_card_x - 36,
            20,
        ),
        (
            ControlRole::PinnedDevice,
            right_card_x + 24,
            356,
            right_card_width - 48,
            30,
        ),
        (ControlRole::ColorMapHeading, inset, 412, full_width, 26),
        (ControlRole::ColorMapDescription, inset, 440, full_width, 20),
        (ControlRole::RedCutoffLabel, inset, 466, cutoff_width, 20),
        (
            ControlRole::YellowCutoffLabel,
            inset + cutoff_width + cutoff_gap,
            466,
            cutoff_width,
            20,
        ),
        (ControlRole::RedCutoff, inset, 486, cutoff_width, 30),
        (
            ControlRole::YellowCutoff,
            inset + cutoff_width + cutoff_gap,
            486,
            cutoff_width,
            30,
        ),
        (ControlRole::ColorMapError, inset, 516, full_width, 20),
        (ControlRole::RedColorLabel, inset, 540, color_width, 20),
        (
            ControlRole::YellowColorLabel,
            inset + color_width + color_gap,
            540,
            color_width,
            20,
        ),
        (
            ControlRole::GreenColorLabel,
            inset + (color_width + color_gap) * 2,
            540,
            color_width,
            20,
        ),
        (ControlRole::RedColor, inset + 30, 560, 116, 30),
        (
            ControlRole::YellowColor,
            inset + color_width + color_gap + 30,
            560,
            116,
            30,
        ),
        (
            ControlRole::GreenColor,
            inset + (color_width + color_gap) * 2 + 30,
            560,
            116,
            30,
        ),
        (ControlRole::PickRed, inset + 152, 560, 82, 30),
        (
            ControlRole::PickYellow,
            inset + color_width + color_gap + 152,
            560,
            82,
            30,
        ),
        (
            ControlRole::PickGreen,
            inset + (color_width + color_gap) * 2 + 152,
            560,
            82,
            30,
        ),
        (
            ControlRole::PreviewLabel,
            inset,
            600,
            preview_track_width,
            20,
        ),
        (
            ControlRole::PreviewLevel,
            inset,
            622,
            preview_track_width,
            38,
        ),
        (
            ControlRole::PreviewValue,
            inset + full_width - 148,
            630,
            48,
            24,
        ),
        (ControlRole::StartupHeading, inset, 682, full_width, 26),
        (
            ControlRole::StartupDescription,
            startup_description_x,
            710,
            startup_description_width,
            20,
        ),
        (
            ControlRole::Startup,
            startup_checkbox_x,
            710,
            startup_checkbox_width,
            24,
        ),
        (
            ControlRole::StartupError,
            startup_error_x,
            710,
            startup_error_width,
            20,
        ),
        (
            ControlRole::Status,
            MARGIN,
            footer_y + 8,
            width - MARGIN * 2 - 160,
            32,
        ),
        (ControlRole::Save, width - MARGIN - 144, footer_y, 144, 36),
    ];
    for (role, x, y, w, h) in rows {
        let handle = control(state, role);
        // The native combo height includes the expanded list, not just the selection field.
        let h = if role.is_combo() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{CB_GETCOUNT, CB_GETITEMHEIGHT};
            let count = send_message(handle, CB_GETCOUNT, 0, 0).clamp(1, 8) as i32;
            let item_height = send_message(handle, CB_GETITEMHEIGHT, 0, 0).max(1) as i32;
            h + count * item_height + 8
        } else {
            h
        };
        // SAFETY: each role resolves to a live child; redraw is deferred until every control moves.
        unsafe {
            SetWindowPos(
                handle,
                ptr::null_mut(),
                x + offset,
                y,
                w.max(1),
                h,
                SWP_NOACTIVATE | SWP_NOREDRAW | SWP_NOZORDER,
            );
        }
    }
    for role in [
        ControlRole::BatteryDivider,
        ControlRole::TrayDivider,
        ControlRole::AppThemeHelp,
        ControlRole::IconStyleHelp,
        ControlRole::IconThemeHelp,
        ControlRole::PercentageHelp,
        ControlRole::PinnedDeviceHelp,
    ] {
        // SAFETY: the old etched separators are replaced by painted section boundaries.
        unsafe {
            ShowWindow(
                control(state, role),
                windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
            )
        };
    }
    // SAFETY: repaint the completed layout and all children once, without an erase flash.
    unsafe {
        windows_sys::Win32::Graphics::Gdi::RedrawWindow(
            window,
            ptr::null(),
            ptr::null_mut(),
            windows_sys::Win32::Graphics::Gdi::RDW_INVALIDATE
                | windows_sys::Win32::Graphics::Gdi::RDW_ALLCHILDREN
                | windows_sys::Win32::Graphics::Gdi::RDW_NOERASE,
        );
    }
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
        app_theme: send_message(control(state, ControlRole::AppTheme), CB_GETCURSEL, 0, 0).max(0)
            as usize,
        red_cutoff: get_control_text(control(state, ControlRole::RedCutoff)),
        yellow_cutoff: get_control_text(control(state, ControlRole::YellowCutoff)),
        red_color: get_control_text(control(state, ControlRole::RedColor)),
        yellow_color: get_control_text(control(state, ControlRole::YellowColor)),
        green_color: get_control_text(control(state, ControlRole::GreenColor)),
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

fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u8::from_str_radix(&digits[0..2], 16).ok()?,
        u8::from_str_radix(&digits[2..4], 16).ok()?,
        u8::from_str_radix(&digits[4..6], 16).ok()?,
    ])
}

fn validate_color_map(
    red_cutoff: &str,
    yellow_cutoff: &str,
    red: &str,
    yellow: &str,
    green: &str,
) -> Result<BatteryColorMap, &'static str> {
    if !red_cutoff.bytes().all(|byte| byte.is_ascii_digit()) || red_cutoff.is_empty() {
        return Err("Enter a red cutoff from 0 to 99.");
    }
    if !yellow_cutoff.bytes().all(|byte| byte.is_ascii_digit()) || yellow_cutoff.is_empty() {
        return Err("Enter a yellow cutoff from 1 to 100.");
    }
    let red_max_percent = red_cutoff
        .parse::<u8>()
        .map_err(|_| "Enter a red cutoff from 0 to 99.")?;
    let yellow_max_percent = yellow_cutoff
        .parse::<u8>()
        .map_err(|_| "Enter a yellow cutoff from 1 to 100.")?;
    if red_max_percent > 99 {
        return Err("The red cutoff must be from 0 to 99.");
    }
    if !(1..=100).contains(&yellow_max_percent) {
        return Err("The yellow cutoff must be from 1 to 100.");
    }
    if red_max_percent >= yellow_max_percent {
        return Err("The red cutoff must be lower than the yellow cutoff.");
    }
    let red = parse_hex_color(red).ok_or("Use a hex color in #RRGGBB format.")?;
    let yellow = parse_hex_color(yellow).ok_or("Use a hex color in #RRGGBB format.")?;
    let green = parse_hex_color(green).ok_or("Use a hex color in #RRGGBB format.")?;
    Ok(BatteryColorMap {
        red_max_percent,
        yellow_max_percent,
        red,
        yellow,
        green,
    })
}

fn canonical_hex_color(value: &str) -> String {
    parse_hex_color(value).map_or_else(
        || value.to_owned(),
        |color| format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]),
    )
}

fn update_validation(window: HWND, state: &SettingsWindow) -> bool {
    let threshold = get_control_text(control(state, ControlRole::ThresholdEdit));
    let cooldown = get_control_text(control(state, ControlRole::CooldownEdit));
    let threshold_error = validate_thresholds(&threshold).err().unwrap_or("");
    let cooldown_error = validate_cooldown(&cooldown).err().unwrap_or("");
    let color_map_error = validate_color_map(
        &get_control_text(control(state, ControlRole::RedCutoff)),
        &get_control_text(control(state, ControlRole::YellowCutoff)),
        &get_control_text(control(state, ControlRole::RedColor)),
        &get_control_text(control(state, ControlRole::YellowColor)),
        &get_control_text(control(state, ControlRole::GreenColor)),
    )
    .err()
    .unwrap_or("");
    set_control_text(control(state, ControlRole::ThresholdError), threshold_error);
    set_control_text(control(state, ControlRole::CooldownError), cooldown_error);
    set_control_text(control(state, ControlRole::ColorMapError), color_map_error);
    let valid =
        threshold_error.is_empty() && cooldown_error.is_empty() && color_map_error.is_empty();
    let dirty = current_form(state).is_some_and(|form| state.original.as_ref() != Some(&form));
    set_control_text(
        control(state, ControlRole::Status),
        if dirty {
            "Unsaved changes"
        } else {
            "Your preferences are up to date."
        },
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
        ControlRole::AppTheme,
        ControlRole::ThresholdEdit,
        ControlRole::CooldownEdit,
        ControlRole::RedCutoff,
        ControlRole::YellowCutoff,
        ControlRole::RedColor,
        ControlRole::YellowColor,
        ControlRole::GreenColor,
        ControlRole::Notifications,
        ControlRole::IconStyle,
        ControlRole::IconTheme,
        ControlRole::PercentageOverlay,
        ControlRole::PinnedDevice,
    ] {
        // SAFETY: each role corresponds to a live child control created by this window.
        unsafe { EnableWindow(control(state, role), i32::from(editable)) };
    }
    for role in [
        ControlRole::PickRed,
        ControlRole::PickYellow,
        ControlRole::PickGreen,
    ] {
        // SAFETY: each role is a live color picker button owned by the settings window.
        unsafe { EnableWindow(control(state, role), i32::from(editable)) };
    }
    // SAFETY: the preview slider is live and should be disabled only in read-only mode.
    unsafe {
        EnableWindow(
            control(state, ControlRole::PreviewLevel),
            i32::from(editable),
        )
    };
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

fn refresh_preview_icon(state: &mut SettingsWindow) {
    let red_cutoff = get_control_text(control(state, ControlRole::RedCutoff));
    let yellow_cutoff = get_control_text(control(state, ControlRole::YellowCutoff));
    let red = get_control_text(control(state, ControlRole::RedColor));
    let yellow = get_control_text(control(state, ControlRole::YellowColor));
    let green = get_control_text(control(state, ControlRole::GreenColor));
    let Ok(color_map) = validate_color_map(&red_cutoff, &yellow_cutoff, &red, &yellow, &green)
    else {
        return;
    };
    let style = match send_message(control(state, ControlRole::IconStyle), CB_GETCURSEL, 0, 0) {
        1 => crate::state::TrayIconStyle::Percentage,
        _ => crate::state::TrayIconStyle::Battery,
    };
    let theme = match send_message(control(state, ControlRole::IconTheme), CB_GETCURSEL, 0, 0) {
        1 => crate::state::TrayIconTheme::Light,
        2 => crate::state::TrayIconTheme::Dark,
        _ => crate::state::TrayIconTheme::System,
    };
    let percentage = send_message(
        control(state, ControlRole::PreviewLevel),
        windows_sys::Win32::UI::WindowsAndMessaging::WM_USER,
        0,
        0,
    )
    .clamp(0, 100) as u8;
    let show_percentage = check_control(control(state, ControlRole::PercentageOverlay));
    let Ok(rgba) = crate::tray::render_settings_preview(
        percentage,
        32,
        color_map,
        theme,
        style,
        show_percentage,
    ) else {
        return;
    };
    let background = state.appearance.surface_rgb();
    let (pixels, _) = rgba.as_chunks::<4>();
    state.preview_bgra = pixels
        .iter()
        .flat_map(|pixel| {
            let alpha = u16::from(pixel[3]);
            let inverse = 255 - alpha;
            let red = ((u16::from(pixel[0]) * alpha + u16::from(background[0]) * inverse + 127)
                / 255) as u8;
            let green = ((u16::from(pixel[1]) * alpha + u16::from(background[1]) * inverse + 127)
                / 255) as u8;
            let blue = ((u16::from(pixel[2]) * alpha + u16::from(background[2]) * inverse + 127)
                / 255) as u8;
            [blue, green, red, 255]
        })
        .collect();
    set_control_text(
        control(state, ControlRole::PreviewValue),
        &format!("{percentage}%"),
    );
}

fn invalidate_preview(window: HWND) {
    let mut client = RECT::default();
    // SAFETY: `window` is the live settings window receiving the update.
    if unsafe { GetClientRect(window, &mut client) } == 0 {
        return;
    }
    let width = (client.right - client.left).min(960);
    let offset = (client.right - width) / 2;
    let preview_x = offset + width - 124;
    let preview = RECT {
        left: preview_x - 8,
        top: 590,
        right: preview_x + 73,
        bottom: 661,
    };
    // SAFETY: only the custom painted icon tile needs repainting; skip background erase.
    unsafe { windows_sys::Win32::Graphics::Gdi::InvalidateRect(window, &preview, 0) };
}

fn invalidate_color_map_preview(window: HWND) {
    let mut client = RECT::default();
    // SAFETY: `window` is the live settings window receiving the update.
    if unsafe { GetClientRect(window, &mut client) } == 0 {
        return;
    }
    let width = (client.right - client.left).min(960);
    let offset = (client.right - width) / 2;
    let color_map = RECT {
        left: offset + 48,
        top: 560,
        right: offset + width - 32,
        bottom: 661,
    };
    // SAFETY: this covers the custom color samples and the icon tile, without erasing the page.
    unsafe { windows_sys::Win32::Graphics::Gdi::InvalidateRect(window, &color_map, 0) };
}

fn open_color_picker(window: HWND, state: &mut SettingsWindow, role: ControlRole) {
    let edit_role = match role {
        ControlRole::PickRed => ControlRole::RedColor,
        ControlRole::PickYellow => ControlRole::YellowColor,
        ControlRole::PickGreen => ControlRole::GreenColor,
        _ => return,
    };
    let current =
        parse_hex_color(&get_control_text(control(state, edit_role))).unwrap_or([210, 48, 48]);
    let mut dialog = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: window,
        rgbResult: u32::from(current[0])
            | (u32::from(current[1]) << 8)
            | (u32::from(current[2]) << 16),
        lpCustColors: state.custom_colors.as_mut_ptr(),
        Flags: CC_FULLOPEN | CC_RGBINIT,
        ..Default::default()
    };
    // SAFETY: the owner, color result, and custom-color storage remain valid through the dialog.
    if unsafe { ChooseColorW(&mut dialog) } != 0 {
        let color = dialog.rgbResult;
        set_control_text(
            control(state, edit_role),
            &format!(
                "#{:02X}{:02X}{:02X}",
                color & 0xff,
                (color >> 8) & 0xff,
                (color >> 16) & 0xff,
            ),
        );
        refresh_preview_icon(state);
        let _ = update_validation(window, state);
        invalidate_color_map_preview(window);
    }
}

fn handle_command(window: HWND, state: &mut SettingsWindow, wparam: WPARAM) -> LRESULT {
    let id = wparam & 0xffff;
    let notification = (wparam >> 16) & 0xffff;
    match id {
        APP_THEME_ID => {
            refresh_appearance(window, state);
            refresh_preview_icon(state);
            let _ = update_validation(window, state);
            0
        }
        PICK_RED_ID => {
            open_color_picker(window, state, ControlRole::PickRed);
            0
        }
        PICK_YELLOW_ID => {
            open_color_picker(window, state, ControlRole::PickYellow);
            0
        }
        PICK_GREEN_ID => {
            open_color_picker(window, state, ControlRole::PickGreen);
            0
        }
        TEST_NOTIFICATION_ID => {
            let status = match state.backend.test_notification() {
                Ok(()) => "Test notification sent.".to_owned(),
                Err(error) => format!("Could not send test notification: {error}"),
            };
            set_control_text(control(state, ControlRole::Status), &status);
            0
        }
        SAVE_ID => {
            save_settings(window, state);
            0
        }
        THRESHOLDS_ID | COOLDOWN_ID | RED_CUTOFF_ID | YELLOW_CUTOFF_ID | RED_COLOR_ID
        | YELLOW_COLOR_ID | GREEN_COLOR_ID
            if notification == windows_sys::Win32::UI::WindowsAndMessaging::EN_CHANGE as usize =>
        {
            if matches!(
                id,
                RED_CUTOFF_ID | YELLOW_CUTOFF_ID | RED_COLOR_ID | YELLOW_COLOR_ID | GREEN_COLOR_ID
            ) {
                refresh_preview_icon(state);
                invalidate_color_map_preview(window);
            }
            let _ = update_validation(window, state);
            0
        }
        ICON_STYLE_ID | ICON_THEME_ID | PERCENTAGE_OVERLAY_ID => {
            refresh_preview_icon(state);
            invalidate_preview(window);
            let _ = update_validation(window, state);
            0
        }
        NOTIFICATIONS_ID | PINNED_DEVICE_ID | STARTUP_ID => {
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

fn refresh_appearance(window: HWND, state: &mut SettingsWindow) {
    let preference = match send_message(control(state, ControlRole::AppTheme), CB_GETCURSEL, 0, 0) {
        1 => AppTheme::Light,
        2 => AppTheme::Dark,
        _ => AppTheme::System,
    };
    match Appearance::new(preference) {
        Ok(next) if !state.appearance.same_colors(&next) => {
            state.appearance = next;
            state.appearance.apply_native_theme(window, state);
        }
        Ok(_) => {}
        Err(error) => set_control_text(
            control(state, ControlRole::Status),
            &format!("Could not update appearance: {error}"),
        ),
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
    let color_map = match validate_color_map(
        &form.red_cutoff,
        &form.yellow_cutoff,
        &form.red_color,
        &form.yellow_color,
        &form.green_color,
    ) {
        Ok(color_map) => color_map,
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
        battery_color_red_max_percent: color_map.red_max_percent,
        battery_color_yellow_max_percent: color_map.yellow_max_percent,
        battery_color_red: canonical_hex_color(&form.red_color),
        battery_color_yellow: canonical_hex_color(&form.yellow_color),
        battery_color_green: canonical_hex_color(&form.green_color),
        app_theme: match form.app_theme {
            1 => "light",
            2 => "dark",
            _ => "system",
        }
        .to_owned(),
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
            layout_controls(window, state);
            state.original = current_form(state);
            refresh_controls(window, state);
            set_control_text(
                control(state, ControlRole::Status),
                "Settings saved. Changes are ready to use.",
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

fn apply_saved_settings(state: &mut SettingsWindow) {
    let Some(settings) = &state.settings else {
        return;
    };
    set_combo_selection(
        control(state, ControlRole::AppTheme),
        match settings.app_theme.as_str() {
            "light" => 1,
            "dark" => 2,
            _ => 0,
        },
    );
    set_control_text(
        control(state, ControlRole::RedCutoff),
        &settings.battery_color_red_max_percent.to_string(),
    );
    set_control_text(
        control(state, ControlRole::YellowCutoff),
        &settings.battery_color_yellow_max_percent.to_string(),
    );
    set_control_text(
        control(state, ControlRole::RedColor),
        &settings.battery_color_red,
    );
    set_control_text(
        control(state, ControlRole::YellowColor),
        &settings.battery_color_yellow,
    );
    set_control_text(
        control(state, ControlRole::GreenColor),
        &settings.battery_color_green,
    );
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
    refresh_preview_icon(state);
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
        windows_sys::Win32::UI::WindowsAndMessaging::WM_MEASUREITEM if lparam != 0 => {
            // SAFETY: WM_MEASUREITEM supplies a writable native control measurement structure.
            let item = unsafe {
                &mut *ptr::with_exposed_provenance_mut::<
                    windows_sys::Win32::UI::Controls::MEASUREITEMSTRUCT,
                >(lparam as usize)
            };
            if item.CtlType == windows_sys::Win32::UI::Controls::ODT_COMBOBOX {
                item.itemHeight = 24;
                return 1;
            }
            win32::default_window_proc(window, message, wparam, lparam)
        }
        windows_sys::Win32::UI::WindowsAndMessaging::WM_DRAWITEM if lparam != 0 => {
            let state = win32::window_state::<SettingsWindow>(window);
            // SAFETY: WM_DRAWITEM supplies a live drawing structure valid through this message.
            let item = unsafe {
                &*ptr::with_exposed_provenance::<windows_sys::Win32::UI::Controls::DRAWITEMSTRUCT>(
                    lparam as usize,
                )
            };
            if !state.is_null() && item.CtlType == windows_sys::Win32::UI::Controls::ODT_COMBOBOX {
                // SAFETY: the parent state and its font remain live through control drawing.
                let state = unsafe { &*state };
                state.appearance.draw_combo_item(item, state.fonts.body);
                return 1;
            }
            win32::default_window_proc(window, message, wparam, lparam)
        }

        WM_CTLCOLORSTATIC | windows_sys::Win32::UI::WindowsAndMessaging::WM_CTLCOLORBTN => {
            let state = win32::window_state::<SettingsWindow>(window);
            if state.is_null() {
                return win32::default_window_proc(window, message, wparam, lparam);
            }
            // SAFETY: the attached state lives until the window's message loop exits.
            let state = unsafe { &*state };
            state.appearance.control_color(wparam, lparam, state)
        }
        windows_sys::Win32::UI::WindowsAndMessaging::WM_ERASEBKGND => {
            let state = win32::window_state::<SettingsWindow>(window);
            if state.is_null() {
                return win32::default_window_proc(window, message, wparam, lparam);
            }
            // SAFETY: the attached state lives until the window's message loop exits.
            unsafe { (*state).appearance.erase(wparam, window) }
        }
        windows_sys::Win32::UI::WindowsAndMessaging::WM_PAINT => {
            let state = win32::window_state::<SettingsWindow>(window);
            if state.is_null() {
                return win32::default_window_proc(window, message, wparam, lparam);
            }
            // SAFETY: the attached state lives until the window's message loop exits.
            unsafe { (*state).appearance.paint(window, &*state) };
            0
        }
        windows_sys::Win32::UI::WindowsAndMessaging::WM_CTLCOLOREDIT
        | windows_sys::Win32::UI::WindowsAndMessaging::WM_CTLCOLORLISTBOX => {
            let state = win32::window_state::<SettingsWindow>(window);
            if state.is_null() {
                return win32::default_window_proc(window, message, wparam, lparam);
            }
            // SAFETY: the attached state lives through the message loop.
            let state = unsafe { &*state };
            state.appearance.input_color(wparam)
        }
        windows_sys::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE
        | windows_sys::Win32::UI::WindowsAndMessaging::WM_THEMECHANGED
        | windows_sys::Win32::UI::WindowsAndMessaging::WM_SYSCOLORCHANGE => {
            let state = win32::window_state::<SettingsWindow>(window);
            if !state.is_null() {
                // SAFETY: the attached state lives through the message loop.
                let state = unsafe { &mut *state };
                if state.create_complete {
                    refresh_appearance(window, state);
                    refresh_preview_icon(state);
                    invalidate_preview(window);
                }
            }
            0
        }
        WM_COMMAND => {
            let state = win32::window_state::<SettingsWindow>(window);
            if state.is_null() {
                0
            } else {
                // SAFETY: the attached Box remains alive until the message loop exits.
                handle_command(window, unsafe { &mut *state }, wparam)
            }
        }
        WM_HSCROLL => {
            let state = win32::window_state::<SettingsWindow>(window);
            if !state.is_null() {
                // SAFETY: the state and trackbar control are live for this parent notification.
                let state = unsafe { &mut *state };
                if lparam as usize == control(state, ControlRole::PreviewLevel).expose_provenance()
                {
                    refresh_preview_icon(state);
                    invalidate_preview(window);
                }
            }
            0
        }
        WM_SIZE => {
            let state = win32::window_state::<SettingsWindow>(window);
            // SAFETY: a non-null state pointer remains attached for the lifetime of this window.
            let created = !state.is_null() && unsafe { (*state).create_complete };
            if created {
                // SAFETY: the attached Box remains alive until the message loop exits.
                layout_controls(window, unsafe { &*state });
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
    use super::{parse_hex_color, validate_color_map, validate_cooldown, validate_thresholds};

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

    #[test]
    fn color_map_accepts_hex_colors_and_ordered_cutoffs() {
        let map = validate_color_map("15", "30", "#D23030", "#ebbe23", "#2DAA4B").unwrap();
        assert_eq!(map.red_max_percent, 15);
        assert_eq!(map.yellow_max_percent, 30);
        assert_eq!(map.color_for(15), [210, 48, 48]);
        assert_eq!(map.color_for(30), [235, 190, 35]);
        assert_eq!(map.color_for(31), [45, 170, 75]);
        assert_eq!(parse_hex_color("#123AbC"), Some([18, 58, 188]));
    }

    #[test]
    fn color_map_rejects_invalid_hex_and_overlapping_cutoffs() {
        assert!(validate_color_map("30", "30", "#D23030", "#EBBE23", "#2DAA4B").is_err());
        assert!(validate_color_map("100", "30", "#D23030", "#EBBE23", "#2DAA4B").is_err());
        assert!(validate_color_map("15", "30", "red", "#EBBE23", "#2DAA4B").is_err());
    }
}
