//! System-tray UI and small platform integrations for Volty.
//!
//! Call [`TrayController::process_menu_events`] from the application's event
//! loop. Menu events are received through tray-icon's process-global receiver,
//! so an application should own one controller.

use std::{
    error::Error,
    fmt,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
};

use image::{Rgba, RgbaImage};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    Icon, TrayIcon, TrayIconBuilder,
};

use crate::config::BatteryColorMap;

const ICON_SIZE: u32 = 16;
const MAX_STATUS_CHARS: usize = 80;
const MAX_TOOLTIP_UTF16_UNITS: usize = 127;
#[cfg(windows)]
const BATTERY_RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
#[cfg(windows)]
const APP_RUN_VALUE: &str = "Volty";
#[cfg(windows)]
const APP_SETTINGS_KEY: &str = "Software\\Volty";
#[cfg(windows)]
const STARTUP_PROMPT_VALUE: &str = "StartupPrompted";
const DISCOVER_ID: &str = "volty.discover";
const CONFIGURATION_ID: &str = "volty.configuration";
const LOG_FILE_ID: &str = "volty.open-log-file";
const DIAGNOSE_ID: &str = "volty.diagnose";
const START_ON_BOOT_ID: &str = "volty.start-on-boot";
const MONITORED_DEVICES_ID: &str = "volty.monitored-devices";
const MONITORED_DEVICE_ITEM_PREFIX: &str = "volty.monitored-device";
const ABOUT_ID: &str = "volty.about";
const QUIT_ID: &str = "volty.quit";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMonitorEntry {
    pub identity: String,
    pub name: String,
    pub monitored: bool,
}

struct MonitoredDeviceMenuItem {
    menu_id: String,
    identity: String,
    item: CheckMenuItem,
}

/// Actions that the application should handle after polling the controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    DiscoverDevicesNow,
    OpenConfiguration,
    OpenLogFile,
    Diagnose,
    ToggleStartOnBoot,
    SetDeviceMonitored { identity: String, monitored: bool },
    About,
    Quit,
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
    start_on_boot_item: CheckMenuItem,
    actions_tx: Sender<TrayAction>,
    actions_rx: Receiver<TrayAction>,
    percentage: Option<u8>,
    battery_color_map: BatteryColorMap,
}

