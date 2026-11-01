#![cfg(windows)]

use std::{
    path::{Path, PathBuf},
    process::Command,
};

const FIXTURES: [&str; 5] = [
    "synthetic-g502x-unified.volty.jsonl",
    "synthetic-battery-voltage.volty.jsonl",
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
            stdout.contains("schema: volty.capture/v2"),
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

#[test]
fn hid_sniff_requires_explicit_input_acknowledgment() {
    let output = volty()
        .args(["hid", "sniff", "--output", "unused.hidtrace.jsonl"])
        .output()
        .expect("run HID sniff command");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--include-input"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn hid_sniff_help_and_duration_validation_are_available_without_hardware() {
    let help = volty()
        .args(["hid", "sniff", "--help"])
        .output()
        .expect("run HID sniff help");
    assert!(help.status.success());
    assert!(
        String::from_utf8_lossy(&help.stdout).contains("raw input reports"),
        "{}",
        String::from_utf8_lossy(&help.stdout)
    );

    let invalid_duration = volty()
        .args([
            "hid",
            "sniff",
            "--include-input",
            "--duration-seconds",
            "0",
            "--output",
            "unused.hidtrace.jsonl",
        ])
        .output()
        .expect("run HID sniff with invalid duration");
    assert!(!invalid_duration.status.success());
    assert!(
        String::from_utf8_lossy(&invalid_duration.stderr).contains("1..=120"),
        "{}",
        String::from_utf8_lossy(&invalid_duration.stderr)
    );
}

#[test]
fn hid_trace_requires_a_trace_file() {
    let output = volty()
        .args(["hid", "trace"])
        .output()
        .expect("run HID trace command");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Usage: volty hid trace"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
