//! System-tray UI and small platform integrations for Volty.
//!
//! Call [`TrayController::process_menu_events`] and
//! [`TrayController::process_tray_icon_events`] from the application's event
//! loop. Tray events are received through tray-icon's process-global receivers,
//! so an application should own one controller.

mod icon;
mod startup;

use icon::{
    battery_icon_at_size_with_color_map, percentage_icon_at_size_with_color_map,
    unknown_battery_icon_at_size, validate_icon_size, IconTheme, ICON_SIZE,
};
#[cfg(windows)]
pub use startup::reset_startup_preferences;
use startup::start_on_boot_for_first_run;
pub use startup::{confirm_reset, set_start_on_boot_enabled, start_on_boot_enabled};
use std::{
    error::Error,
    fmt,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
};
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

const WINDOWS_PERSONALIZATION_KEY: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
const SYSTEM_LIGHT_THEME_VALUE: &str = "SystemUsesLightTheme";

fn current_icon_theme() -> Result<IconTheme, TrayError> {
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(WINDOWS_PERSONALIZATION_KEY)
        .map_err(|error| {
            TrayError::new(format!(
                "could not read Windows theme settings for tray icon: {error}"
            ))
        })?;
    let is_light = key
        .get_value::<u32, _>(SYSTEM_LIGHT_THEME_VALUE)
        .map_err(|error| {
            TrayError::new(format!(
                "could not read the Windows system theme for tray icon: {error}"
            ))
        })?;
    Ok(if is_light == 0 {
        IconTheme::Dark
    } else {
        IconTheme::Light
    })
}

use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use crate::{config::BatteryColorMap, state::TrayIconStyle, update::UpdateInfo};

const MAX_STATUS_CHARS: usize = 80;
const MAX_TOOLTIP_UTF16_UNITS: usize = 127;
const MENU_ITEMS_AFTER_ADVANCED_OPTIONS: usize = 4; // Check for Updates, About, separator, Quit.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItemId {
    Status,
    OpenDashboard,
    Discover,
    Configuration,
    LogFile,
    SaveHardwareCapture,
    Diagnose,
    StartOnBoot,
    ShowAdvancedOptions,
    ResetVolty,
    MonitoredDevices,
    MonitoredDevice(u64),
    TrayDisplay,
    TrayIconStyleBattery,
    TrayIconStylePercentage,
    PinnedDeviceLowest,
    PinnedDevice(u64),
    CheckForUpdates,
    About,
    Quit,
}

impl MenuItemId {
    fn as_id(self) -> String {
        match self {
            Self::Status => "volty.status".to_owned(),
            Self::OpenDashboard => "volty.open-dashboard".to_owned(),
            Self::Discover => "volty.discover".to_owned(),
            Self::Configuration => "volty.configuration".to_owned(),
            Self::LogFile => "volty.open-log-file".to_owned(),
            Self::SaveHardwareCapture => "volty.save-hardware-capture".to_owned(),
            Self::Diagnose => "volty.diagnose".to_owned(),
            Self::StartOnBoot => "volty.start-on-boot".to_owned(),
            Self::ShowAdvancedOptions => "volty.show-advanced-options".to_owned(),
            Self::ResetVolty => "volty.reset".to_owned(),
            Self::MonitoredDevices => "volty.monitored-devices".to_owned(),
            Self::MonitoredDevice(sequence) => {
                format!("volty.monitored-device.{sequence}")
            }
            Self::TrayDisplay => "volty.tray-display".to_owned(),
            Self::TrayIconStyleBattery => "volty.tray-icon-style.battery".to_owned(),
            Self::TrayIconStylePercentage => "volty.tray-icon-style.percentage".to_owned(),
            Self::PinnedDeviceLowest => "volty.pinned-device.lowest".to_owned(),
            Self::PinnedDevice(sequence) => format!("volty.pinned-device.{sequence}"),
            Self::CheckForUpdates => "volty.check-for-updates".to_owned(),
            Self::About => "volty.about".to_owned(),
            Self::Quit => "volty.quit".to_owned(),
        }
    }