impl TrayController {
    /// Creates the tray icon and menu.
    ///
    /// The Start on Boot state is initialized from the Windows Run key. On
    /// other platforms the check item starts unchecked and the action reports
    /// that autostart is unsupported when selected.
    pub fn new() -> Result<Self, TrayError> {
        let start_on_boot = start_on_boot_for_first_run()?;
        let status_item = MenuItem::with_id(
            "volty.status",
            "Searching for supported devices",
            false,
            None,
        );
        let discover_item = MenuItem::with_id(DISCOVER_ID, "Discover Devices Now", true, None);
        let configuration_item =
            MenuItem::with_id(CONFIGURATION_ID, "Open Configuration", true, None);
        let open_log_item = MenuItem::with_id(LOG_FILE_ID, "Open Log File", true, None);
        let diagnose_item =
            MenuItem::with_id(DIAGNOSE_ID, "Diagnose / Report an Issue", true, None);
        let start_on_boot_item =
            CheckMenuItem::with_id(START_ON_BOOT_ID, "Start on Boot", true, start_on_boot, None);
        let monitored_devices_menu =
            Submenu::with_id(MONITORED_DEVICES_ID, "Monitored Devices", true);
        let monitored_devices_empty_item =
            MenuItem::new("No supported devices discovered", false, None);
        monitored_devices_menu
            .append(&monitored_devices_empty_item)
            .map_err(|error| {
                TrayError::new(format!(
                    "could not add empty monitored-devices status: {error}"
                ))
            })?;
        let about_item = MenuItem::with_id(ABOUT_ID, "About Volty", true, None);
        let quit_item = MenuItem::with_id(QUIT_ID, "Quit", true, None);
        let menu = Menu::new();
        menu.append(&status_item)
            .map_err(|error| TrayError::new(format!("could not add tray status: {error}")))?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
        menu.append(&monitored_devices_menu).map_err(|error| {
            TrayError::new(format!("could not add monitored-devices menu: {error}"))
        })?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
        menu.append(&discover_item).map_err(|error| {
            TrayError::new(format!("could not add device-discovery action: {error}"))
        })?;
        menu.append(&configuration_item).map_err(|error| {
            TrayError::new(format!("could not add configuration action: {error}"))
        })?;
        menu.append(&open_log_item)
            .map_err(|error| TrayError::new(format!("could not add log-file action: {error}")))?;
        menu.append(&diagnose_item)
            .map_err(|error| TrayError::new(format!("could not add Diagnose action: {error}")))?;
        menu.append(&start_on_boot_item).map_err(|error| {
            TrayError::new(format!("could not add start-on-boot action: {error}"))
        })?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
        menu.append(&about_item)
            .map_err(|error| TrayError::new(format!("could not add About action: {error}")))?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
        menu.append(&quit_item)
            .map_err(|error| TrayError::new(format!("could not add quit action: {error}")))?;

        let icon = TrayIconBuilder::new()
            .with_tooltip("Volty — Searching for supported devices")
            .with_icon(unknown_battery_icon_at_size(ICON_SIZE)?)
            .with_menu(Box::new(menu.clone()))
            .build()
            .map_err(|error| TrayError::new(format!("could not create tray icon: {error}")))?;
        let (actions_tx, actions_rx) = mpsc::channel();

        Ok(Self {
            icon,
            menu,
            status_item,
            device_status_items: Vec::new(),
            monitored_devices_menu,
            monitored_devices_empty_item: Some(monitored_devices_empty_item),
            monitored_device_items: Vec::new(),
            known_monitored_devices: Vec::new(),
            next_monitored_device_menu_id: 0,
            start_on_boot_item,
            actions_tx,
            actions_rx,
            percentage: None,
            battery_color_map: BatteryColorMap::default(),
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
        if let Some(percentage) = percentage {
            self.percentage = Some(percentage);
        }
        let icon_percentage = percentage.or(self.percentage);
        let live_status = match percentage {
            Some(percentage) => format!("{device_name}: {percentage}% ({status})"),
            None if device_name.is_empty() => status.to_owned(),
            None => format!("{device_name}: {status}"),
        };
        let live_status = shorten_status(&live_status);
        self.update_display(&live_status, &live_status, icon_percentage, icon_size, &[])
    }

    /// Displays each readable device at the top of the tray menu. The tooltip
    /// contains the full list when it fits.
    pub fn update_devices(
        &mut self,
        summary: &str,
        device_statuses: &[String],
        percentage: Option<u8>,
    ) -> Result<(), TrayError> {
        let percentage = percentage.map(|value| value.min(100));
        if let Some(percentage) = percentage {
            self.percentage = Some(percentage);
        } else if device_statuses.is_empty() {
            self.percentage = None;
        }
        let icon_percentage = percentage.or(self.percentage);
        let tooltip = tooltip_for_devices(summary, device_statuses);
        let empty_status = "No device status available".to_owned();
        let menu_device_statuses = if device_statuses.is_empty() {
            std::slice::from_ref(&empty_status)
        } else {
            device_statuses
        };
        self.update_display(
            &shorten_status(summary),
            &tooltip,
            icon_percentage,
            ICON_SIZE,
            menu_device_statuses,
        )
    }

    pub fn update_monitored_devices(
        &mut self,
        devices: &[DeviceMonitorEntry],
    ) -> Result<(), TrayError> {
        let mut devices = devices.to_vec();
        devices.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.identity.cmp(&right.identity))
        });
        if devices == self.known_monitored_devices {
            return Ok(());
        }

