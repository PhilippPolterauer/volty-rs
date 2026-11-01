//! Local battery history and the `volty history` CLI.

use std::{
    collections::BTreeMap,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::capture::unix_to_rfc3339;

const DEFAULT_DAYS: u32 = 7;
const RETENTION_SECONDS: i64 = 30 * 86_400;
const SAMPLE_INTERVAL_SECONDS: i64 = 5 * 60;
const EMPTY_MESSAGE: &str = "No battery history recorded yet.";

const HISTORY_USAGE: &str =
    "Usage: volty history [--days N] [--device SUBSTR] [--csv] [--database PATH]

Shows locally recorded battery samples. The default view covers the last 7 days.
Samples older than 30 days are pruned automatically. Nothing is uploaded.

Options:
  --days N          Number of days to include (default 7)
  --device SUBSTR   Only devices whose identity or name contains SUBSTR
  --csv             Print timestamp_utc,identity,name,percent,charging,voltage_mv rows
  --database PATH   Read a specific SQLite database instead of the default";

/// One stored battery reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistorySample {
    pub identity: String,
    pub name: String,
    pub recorded_at: i64,
    pub percent: Option<u8>,
    pub charging: bool,
    pub voltage_mv: Option<u16>,
}

/// Per-device aggregates over a history window.
#[derive(Clone, Debug, PartialEq)]
pub struct HistorySummary {
    pub identity: String,
    pub name: String,
    pub samples: u64,
    pub first_at: i64,
    pub last_at: i64,
    pub latest_percent: Option<u8>,
    pub min_percent: Option<u8>,
    pub max_percent: Option<u8>,
    /// Percentage points lost per day, when the observed percentage decreased
    /// across a span of at least one hour.
    pub percent_per_day: Option<f64>,
}

pub struct HistoryStore {
    connection: Connection,
}

impl HistoryStore {
    pub fn open_local() -> Result<Self, Box<dyn Error>> {
        let path = Self::local_path()?;
        Self::open(&path).map_err(|error| {
            io::Error::other(format!(
                "could not open battery history database {}: {error}",
                path.display()
            ))
            .into()
        })
    }

