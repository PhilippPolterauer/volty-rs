//! Records when Volty stopped without saying so: a run marker that survives crashes and kills,
//! and a wall-clock watchdog for stretches where the tray loop did not run at all.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MARKER_NAME: &str = "running.marker";
/// The loop wakes at least every minute, so far longer gaps mean sleep, suspension or a hang.
pub(super) const STALL_THRESHOLD: Duration = Duration::from_secs(150);

pub(super) struct Liveness {
    marker: PathBuf,
    last_tick: SystemTime,
}

impl Liveness {
    /// Arms the marker and returns a description of the previous run if it did not exit cleanly.
    pub(super) fn start(directory: &Path) -> (Self, Option<String>) {
        let marker = directory.join(MARKER_NAME);
        let previous = fs::read_to_string(&marker).ok().map(|text| {
            let heartbeat = text.trim().parse::<u64>().ok();
            describe_unclean_exit(heartbeat)
        });
        let liveness = Self {
            marker,
            last_tick: SystemTime::now(),
        };
        let _ = liveness.write_heartbeat();
        (liveness, previous)
    }

    /// Returns how long the loop was unresponsive if the wall clock jumped past the threshold.
    pub(super) fn tick(&mut self) -> Option<Duration> {
        let now = SystemTime::now();
        let gap = now.duration_since(self.last_tick).ok();
        self.last_tick = now;
        gap.filter(|gap| *gap > STALL_THRESHOLD)
    }

    pub(super) fn write_heartbeat(&self) -> io::Result<()> {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs();
        fs::write(&self.marker, seconds.to_string())
    }

    pub(super) fn clean_exit(&self) {
        let _ = fs::remove_file(&self.marker);
    }
}

fn describe_unclean_exit(heartbeat: Option<u64>) -> String {
    match heartbeat {
        Some(seconds) => format!(
            "Previous Volty run did not shut down cleanly (killed, crashed or power loss); last heartbeat at unix time {seconds}"
        ),
        None => "Previous Volty run did not shut down cleanly (killed, crashed or power loss)"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("volty-liveness-{label}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn clean_exit_leaves_no_marker_for_the_next_run() {
        let dir = temp_dir("clean");
        let (liveness, previous) = Liveness::start(&dir);
        assert!(previous.is_none());
        liveness.clean_exit();
        let (_, previous) = Liveness::start(&dir);
        assert!(previous.is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn leftover_marker_reports_an_unclean_exit_with_its_heartbeat() {
        let dir = temp_dir("unclean");
        let (_first, _) = Liveness::start(&dir);
        let (_second, previous) = Liveness::start(&dir);
        assert!(previous.unwrap().contains("last heartbeat at unix time"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn tick_reports_only_gaps_past_the_threshold() {
        let dir = temp_dir("tick");
        let (mut liveness, _) = Liveness::start(&dir);
        assert!(liveness.tick().is_none());
        liveness.last_tick = SystemTime::now() - STALL_THRESHOLD - Duration::from_secs(10);
        assert!(liveness.tick().unwrap() > STALL_THRESHOLD);
        let _ = fs::remove_dir_all(dir);
    }
}