        for entry in self.monitored_device_items.drain(..) {
            self.monitored_devices_menu
                .remove(&entry.item)
                .map_err(|error| {
                    TrayError::new(format!("could not clear monitored devices: {error}"))
                })?;
        }

        if devices.is_empty() {
            if self.monitored_devices_empty_item.is_none() {
                let item = MenuItem::new("No supported devices discovered", false, None);
                self.monitored_devices_menu.append(&item).map_err(|error| {
                    TrayError::new(format!(
                        "could not update empty monitored-devices status: {error}"
                    ))
                })?;
                self.monitored_devices_empty_item = Some(item);
            }
            self.known_monitored_devices = devices;
            return Ok(());
        }

        if let Some(item) = self.monitored_devices_empty_item.take() {
            self.monitored_devices_menu.remove(&item).map_err(|error| {
                TrayError::new(format!(
                    "could not clear empty monitored-devices status: {error}"
                ))
            })?;
        }

        for device in &devices {
            let sequence = self.next_monitored_device_menu_id;
            self.next_monitored_device_menu_id = sequence
                .checked_add(1)
                .ok_or_else(|| TrayError::new("monitored-device menu ID space exhausted"))?;
            let menu_id = format!("{MONITORED_DEVICE_ITEM_PREFIX}.{sequence}");
            let item = CheckMenuItem::with_id(&menu_id, &device.name, true, device.monitored, None);
            self.monitored_devices_menu.append(&item).map_err(|error| {
                TrayError::new(format!("could not add monitored device: {error}"))
            })?;
            self.monitored_device_items.push(MonitoredDeviceMenuItem {
                menu_id,
                identity: device.identity.clone(),
                item,
            });
        }
        self.known_monitored_devices = devices;
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
        self.status_item.set_text(menu_status);
        self.replace_device_status_items(device_statuses)?;
        let icon = match percentage {
            Some(percentage) => {
                battery_icon_at_size_with_color_map(percentage, icon_size, self.battery_color_map)?
            }
            None => unknown_battery_icon_at_size(icon_size)?,
        };
        self.icon
            .set_icon(Some(icon))
            .map_err(|error| TrayError::new(format!("could not update tray icon: {error}")))?;
        self.icon
            .set_tooltip(Some(tooltip))
            .map_err(|error| TrayError::new(format!("could not update tray tooltip: {error}")))?;
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
        // Keep channel disconnection distinct from an empty event queue.
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

            let id = event.id.0.as_str();
            if id == DISCOVER_ID {
                self.actions_tx
                    .send(TrayAction::DiscoverDevicesNow)
                    .map_err(|_| TrayError::new("tray action receiver has been dropped"))?;
                handled += 1;
            } else if id == CONFIGURATION_ID {
                self.actions_tx
                    .send(TrayAction::OpenConfiguration)
                    .map_err(|_| TrayError::new("tray action receiver has been dropped"))?;
                handled += 1;
            } else if id == LOG_FILE_ID {
                self.actions_tx
                    .send(TrayAction::OpenLogFile)
                    .map_err(|_| TrayError::new("tray action receiver has been dropped"))?;
                handled += 1;
            } else if id == DIAGNOSE_ID {
                self.actions_tx
                    .send(TrayAction::Diagnose)
                    .map_err(|_| TrayError::new("tray action receiver has been dropped"))?;
                handled += 1;
            } else if id == START_ON_BOOT_ID {
                self.actions_tx
                    .send(TrayAction::ToggleStartOnBoot)
                    .map_err(|_| TrayError::new("tray action receiver has been dropped"))?;
                handled += 1;
            } else if let Some(entry) = self
                .monitored_device_items
                .iter()
                .find(|entry| entry.menu_id == id)
            {
                self.actions_tx
                    .send(TrayAction::SetDeviceMonitored {
                        identity: entry.identity.clone(),
                        monitored: entry.item.is_checked(),
                    })
                    .map_err(|_| TrayError::new("tray action receiver has been dropped"))?;
                handled += 1;
            } else if id == ABOUT_ID {
                self.actions_tx
                    .send(TrayAction::About)
                    .map_err(|_| TrayError::new("tray action receiver has been dropped"))?;
                handled += 1;
            } else if id == QUIT_ID {
                self.actions_tx
                    .send(TrayAction::Quit)
                    .map_err(|_| TrayError::new("tray action receiver has been dropped"))?;
                handled += 1;
            }
        }
        Ok(handled)
    }

    /// Gets the next app action, if one has been queued.
    pub fn try_recv_action(&self) -> Result<TrayAction, TryRecvError> {
        self.actions_rx.try_recv()
    }

    pub fn set_start_on_boot_checked(&self, checked: bool) {
        self.start_on_boot_item.set_checked(checked);
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

    pub fn set_battery_color_map(
        &mut self,
        battery_color_map: BatteryColorMap,
    ) -> Result<(), TrayError> {
        if self.battery_color_map == battery_color_map {
            return Ok(());
        }
        if let Some(percentage) = self.percentage {
            let icon =
                battery_icon_at_size_with_color_map(percentage, ICON_SIZE, battery_color_map)?;
            self.icon
                .set_icon(Some(icon))
                .map_err(|error| TrayError::new(format!("could not update tray icon: {error}")))?;
        }
        self.battery_color_map = battery_color_map;
        Ok(())
    }
}