    fn parse(id: &str) -> Option<Self> {
        match id {
            "volty.status" => Some(Self::Status),
            "volty.open-dashboard" => Some(Self::OpenDashboard),
            "volty.discover" => Some(Self::Discover),
            "volty.configuration" => Some(Self::Configuration),
            "volty.open-log-file" => Some(Self::LogFile),
            "volty.save-hardware-capture" => Some(Self::SaveHardwareCapture),
            "volty.diagnose" => Some(Self::Diagnose),
            "volty.start-on-boot" => Some(Self::StartOnBoot),
            "volty.show-advanced-options" => Some(Self::ShowAdvancedOptions),
            "volty.reset" => Some(Self::ResetVolty),
            "volty.monitored-devices" => Some(Self::MonitoredDevices),
            "volty.tray-display" => Some(Self::TrayDisplay),
            "volty.tray-icon-style.battery" => Some(Self::TrayIconStyleBattery),
            "volty.tray-icon-style.percentage" => Some(Self::TrayIconStylePercentage),
            "volty.pinned-device.lowest" => Some(Self::PinnedDeviceLowest),
            "volty.check-for-updates" => Some(Self::CheckForUpdates),
            "volty.about" => Some(Self::About),
            "volty.quit" => Some(Self::Quit),
            _ => id
                .strip_prefix("volty.monitored-device.")
                .and_then(|sequence| sequence.parse().ok())
                .map(Self::MonitoredDevice)
                .or_else(|| {
                    id.strip_prefix("volty.pinned-device.")
                        .and_then(|sequence| sequence.parse().ok())
                        .map(Self::PinnedDevice)
                }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMonitorEntry {
    pub identity: String,
    pub name: String,
    pub monitored: bool,
}

struct MonitoredDeviceMenuItem {
    menu_id: MenuItemId,
    identity: String,
    item: CheckMenuItem,
}

struct PinnedDeviceMenuItem {
    menu_id: MenuItemId,
    identity: String,
    item: CheckMenuItem,
}

/// Actions the application should handle after menu events or notification clicks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    OpenDashboard,
    DashboardResult(Result<(), String>),
    DiscoverDevicesNow,
    OpenConfiguration,
    OpenLogFile,
    SaveHardwareCapture,
    Diagnose,
    ToggleStartOnBoot,
    SetAdvancedOptionsVisible {
        visible: bool,
    },
    ResetVolty,
    OpenSavedHardwareCapture {
        path: PathBuf,
    },
    SetDeviceMonitored {
        identity: String,
        monitored: bool,
    },
    SetTrayIconStyle {
        style: TrayIconStyle,
    },
    SetPinnedDevice {
        identity: Option<String>,
    },
    CheckForUpdates,
    UpdateCheckResult {
        result: Result<Option<UpdateInfo>, String>,
        origin: UpdateCheckOrigin,
    },
    UpdateInstallResult(Result<(), String>),
    About,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateCheckOrigin {
    Automatic,
    Manual,
}

/// Errors returned while creating or updating the tray UI.
#[derive(Debug)]
pub struct TrayError(String);

impl TrayError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for TrayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for TrayError {}

/// Owns the native tray icon, its menu and the receiver for application actions.
pub struct TrayController {
    icon: TrayIcon,
    menu: Menu,
    status_item: MenuItem,
    device_status_items: Vec<MenuItem>,
    monitored_devices_menu: Submenu,
    monitored_devices_empty_item: Option<MenuItem>,
    monitored_device_items: Vec<MonitoredDeviceMenuItem>,
    known_monitored_devices: Vec<DeviceMonitorEntry>,
    next_monitored_device_menu_id: u64,
    show_advanced_options_item: CheckMenuItem,
    advanced_menu_items: AdvancedMenuItems,
    advanced_options_visible: bool,
    pinned_device_items: Vec<PinnedDeviceMenuItem>,
    next_pinned_device_menu_id: u64,
    pinned_device: Option<String>,
    tray_icon_style: TrayIconStyle,
    start_on_boot_item: CheckMenuItem,
    check_for_updates_item: MenuItem,
    actions_tx: Sender<TrayAction>,
    actions_rx: Receiver<TrayAction>,
    percentage: Option<u8>,
    battery_color_map: BatteryColorMap,
    displayed_status: Option<String>,
    displayed_tooltip: Option<String>,
    displayed_device_statuses: Vec<String>,
    displayed_icon: Option<(Option<u8>, u32, BatteryColorMap, TrayIconStyle, IconTheme)>,
}

struct TrayMenu {
    menu: Menu,
    status_item: MenuItem,
    monitored_devices_menu: Submenu,
    monitored_devices_empty_item: MenuItem,
    show_advanced_options_item: CheckMenuItem,
    advanced_menu_items: AdvancedMenuItems,
    start_on_boot_item: CheckMenuItem,
    check_for_updates_item: MenuItem,
}

struct TrayMenuItems {
    status_item: MenuItem,
    open_dashboard_item: MenuItem,
    monitored_devices_menu: Submenu,
    monitored_devices_empty_item: MenuItem,
    start_on_boot_item: CheckMenuItem,
    show_advanced_options_item: CheckMenuItem,
    advanced_options_separator: PredefinedMenuItem,
    advanced_menu_items: AdvancedMenuItems,
    check_for_updates_item: MenuItem,
    about_item: MenuItem,
    about_separator: PredefinedMenuItem,
    quit_item: MenuItem,
}

struct AdvancedMenuItems {
    tray_display_menu: Submenu,
    battery_style_item: CheckMenuItem,
    percentage_style_item: CheckMenuItem,
    pinned_lowest_item: CheckMenuItem,
    tray_display_separator: PredefinedMenuItem,
    discover_item: MenuItem,
    configuration_item: MenuItem,
    open_log_item: MenuItem,
    save_capture_item: MenuItem,
    diagnose_item: MenuItem,
    reset_item: MenuItem,
    end_separator: PredefinedMenuItem,
}

impl AdvancedMenuItems {
    fn menu_items(&self) -> [&dyn tray_icon::menu::IsMenuItem; 9] {
        [
            &self.tray_display_menu,
            &self.tray_display_separator,
            &self.discover_item,
            &self.configuration_item,
            &self.open_log_item,
            &self.save_capture_item,
            &self.diagnose_item,
            &self.reset_item,
            &self.end_separator,
        ]
    }
}

fn create_tray_menu(
    start_on_boot: bool,
    advanced_options_visible: bool,
) -> Result<TrayMenu, TrayError> {
    let status_item = MenuItem::with_id(
        MenuItemId::Status.as_id(),
        "Searching for supported devices",
        false,
        None,
    );
    let start_on_boot_item = CheckMenuItem::with_id(
        MenuItemId::StartOnBoot.as_id(),
        "Start on Boot",
        true,
        start_on_boot,
        None,
    );
    let show_advanced_options_item = CheckMenuItem::with_id(
        MenuItemId::ShowAdvancedOptions.as_id(),
        "Show Advanced Options",
        true,
        advanced_options_visible,
        None,
    );
    let monitored_devices_menu = Submenu::with_id(
        MenuItemId::MonitoredDevices.as_id(),
        "Monitored Devices",
        true,
    );
    let monitored_devices_empty_item =
        MenuItem::new("No supported devices discovered", false, None);
    monitored_devices_menu
        .append(&monitored_devices_empty_item)
        .map_err(|error| {
            TrayError::new(format!(
                "could not add empty monitored-devices status: {error}"
            ))
        })?;

    let battery_style_item = CheckMenuItem::with_id(
        MenuItemId::TrayIconStyleBattery.as_id(),
        "Battery",
        true,
        true,
        None,
    );
    let percentage_style_item = CheckMenuItem::with_id(
        MenuItemId::TrayIconStylePercentage.as_id(),
        "Percentage",
        true,
        false,
        None,
    );
    let pinned_lowest_item = CheckMenuItem::with_id(
        MenuItemId::PinnedDeviceLowest.as_id(),
        "Lowest battery (default)",
        true,
        true,
        None,
    );
    let tray_display_menu = Submenu::with_id(MenuItemId::TrayDisplay.as_id(), "Tray Display", true);
    for (item, description) in [
        (&battery_style_item, "battery icon style"),
        (&percentage_style_item, "percentage icon style"),
        (&pinned_lowest_item, "lowest-battery pin"),
    ] {
        tray_display_menu
            .append(item)
            .map_err(|error| TrayError::new(format!("could not add {description}: {error}")))?;
    }

    let advanced_menu_items = AdvancedMenuItems {
        tray_display_menu,
        battery_style_item,
        percentage_style_item,
        pinned_lowest_item,
        tray_display_separator: PredefinedMenuItem::separator(),
        discover_item: MenuItem::with_id(
            MenuItemId::Discover.as_id(),
            "Discover Devices Now",
            true,
            None,
        ),
        configuration_item: MenuItem::with_id(
            MenuItemId::Configuration.as_id(),
            "Open Configuration",
            true,
            None,
        ),
        open_log_item: MenuItem::with_id(MenuItemId::LogFile.as_id(), "Open Log File", true, None),
        save_capture_item: MenuItem::with_id(
            MenuItemId::SaveHardwareCapture.as_id(),
            "Save Hardware Capture",
            true,
            None,
        ),
        diagnose_item: MenuItem::with_id(
            MenuItemId::Diagnose.as_id(),
            "Diagnose / Report an Issue",
            true,
            None,
        ),
        reset_item: MenuItem::with_id(MenuItemId::ResetVolty.as_id(), "Reset Volty...", true, None),
        end_separator: PredefinedMenuItem::separator(),
    };

    let items = TrayMenuItems {
        status_item,
        open_dashboard_item: MenuItem::with_id(
            MenuItemId::OpenDashboard.as_id(),
            "Open Dashboard",
            true,
            None,
        ),
        monitored_devices_menu,
        monitored_devices_empty_item,
        start_on_boot_item,
        show_advanced_options_item,
        advanced_options_separator: PredefinedMenuItem::separator(),
        advanced_menu_items,
        check_for_updates_item: MenuItem::with_id(
            MenuItemId::CheckForUpdates.as_id(),
            "Check for Updates...",
            true,
            None,
        ),
        about_item: MenuItem::with_id(MenuItemId::About.as_id(), "About Volty", true, None),
        about_separator: PredefinedMenuItem::separator(),
        quit_item: MenuItem::with_id(MenuItemId::Quit.as_id(), "Quit", true, None),
    };
    let menu = Menu::new();
    append_tray_menu_items(&menu, &items, advanced_options_visible)?;
    Ok(TrayMenu {
        menu,
        status_item: items.status_item,
        monitored_devices_menu: items.monitored_devices_menu,
        monitored_devices_empty_item: items.monitored_devices_empty_item,
        show_advanced_options_item: items.show_advanced_options_item,
        advanced_menu_items: items.advanced_menu_items,
        start_on_boot_item: items.start_on_boot_item,
        check_for_updates_item: items.check_for_updates_item,
    })
}

fn append_tray_menu_items(
    menu: &Menu,
    items: &TrayMenuItems,
    advanced_options_visible: bool,
) -> Result<(), TrayError> {
    menu.append(&items.status_item)
        .map_err(|error| TrayError::new(format!("could not add tray status: {error}")))?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
    menu.append(&items.monitored_devices_menu)
        .map_err(|error| {
            TrayError::new(format!("could not add monitored-devices menu: {error}"))
        })?;
    menu.append(&items.open_dashboard_item)
        .map_err(|error| TrayError::new(format!("could not add dashboard action: {error}")))?;
    menu.append(&items.start_on_boot_item)
        .map_err(|error| TrayError::new(format!("could not add start-on-boot action: {error}")))?;
    menu.append(&items.show_advanced_options_item)
        .map_err(|error| {
            TrayError::new(format!("could not add advanced-options toggle: {error}"))
        })?;
    menu.append(&items.advanced_options_separator)
        .map_err(|error| {
            TrayError::new(format!("could not add advanced-options separator: {error}"))
        })?;
    if advanced_options_visible {
        append_advanced_menu_items(menu, &items.advanced_menu_items)?;
    }
    menu.append(&items.check_for_updates_item)
        .map_err(|error| {
            TrayError::new(format!("could not add Check for Updates action: {error}"))
        })?;
    menu.append(&items.about_item)
        .map_err(|error| TrayError::new(format!("could not add About action: {error}")))?;
    menu.append(&items.about_separator)
        .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
    menu.append(&items.quit_item)
        .map_err(|error| TrayError::new(format!("could not add quit action: {error}")))?;
    Ok(())
}

fn append_advanced_menu_items(menu: &Menu, items: &AdvancedMenuItems) -> Result<(), TrayError> {
    for (index, item) in items.menu_items().into_iter().enumerate() {
        menu.append(item).map_err(|error| {
            TrayError::new(format!(
                "could not add advanced tray option {}: {error}",
                index + 1
            ))
        })?;
    }
    Ok(())
}

fn insert_advanced_menu_items(
    menu: &Menu,
    items: &AdvancedMenuItems,
    position: usize,
) -> Result<(), TrayError> {
    let mut inserted = Vec::new();
    for (offset, item) in items.menu_items().into_iter().enumerate() {
        if let Err(error) = menu.insert(item, position + offset) {
            for inserted_item in inserted.into_iter().rev() {
                let _ = menu.remove(inserted_item);
            }
            return Err(TrayError::new(format!(
                "could not show advanced tray options: {error}"
            )));
        }
        inserted.push(item);
    }
    Ok(())
}

fn remove_advanced_menu_items(menu: &Menu, items: &AdvancedMenuItems) -> Result<(), TrayError> {
    for item in items.menu_items().into_iter().rev() {
        menu.remove(item).map_err(|error| {
            TrayError::new(format!("could not hide advanced tray options: {error}"))
        })?;
    }
    Ok(())
}

impl TrayController {
    /// Creates the tray icon and menu with the saved advanced-menu preference.
    ///
    /// The Start on Boot state is initialized from the Windows Run key. On
    /// other platforms the check item starts unchecked and the action reports
    /// that autostart is unsupported when selected.
    pub fn new(advanced_options_visible: bool) -> Result<Self, TrayError> {
        #[cfg(windows)]
        crate::win32::enable_system_dark_menus();

        let tray_menu = create_tray_menu(start_on_boot_for_first_run()?, advanced_options_visible)?;
        let icon_theme = current_icon_theme()?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("Volty — Searching for supported devices")
            .with_icon(unknown_battery_icon_at_size(ICON_SIZE, icon_theme)?)
            .with_menu(Box::new(tray_menu.menu.clone()))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|error| TrayError::new(format!("could not create tray icon: {error}")))?;
        #[cfg(windows)]
        crate::win32::allow_dark_mode_for_current_thread_windows();

        let (actions_tx, actions_rx) = mpsc::channel();

        Ok(Self {
            icon,
            menu: tray_menu.menu,
            status_item: tray_menu.status_item,
            device_status_items: Vec::new(),
            monitored_devices_menu: tray_menu.monitored_devices_menu,
            monitored_devices_empty_item: Some(tray_menu.monitored_devices_empty_item),
            monitored_device_items: Vec::new(),
            known_monitored_devices: Vec::new(),
            next_monitored_device_menu_id: 0,
            show_advanced_options_item: tray_menu.show_advanced_options_item,
            advanced_menu_items: tray_menu.advanced_menu_items,
            advanced_options_visible,
            pinned_device_items: Vec::new(),
            next_pinned_device_menu_id: 0,
            pinned_device: None,
            tray_icon_style: TrayIconStyle::Battery,
            start_on_boot_item: tray_menu.start_on_boot_item,
            check_for_updates_item: tray_menu.check_for_updates_item,
            actions_tx,
            actions_rx,
            percentage: None,
            battery_color_map: BatteryColorMap::default(),
            displayed_status: None,
            displayed_tooltip: None,
            displayed_device_statuses: Vec::new(),
            displayed_icon: None,
        })
    }

    /// Updates the tray icon, menu, and tooltip with the current device status.
    ///
    /// For example, `update_status("G502 X", "Discharging", Some(85))`
    /// displays `G502 X: 85% (Discharging)`. Pass `None` when the device or
    /// percentage is unavailable; the last battery level is retained if one
    /// has already been rendered.
    pub fn update_status(
        &mut self,
        device_name: &str,
        status: &str,
        percentage: Option<u8>,
    ) -> Result<(), TrayError> {
        self.update_status_with_icon_size(device_name, status, percentage, ICON_SIZE)
    }

    pub fn reset_status(&mut self, status: &str) -> Result<(), TrayError> {
        self.percentage = None;
        let status = shorten_status(status);
        self.update_display(&status, &status, None, ICON_SIZE, &[])
    }

    /// Updates the tray status while rendering its icon at 16×16 or 32×32.
    pub fn update_status_with_icon_size(
        &mut self,
        device_name: &str,
        status: &str,
        percentage: Option<u8>,
        icon_size: u32,
    ) -> Result<(), TrayError> {
        validate_icon_size(icon_size)?;
        let percentage = percentage.map(|value| value.min(100));
        let icon_percentage = self.update_percentage(percentage, false);

        let live_status = match percentage {
            Some(percentage) => format!("{device_name}: {percentage}% ({status})"),
            None if device_name.is_empty() => status.to_owned(),
            None => format!("{device_name}: {status}"),
        };
        let live_status = shorten_status(&live_status);
        self.update_display(&live_status, &live_status, icon_percentage, icon_size, &[])
    }

    /// Displays each readable device at the top of the tray menu. The tooltip
    /// shows each device on its own compact line when they fit.
    pub fn update_devices(
        &mut self,
        summary: &str,
        device_statuses: &[String],
        tooltip_statuses: &[String],
        percentage: Option<u8>,
    ) -> Result<(), TrayError> {
        let percentage = percentage.map(|value| value.min(100));
        let icon_percentage = self.update_percentage(percentage, device_statuses.is_empty());

        let tooltip = tooltip_for_devices(summary, tooltip_statuses);
        let menu_device_statuses: Vec<_> = device_statuses
            .iter()
            .map(|status| shorten_status(status))
            .collect();
        self.update_display(
            &shorten_status(summary),
            &tooltip,
            icon_percentage,
            ICON_SIZE,
            &menu_device_statuses,
        )
    }

    pub fn update_monitored_devices(
        &mut self,
        devices: &[DeviceMonitorEntry],
    ) -> Result<(), TrayError> {
        let Some(devices) =
            sorted_monitored_devices_if_changed(&self.known_monitored_devices, devices)
        else {
            return Ok(());
        };

        self.clear_monitored_device_items()?;
        if devices.is_empty() {
            self.show_empty_monitored_devices()?;
        } else {
            self.remove_empty_monitored_devices()?;
            for device in &devices {
                self.append_monitored_device(device)?;
            }
        }
        self.clear_pinned_device_items()?;
        for device in &devices {
            self.append_pinned_device(device)?;
        }
        self.apply_pinned_device_checks();
        self.known_monitored_devices = devices;
        Ok(())
    }

    fn clear_pinned_device_items(&mut self) -> Result<(), TrayError> {
        for entry in self.pinned_device_items.drain(..) {
            self.advanced_menu_items
                .tray_display_menu
                .remove(&entry.item)
                .map_err(|error| {
                    TrayError::new(format!("could not clear pinned devices: {error}"))
                })?;
        }
        Ok(())
    }

    fn append_pinned_device(&mut self, device: &DeviceMonitorEntry) -> Result<(), TrayError> {
        let sequence = self.next_pinned_device_menu_id;
        self.next_pinned_device_menu_id = sequence
            .checked_add(1)
            .ok_or_else(|| TrayError::new("pinned-device menu ID space exhausted"))?;
        let menu_id = MenuItemId::PinnedDevice(sequence);
        let checked = self.pinned_device.as_deref() == Some(device.identity.as_str());
        let item = CheckMenuItem::with_id(
            menu_id.as_id(),
            shorten_status(&device.name),
            true,
            checked,
            None,
        );
        self.advanced_menu_items
            .tray_display_menu
            .append(&item)
            .map_err(|error| TrayError::new(format!("could not add pinned device: {error}")))?;
        self.pinned_device_items.push(PinnedDeviceMenuItem {
            menu_id,
            identity: device.identity.clone(),
            item,
        });
        Ok(())
    }

    fn clear_monitored_device_items(&mut self) -> Result<(), TrayError> {
        for entry in self.monitored_device_items.drain(..) {
            self.monitored_devices_menu
                .remove(&entry.item)
                .map_err(|error| {
                    TrayError::new(format!("could not clear monitored devices: {error}"))
                })?;
        }
        Ok(())
    }

    fn show_empty_monitored_devices(&mut self) -> Result<(), TrayError> {
        if self.monitored_devices_empty_item.is_none() {
            let item = MenuItem::new("No supported devices discovered", false, None);
            self.monitored_devices_menu.append(&item).map_err(|error| {
                TrayError::new(format!(
                    "could not update empty monitored-devices status: {error}"
                ))
            })?;
            self.monitored_devices_empty_item = Some(item);
        }
        Ok(())
    }

    fn remove_empty_monitored_devices(&mut self) -> Result<(), TrayError> {
        if let Some(item) = self.monitored_devices_empty_item.take() {
            self.monitored_devices_menu.remove(&item).map_err(|error| {
                TrayError::new(format!(
                    "could not clear empty monitored-devices status: {error}"
                ))
            })?;
        }
        Ok(())
    }

    fn append_monitored_device(&mut self, device: &DeviceMonitorEntry) -> Result<(), TrayError> {
        let sequence = self.next_monitored_device_menu_id;
        self.next_monitored_device_menu_id = sequence
            .checked_add(1)
            .ok_or_else(|| TrayError::new("monitored-device menu ID space exhausted"))?;
        let menu_id = MenuItemId::MonitoredDevice(sequence);
        let item = CheckMenuItem::with_id(
            menu_id.as_id(),
            shorten_status(&device.name),
            true,
            device.monitored,
            None,
        );
        self.monitored_devices_menu
            .append(&item)
            .map_err(|error| TrayError::new(format!("could not add monitored device: {error}")))?;
        self.monitored_device_items.push(MonitoredDeviceMenuItem {
            menu_id,
            identity: device.identity.clone(),
            item,
        });
        Ok(())
    }

    fn update_display(
        &mut self,
        menu_status: &str,
        tooltip: &str,
        percentage: Option<u8>,
        icon_size: u32,
        device_statuses: &[String],
    ) -> Result<(), TrayError> {
        if self.displayed_status.as_deref() != Some(menu_status) {
            self.status_item.set_text(menu_status);
            self.displayed_status = Some(menu_status.to_owned());
        }
        if self.displayed_device_statuses != device_statuses {
            self.replace_device_status_items(device_statuses)?;
            self.displayed_device_statuses = device_statuses.to_vec();
        }
        let icon_theme = current_icon_theme()?;
        let icon_key = (
            percentage,
            icon_size,
            self.battery_color_map,
            self.tray_icon_style,
            icon_theme,
        );
        if self.displayed_icon != Some(icon_key) {
            let icon =
                self.render_icon(percentage, icon_size, self.battery_color_map, icon_theme)?;
            self.icon
                .set_icon(Some(icon))
                .map_err(|error| TrayError::new(format!("could not update tray icon: {error}")))?;
            self.displayed_icon = Some(icon_key);
        }
        if self.displayed_tooltip.as_deref() != Some(tooltip) {
            self.icon.set_tooltip(Some(tooltip)).map_err(|error| {
                TrayError::new(format!("could not update tray tooltip: {error}"))
            })?;
            self.displayed_tooltip = Some(tooltip.to_owned());
        }
        Ok(())
    }

    fn replace_device_status_items(&mut self, device_statuses: &[String]) -> Result<(), TrayError> {
        for item in self.device_status_items.drain(..) {
            self.menu.remove(&item).map_err(|error| {
                TrayError::new(format!("could not clear tray device status: {error}"))
            })?;
        }

        for (index, status) in device_statuses.iter().enumerate() {
            let item = MenuItem::new(status, false, None);
            self.menu.insert(&item, index + 1).map_err(|error| {
                TrayError::new(format!("could not update tray device status: {error}"))
            })?;
            self.device_status_items.push(item);
        }
        Ok(())
    }

    /// Processes pending native menu events and returns the number handled.
    ///
    /// A Windows Run-key error is returned to the caller so it can be logged
    /// or displayed rather than silently leaving the check item out of sync.
    pub fn process_menu_events(&mut self) -> Result<usize, TrayError> {
        let mut handled = 0;
        #[allow(clippy::while_let_loop)]
        loop {
            let event = match MenuEvent::receiver().try_recv() {
                Ok(event) => event,
                Err(error) if error.is_empty() => break,
                Err(error) => {
                    return Err(TrayError::new(format!(
                        "tray menu event channel disconnected: {error}"
                    )))
                }
            };
            let Some(id) = MenuItemId::parse(event.id.0.as_str()) else {
                continue;
            };
            let Some(action) = self.action_for_menu_id(id) else {
                continue;
            };
            self.actions_tx
                .send(action)
                .map_err(|_| TrayError::new("tray action receiver has been dropped"))?;
            handled += 1;
        }
        Ok(handled)
    }

    /// Processes pending tray-icon events and returns the number handled.
    pub fn process_tray_icon_events(&mut self) -> Result<usize, TrayError> {
        let mut handled = 0;
        #[allow(clippy::while_let_loop)]
        loop {
            let event = match TrayIconEvent::receiver().try_recv() {
                Ok(event) => event,
                Err(error) if error.is_empty() => break,
                Err(error) => {
                    return Err(TrayError::new(format!(
                        "tray icon event channel disconnected: {error}"
                    )))
                }
            };
            if let TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = event
            {
                self.actions_tx
                    .send(TrayAction::OpenDashboard)
                    .map_err(|_| TrayError::new("tray action receiver has been dropped"))?;
                handled += 1;
            }
        }
        Ok(handled)
    }

    fn action_for_menu_id(&self, id: MenuItemId) -> Option<TrayAction> {
        match id {
            MenuItemId::OpenDashboard => Some(TrayAction::OpenDashboard),
            MenuItemId::Discover => Some(TrayAction::DiscoverDevicesNow),
            MenuItemId::Configuration => Some(TrayAction::OpenConfiguration),
            MenuItemId::LogFile => Some(TrayAction::OpenLogFile),
            MenuItemId::SaveHardwareCapture => Some(TrayAction::SaveHardwareCapture),
            MenuItemId::Diagnose => Some(TrayAction::Diagnose),
            MenuItemId::StartOnBoot => Some(TrayAction::ToggleStartOnBoot),
            MenuItemId::ShowAdvancedOptions => Some(TrayAction::SetAdvancedOptionsVisible {
                visible: self.show_advanced_options_item.is_checked(),
            }),
            MenuItemId::MonitoredDevice(menu_id) => self
                .monitored_device_items
                .iter()
                .find(|entry| entry.menu_id == MenuItemId::MonitoredDevice(menu_id))
                .map(|entry| TrayAction::SetDeviceMonitored {
                    identity: entry.identity.clone(),
                    monitored: entry.item.is_checked(),
                }),
            MenuItemId::TrayIconStyleBattery => Some(TrayAction::SetTrayIconStyle {
                style: TrayIconStyle::Battery,
            }),
            MenuItemId::TrayIconStylePercentage => Some(TrayAction::SetTrayIconStyle {
                style: TrayIconStyle::Percentage,
            }),
            MenuItemId::PinnedDeviceLowest => Some(TrayAction::SetPinnedDevice { identity: None }),
            MenuItemId::PinnedDevice(menu_id) => self
                .pinned_device_items
                .iter()
                .find(|entry| entry.menu_id == MenuItemId::PinnedDevice(menu_id))
                .map(|entry| TrayAction::SetPinnedDevice {
                    identity: Some(entry.identity.clone()),
                }),
            MenuItemId::CheckForUpdates => Some(TrayAction::CheckForUpdates),
            MenuItemId::About => Some(TrayAction::About),
            MenuItemId::ResetVolty => Some(TrayAction::ResetVolty),
            MenuItemId::Quit => Some(TrayAction::Quit),
            MenuItemId::Status | MenuItemId::MonitoredDevices | MenuItemId::TrayDisplay => None,
        }
    }
    /// Gets the next app action, if one has been queued.
    pub fn try_recv_action(&self) -> Result<TrayAction, TryRecvError> {
        self.actions_rx.try_recv()
    }

    pub fn set_start_on_boot_checked(&self, checked: bool) {
        self.start_on_boot_item.set_checked(checked);
    }

    pub fn set_advanced_options_visible(&mut self, visible: bool) -> Result<(), TrayError> {
        if visible == self.advanced_options_visible {
            self.show_advanced_options_item.set_checked(visible);
            return Ok(());
        }

        if visible {
            let position = self
                .menu
                .items()
                .len()
                .checked_sub(MENU_ITEMS_AFTER_ADVANCED_OPTIONS)
                .ok_or_else(|| TrayError::new("tray menu is missing its About and Quit items"))?;
            insert_advanced_menu_items(&self.menu, &self.advanced_menu_items, position)?;
        } else {
            remove_advanced_menu_items(&self.menu, &self.advanced_menu_items)?;
        }

        self.advanced_options_visible = visible;
        self.show_advanced_options_item.set_checked(visible);
        Ok(())
    }

    pub(crate) fn action_sender(&self) -> Sender<TrayAction> {
        self.actions_tx.clone()
    }

    pub fn set_check_for_updates_enabled(&self, enabled: bool) {
        self.check_for_updates_item.set_enabled(enabled);
    }

    pub fn set_update_available(&self, available: bool) {
        self.check_for_updates_item.set_text(if available {
            "Update available!"
        } else {
            "Check for Updates..."
        });
    }

    pub fn set_device_monitored_checked(&self, identity: &str, checked: bool) {
        if let Some(entry) = self
            .monitored_device_items
            .iter()
            .find(|entry| entry.identity == identity)
        {
            entry.item.set_checked(checked);
        }
    }

    /// Applies a persisted tray icon style and clears the sibling check item:
    /// the native menu has no radio groups.
    pub fn set_tray_icon_style(&mut self, style: TrayIconStyle) {
        self.tray_icon_style = style;
        self.advanced_menu_items
            .battery_style_item
            .set_checked(style == TrayIconStyle::Battery);
        self.advanced_menu_items
            .percentage_style_item
            .set_checked(style == TrayIconStyle::Percentage);
    }

    /// Applies a persisted pinned device and clears the sibling check items.
    pub fn set_pinned_device_checked(&mut self, identity: Option<&str>) {
        self.pinned_device = identity.map(str::to_owned);
        self.apply_pinned_device_checks();
    }

    fn apply_pinned_device_checks(&self) {
        self.advanced_menu_items
            .pinned_lowest_item
            .set_checked(self.pinned_device.is_none());
        for entry in &self.pinned_device_items {
            entry
                .item
                .set_checked(self.pinned_device.as_deref() == Some(entry.identity.as_str()));
        }
    }

    pub fn set_battery_color_map(
        &mut self,
        battery_color_map: BatteryColorMap,
    ) -> Result<(), TrayError> {
        if self.battery_color_map == battery_color_map {
            return Ok(());
        }
        if let Some(percentage) = self.percentage {
            let icon_theme = current_icon_theme()?;
            let icon =
                self.render_icon(Some(percentage), ICON_SIZE, battery_color_map, icon_theme)?;
            self.icon
                .set_icon(Some(icon))
                .map_err(|error| TrayError::new(format!("could not update tray icon: {error}")))?;
            self.displayed_icon = Some((
                Some(percentage),
                ICON_SIZE,
                battery_color_map,
                self.tray_icon_style,
                icon_theme,
            ));
        }
        self.battery_color_map = battery_color_map;
        Ok(())
    }

    fn render_icon(
        &self,
        percentage: Option<u8>,
        icon_size: u32,
        battery_color_map: BatteryColorMap,
        icon_theme: IconTheme,
    ) -> Result<Icon, TrayError> {
        match percentage {
            Some(percentage) if self.tray_icon_style == TrayIconStyle::Percentage => {
                percentage_icon_at_size_with_color_map(percentage, icon_size, battery_color_map)
            }
            Some(percentage) => battery_icon_at_size_with_color_map(
                percentage,
                icon_size,
                battery_color_map,
                icon_theme,
            ),
            None => unknown_battery_icon_at_size(icon_size, icon_theme),
        }
    }
    fn update_percentage(
        &mut self,
        percentage: Option<u8>,
        clear_when_missing: bool,
    ) -> Option<u8> {
        match percentage {
            Some(percentage) => self.percentage = Some(percentage),
            None if clear_when_missing => self.percentage = None,
            None => {}
        }
        percentage.or(self.percentage)
    }
}

fn sorted_monitored_devices_if_changed(
    known_devices: &[DeviceMonitorEntry],
    devices: &[DeviceMonitorEntry],
) -> Option<Vec<DeviceMonitorEntry>> {
    let mut devices = devices.to_vec();
    devices.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.identity.cmp(&right.identity))
    });
    (devices != known_devices).then_some(devices)
}

