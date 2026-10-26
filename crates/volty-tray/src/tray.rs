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
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

use crate::config::BatteryColorMap;

const ICON_SIZE: u32 = 16;
const MAX_STATUS_CHARS: usize = 80;
#[cfg(windows)]
const BATTERY_RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
#[cfg(windows)]
const APP_RUN_VALUE: &str = "Volty";
const REFRESH_ID: &str = "volty.refresh";
const CONFIGURATION_ID: &str = "volty.configuration";
const LOG_FILE_ID: &str = "volty.open-log-file";
const DIAGNOSE_ID: &str = "volty.diagnose";
const START_ON_BOOT_ID: &str = "volty.start-on-boot";
const ABOUT_ID: &str = "volty.about";
const QUIT_ID: &str = "volty.quit";

/// Actions that the application should handle after polling the controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    RefreshNow,
    OpenConfiguration,
    OpenLogFile,
    Diagnose,
    ToggleStartOnBoot,
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
    status_item: MenuItem,
    start_on_boot_item: CheckMenuItem,
    actions_tx: Sender<TrayAction>,
    actions_rx: Receiver<TrayAction>,
    percentage: Option<u8>,
    device_name: String,
    charge_status: String,
    battery_color_map: BatteryColorMap,
}

impl TrayController {
    /// Creates the tray icon and menu.
    ///
    /// The Start on Boot state is initialized from the Windows Run key. On
    /// other platforms the check item starts unchecked and the action reports
    /// that autostart is unsupported when selected.
    pub fn new() -> Result<Self, TrayError> {
        let start_on_boot = start_on_boot_enabled()?;
        let status_item =
            MenuItem::with_id("volty.status", "Searching for Logitech device", false, None);
        let refresh_item = MenuItem::with_id(REFRESH_ID, "Refresh Now", true, None);
        let configuration_item =
            MenuItem::with_id(CONFIGURATION_ID, "Open Configuration", true, None);
        let open_log_item = MenuItem::with_id(LOG_FILE_ID, "Open Log File", true, None);
        let diagnose_item =
            MenuItem::with_id(DIAGNOSE_ID, "Diagnose / Report an Issue", true, None);
        let start_on_boot_item =
            CheckMenuItem::with_id(START_ON_BOOT_ID, "Start on Boot", true, start_on_boot, None);
        let about_item = MenuItem::with_id(ABOUT_ID, "About Volty", true, None);
        let quit_item = MenuItem::with_id(QUIT_ID, "Quit", true, None);
        let menu = Menu::new();
        menu.append(&status_item)
            .map_err(|error| TrayError::new(format!("could not add tray status: {error}")))?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
        menu.append(&refresh_item)
            .map_err(|error| TrayError::new(format!("could not add refresh action: {error}")))?;
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
            .with_tooltip("Volty — Searching for Logitech device")
            .with_icon(unknown_battery_icon_at_size(ICON_SIZE)?)
            .with_menu(Box::new(menu))
            .build()
            .map_err(|error| TrayError::new(format!("could not create tray icon: {error}")))?;
        let (actions_tx, actions_rx) = mpsc::channel();

        Ok(Self {
            icon,
            status_item,
            start_on_boot_item,
            actions_tx,
            actions_rx,
            percentage: None,
            device_name: String::new(),
            charge_status: "Unknown".to_owned(),
            battery_color_map: BatteryColorMap::default(),
        })
    }

    /// Updates the tray icon, menu, and tooltip with the current device status.
    ///
    /// For example, `update_status("G502 X", "Discharging", Some(85))`
    /// displays `G502 X: 85% (Discharging)`. Pass `None` when the device or
    /// percentage is unavailable; the last numeric icon is retained if one
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
        self.device_name = device_name.to_owned();
        self.charge_status = status.to_owned();

