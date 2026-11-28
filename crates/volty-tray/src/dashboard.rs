//! CLI orchestration for the local HTML battery-history dashboard.

#[cfg(windows)]
mod ipc;
#[cfg(windows)]
mod management;
mod model;
mod render;
#[cfg(windows)]
mod settings_window;
#[cfg(windows)]
mod window;

use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use crate::history::{unix_now, HistoryStore};
#[cfg(windows)]
use management::DashboardBackend;
use render::render_dashboard;
#[cfg(windows)]
use render::render_dashboard_data;

const DEFAULT_DAYS: u32 = 7;
const SECONDS_PER_DAY: i64 = 86_400;
#[cfg(windows)]
const EMPTY_MESSAGE: &str = "No battery history recorded yet.";

#[cfg(windows)]
pub(crate) fn settings_instance_mutex_name() -> &'static str {
    settings_window::instance_mutex_name()
}

#[cfg(target_os = "linux")]
const DASHBOARD_USAGE: &str =
    "Usage: volty dashboard [--days N] [--database PATH] [--output PATH] [--open]

Writes a standalone HTML battery-history dashboard. The default output is
$XDG_CACHE_HOME/volty/dashboard.html. Use --open to launch the default browser.";

#[cfg(windows)]
const DASHBOARD_USAGE: &str =
    "Usage: volty dashboard [--window | --settings] [--days N] [--database PATH] [--output PATH] [--no-open]

Generates a self-contained local battery-history dashboard. Nothing is uploaded.

Options:
  --window         Show the battery dashboard in a native WebView2 window
  --settings       Open native Windows settings
  --days N          Number of days to include (default 7)
  --database PATH   Read a specific SQLite database instead of the default
  --output PATH     Write the HTML file to PATH (default %TEMP%\\volty-dashboard.html)
  --no-open         Write the HTML file without opening it in the default browser";

#[derive(Clone, Debug, PartialEq, Eq)]
struct DashboardOptions {
    days: u32,
    database: Option<PathBuf>,
    output: Option<PathBuf>,
    no_open: bool,
    #[cfg(windows)]
    window: bool,
    #[cfg(windows)]
    settings: bool,
    help: bool,
}

impl Default for DashboardOptions {
    fn default() -> Self {
        Self {
            days: DEFAULT_DAYS,
            database: None,
            output: None,
            no_open: cfg!(target_os = "linux"),
            #[cfg(windows)]
            window: false,
            #[cfg(windows)]
            settings: false,
            help: false,
        }
    }
}

pub fn run_cli(args: &[String]) -> Result<(), Box<dyn Error>> {
    let options = parse_args(args)?;
    if options.help {
        println!("{DASHBOARD_USAGE}");
        return Ok(());
    }
    let database = match &options.database {
        Some(path) => path.clone(),
        None => HistoryStore::local_path()?,
    };
    #[cfg(windows)]
    let output = options
        .output
        .unwrap_or_else(|| std::env::temp_dir().join("volty-dashboard.html"));
    #[cfg(target_os = "linux")]
    let output = match options.output {
        Some(path) => path,
        None => crate::linux::cache_dir()?.join("dashboard.html"),
    };

    #[cfg(windows)]
    if options.settings {
        let backend = DashboardBackend::new(database, options.database.is_none());
        return settings_window::open_settings_window(backend, options.days);
    }

    #[cfg(windows)]
    if options.window {
        let backend = DashboardBackend::new(database, options.database.is_none());
        let mut data = backend.get_dashboard_data(options.days)?;
        data.settings = None;
        let html = render_dashboard_data(&data)?;
        if let Err(error) = window::open_dashboard_window(&html, backend) {
            eprintln!(
                "Could not open the native dashboard window ({error}); opening the HTML dashboard in the default browser."
            );
            fs::write(&output, html)?;
            println!("Wrote battery dashboard to {}", output.display());
            open_dashboard(&output)?;
        }
        return Ok(());
    }

    let generated_at = unix_now()?;
    let samples = if database.exists() {
        let store = HistoryStore::open(&database)?;
        let window_start =
            generated_at.saturating_sub(i64::from(options.days).saturating_mul(SECONDS_PER_DAY));
        store.samples_since(window_start)?
    } else {
        Vec::new()
    };
    #[cfg(windows)]
    if samples.is_empty() {
        println!("{EMPTY_MESSAGE}");
        return Ok(());
    }
    let html = render_dashboard(samples, options.days, generated_at)?;
    #[cfg(windows)]
    fs::write(&output, html)?;
    #[cfg(target_os = "linux")]
    crate::linux::write_private_file(&output, html.as_bytes())?;
    println!("Wrote battery dashboard to {}", output.display());

    if !options.no_open {
        open_dashboard(&output)?;
    }
    Ok(())
}

fn open_dashboard(path: &Path) -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "linux")]
    let status = Command::new("xdg-open")
        .arg(fs::canonicalize(path)?)
        .status()?;
    #[cfg(windows)]
    let status = Command::new("cmd")
        .arg("/c")
        .arg("start")
        .arg("")
        .arg(path)
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "could not open dashboard in the default browser ({status})"
        ))
        .into());
    }
    Ok(())
}

fn parse_args(args: &[String]) -> Result<DashboardOptions, Box<dyn Error>> {
    let mut options = DashboardOptions::default();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        match argument.as_str() {
            "-h" | "--help" | "help" => options.help = true,
            "--no-open" => options.no_open = true,
            #[cfg(target_os = "linux")]
            "--open" => options.no_open = false,
            #[cfg(windows)]
            "--window" => options.window = true,
            #[cfg(windows)]
            "--settings" => options.settings = true,
            "--days" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| usage_error("--days requires a value"))?;
                options.days = parse_days(value)?;
            }
            value if value.starts_with("--days=") => {
                options.days = parse_days(&value["--days=".len()..])?;
            }
            "--database" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| usage_error("--database requires a path"))?;
                options.database = Some(parse_path("--database", value)?);
            }
            value if value.starts_with("--database=") => {
                options.database = Some(parse_path("--database", &value["--database=".len()..])?);
            }
            "--output" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| usage_error("--output requires a path"))?;
                options.output = Some(parse_path("--output", value)?);
            }
            value if value.starts_with("--output=") => {
                options.output = Some(parse_path("--output", &value["--output=".len()..])?);
            }
            other => return Err(usage_error(format!("unexpected argument {other:?}"))),
        }
        index += 1;
    }
    Ok(options)
}

fn parse_days(value: &str) -> Result<u32, Box<dyn Error>> {
    let days = value
        .parse::<u32>()
        .map_err(|_| usage_error(format!("invalid --days value {value:?}")))?;
    if days == 0 {
        return Err(usage_error("--days must be at least 1"));
    }
    Ok(days)
}

fn parse_path(option: &str, value: &str) -> Result<PathBuf, Box<dyn Error>> {
    if value.trim().is_empty() {
        return Err(usage_error(format!("{option} requires a path")));
    }
    Ok(PathBuf::from(value))
}

fn usage_error(message: impl std::fmt::Display) -> Box<dyn Error> {
    io::Error::other(format!("{message}\n{DASHBOARD_USAGE}")).into()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn window_option_keeps_days_argument() {
        let options =
            parse_args(&["--window".to_owned(), "--days".to_owned(), "14".to_owned()]).unwrap();

        assert!(options.window);
        assert_eq!(options.days, 14);
    }

    #[test]
    fn settings_option_selects_the_native_settings_window() {
        let options = parse_args(&["--settings".to_owned()]).unwrap();

        assert!(options.settings);
        assert!(!options.window);
    }
}