fn tooltip_for_devices(summary: &str, device_statuses: &[String]) -> String {
    let details = device_statuses.join("; ");
    if details.encode_utf16().count() <= MAX_TOOLTIP_UTF16_UNITS {
        return details;
    }
    shorten_tooltip(&format!("{summary}; right-click for all device stats"))
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

/// Renders an RGBA battery-level icon. Supported sizes are 16 and 32 pixels square.
#[cfg(test)]
fn battery_rgba(percentage: u8, size: u32) -> Result<Vec<u8>, TrayError> {
    battery_rgba_with_color_map(percentage, size, BatteryColorMap::default())
}

fn battery_rgba_with_color_map(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    Ok(render_battery(percentage, size, battery_color_map).into_raw())
}

/// Produces a battery-level icon at 16×16 or 32×32 pixels.
#[cfg(test)]
fn battery_icon_at_size(percentage: u8, size: u32) -> Result<Icon, TrayError> {
    let rgba = battery_rgba(percentage, size)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create battery icon: {error}")))
}

fn battery_icon_at_size_with_color_map(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
) -> Result<Icon, TrayError> {
    let rgba = battery_rgba_with_color_map(percentage, size, battery_color_map)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create battery icon: {error}")))
}

fn render_battery(percentage: u8, size: u32, battery_color_map: BatteryColorMap) -> RgbaImage {
    let percentage = percentage.min(100);
    let scale = size as f32 * SUPERSAMPLE as f32 / ICON_SIZE as f32;
    let mut image =
        RgbaImage::from_pixel(size * SUPERSAMPLE, size * SUPERSAMPLE, Rgba([0, 0, 0, 0]));
    let fill_color = battery_color_map.color_for(percentage);

    if percentage > 0 {
        let fill_width = 10.25 * (f32::from(percentage) / 100.0).max(0.08);
        fill_rounded_rect(
            &mut image,
            scale,
            RoundedRect {
                x: 2.25,
                y: 3.25,
                width: fill_width,
                height: 9.55,
                radius: 1.2,
            },
            Rgba([fill_color[0], fill_color[1], fill_color[2], 255]),
        );
    }
    draw_battery_shell(&mut image, scale, Rgba([232, 238, 243, 255]));

    downsample_icon(image, size)
}

/// Renders the neutral unknown/error icon as an RGBA buffer (16×16 or 32×32).
pub fn unknown_battery_rgba(size: u32) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    Ok(render_unknown_battery(size).into_raw())
}

