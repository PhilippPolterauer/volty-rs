use std::{
    error::Error,
    sync::mpsc::{self, RecvTimeoutError, TryRecvError},
    thread,
    time::Duration,
};

use volty_hid::{BatteryLevel, BatteryState, BatteryStatus};

use crate::{
    notify::{BatterySample, NotificationEngine},
    tray::{TrayAction, TrayController},
};

const POLL_INTERVAL: Duration = Duration::from_secs(120);
const RECOVERY_INTERVAL: Duration = Duration::from_secs(15);
const UI_POLL_INTERVAL: Duration = Duration::from_millis(100);

enum PollCommand {
    Refresh,
    Quit,
}

struct DeviceReading {
    name: String,
    battery: BatteryStatus,
}

type PollResult = Result<DeviceReading, String>;

pub fn run() -> Result<(), Box<dyn Error>> {
    let mut tray = TrayController::new()?;
    tray.update_status("", "Searching for Logitech device", None)?;

    let (mut notifications, notification_config_error) =
        match NotificationEngine::from_local_config() {
            Ok(engine) => (engine, None),
            Err(error) => {
                let message = error.to_string();
                eprintln!("Volty notification configuration error: {error}");
                tray.update_status("", &format!("Notification config error: {message}"), None)?;
                (NotificationEngine::new(), Some(message))
            }
        };

    let (commands_tx, commands_rx) = mpsc::channel();
    let (results_tx, results_rx) = mpsc::channel();
    thread::Builder::new()
        .name("volty-device-poller".to_owned())
        .spawn(move || poll_worker(commands_rx, results_tx))?;

    loop {
        if let Err(error) = tray.process_menu_events() {
            report_error(&mut tray, "Tray menu error", &error.to_string());
        }

        loop {
            match tray.try_recv_action() {
                Ok(TrayAction::RefreshNow) => {
                    if commands_tx.send(PollCommand::Refresh).is_err() {
                        report_error(&mut tray, "Refresh error", "device poller has stopped");
                    }
                }
                Ok(TrayAction::Quit) => {
                    let _ = commands_tx.send(PollCommand::Quit);
                    return Ok(());
                }
                Ok(TrayAction::StartOnBootChanged(_)) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    report_error(&mut tray, "Tray error", "tray action channel disconnected");
                    break;
                }
            }
        }

        // Match explicitly so an unexpected worker disconnect is not confused
        // with an empty result queue.
        #[allow(clippy::while_let_loop)]
        loop {
            match results_rx.try_recv() {
                Ok(result) => apply_poll_result(
                    &mut tray,
                    &mut notifications,
                    result,
                    notification_config_error.as_deref(),
                ),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err("device polling worker stopped unexpectedly".into());
                }
            }
        }

        thread::sleep(UI_POLL_INTERVAL);
    }
}

fn poll_worker(commands: mpsc::Receiver<PollCommand>, results: mpsc::Sender<PollResult>) {
    loop {
        let reading = query_devices();
        let next_poll = if reading.is_ok() {
            POLL_INTERVAL
        } else {
            RECOVERY_INTERVAL
        };
        if results.send(reading).is_err() {
            return;
        }

        match commands.recv_timeout(next_poll) {
            Ok(PollCommand::Refresh) => {}
            Ok(PollCommand::Quit) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn query_devices() -> PollResult {
    use volty_hid::{enumerate_logitech_devices, query_battery};

    let devices = enumerate_logitech_devices().map_err(|error| error.to_string())?;
    if devices.is_empty() {
        return Err("No Logitech HID device found".to_owned());
    }

    let mut errors = Vec::new();
    for device in devices {
        let name = device
            .product_string
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Logitech device {:04x}", device.product_id));

        // The HID++ API accepts a paired-device index but doesn't enumerate
        // receiver pairings. Probe the common device slots so receivers work too.
        for device_index in [0xff, 1, 2, 3, 4, 5, 6] {
            match query_battery(&device, device_index) {
                Ok(battery) => return Ok(DeviceReading { name, battery }),
                Err(error) => errors.push(format!("{name} (slot {device_index}): {error}")),
            }
        }
    }

    Err(format!(
        "Could not read a Logitech battery: {}",
        errors.join("; ")
    ))
}

fn apply_poll_result(
    tray: &mut TrayController,
    notifications: &mut NotificationEngine,
    result: PollResult,
    notification_config_error: Option<&str>,
) {
    let reading = match result {
        Ok(reading) => reading,
        Err(error) => {
            eprintln!("Volty device polling error: {error}");
            let mut status = format!("Error: {error}");
            append_config_error(&mut status, notification_config_error);
            if let Err(tray_error) = tray.update_status("", &status, None) {
                eprintln!("Volty tray update error: {tray_error}");
            }
            return;
        }
    };

    let percentage = reading.battery.level_percent;
    let mut status = battery_status_label(&reading.battery);
    let charging = matches!(
        reading.battery.state,
        BatteryState::Charging | BatteryState::ChargingNearlyFull | BatteryState::ChargingSlow
    );
    let full = matches!(reading.battery.state, BatteryState::Charged)
        || reading.battery.level == BatteryLevel::Full
        || percentage.is_some_and(|percentage| percentage >= 100);
    if percentage.is_some() || charging || full {
        let sample = BatterySample {
            // Percentage is irrelevant while charging/full because the engine
            // resets its discharge-cycle guard before checking the threshold.
            percentage: percentage.unwrap_or(0),
            charging,
            full,
        };
        if let Err(error) = notifications.update(sample) {
            let error = error.to_string();
            eprintln!("Volty notification error: {error}");
            status.push_str("; notification error: ");
            status.push_str(&error);
        }
    }
    if percentage.is_none() {
        status.push_str("; percentage unavailable");
    }
    append_config_error(&mut status, notification_config_error);

    if let Err(error) = tray.update_status(&reading.name, &status, percentage) {
        eprintln!("Volty tray update error: {error}");
    }
}

fn append_config_error(status: &mut String, error: Option<&str>) {
    if let Some(error) = error {
        status.push_str("; notification config error: ");
        status.push_str(error);
    }
}

fn battery_status_label(battery: &BatteryStatus) -> String {
    let state = match battery.state {
        BatteryState::Unknown => "Unknown charge state",
        BatteryState::Discharging => "Discharging",
        BatteryState::Charging => "Charging",
        BatteryState::ChargingNearlyFull => "Charging — nearly full",
        BatteryState::Charged => "Fully charged",
        BatteryState::ChargingSlow => "Charging slowly",
        BatteryState::InvalidBattery => "Battery error: invalid battery",
        BatteryState::ThermalError => "Battery error: thermal",
        BatteryState::ChargingError => "Battery error: charging",
    };
    let level = match battery.level {
        BatteryLevel::Unknown => None,
        BatteryLevel::Critical => Some("critical level"),
        BatteryLevel::Low => Some("low level"),
        BatteryLevel::Good => Some("good level"),
        BatteryLevel::Full => Some("full"),
    };
    match level {
        Some(level) => format!("{state} ({level})"),
        None => state.to_owned(),
    }
}

fn report_error(tray: &mut TrayController, title: &str, message: &str) {
    eprintln!("Volty {title}: {message}");
    if let Err(error) = tray.update_status("", &format!("{title}: {message}"), None) {
        eprintln!("Volty tray update error: {error}");
    }
}
