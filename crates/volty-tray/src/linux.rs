use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs,
    io::{self, Write},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use volty_battery::{
    BatteryPercentage, BatteryProvider, DeviceId, ProviderEvent, ScanMode, SourceReading,
};

const USAGE: &str = "Usage: volty [tray | status | history | dashboard | capture | autostart]

  tray                      Monitor batteries in the desktop session (default)
  status                    Print current peripheral readings and their sources
  history                   Show recorded samples and summaries
  dashboard                 Export a local HTML dashboard (--open launches it)
  capture                   Capture, inspect or replay battery diagnostics
  autostart enable|disable  Start the installed tray when you log in";

pub(crate) fn run(command: Option<&str>, args: &[String]) -> Result<(), Box<dyn Error>> {
    match command {
        None | Some("tray") if args.is_empty() => crate::linux_desktop::run(),
        Some("status") if args.is_empty() => {
            let (readings, warnings) = Monitor::new().poll()?;
            for warning in warnings {
                eprintln!("volty: {warning}");
            }
            if readings.is_empty() {
                println!("No peripheral battery readings available.");
            }
            for reading in readings {
                let battery = reading.battery;
                let level = match battery.percentage {
                    BatteryPercentage::Exact(value) => format!("{value}%"),
                    BatteryPercentage::Estimated { value, .. } => format!("~{value}%"),
                    BatteryPercentage::Unavailable => format!("{:?}", battery.level),
                };
                println!(
                    "{}\t{level}\t{:?}\t{}",
                    reading.label.display(),
                    battery.state,
                    battery.source
                );
            }
            Ok(())
        }
        Some("dashboard") => crate::dashboard::run_cli(args),
        Some("autostart") => autostart(args),
        Some("help" | "-h" | "--help") if args.is_empty() => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(format!("invalid command or arguments\n{USAGE}").into()),
    }
}

pub(crate) struct Monitor {
    providers: Vec<Box<dyn BatteryProvider>>,
    next_discovery: Instant,
}

type PollResult = Result<(Vec<SourceReading>, Vec<String>), Box<dyn Error>>;

impl Monitor {
    pub(crate) fn new() -> Self {
        Self {
            providers: vec![crate::linux_hid::provider(), crate::linux_power::provider()],
            next_discovery: Instant::now(),
        }
    }

    pub(crate) fn poll(&mut self) -> PollResult {
        let mut readings = BTreeMap::<DeviceId, SourceReading>::new();
        let mut warnings = Vec::new();
        let mode = if Instant::now() >= self.next_discovery {
            self.next_discovery = Instant::now() + Duration::from_secs(60);
            ScanMode::Discover
        } else {
            ScanMode::Refresh
        };
        for provider in &mut self.providers {
            let mut names = BTreeMap::new();
            let outcome = provider
                .poll(mode, &mut |event| {
                    match event {
                        ProviderEvent::Reading(reading) => {
                            let entry = readings.entry(reading.id.clone());
                            entry
                                .and_modify(|previous| {
                                    if previous.battery.percentage.exact().is_none()
                                        && reading.battery.percentage.exact().is_some()
                                    {
                                        *previous = reading.clone();
                                    }
                                })
                                .or_insert(reading);
                        }
                        ProviderEvent::DeviceName { id, label, error } => {
                            names.insert(id, label);
                            warnings.extend(error);
                        }
                        ProviderEvent::Warning(message) => warnings.push(message),
                        ProviderEvent::Trace(_) => {}
                    }
                    Ok(())
                })
                .map_err(|_| {
                    io::Error::other("battery provider stopped before finishing a scan")
                })?;
            warnings.extend(outcome.errors);
            for (id, name) in names {
                if let Some(reading) = readings.get_mut(&id) {
                    reading.label = name;
                }
            }
        }
        warnings.sort_unstable();
        warnings.dedup();
        Ok((readings.into_values().collect(), warnings))
    }
}

fn xdg_dir(variable: &str, fallback: &str) -> io::Result<PathBuf> {
    let absolute = |key| {
        env::var_os(key)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    absolute(variable)
        .or_else(|| absolute("HOME").map(|path| path.join(fallback)))
        .ok_or_else(|| io::Error::other(format!("set an absolute {variable} or HOME directory")))
}

pub(crate) fn state_dir() -> io::Result<PathBuf> {
    Ok(xdg_dir("XDG_STATE_HOME", ".local/state")?.join("volty"))
}

pub(crate) fn cache_dir() -> io::Result<PathBuf> {
    Ok(xdg_dir("XDG_CACHE_HOME", ".cache")?.join("volty"))
}

pub(crate) fn write_private_file(path: &Path, data: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(data)?;
    file.persist(path)?;
    Ok(())
}

fn autostart(args: &[String]) -> Result<(), Box<dyn Error>> {
    let [action] = args else {
        return Err("Usage: volty autostart enable|disable".into());
    };
    let path = xdg_dir("XDG_CONFIG_HOME", ".config")?.join("autostart/volty.desktop");
    match action.as_str() {
        "enable" => write_private_file(
            &path,
            include_bytes!("../../../packaging/linux/volty.desktop"),
        )?,
        "disable" => match fs::remove_file(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            result => result?,
        },
        _ => return Err("Usage: volty autostart enable|disable".into()),
    }
    Ok(())
}