fn unknown_battery_icon_at_size(size: u32) -> Result<Icon, TrayError> {
    let rgba = unknown_battery_rgba(size)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create unknown battery icon: {error}")))
}

fn render_unknown_battery(size: u32) -> RgbaImage {
    let scale = size as f32 * SUPERSAMPLE as f32 / ICON_SIZE as f32;
    let mut image =
        RgbaImage::from_pixel(size * SUPERSAMPLE, size * SUPERSAMPLE, Rgba([0, 0, 0, 0]));
    let outline = Rgba([232, 238, 243, 255]);
    draw_battery_shell(&mut image, scale, outline);

    let question_mark = [
        (5.5, 6.45),
        (5.8, 5.8),
        (6.5, 5.3),
        (7.4, 5.1),
        (8.4, 5.3),
        (9.0, 5.85),
        (9.2, 6.5),
        (9.0, 7.2),
        (8.4, 7.8),
        (7.55, 8.45),
    ];
    for points in question_mark.windows(2) {
        draw_line_segment(&mut image, scale, points[0], points[1], 1.25, outline);
    }
    fill_circle(&mut image, scale, 7.55, 10.0, 0.75, outline);

    downsample_icon(image, size)
}

fn validate_icon_size(size: u32) -> Result<(), TrayError> {
    if matches!(size, 16 | 32) {
        Ok(())
    } else {
        Err(TrayError::new("tray icon size must be 16 or 32 pixels"))
    }
}

const SUPERSAMPLE: u32 = 8;

#[derive(Clone, Copy)]
struct RoundedRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
}

fn draw_battery_shell(image: &mut RgbaImage, scale: f32, color: Rgba<u8>) {
    stroke_rounded_rect(
        image,
        scale,
        RoundedRect {
            x: 0.65,
            y: 1.55,
            width: 13.4,
            height: 12.9,
            radius: 2.4,
        },
        1.3,
        color,
    );
    fill_rounded_rect(
        image,
        scale,
        RoundedRect {
            x: 13.75,
            y: 6.2,
            width: 1.5,
            height: 3.6,
            radius: 0.75,
        },
        color,
    );
}

fn fill_rounded_rect(image: &mut RgbaImage, scale: f32, rect: RoundedRect, color: Rgba<u8>) {
    let left = (rect.x.max(0.0) * scale).floor() as u32;
    let top = (rect.y.max(0.0) * scale).floor() as u32;
    let right = ((rect.x + rect.width).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.width() as f32) as u32;
    let bottom = ((rect.y + rect.height).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.height() as f32) as u32;

    for pixel_y in top..bottom {
        for pixel_x in left..right {
            let point_x = (pixel_x as f32 + 0.5) / scale;
            let point_y = (pixel_y as f32 + 0.5) / scale;
            if inside_rounded_rect(point_x, point_y, rect) {
                image.put_pixel(pixel_x, pixel_y, color);
            }
        }
    }
}

fn stroke_rounded_rect(
    image: &mut RgbaImage,
    scale: f32,
    rect: RoundedRect,
    thickness: f32,
    color: Rgba<u8>,
) {
    let left = (rect.x.max(0.0) * scale).floor() as u32;
    let top = (rect.y.max(0.0) * scale).floor() as u32;
    let right = ((rect.x + rect.width).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.width() as f32) as u32;
    let bottom = ((rect.y + rect.height).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.height() as f32) as u32;

    for pixel_y in top..bottom {
        for pixel_x in left..right {
            let point_x = (pixel_x as f32 + 0.5) / scale;
            let point_y = (pixel_y as f32 + 0.5) / scale;
            let outer = inside_rounded_rect(point_x, point_y, rect);
            let inner = inside_rounded_rect(
                point_x,
                point_y,
                RoundedRect {
                    x: rect.x + thickness,
                    y: rect.y + thickness,
                    width: rect.width - 2.0 * thickness,
                    height: rect.height - 2.0 * thickness,
                    radius: (rect.radius - thickness).max(0.0),
                },
            );
            if outer && !inner {
                image.put_pixel(pixel_x, pixel_y, color);
            }
        }
    }
}

