//! Disposable fixtures for iterating on the native renderers.

use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    config::AppTheme,
    history::{unix_now, HistoryStore},
    state::DeviceState,
};

use super::{
    live::{LiveDevice, LiveLevel, LiveReading},
    management::DashboardBackend,
    settings_window, window,
};

const USAGE: &str = "Usage: volty ui-preview [history|manage|cleanup|settings|invalid-settings]
                       [--theme dark|light|system] [--size WIDTHxHEIGHT] [--empty]";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    History,
    Manage,
    Cleanup,
    Settings,
    InvalidSettings,
}

struct Options {
    view: View,
    theme: AppTheme,
    size: Option<[u32; 2]>,
    empty: bool,
}

impl Options {
    fn parse(args: &[String]) -> io::Result<Self> {
        let mut args = args.iter().map(String::as_str).peekable();
        let view = if args.peek().is_some_and(|arg| !arg.starts_with("--")) {
            match args.next().unwrap() {
                "history" => View::History,
                "manage" => View::Manage,
                "cleanup" => View::Cleanup,
                "settings" => View::Settings,
                "invalid-settings" => View::InvalidSettings,
                value => return Err(invalid(format!("Unknown preview view {value:?}"))),
            }
        } else {
            View::History
        };
        let mut options = Self {
            view,
            theme: AppTheme::Dark,
            size: None,
            empty: false,
        };
        while let Some(arg) = args.next() {
            match arg {
                "--empty" => options.empty = true,
                "--theme" => {
                    options.theme = args
                        .next()
                        .and_then(AppTheme::parse)
                        .ok_or_else(|| invalid("Theme must be dark, light, or system"))?
                }
                "--size" => options.size = Some(parse_size(args.next().unwrap_or_default())?),
                value => return Err(invalid(format!("Unknown preview option {value:?}"))),
            }
        }
        if options.empty && options.view == View::Cleanup {
            return Err(invalid("The cleanup preview requires sample history"));
        }
        Ok(options)
    }
}

fn parse_size(value: &str) -> io::Result<[u32; 2]> {
    let size = value.split_once('x').and_then(|(width, height)| {
        let width = width.parse().ok()?;
        let height = height.parse().ok()?;
        ((660..=3840).contains(&width) && (460..=2160).contains(&height)).then_some([width, height])
    });
    size.ok_or_else(|| invalid("Size must be WIDTHxHEIGHT within 660x460 to 3840x2160"))
}

fn invalid(message: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{message}\n{USAGE}"))
}

pub(crate) fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("{USAGE}");
        return Ok(());
    }
    let options = Options::parse(args)?;
    let profile = Profile::create()?;
    let live = seed(&profile.0, options.theme, options.empty)?;
    let backend = DashboardBackend::preview(profile.0.clone());
    match options.view {
        View::Settings | View::InvalidSettings => settings_window::open_settings_window(
            backend,
            settings_window::WindowOptions {
                size: options.size,
                invalid_preview: options.view == View::InvalidSettings,
            },
        ),
        _ => window::open_dashboard_window(
            backend,
            super::DEFAULT_DAYS,
            window::WindowOptions {
                size: options.size,
                preview: Some(window::PreviewOptions {
                    theme: options.theme,
                    live,
                    manage: matches!(options.view, View::Manage | View::Cleanup),
                    cleanup: options.view == View::Cleanup,
                }),
            },
        ),
    }
}

struct Profile(PathBuf);

impl Profile {
    fn create() -> io::Result<Self> {
        let root = fs::canonicalize(std::env::temp_dir())?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?;
        let directory = root.join(format!(
            "volty-ui-preview-{}-{}",
            std::process::id(),
            timestamp.as_nanos()
        ));
        fs::create_dir(&directory)?;
        Ok(Self(directory))
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        // This directory was created exclusively by this process under the resolved temp root.
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!(
                "Could not remove preview profile {}: {error}",
                self.0.display()
            );
        }
    }
}

