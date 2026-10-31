#![cfg(windows)]

use std::{
    path::{Path, PathBuf},
    process::Command,
};

const FIXTURES: [&str; 4] = [
    "synthetic-g502x-unified.volty.jsonl",
    "g502x-plus-unified.volty.jsonl",
    "synthetic-generic-battery.volty.jsonl",
    "synthetic-bluetooth-headset.volty.jsonl",
];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/hardware")
        .join(name)
}

fn volty() -> Command {
    Command::new(env!("CARGO_BIN_EXE_volty"))
}

#[test]
fn capture_inspect_and_replay_accept_hardware_fixtures() {
    for name in FIXTURES {
        let path = fixture(name);

        let inspect = volty()
            .args(["capture", "inspect"])
            .arg(&path)
            .output()
            .expect("run capture inspect");
        assert!(
            inspect.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&inspect.stderr)
        );
        let stdout = String::from_utf8_lossy(&inspect.stdout);
        assert!(
            stdout.contains("schema: volty.capture/v1"),
            "{name}: {stdout}"
        );
        assert!(stdout.contains("footer: complete=true"), "{name}: {stdout}");

        let replay = volty()
            .args(["capture", "replay"])
            .arg(&path)
            .output()
            .expect("run capture replay");
        assert!(
            replay.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&replay.stderr)
        );
        let stdout = String::from_utf8_lossy(&replay.stdout);
        assert!(
            stdout.contains("Replayed hardware capture:"),
            "{name}: {stdout}"
        );
    }
}

#[test]
fn capture_subcommands_reject_a_missing_path() {
    for command in ["inspect", "replay"] {
        let output = volty()
            .args(["capture", command])
            .output()
            .expect("run capture command");
        assert!(!output.status.success(), "{command} accepted no path");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("requires a capture path"),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