fn inside_rounded_rect(point_x: f32, point_y: f32, rect: RoundedRect) -> bool {
    let radius = rect.radius.min(rect.width / 2.0).min(rect.height / 2.0);
    let half_width = (rect.width / 2.0 - radius).max(0.0);
    let half_height = (rect.height / 2.0 - radius).max(0.0);
    let delta_x = (point_x - (rect.x + rect.width / 2.0)).abs() - half_width;
    let delta_y = (point_y - (rect.y + rect.height / 2.0)).abs() - half_height;
    delta_x.max(0.0).hypot(delta_y.max(0.0)) + delta_x.max(delta_y).min(0.0) <= radius
}

fn draw_line_segment(
    image: &mut RgbaImage,
    scale: f32,
    start: (f32, f32),
    end: (f32, f32),
    thickness: f32,
    color: Rgba<u8>,
) {
    let radius = thickness / 2.0;
    let left = ((start.0.min(end.0) - radius).max(0.0) * scale).floor() as u32;
    let top = ((start.1.min(end.1) - radius).max(0.0) * scale).floor() as u32;
    let right = ((start.0.max(end.0) + radius).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.width() as f32) as u32;
    let bottom = ((start.1.max(end.1) + radius).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.height() as f32) as u32;
    let vector_x = end.0 - start.0;
    let vector_y = end.1 - start.1;
    let length_squared = vector_x * vector_x + vector_y * vector_y;

    for pixel_y in top..bottom {
        for pixel_x in left..right {
            let point_x = (pixel_x as f32 + 0.5) / scale;
            let point_y = (pixel_y as f32 + 0.5) / scale;
            let projection = (((point_x - start.0) * vector_x + (point_y - start.1) * vector_y)
                / length_squared)
                .clamp(0.0, 1.0);
            let nearest_x = start.0 + projection * vector_x;
            let nearest_y = start.1 + projection * vector_y;
            if (point_x - nearest_x).powi(2) + (point_y - nearest_y).powi(2) <= radius * radius {
                image.put_pixel(pixel_x, pixel_y, color);
            }
        }
    }
}

fn fill_circle(
    image: &mut RgbaImage,
    scale: f32,
    center_x: f32,
    center_y: f32,
    radius: f32,
    color: Rgba<u8>,
) {
    let left = ((center_x - radius).max(0.0) * scale).floor() as u32;
    let top = ((center_y - radius).max(0.0) * scale).floor() as u32;
    let right = ((center_x + radius).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.width() as f32) as u32;
    let bottom = ((center_y + radius).min(ICON_SIZE as f32) * scale)
        .ceil()
        .min(image.height() as f32) as u32;

    for pixel_y in top..bottom {
        for pixel_x in left..right {
            let point_x = (pixel_x as f32 + 0.5) / scale;
            let point_y = (pixel_y as f32 + 0.5) / scale;
            if (point_x - center_x).powi(2) + (point_y - center_y).powi(2) <= radius * radius {
                image.put_pixel(pixel_x, pixel_y, color);
            }
        }
    }
}

fn downsample_icon(image: RgbaImage, size: u32) -> RgbaImage {
    let mut output = RgbaImage::new(size, size);
    let sample_count = u64::from(SUPERSAMPLE * SUPERSAMPLE);

    // Premultiplied averaging keeps transparent edges clean on any taskbar background.
    for y in 0..size {
        for x in 0..size {
            let mut alpha_sum = 0_u64;
            let mut color_sum = [0_u64; 3];
            for sample_y in 0..SUPERSAMPLE {
                for sample_x in 0..SUPERSAMPLE {
                    let pixel =
                        image.get_pixel(x * SUPERSAMPLE + sample_x, y * SUPERSAMPLE + sample_y);
                    let alpha = u64::from(pixel[3]);
                    alpha_sum += alpha;
                    for channel in 0..3 {
                        color_sum[channel] += u64::from(pixel[channel]) * alpha;
                    }
                }
            }

            if alpha_sum > 0 {
                let alpha = ((alpha_sum + sample_count / 2) / sample_count) as u8;
                let color = color_sum.map(|channel| ((channel + alpha_sum / 2) / alpha_sum) as u8);
                output.put_pixel(x, y, Rgba([color[0], color[1], color[2], alpha]));
            }
        }
    }

    output
}