    pub(crate) fn open(path: &Path) -> Result<Self, Box<dyn Error>> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS samples (
                identity TEXT NOT NULL,
                name TEXT NOT NULL,
                recorded_at INTEGER NOT NULL,
                percent INTEGER,
                charging INTEGER NOT NULL,
                voltage_mv INTEGER
            );
            CREATE INDEX IF NOT EXISTS samples_identity_recorded_at
                ON samples (identity, recorded_at);
            CREATE INDEX IF NOT EXISTS samples_recorded_at
                ON samples (recorded_at);",
        )?;
        let store = Self { connection };
        store.prune(retention_cutoff()?)?;
        Ok(store)
    }

    pub(crate) fn local_path() -> Result<PathBuf, io::Error> {
        Ok(crate::state::local_state_path()?.with_file_name("battery-history.sqlite3"))
    }

    /// Stores a reading unless the latest sample for the identity has the same
    /// values and is less than five minutes old.
    pub fn record(&mut self, sample: &HistorySample) -> Result<bool, Box<dyn Error>> {
        if sample.identity.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "battery history identity must not be empty",
            )
            .into());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let latest = transaction
            .query_row(
                "SELECT recorded_at, percent, charging, voltage_mv
                 FROM samples
                 WHERE identity = ?1
                 ORDER BY recorded_at DESC, rowid DESC
                 LIMIT 1",
                [sample.identity.as_str()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<u8>>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, Option<u16>>(3)?,
                    ))
                },
            )
            .optional()?;
        let should_insert = match latest {
            Some((recorded_at, percent, charging, voltage_mv)) => {
                let changed = percent != sample.percent
                    || charging != sample.charging
                    || voltage_mv != sample.voltage_mv;
                changed || sample.recorded_at.saturating_sub(recorded_at) >= SAMPLE_INTERVAL_SECONDS
            }
            None => true,
        };
        if !should_insert {
            transaction.commit()?;
            return Ok(false);
        }
        transaction.execute(
            "INSERT INTO samples (identity, name, recorded_at, percent, charging, voltage_mv)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &sample.identity,
                &sample.name,
                sample.recorded_at,
                sample.percent,
                sample.charging,
                sample.voltage_mv
            ],
        )?;
        transaction.execute(
            "DELETE FROM samples WHERE recorded_at < ?1",
            [retention_cutoff()?],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    /// Samples in the requested window, ordered by timestamp then identity.
    pub fn samples(&self, days: u32) -> Result<Vec<HistorySample>, Box<dyn Error>> {
        self.samples_since(window_start(days)?)
    }

    /// Samples since a Unix timestamp, ordered by timestamp then identity.
    pub(crate) fn samples_since(&self, since: i64) -> Result<Vec<HistorySample>, Box<dyn Error>> {
        let mut statement = self.connection.prepare(
            "SELECT identity, name, recorded_at, percent, charging, voltage_mv
             FROM samples
             WHERE recorded_at >= ?1
             ORDER BY recorded_at, identity, rowid",
        )?;
        let rows = statement.query_map([since], |row| {
            Ok(HistorySample {
                identity: row.get(0)?,
                name: row.get(1)?,
                recorded_at: row.get(2)?,
                percent: row.get(3)?,
                charging: row.get(4)?,
                voltage_mv: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Per-device aggregates for the requested window.
    pub fn summary(&self, days: u32) -> Result<Vec<HistorySummary>, Box<dyn Error>> {
        let samples = self.samples(days)?;
        Ok(Self::summaries_from_samples(&samples))
    }

    /// Per-device aggregates for an already queried set of samples.
    pub(crate) fn summaries_from_samples(samples: &[HistorySample]) -> Vec<HistorySummary> {
        let mut summaries: BTreeMap<String, SummaryAccumulator> = BTreeMap::new();
        for sample in samples {
            let entry =
                summaries
                    .entry(sample.identity.clone())
                    .or_insert_with(|| SummaryAccumulator {
                        name: sample.name.clone(),
                        samples: 0,
                        first_at: sample.recorded_at,
                        last_at: sample.recorded_at,
                        latest_percent: sample.percent,
                        min_percent: sample.percent,
                        max_percent: sample.percent,
                        first_percent: sample.percent.map(|percent| (sample.recorded_at, percent)),
                        last_percent: sample.percent.map(|percent| (sample.recorded_at, percent)),
                    });
            entry.samples += 1;
            entry.last_at = sample.recorded_at;
            entry.latest_percent = sample.percent;
            if !sample.name.is_empty() {
                entry.name.clone_from(&sample.name);
            }
            if let Some(percent) = sample.percent {
                entry.min_percent = Some(
                    entry
                        .min_percent
                        .map_or(percent, |current| current.min(percent)),
                );
                entry.max_percent = Some(
                    entry
                        .max_percent
                        .map_or(percent, |current| current.max(percent)),
                );
                if entry.first_percent.is_none() {
                    entry.first_percent = Some((sample.recorded_at, percent));
                }
                entry.last_percent = Some((sample.recorded_at, percent));
            }
        }
        summaries
            .into_iter()
            .map(|(identity, accumulator)| accumulator.finish(identity))
            .collect()
    }

    fn prune(&self, before: i64) -> Result<(), Box<dyn Error>> {
        self.connection
            .execute("DELETE FROM samples WHERE recorded_at < ?1", [before])?;
        Ok(())
    }
}

struct SummaryAccumulator {
    name: String,
    samples: u64,
    first_at: i64,
    last_at: i64,
    latest_percent: Option<u8>,
    min_percent: Option<u8>,
    max_percent: Option<u8>,
    first_percent: Option<(i64, u8)>,
    last_percent: Option<(i64, u8)>,
}

impl SummaryAccumulator {
    fn finish(self, identity: String) -> HistorySummary {
        let percent_per_day = match (self.first_percent, self.last_percent) {
            (Some((first_at, first_percent)), Some((last_at, last_percent)))
                if last_at.saturating_sub(first_at) >= 3_600 && last_percent < first_percent =>
            {
                let days = (last_at - first_at) as f64 / 86_400.0;
                Some((f64::from(first_percent) - f64::from(last_percent)) / days)
            }
            _ => None,
        };
        HistorySummary {
            identity,
            name: self.name,
            samples: self.samples,
            first_at: self.first_at,
            last_at: self.last_at,
            latest_percent: self.latest_percent,
            min_percent: self.min_percent,
            max_percent: self.max_percent,
            percent_per_day,
        }
    }
}

fn retention_cutoff() -> Result<i64, Box<dyn Error>> {
    Ok(unix_now()?.saturating_sub(RETENTION_SECONDS))
}

fn window_start(days: u32) -> Result<i64, Box<dyn Error>> {
    Ok(unix_now()?.saturating_sub(i64::from(days).saturating_mul(86_400)))
}

pub(crate) fn unix_now() -> Result<i64, Box<dyn Error>> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            io::Error::other(format!(
                "the system clock is before the Unix epoch: {error}"
            ))
        })?;
    let seconds = i64::try_from(duration.as_secs())
        .map_err(|_| io::Error::other("the system clock is outside the supported range"))?;
    Ok(seconds)
}

fn format_timestamp(seconds: i64) -> String {
    u64::try_from(seconds).map_or_else(|_| seconds.to_string(), unix_to_rfc3339)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HistoryOptions {
    days: u32,
    device: Option<String>,
    csv: bool,
    database: Option<PathBuf>,
    help: bool,
}

impl Default for HistoryOptions {
    fn default() -> Self {
        Self {
            days: DEFAULT_DAYS,
            device: None,
            csv: false,
            database: None,
            help: false,
        }
    }
}

pub fn run_cli(args: &[String]) -> Result<(), Box<dyn Error>> {
    let options = parse_args(args)?;
    let mut stdout = io::stdout();
    run_with_output(&options, &mut stdout)
}

fn run_with_output(
    options: &HistoryOptions,
    output: &mut impl io::Write,
) -> Result<(), Box<dyn Error>> {
    if options.help {
        writeln!(output, "{HISTORY_USAGE}")?;
        return Ok(());
    }

    let database = match &options.database {
        Some(path) => path.clone(),
        None => HistoryStore::local_path()?,
    };
    if !database.exists() {
        writeln!(output, "{EMPTY_MESSAGE}")?;
        return Ok(());
    }
    let store = match &options.database {
        Some(path) => HistoryStore::open(path)?,
        None => HistoryStore::open_local()?,
    };
    let filter = options.device.as_ref().map(|device| device.to_lowercase());

    if options.csv {
        let rows = store
            .samples(options.days)?
            .into_iter()
            .filter(|row| device_matches(&row.identity, &row.name, filter.as_deref()))
            .collect::<Vec<_>>();
        if rows.is_empty() {
            writeln!(output, "{EMPTY_MESSAGE}")?;
            return Ok(());
        }
        output.write_all(render_csv(rows).as_bytes())?;
    } else {
        let summaries = store
            .summary(options.days)?
            .into_iter()
            .filter(|summary| device_matches(&summary.identity, &summary.name, filter.as_deref()))
            .collect::<Vec<_>>();
        if summaries.is_empty() {
            writeln!(output, "{EMPTY_MESSAGE}")?;
            return Ok(());
        }
        output.write_all(render_table(&summaries).as_bytes())?;
    }
    Ok(())
}

fn device_matches(identity: &str, name: &str, filter: Option<&str>) -> bool {
    match filter {
        Some(filter) => {
            identity.to_lowercase().contains(filter) || name.to_lowercase().contains(filter)
        }
        None => true,
    }
}

fn parse_args(args: &[String]) -> Result<HistoryOptions, Box<dyn Error>> {
    let mut options = HistoryOptions::default();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        match argument.as_str() {
            "-h" | "--help" | "help" => options.help = true,
            "--csv" => options.csv = true,
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
            "--device" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| usage_error("--device requires a substring"))?;
                options.device = Some(parse_device(value)?);
            }
            value if value.starts_with("--device=") => {
                options.device = Some(parse_device(&value["--device=".len()..])?);
            }
            "--database" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| usage_error("--database requires a path"))?;
                options.database = Some(parse_database(value)?);
            }
            value if value.starts_with("--database=") => {
                options.database = Some(parse_database(&value["--database=".len()..])?);
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

fn parse_device(value: &str) -> Result<String, Box<dyn Error>> {
    let value = value.trim();
    if value.is_empty() {
        return Err(usage_error("--device requires a non-empty substring"));
    }
    Ok(value.to_owned())
}

fn parse_database(value: &str) -> Result<PathBuf, Box<dyn Error>> {
    if value.trim().is_empty() {
        return Err(usage_error("--database requires a path"));
    }
    Ok(PathBuf::from(value))
}

fn usage_error(message: impl std::fmt::Display) -> Box<dyn Error> {
    io::Error::other(format!("{message}\n{HISTORY_USAGE}")).into()
}

fn render_table(summaries: &[HistorySummary]) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "{:<32} {:>7}  {:<20}  {:<20}  {:>8}  {:>5}  {:>5}  {:>10}\n",
        "Device", "Samples", "First", "Last", "Latest %", "Min %", "Max %", "Est. %/day"
    ));
    for summary in summaries {
        let name = if summary.name.trim().is_empty() {
            &summary.identity
        } else {
            &summary.name
        };
        output.push_str(&format!(
            "{:<32} {:>7}  {:<20}  {:<20}  {:>8}  {:>5}  {:>5}  {:>10}\n",
            name,
            summary.samples,
            format_timestamp(summary.first_at),
            format_timestamp(summary.last_at),
            format_percent(summary.latest_percent),
            format_percent(summary.min_percent),
            format_percent(summary.max_percent),
            summary
                .percent_per_day
                .map_or_else(|| "-".to_owned(), |rate| format!("{rate:.1}")),
        ));
    }
    output
}