        let icon = match icon_percentage {
            Some(percentage) => numeric_battery_icon_at_size_with_color_map(
                percentage,
                icon_size,
                self.battery_color_map,
            )?,
            None => unknown_battery_icon_at_size(icon_size)?,
        };
        self.icon
            .set_icon(Some(icon))
            .map_err(|error| TrayError::new(format!("could not update tray icon: {error}")))?;
        let live_status = match percentage {
            Some(percentage) => format!("{device_name}: {percentage}% ({status})"),
            None if device_name.is_empty() => status.to_owned(),
            None => format!("{device_name}: {status}"),
        };
        let live_status = shorten_status(&live_status);
        self.status_item.set_text(&live_status);
        self.icon
            .set_tooltip(Some(&live_status))
            .map_err(|error| TrayError::new(format!("could not update tray tooltip: {error}")))?;
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
            if id == REFRESH_ID {
                self.actions_tx
                    .send(TrayAction::RefreshNow)
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

    pub fn set_battery_color_map(
        &mut self,
        battery_color_map: BatteryColorMap,
    ) -> Result<(), TrayError> {
        if self.battery_color_map == battery_color_map {
            return Ok(());
        }
        if let Some(percentage) = self.percentage {
            let icon = numeric_battery_icon_at_size_with_color_map(
                percentage,
                ICON_SIZE,
                battery_color_map,
            )?;
            self.icon
                .set_icon(Some(icon))
                .map_err(|error| TrayError::new(format!("could not update tray icon: {error}")))?;
        }
        self.battery_color_map = battery_color_map;
        Ok(())
    }
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

/// Renders an RGBA numeric icon. Supported sizes are 16 and 32 pixels square.
#[cfg(test)]
fn numeric_battery_rgba(percentage: u8, size: u32) -> Result<Vec<u8>, TrayError> {
    numeric_battery_rgba_with_color_map(percentage, size, BatteryColorMap::default())
}

fn numeric_battery_rgba_with_color_map(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    Ok(scale_icon(render_numeric_battery(percentage, battery_color_map), size).into_raw())
}

/// Produces a numeric battery icon at 16×16 or 32×32 pixels.
#[cfg(test)]
fn numeric_battery_icon_at_size(percentage: u8, size: u32) -> Result<Icon, TrayError> {
    let rgba = numeric_battery_rgba(percentage, size)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create battery icon: {error}")))
}

fn numeric_battery_icon_at_size_with_color_map(
    percentage: u8,
    size: u32,
    battery_color_map: BatteryColorMap,
) -> Result<Icon, TrayError> {
    let rgba = numeric_battery_rgba_with_color_map(percentage, size, battery_color_map)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create battery icon: {error}")))
}

fn render_numeric_battery(percentage: u8, battery_color_map: BatteryColorMap) -> RgbaImage {
    let percentage = percentage.min(100);
    let color = battery_color_map.color_for(percentage);
    let mut image = RgbaImage::from_pixel(ICON_SIZE, ICON_SIZE, Rgba([0, 0, 0, 0]));
    let outline = Rgba([225, 232, 238, 255]);
    let interior = Rgba([30, 36, 42, 255]);

    // Battery body and terminal.
    for x in 1..14 {
        image.put_pixel(x, 2, outline);
        image.put_pixel(x, 13, outline);
    }
    for y in 2..=13 {
        image.put_pixel(1, y, outline);
        image.put_pixel(13, y, outline);
    }
    for x in 14..16 {
        for y in 6..10 {
            image.put_pixel(x, y, outline);
        }
    }
    for x in 2..13 {
        for y in 3..13 {
            image.put_pixel(x, y, interior);
        }
    }

    // The fill gives a quick visual indication in addition to the number.
    let fill_width = (u32::from(percentage) * 9 / 100).max(if percentage > 0 { 1 } else { 0 });
    for x in 3..(3 + fill_width) {
        for y in 10..13 {
            image.put_pixel(x, y, Rgba([color[0], color[1], color[2], 255]));
        }
    }

    let text_color = if ((battery_color_map.red_max_percent + 1)
        ..=battery_color_map.yellow_max_percent)
        .contains(&percentage)
    {
        Rgba([20, 24, 27, 255])
    } else {
        Rgba([255, 255, 255, 255])
    };
    draw_number(&mut image, percentage, text_color);

    image
}

/// Renders the neutral unknown/error icon as an RGBA buffer (16×16 or 32×32).
pub fn unknown_battery_rgba(size: u32) -> Result<Vec<u8>, TrayError> {
    validate_icon_size(size)?;
    Ok(scale_icon(render_unknown_battery(), size).into_raw())
}

fn unknown_battery_icon_at_size(size: u32) -> Result<Icon, TrayError> {
    let rgba = unknown_battery_rgba(size)?;
    Icon::from_rgba(rgba, size, size)
        .map_err(|error| TrayError::new(format!("could not create unknown battery icon: {error}")))
}

fn render_unknown_battery() -> RgbaImage {
    let mut image = RgbaImage::from_pixel(ICON_SIZE, ICON_SIZE, Rgba([0, 0, 0, 0]));
    let outline = Rgba([225, 232, 238, 255]);
    let interior = Rgba([54, 61, 68, 255]);
    for x in 1..14 {
        image.put_pixel(x, 2, outline);
        image.put_pixel(x, 13, outline);
    }
    for y in 2..=13 {
        image.put_pixel(1, y, outline);
        image.put_pixel(13, y, outline);
    }
    for x in 14..16 {
        for y in 6..10 {
            image.put_pixel(x, y, outline);
        }
    }
    for x in 2..13 {
        for y in 3..13 {
            image.put_pixel(x, y, interior);
        }
    }

    let question_mark = [0b111, 0b001, 0b011, 0b000, 0b010];
    for (row, bits) in question_mark.iter().enumerate() {
        for column in 0..3 {
            if bits & (1 << (2 - column)) != 0 {
                image.put_pixel(6 + column, 5 + row as u32, Rgba([255, 255, 255, 255]));
            }
        }
    }
    image
}

fn validate_icon_size(size: u32) -> Result<(), TrayError> {
    if matches!(size, 16 | 32) {
        Ok(())
    } else {
        Err(TrayError::new("tray icon size must be 16 or 32 pixels"))
    }
}

fn scale_icon(image: RgbaImage, size: u32) -> RgbaImage {
    if size == ICON_SIZE {
        return image;
    }

    let mut scaled = RgbaImage::new(size, size);
    for y in 0..size {
        for x in 0..size {
            scaled.put_pixel(
                x,
                y,
                *image.get_pixel(x * ICON_SIZE / size, y * ICON_SIZE / size),
            );
        }
    }
    scaled
}

fn draw_number(image: &mut RgbaImage, number: u8, color: Rgba<u8>) {
    let digits = number.to_string();
    let width = digits.len() as u32 * 3 + digits.len() as u32 - 1;
    let start_x = 2 + (11 - width) / 2;
    let start_y = 5;
    for (index, digit) in digits.bytes().enumerate() {
        let rows = DIGIT_GLYPHS[usize::from(digit - b'0')];
        for (row, bits) in rows.iter().enumerate() {
            for column in 0..3 {
                if bits & (1 << (2 - column)) != 0 {
                    image.put_pixel(
                        start_x + index as u32 * 4 + column,
                        start_y + row as u32,
                        color,
                    );
                }
            }
        }
    }
}

// Five rows of three pixels, read left-to-right from the high bit.
const DIGIT_GLYPHS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111], // 0
    [0b010, 0b110, 0b010, 0b010, 0b111], // 1
    [0b111, 0b001, 0b111, 0b100, 0b111], // 2
    [0b111, 0b001, 0b111, 0b001, 0b111], // 3
    [0b101, 0b101, 0b111, 0b001, 0b001], // 4
    [0b111, 0b100, 0b111, 0b001, 0b111], // 5
    [0b111, 0b100, 0b111, 0b101, 0b111], // 6
    [0b111, 0b001, 0b010, 0b010, 0b010], // 7
    [0b111, 0b101, 0b111, 0b101, 0b111], // 8
    [0b111, 0b101, 0b111, 0b001, 0b111], // 9
];

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
    fn icon_renderer_accepts_boundary_percentages() {
        for percentage in [0, 15, 16, 30, 31, 100] {
            let icon =
                numeric_battery_icon_at_size(percentage, ICON_SIZE).expect("valid 16x16 icon");
            let _ = icon;
        }
    }

    #[test]
    fn rgba_renderers_support_both_dpi_sizes() {
        for size in [16, 32] {
            assert_eq!(
                numeric_battery_rgba(85, size).unwrap().len(),
                (size * size * 4) as usize
            );
            assert_eq!(
                unknown_battery_rgba(size).unwrap().len(),
                (size * size * 4) as usize
            );
        }
        assert!(numeric_battery_rgba(85, 24).is_err());
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
            let rgba = numeric_battery_rgba_with_color_map(percentage, ICON_SIZE, map).unwrap();
            let index = ((10 * ICON_SIZE + 3) * 4) as usize;
            [rgba[index], rgba[index + 1], rgba[index + 2]]
        };

        assert_eq!(pixel(10), [1, 2, 3]);
        assert_eq!(pixel(11), [4, 5, 6]);
        assert_eq!(pixel(40), [4, 5, 6]);
        assert_eq!(pixel(41), [7, 8, 9]);
    }
}