/// Checks whether the Windows Run-key entry exists.
pub fn start_on_boot_enabled() -> Result<bool, TrayError> {
    #[cfg(windows)]
    {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};

        let current_user = RegKey::predef(HKEY_CURRENT_USER);
        let run_key =
            match current_user.open_subkey_with_flags(BATTERY_RUN_KEY, winreg::enums::KEY_READ) {
                Ok(key) => key,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => {
                    return Err(TrayError::new(format!(
                        "could not read Windows startup setting: {error}"
                    )))
                }
            };
        match run_key.get_value::<String, _>(APP_RUN_VALUE) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(TrayError::new(format!(
                "could not read Windows startup setting: {error}"
            ))),
        }
    }

    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

#[cfg(windows)]
fn start_on_boot_for_first_run() -> Result<bool, TrayError> {
    let enabled = start_on_boot_enabled()?;
    if enabled || startup_prompt_shown()? {
        return Ok(enabled);
    }

    let text = to_wide(
        "Would you like Volty to start automatically when you sign in to Windows?\n\n\
         If you choose Yes, Windows will start Volty from this executable's current location. \
         Keep the file there. You can change this later from the tray menu.",
    );
    let title = to_wide("Start Volty with Windows?");
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
            std::io::Error::last_os_error()
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
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let key = match current_user.open_subkey_with_flags(APP_SETTINGS_KEY, winreg::enums::KEY_READ) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(TrayError::new(format!(
                "could not read first-run startup preference: {error}"
            )))
        }
    };
    match key.get_value::<u32, _>(STARTUP_PROMPT_VALUE) {
        Ok(value) => Ok(value != 0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(TrayError::new(format!(
            "could not read first-run startup preference: {error}"
        ))),
    }
}

#[cfg(windows)]
fn set_startup_prompt_shown() -> Result<(), TrayError> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = current_user
        .create_subkey(APP_SETTINGS_KEY)
        .map_err(|error| {
            TrayError::new(format!(
                "could not save first-run startup preference: {error}"
            ))
        })?;
    key.set_value(STARTUP_PROMPT_VALUE, &1u32).map_err(|error| {
        TrayError::new(format!(
            "could not save first-run startup preference: {error}"
        ))
    })
}

#[cfg(windows)]
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