fn format_percent(percent: Option<u8>) -> String {
    percent.map_or_else(|| "-".to_owned(), |percent| percent.to_string())
}

fn render_csv(mut rows: Vec<HistorySample>) -> String {
    rows.sort_by(|left, right| {
        left.recorded_at
            .cmp(&right.recorded_at)
            .then_with(|| left.identity.cmp(&right.identity))
    });
    let mut output = String::from("timestamp_utc,identity,name,percent,charging,voltage_mv\n");
    for row in rows {
        output.push_str(&format!(
            "{},{},{},{},{},{}\n",
            format_timestamp(row.recorded_at),
            csv_field(&row.identity),
            csv_field(&row.name),
            row.percent
                .map_or_else(String::new, |percent| percent.to_string()),
            if row.charging { "true" } else { "false" },
            row.voltage_mv
                .map_or_else(String::new, |voltage| voltage.to_string()),
        ));
    }
    output
}

fn csv_field(value: &str) -> String {
    if value
        .chars()
        .any(|character| matches!(character, ',' | '"' | '\n' | '\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDatabase {
        path: PathBuf,
    }

    impl TempDatabase {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "volty-history-{label}-{}-{}.sqlite3",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDatabase {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let mut path = self.path.as_os_str().to_owned();
                path.push(suffix);
                let _ = fs::remove_file(PathBuf::from(path));
            }
        }
    }

    fn sample(
        identity: &str,
        name: &str,
        recorded_at: i64,
        percent: Option<u8>,
        charging: bool,
        voltage_mv: Option<u16>,
    ) -> HistorySample {
        HistorySample {
            identity: identity.to_owned(),
            name: name.to_owned(),
            recorded_at,
            percent,
            charging,
            voltage_mv,
        }
    }

    fn summary_for<'a>(summaries: &'a [HistorySummary], identity: &str) -> &'a HistorySummary {
        summaries
            .iter()
            .find(|summary| summary.identity == identity)
            .unwrap_or_else(|| panic!("missing summary for {identity}"))
    }

    #[test]
    fn schema_is_created_with_wal_and_parent_directories() {
        let directory = std::env::temp_dir().join(format!(
            "volty-history-schema-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("nested").join("battery-history.sqlite3");

        {
            let store = HistoryStore::open(&path).unwrap();
            let journal_mode: String = store
                .connection
                .pragma_query_value(None, "journal_mode", |row| row.get(0))
                .unwrap();
            assert_eq!(journal_mode, "wal");
            let columns = store
                .connection
                .prepare("PRAGMA table_info(samples)")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(
                columns,
                [
                    "identity",
                    "name",
                    "recorded_at",
                    "percent",
                    "charging",
                    "voltage_mv"
                ]
            );
            let indexes: i64 = store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'index' AND name = 'samples_identity_recorded_at'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(indexes, 1);
        }

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unchanged_readings_are_deduplicated_within_five_minutes() {
        let database = TempDatabase::new("dedupe");
        let mut store = HistoryStore::open(database.path()).unwrap();
        let now = unix_now().unwrap();

        assert!(store
            .record(&sample("hid:a", "Mouse", now, Some(80), false, Some(4_000)))
            .unwrap());
        assert!(!store
            .record(&sample(
                "hid:a",
                "Mouse",
                now + SAMPLE_INTERVAL_SECONDS - 1,
                Some(80),
                false,
                Some(4_000)
            ))
            .unwrap());
        assert!(store
            .record(&sample(
                "hid:a",
                "Mouse",
                now + SAMPLE_INTERVAL_SECONDS,
                Some(80),
                false,
                Some(4_000)
            ))
            .unwrap());
        assert!(!store
            .record(&sample(
                "hid:a",
                "Mouse",
                now + SAMPLE_INTERVAL_SECONDS * 2 - 1,
                Some(80),
                false,
                Some(4_000)
            ))
            .unwrap());

        let rows = store.samples(DEFAULT_DAYS).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].recorded_at, now);
        assert_eq!(rows[1].recorded_at, now + SAMPLE_INTERVAL_SECONDS);
    }

    #[test]
    fn changed_readings_are_inserted_immediately() {
        let database = TempDatabase::new("changed");
        let mut store = HistoryStore::open(database.path()).unwrap();
        let now = unix_now().unwrap();

        assert!(store
            .record(&sample("hid:a", "Mouse", now, Some(80), false, Some(4_000)))
            .unwrap());
        assert!(store
            .record(&sample(
                "hid:a",
                "Mouse",
                now + 1,
                Some(80),
                true,
                Some(4_000)
            ))
            .unwrap());
        assert!(store
            .record(&sample(
                "hid:a",
                "Mouse",
                now + 2,
                Some(79),
                true,
                Some(4_000)
            ))
            .unwrap());
        assert!(store
            .record(&sample(
                "hid:a",
                "Mouse",
                now + 3,
                Some(79),
                true,
                Some(4_001)
            ))
            .unwrap());
        assert!(!store
            .record(&sample(
                "hid:a",
                "Mouse",
                now + 4,
                Some(79),
                true,
                Some(4_001)
            ))
            .unwrap());

        assert_eq!(store.samples(DEFAULT_DAYS).unwrap().len(), 4);
    }

    #[test]
    fn samples_older_than_thirty_days_are_pruned_on_open_and_after_inserts() {
        let database = TempDatabase::new("retention");
        let now = unix_now().unwrap();

        {
            let store = HistoryStore::open(database.path()).unwrap();
            store
                .connection
                .execute(
                    "INSERT INTO samples
                        (identity, name, recorded_at, percent, charging, voltage_mv)
                     VALUES ('old', 'Old', ?1, 50, 0, NULL),
                            ('recent', 'Recent', ?2, 60, 0, NULL)",
                    params![now - RETENTION_SECONDS - 60, now],
                )
                .unwrap();
        }

        let mut store = HistoryStore::open(database.path()).unwrap();
        let old: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM samples WHERE identity = 'old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let recent: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM samples WHERE identity = 'recent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old, 0);
        assert_eq!(recent, 1);

        assert!(store
            .record(&sample(
                "ancient",
                "Ancient",
                now - RETENTION_SECONDS - 60,
                Some(40),
                false,
                None
            ))
            .unwrap());
        let ancient: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM samples WHERE identity = 'ancient'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ancient, 0);
    }

    #[test]
    fn summary_reports_latest_extremes_and_discharge_rate() {
        let database = TempDatabase::new("summary");
        let mut store = HistoryStore::open(database.path()).unwrap();
        let now = unix_now().unwrap();
        const HOUR: i64 = 3_600;

        for (recorded_at, percent) in [(now - 3 * HOUR, 90), (now - 2 * HOUR, 80), (now - HOUR, 70)]
        {
            store
                .record(&sample(
                    "hid:a",
                    "Mouse",
                    recorded_at,
                    Some(percent),
                    false,
                    None,
                ))
                .unwrap();
        }
        for (recorded_at, percent) in [(now - 2 * HOUR, 40), (now - HOUR, 40)] {
            store
                .record(&sample(
                    "hid:b",
                    "Headset",
                    recorded_at,
                    Some(percent),
                    false,
                    None,
                ))
                .unwrap();
        }
        for (recorded_at, percent) in [(now - 1_800, 60), (now - 600, 60)] {
            store
                .record(&sample(
                    "hid:c",
                    "Keyboard",
                    recorded_at,
                    Some(percent),
                    false,
                    None,
                ))
                .unwrap();
        }
        for (recorded_at, percent) in [
            (now - 3 * HOUR, Some(85)),
            (now - 2 * HOUR, Some(80)),
            (now - HOUR, None),
            (now - 1_800, Some(75)),
        ] {
            store
                .record(&sample(
                    "hid:d",
                    "Receiver",
                    recorded_at,
                    percent,
                    false,
                    None,
                ))
                .unwrap();
        }

        let summaries = store.summary(DEFAULT_DAYS).unwrap();
        assert_eq!(summaries.len(), 4);

        let a = summary_for(&summaries, "hid:a");
        assert_eq!(a.name, "Mouse");
        assert_eq!(a.samples, 3);
        assert_eq!(a.first_at, now - 3 * HOUR);
        assert_eq!(a.last_at, now - HOUR);
        assert_eq!(a.latest_percent, Some(70));
        assert_eq!(a.min_percent, Some(70));
        assert_eq!(a.max_percent, Some(90));
        assert!((a.percent_per_day.unwrap() - 240.0).abs() < 1e-9);

        let b = summary_for(&summaries, "hid:b");
        assert_eq!(b.samples, 2);
        assert_eq!(b.min_percent, Some(40));
        assert_eq!(b.max_percent, Some(40));
        assert_eq!(b.percent_per_day, None);

        let c = summary_for(&summaries, "hid:c");
        assert_eq!(c.samples, 2);
        assert_eq!(c.percent_per_day, None);

        let d = summary_for(&summaries, "hid:d");
        assert_eq!(d.samples, 4);
        assert_eq!(d.latest_percent, Some(75));
        assert_eq!(d.min_percent, Some(75));
        assert_eq!(d.max_percent, Some(85));
        assert!((d.percent_per_day.unwrap() - 96.0).abs() < 1e-9);
    }

    #[test]
    fn csv_output_has_a_stable_header_order_and_escaped_fields() {
        let rows = vec![
            sample(
                "hid:b",
                "Mouse, \"Pro\"",
                1_700_000_060,
                Some(70),
                true,
                None,
            ),
            sample("hid:a", "Receiver", 1_700_000_000, None, false, Some(3_980)),
            sample(
                "hid:a",
                "Line\nBreak",
                1_700_000_000,
                None,
                false,
                Some(3_980),
            ),
        ];

        assert_eq!(
            render_csv(rows),
            concat!(
                "timestamp_utc,identity,name,percent,charging,voltage_mv\n",
                "2023-11-14T22:13:20Z,hid:a,Receiver,,false,3980\n",
                "2023-11-14T22:13:20Z,hid:a,\"Line\nBreak\",,false,3980\n",
                "2023-11-14T22:14:20Z,hid:b,\"Mouse, \"\"Pro\"\"\",70,true,\n",
            )
        );
    }

    #[test]
    fn table_output_has_per_device_columns() {
        let table = render_table(&[HistorySummary {
            identity: "hid:a".to_owned(),
            name: "Mouse".to_owned(),
            samples: 3,
            first_at: 1_700_000_000,
            last_at: 1_700_003_600,
            latest_percent: Some(70),
            min_percent: Some(70),
            max_percent: Some(90),
            percent_per_day: Some(240.0),
        }]);

        let header = table.lines().next().unwrap();
        for column in [
            "Device",
            "Samples",
            "First",
            "Last",
            "Latest %",
            "Min %",
            "Max %",
            "Est. %/day",
        ] {
            assert!(header.contains(column), "missing column {column}");
        }
        assert!(table.contains("Mouse"));
        assert!(table.contains("2023-11-14T22:13:20Z"));
        assert!(table.contains("2023-11-14T23:13:20Z"));
        assert!(table.contains("240.0"));
    }

    #[test]
    fn cli_options_parse_defaults_and_equivalents() {
        let defaults = parse_args(&[]).unwrap();
        assert_eq!(defaults.days, 7);
        assert_eq!(defaults.device, None);
        assert!(!defaults.csv);
        assert_eq!(defaults.database, None);
        assert!(!defaults.help);

        let separated = parse_args(&[
            "--days".to_owned(),
            "30".to_owned(),
            "--device".to_owned(),
            "receiver".to_owned(),
            "--csv".to_owned(),
            "--database".to_owned(),
            "history.sqlite3".to_owned(),
            "--help".to_owned(),
        ])
        .unwrap();
        assert_eq!(separated.days, 30);
        assert_eq!(separated.device.as_deref(), Some("receiver"));
        assert!(separated.csv);
        assert_eq!(separated.database, Some(PathBuf::from("history.sqlite3")));
        assert!(separated.help);

        let equals = parse_args(&[
            "--days=14".to_owned(),
            "--device=hid:".to_owned(),
            "--database=C:\\tmp\\history.sqlite3".to_owned(),
        ])
        .unwrap();
        assert_eq!(equals.days, 14);
        assert_eq!(equals.device.as_deref(), Some("hid:"));
        assert_eq!(
            equals.database,
            Some(PathBuf::from("C:\\tmp\\history.sqlite3"))
        );

        for args in [
            vec!["--days".to_owned()],
            vec!["--days".to_owned(), "0".to_owned()],
            vec!["--days".to_owned(), "seven".to_owned()],
            vec!["--device".to_owned()],
            vec!["--device".to_owned(), String::new()],
            vec!["--database".to_owned()],
            vec!["--bogus".to_owned()],
        ] {
            assert!(parse_args(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn cli_reports_a_missing_database_without_creating_it() {
        let database = TempDatabase::new("missing");
        assert!(!database.path().exists());
        let options = parse_args(&[
            "--database".to_owned(),
            database.path().to_string_lossy().into_owned(),
        ])
        .unwrap();
        let mut output = Vec::new();

        run_with_output(&options, &mut output).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!("{EMPTY_MESSAGE}\n")
        );
        assert!(!database.path().exists());
    }

    #[test]
    fn cli_reports_an_empty_database() {
        let database = TempDatabase::new("empty");
        drop(HistoryStore::open(database.path()).unwrap());
        let options = parse_args(&[
            "--database".to_owned(),
            database.path().to_string_lossy().into_owned(),
        ])
        .unwrap();
        let mut output = Vec::new();

        run_with_output(&options, &mut output).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!("{EMPTY_MESSAGE}\n")
        );
    }

    #[test]
    fn cli_csv_filters_devices_and_orders_rows() {
        let database = TempDatabase::new("cli-csv");
        let now = unix_now().unwrap();
        {
            let mut store = HistoryStore::open(database.path()).unwrap();
            store
                .record(&sample(
                    "hid:a",
                    "Receiver",
                    now - 60,
                    Some(80),
                    false,
                    Some(4_000),
                ))
                .unwrap();
            store
                .record(&sample(
                    "hid:b",
                    "Mouse, \"Pro\"",
                    now - 120,
                    Some(60),
                    true,
                    None,
                ))
                .unwrap();
        }
        let options = parse_args(&[
            "--database".to_owned(),
            database.path().to_string_lossy().into_owned(),
            "--csv".to_owned(),
            "--device".to_owned(),
            "receiv".to_owned(),
        ])
        .unwrap();
        let mut output = Vec::new();

        run_with_output(&options, &mut output).unwrap();

        let text = String::from_utf8(output).unwrap();
        assert!(text.starts_with("timestamp_utc,identity,name,percent,charging,voltage_mv\n"));
        assert!(text.contains(",hid:a,Receiver,80,false,4000\n"));
        assert!(!text.contains("hid:b"));
    }
}