fn tooltip_for_devices(summary: &str, tooltip_statuses: &[String]) -> String {
    if tooltip_statuses.is_empty() {
        return shorten_tooltip(summary);
    }
    let details = tooltip_statuses.join("\n");
    if details.encode_utf16().count() <= MAX_TOOLTIP_UTF16_UNITS {
        return details;
    }
    shorten_tooltip(&format!(
        "{summary}; left-click for dashboard, right-click for device stats"
    ))
}

fn shorten_tooltip(status: &str) -> String {
    let mut tooltip = String::new();
    let mut utf16_units = 0;
    for character in status.chars() {
        let character_units = character.len_utf16();
        if utf16_units + character_units > MAX_TOOLTIP_UTF16_UNITS - 1 {
            tooltip.push('…');
            return tooltip;
        }
        tooltip.push(character);
        utf16_units += character_units;
    }
    tooltip
}

fn shorten_status(status: &str) -> String {
    let mut characters = status.chars();
    let mut shortened: String = characters.by_ref().take(MAX_STATUS_CHARS).collect();
    if characters.next().is_some() {
        shortened.pop();
        shortened.push('…');
    }
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_shortened_only_when_over_limit() {
        let short = "Battery status";
        assert_eq!(shorten_status(short), short);

        let exact_limit = "x".repeat(MAX_STATUS_CHARS);
        assert_eq!(shorten_status(&exact_limit), exact_limit);

        let long = "x".repeat(MAX_STATUS_CHARS + 10);
        let shortened = shorten_status(&long);
        assert_eq!(shortened.chars().count(), MAX_STATUS_CHARS);
        assert!(shortened.ends_with('…'));
    }

    #[test]
    fn menu_item_ids_round_trip() {
        for id in [
            MenuItemId::Status,
            MenuItemId::OpenDashboard,
            MenuItemId::Discover,
            MenuItemId::Configuration,
            MenuItemId::LogFile,
            MenuItemId::SaveHardwareCapture,
            MenuItemId::Diagnose,
            MenuItemId::StartOnBoot,
            MenuItemId::ShowAdvancedOptions,
            MenuItemId::ResetVolty,
            MenuItemId::MonitoredDevices,
            MenuItemId::MonitoredDevice(42),
            MenuItemId::TrayDisplay,
            MenuItemId::TrayIconStyleBattery,
            MenuItemId::TrayIconStylePercentage,
            MenuItemId::PinnedDeviceLowest,
            MenuItemId::PinnedDevice(7),
            MenuItemId::CheckForUpdates,
            MenuItemId::About,
            MenuItemId::Quit,
        ] {
            assert_eq!(MenuItemId::parse(&id.as_id()), Some(id));
        }
        assert_eq!(
            MenuItemId::parse("volty.monitored-device.not-a-number"),
            None
        );
        assert_eq!(MenuItemId::parse("volty.monitored-device."), None);
        assert_eq!(MenuItemId::parse("volty.pinned-device.not-a-number"), None);
        assert_eq!(MenuItemId::parse("volty.pinned-device."), None);
        assert_eq!(
            MenuItemId::parse("volty.pinned-device.lowest"),
            Some(MenuItemId::PinnedDeviceLowest)
        );
        assert_eq!(
            MenuItemId::parse(&format!("volty.monitored-device.{}", u64::MAX)),
            Some(MenuItemId::MonitoredDevice(u64::MAX))
        );
        assert_eq!(
            MenuItemId::parse("volty.monitored-device.18446744073709551616"),
            None
        );
    }

    fn menu_item_ids(menu: &Menu) -> Vec<String> {
        menu.items()
            .iter()
            .map(|item| item.id().0.clone())
            .collect()
    }

    #[test]
    fn advanced_options_are_hidden_by_default_and_toggle_in_place() {
        let tray_menu = create_tray_menu(false, false).unwrap();
        let basic_ids = menu_item_ids(&tray_menu.menu);
        assert!(basic_ids.contains(&MenuItemId::ShowAdvancedOptions.as_id()));
        assert!(basic_ids.contains(&MenuItemId::MonitoredDevices.as_id()));
        assert!(basic_ids.contains(&MenuItemId::StartOnBoot.as_id()));
        assert!(basic_ids.contains(&MenuItemId::OpenDashboard.as_id()));
        assert!(basic_ids.contains(&MenuItemId::CheckForUpdates.as_id()));
        assert!(
            basic_ids
                .iter()
                .position(|id| id == &MenuItemId::MonitoredDevices.as_id())
                .unwrap()
                < basic_ids
                    .iter()
                    .position(|id| id == &MenuItemId::OpenDashboard.as_id())
                    .unwrap()
        );
        for id in [
            MenuItemId::TrayDisplay,
            MenuItemId::Discover,
            MenuItemId::Configuration,
            MenuItemId::LogFile,
            MenuItemId::SaveHardwareCapture,
            MenuItemId::Diagnose,
            MenuItemId::ResetVolty,
        ] {
            assert!(!basic_ids.contains(&id.as_id()), "{id:?} should be hidden");
        }

        let insertion_position = basic_ids.len() - MENU_ITEMS_AFTER_ADVANCED_OPTIONS;
        insert_advanced_menu_items(
            &tray_menu.menu,
            &tray_menu.advanced_menu_items,
            insertion_position,
        )
        .unwrap();
        let advanced_ids = menu_item_ids(&tray_menu.menu);
        for id in [
            MenuItemId::TrayDisplay,
            MenuItemId::Discover,
            MenuItemId::Configuration,
            MenuItemId::LogFile,
            MenuItemId::SaveHardwareCapture,
            MenuItemId::Diagnose,
            MenuItemId::ResetVolty,
        ] {
            assert!(
                advanced_ids.contains(&id.as_id()),
                "{id:?} should be visible"
            );
        }
        assert!(
            advanced_ids
                .iter()
                .position(|id| id == &MenuItemId::ShowAdvancedOptions.as_id())
                .unwrap()
                < advanced_ids
                    .iter()
                    .position(|id| id == &MenuItemId::TrayDisplay.as_id())
                    .unwrap()
        );
        assert!(
            advanced_ids
                .iter()
                .position(|id| id == &MenuItemId::TrayDisplay.as_id())
                .unwrap()
                < advanced_ids
                    .iter()
                    .position(|id| id == &MenuItemId::About.as_id())
                    .unwrap()
        );

        remove_advanced_menu_items(&tray_menu.menu, &tray_menu.advanced_menu_items).unwrap();
        assert_eq!(menu_item_ids(&tray_menu.menu), basic_ids);

        let restored_menu = create_tray_menu(false, true).unwrap();
        assert!(
            menu_item_ids(&restored_menu.menu).contains(&MenuItemId::SaveHardwareCapture.as_id())
        );
    }

    fn monitor_entry(identity: &str, name: &str, monitored: bool) -> DeviceMonitorEntry {
        DeviceMonitorEntry {
            identity: identity.to_owned(),
            name: name.to_owned(),
            monitored,
        }
    }

    #[test]
    fn monitored_device_refresh_sorts_entries_and_detects_only_real_changes() {
        let sorted = vec![
            monitor_entry("a", "Mouse A", true),
            monitor_entry("z", "Mouse A", false),
            monitor_entry("b", "Mouse B", true),
        ];
        let incoming = vec![sorted[2].clone(), sorted[1].clone(), sorted[0].clone()];

        assert_eq!(
            sorted_monitored_devices_if_changed(&sorted, &incoming),
            None
        );

        let mut changed = incoming;
        changed[1].monitored = true;
        let mut expected = sorted.clone();
        expected[1].monitored = true;
        assert_eq!(
            sorted_monitored_devices_if_changed(&sorted, &changed),
            Some(expected)
        );
    }

    #[test]
    fn tooltip_includes_all_device_statuses_when_they_fit() {
        let statuses = vec![
            "Mouse A (slot 1): 85%".to_owned(),
            "Mouse B (slot 2): 42% · ETA 3h 12m".to_owned(),
        ];

        assert_eq!(
            tooltip_for_devices("2 devices monitored", &statuses),
            statuses.join("\n")
        );
    }

    #[test]
    fn tooltip_shows_two_compact_device_lines() {
        let statuses = vec![
            "G502 X PLUS (slot 1): 56% · ETA 1d 8h".to_owned(),
            "Baseus Bass BS2 Lite Hands-Free AG: 90% · ETA calculating".to_owned(),
        ];

        let tooltip = tooltip_for_devices("2 devices monitored", &statuses);

        assert_eq!(tooltip.lines().count(), 2);
        assert!(tooltip.starts_with("G502 X PLUS (slot 1): 56% · ETA 1d 8h\n"));
        assert!(tooltip.ends_with("Baseus Bass BS2 Lite Hands-Free AG: 90% · ETA calculating"));
        assert!(tooltip.encode_utf16().count() <= MAX_TOOLTIP_UTF16_UNITS);
    }

    #[test]
    fn empty_tooltip_falls_back_to_the_summary() {
        assert_eq!(
            tooltip_for_devices("No supported devices discovered", &[]),
            "No supported devices discovered"
        );
    }

    #[test]
    fn tooltip_respects_utf16_length_boundaries() {
        let exact_ascii = vec!["x".repeat(MAX_TOOLTIP_UTF16_UNITS)];
        assert_eq!(tooltip_for_devices("summary", &exact_ascii), exact_ascii[0]);

        let exact_unicode = vec![format!("{}{}", "🙂".repeat(32), "x".repeat(63))];
        assert_eq!(
            exact_unicode[0].encode_utf16().count(),
            MAX_TOOLTIP_UTF16_UNITS
        );
        assert_eq!(
            tooltip_for_devices("summary", &exact_unicode),
            exact_unicode[0]
        );

        let overflow = vec![format!("{}{}", "🙂".repeat(32), "x".repeat(64))];
        let tooltip = tooltip_for_devices("summary", &overflow);
        assert!(tooltip.contains("left-click for dashboard, right-click for device stats"));
        assert!(tooltip.encode_utf16().count() <= MAX_TOOLTIP_UTF16_UNITS);
    }

    #[test]
    fn long_tooltip_points_to_full_device_status_menu() {
        let statuses = vec!["x".repeat(MAX_TOOLTIP_UTF16_UNITS); 2];
        let tooltip = tooltip_for_devices("2 devices monitored", &statuses);

        assert!(tooltip.encode_utf16().count() <= MAX_TOOLTIP_UTF16_UNITS);
        assert!(tooltip.contains("left-click for dashboard, right-click for device stats"));
    }
}