/// Enables or disables Windows startup using the current executable's Run entry.
pub fn set_start_on_boot_enabled(enabled: bool) -> Result<(), TrayError> {
    #[cfg(windows)]
    {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};

        let current_user = RegKey::predef(HKEY_CURRENT_USER);
        if enabled {
            let (run_key, _) = current_user
                .create_subkey(BATTERY_RUN_KEY)
                .map_err(|error| {
                    TrayError::new(format!("could not enable Windows startup: {error}"))
                })?;
            let executable = std::env::current_exe().map_err(|error| {
                TrayError::new(format!("could not locate executable for startup: {error}"))
            })?;
            run_key
                .set_value(APP_RUN_VALUE, &format!("\"{}\"", executable.display()))
                .map_err(|error| {
                    TrayError::new(format!("could not enable Windows startup: {error}"))
                })?;
        } else {
            let run_key = match current_user
                .open_subkey_with_flags(BATTERY_RUN_KEY, winreg::enums::KEY_SET_VALUE)
            {
                Ok(key) => key,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => {
                    return Err(TrayError::new(format!(
                        "could not disable Windows startup: {error}"
                    )))
                }
            };
            match run_key.delete_value(APP_RUN_VALUE) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(TrayError::new(format!(
                        "could not disable Windows startup: {error}"
                    )))
                }
            }
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err(TrayError::new("Start on Boot is only supported on Windows"))
    }
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
    fn tooltip_includes_all_device_statuses_when_they_fit() {
        let statuses = vec![
            "Mouse A (slot 1): 85% (Discharging)".to_owned(),
            "Mouse B (slot 2): 42% (Charging)".to_owned(),
        ];

        assert_eq!(
            tooltip_for_devices("2 devices monitored", &statuses),
            statuses.join("; ")
        );
    }

    #[test]
    fn long_tooltip_points_to_full_device_status_menu() {
        let statuses = vec!["x".repeat(MAX_TOOLTIP_UTF16_UNITS); 2];
        let tooltip = tooltip_for_devices("2 devices monitored", &statuses);

        assert!(tooltip.encode_utf16().count() <= MAX_TOOLTIP_UTF16_UNITS);
        assert!(tooltip.contains("right-click for all device stats"));
    }

    #[test]
    fn icon_renderer_accepts_boundary_percentages() {
        for percentage in [0, 15, 16, 30, 31, 100] {
            let icon = battery_icon_at_size(percentage, ICON_SIZE).expect("valid 16x16 icon");
            let _ = icon;
        }
    }

    #[test]
    fn rgba_renderers_support_both_dpi_sizes() {
        for size in [16, 32] {
            assert_eq!(
                battery_rgba(85, size).unwrap().len(),
                (size * size * 4) as usize
            );
            assert_eq!(
                unknown_battery_rgba(size).unwrap().len(),
                (size * size * 4) as usize
            );
        }
        assert!(battery_rgba(85, 24).is_err());
    }

    #[test]
    fn rgba_renderers_antialias_icon_edges() {
        for rgba in [
            battery_rgba(85, ICON_SIZE).unwrap(),
            unknown_battery_rgba(ICON_SIZE).unwrap(),
        ] {
            assert!(rgba
                .iter()
                .skip(3)
                .step_by(4)
                .any(|alpha| *alpha > 0 && *alpha < 255));
        }
    }

    #[test]
    fn battery_icon_uses_nearly_the_full_canvas() {
        let rgba = battery_rgba(85, ICON_SIZE).unwrap();
        let mut bounds = [ICON_SIZE, ICON_SIZE, 0, 0];

        for y in 0..ICON_SIZE {
            for x in 0..ICON_SIZE {
                let alpha_index = ((y * ICON_SIZE + x) * 4 + 3) as usize;
                if rgba[alpha_index] > 0 {
                    bounds[0] = bounds[0].min(x);
                    bounds[1] = bounds[1].min(y);
                    bounds[2] = bounds[2].max(x);
                    bounds[3] = bounds[3].max(y);
                }
            }
        }

        assert_eq!(bounds, [0, 1, ICON_SIZE - 1, ICON_SIZE - 2]);
    }

    #[test]
    fn rgba_renderer_uses_custom_color_map_boundaries() {
        let map = BatteryColorMap {
            red_max_percent: 10,
            yellow_max_percent: 40,
            red: [1, 2, 3],
            yellow: [4, 5, 6],
            green: [7, 8, 9],
        };
        let pixel = |percentage| {
            let rgba = battery_rgba_with_color_map(percentage, ICON_SIZE, map).unwrap();
            let index = ((10 * ICON_SIZE + 3) * 4) as usize;
            [rgba[index], rgba[index + 1], rgba[index + 2]]
        };

        assert_eq!(pixel(10), [1, 2, 3]);
        assert_eq!(pixel(11), [4, 5, 6]);
        assert_eq!(pixel(40), [4, 5, 6]);
        assert_eq!(pixel(41), [7, 8, 9]);
    }
}