fn seed(directory: &Path, theme: AppTheme, empty: bool) -> Result<Vec<LiveDevice>, Box<dyn Error>> {
    fs::write(
        directory.join("config.toml"),
        format!("app_theme = \"{}\"\n", theme.as_str()),
    )?;
    if empty {
        return Ok(Vec::new());
    }

    let now = unix_now()? / 300 * 300;
    let history_path = directory.join("battery-history.sqlite3");
    drop(HistoryStore::open(&history_path)?);
    let mut history = rusqlite::Connection::open(&history_path)?;
    let transaction = history.transaction()?;
    let mut state = DeviceState::open(&directory.join("device-state.sqlite3"))?;
    let devices = [
        ("bluetooth:fixture-headset", "Studio Headphones"),
        ("hid:fixture-mouse", "Wireless Precision Mouse"),
        ("xinput:fixture-controller", "Xbox Wireless Controller"),
    ];
    {
        let mut insert = transaction.prepare(
            "INSERT INTO samples (identity, name, recorded_at, percent, charging, voltage_mv)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        const STEPS: i64 = 16 * 24 * 6;
        for (index, (identity, name)) in devices.into_iter().enumerate() {
            for step in 0..=STEPS {
                let minutes_ago = (STEPS - step) * 10;
                if (index == 1 && minutes_ago < 54 * 60)
                    || (80 * 60..=92 * 60).contains(&minutes_ago)
                {
                    continue;
                }
                let cycle = (step + 19 + index as i64 * 26) % 288;
                let charging = cycle < 24;
                let percent = (index != 2 || step % 17 != 0).then_some(if charging {
                    36 + cycle * 2
                } else {
                    84 - (cycle - 24) * 48 / 264
                });
                insert.execute(rusqlite::params![
                    identity,
                    name,
                    now - minutes_ago * 60,
                    percent,
                    charging,
                    (index == 1).then_some(3780),
                ])?;
            }
        }
    }
    transaction.commit()?;
    state.set_pinned_device(devices[0].0)?;
    state.set_history_recording_enabled(devices[1].0, false)?;
    let reports = directory.join("reports");
    fs::create_dir(&reports)?;
    fs::write(
        reports.join("volty-diagnose-preview.txt"),
        "Volty UI preview diagnostic fixture.\n",
    )?;

    Ok(vec![
        LiveDevice {
            identity: devices[0].0.to_owned(),
            name: devices[0].1.to_owned(),
            reading: Some(LiveReading {
                level: LiveLevel::Exact(74),
                charging: true,
                remaining_seconds: None,
            }),
        },
        LiveDevice {
            identity: "hid:fixture-keyboard".to_owned(),
            name: "Compact Wireless Keyboard".to_owned(),
            reading: Some(LiveReading {
                level: LiveLevel::Unavailable,
                charging: false,
                remaining_seconds: None,
            }),
        },
        LiveDevice {
            identity: devices[2].0.to_owned(),
            name: devices[2].1.to_owned(),
            reading: Some(LiveReading {
                level: LiveLevel::Estimated(50),
                charging: false,
                remaining_seconds: None,
            }),
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DashboardSettingsUpdate;

    #[test]
    fn preview_requires_history_for_cleanup_and_checks_dimensions() {
        assert!(Options::parse(&["cleanup".into(), "--empty".into()]).is_err());
        assert_eq!(parse_size("700x520").unwrap(), [700, 520]);
        assert!(parse_size("200x520").is_err());
    }

    #[test]
    fn fixture_runs_real_cleanup_and_saves_to_the_disposable_profile() {
        use super::super::management::CleanupKind;

        let profile = Profile::create().unwrap();
        let live = seed(&profile.0, AppTheme::Light, false).unwrap();
        assert!(live[0].reading.unwrap().charging);
        let backend = DashboardBackend::preview(profile.0.clone());
        let cleanup = backend
            .preview_cleanup(CleanupKind::OlderThan, Some(7), None)
            .unwrap();
        assert!(cleanup.samples > 0);
        assert!(backend.execute_cleanup(cleanup).unwrap().deleted);
        let settings = backend.get_settings_data().unwrap().settings.unwrap();
        assert_eq!(settings.app_theme, "light");
        backend
            .update_settings(&DashboardSettingsUpdate {
                app_theme: "dark".to_owned(),
                battery_color_red_max_percent: settings.battery_color_red_max_percent,
                battery_color_yellow_max_percent: settings.battery_color_yellow_max_percent,
                battery_color_red: settings.battery_color_red,
                battery_color_yellow: settings.battery_color_yellow,
                battery_color_green: settings.battery_color_green,
                low_battery_thresholds: settings.low_battery_thresholds,
                low_battery_cooldown_minutes: settings.low_battery_cooldown_minutes,
                notifications_enabled: settings.notifications_enabled,
                tray_icon_style: settings.tray_icon_style,
                tray_icon_theme: settings.tray_icon_theme,
                show_percentage_in_battery_icon: settings.show_percentage_in_battery_icon,
                pinned_device: settings.pinned_device,
                start_on_boot: Some(true),
            })
            .unwrap();
        let settings = backend.get_settings_data().unwrap().settings.unwrap();
        assert_eq!(settings.start_on_boot, Some(true));
        assert_eq!(settings.app_theme, "dark");
        let artifact = backend.list_artifacts().unwrap().pop().unwrap();
        assert!(backend.delete_artifact(&artifact.id).unwrap().deleted);
    }
}
