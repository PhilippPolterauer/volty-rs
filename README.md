# Volty (`volty-rs`)

<p align="center">
  <strong>A low-overhead Windows system-tray battery monitor for HID peripherals, written in Rust.</strong>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#building-from-source">Building</a> •
  <a href="#license">License</a>
</p>

---

## 👁️ The Vision

Modern gaming mice like the **Logitech G502 X / G502 X Plus** are the initial use case: monitoring their battery level on Windows often forces users to run background suites like **Logitech G HUB**—consuming hundreds of megabytes of RAM and running unnecessary background telemetries.

Standard Windows battery APIs (like ACPI) only report system battery status for laptops and fail to pick up proprietary USB HID peripherals.

**Volty** aims to monitor battery levels across HID peripherals through protocol-specific backends, then present supported readings in the Windows taskbar tray. Logitech HID++ 2.0 is the first implemented backend; generic HID battery reports and other vendors are not supported yet.

> **Project status:** Volty is a vibe-coded hobby project. It is experimental,
> has limited hardware testing, and is not production-ready or affiliated with
> Logitech. Treat device support and battery readings as best-effort.

### Core Philosophy

* **⚡ Low Idle Overhead:** Written in Rust with a timer-driven device worker. On the local Windows MSVC run, idle CPU time was 0.062 seconds over 20 seconds; private working set was 1.32 MB and total working set was 11.23 MB. Shared system pages account for the difference. Memory varies with Windows and system state, so only the private working set was verified below 10 MB on this host.
* **🔒 Zero Bloat:** No Electron, no heavy frameworks, no telemetry daemons, and no G HUB dependency.
* **🛡️ Protocol Adapters:** Keeps device-specific discovery and decoding behind protocol backends; Logitech HID++ 2.0 is the first implementation.
* **🔔 Smart Notifications:** Stays out of your way and fires a native Windows toast notification only when your battery crosses critical thresholds (e.g., 20%).

---

## ✨ Features

* **Dynamic Tray Icon:** High-DPI rasterized numeric percentage rendered live inside the Windows system tray.
* **Configurable Color Coding:** The default tray map is green above 30%, yellow from 16–30%, and red at 15% or below. Thresholds and colors can be overridden in the TOML configuration.
* **Threshold Alerts:** One-time native Windows toast notification per discharge cycle when battery hits warning thresholds.
* **One-Click Diagnostics:** A native Diagnose window shows recent app logs and HID details, with buttons to copy the log or open a GitHub issue draft.
* **Smart Sleep Detection:** Handles mouse sleep cycles and wireless reconnects gracefully without crashing or throwing spurious errors.
* **Single Portable Binary:** Runs as a standalone `.exe` with zero installer overhead or system modifications.

---

## 🏗️ Architecture

`volty-rs` is structured as a modular Cargo workspace, keeping hardware protocol handling strictly separated from UI rendering:

```text
volty-rs/
├── crates/
│   ├── volty-hid/             # Zero-UI, zero-Win32 protocol crate
│   │   ├── src/
│   │   │   ├── lib.rs         # Public API and optional HID device access
│   │   │   ├── protocol.rs    # Extensible protocol and mouse-model traits
│   │   │   ├── logitech_hidpp.rs # First and currently only protocol backend
│   │   │   └── battery.rs     # UnifiedBattery and legacy status parsing
│   │   │   ├── command.rs     # Validated profile command execution
│   │   │   └── profile.rs     # TOML command profiles and response decoders
│   │   └── Cargo.toml
│   │
│   └── volty-tray/            # Windows binary application
│       ├── src/
│       │   ├── main.rs        # Single-instance guard
│       │   ├── hid_cli.rs     # On-demand HID++ diagnostics
│       │   ├── app.rs         # Polling state machine and worker
│       │   ├── config.rs      # Per-user TOML settings
│       │   ├── tray.rs        # Dynamic icon generation & context menu
│       │   ├── diagnose.rs    # Native diagnostic window and clipboard actions
│       │   ├── report.rs      # Local diagnostic report generation
│       │   ├── logging.rs     # Level-filtered local log file
│       │   └── notify.rs      # Windows toast dispatcher
│       └── Cargo.toml
│
└── Cargo.toml                 # Workspace manifest
```

The HID crate defines `BatteryProtocol` and `MouseModel` traits and keeps
registered protocols and mouse families separate. Logitech HID++ 2.0 is the
first and currently only registered protocol; its mouse-family matcher uses
that backend, which discovers battery features at runtime. Adding a backend
does not require changing battery presentation or tray polling. Generic HID
battery-report discovery and other vendors are not implemented yet. The tray
does not load command profiles. See
[Generic HID battery-level support](./docs/hid-battery-support.md) for the
component flow, current limits, and profile packaging options.

## Quick start

Run the application on Windows from the workspace root:

```powershell
cargo run --release --package volty-tray
```

The first battery query is made at startup; subsequent polling occurs every two
minutes. Use the tray menu to refresh, open configuration, enable startup, or
quit. **About Volty** opens a small native window with the app version and a
button to visit the project on GitHub.

Low-battery alerts default to 20% and fire once per discharge cycle. Select
**Open Configuration** in the tray menu to create or open
`%APPDATA%\Volty\config.toml` in your default editor (Notepad is used if no
editor is associated with `.toml` files). Supported settings are:

```toml
low_battery_threshold = 20 # 1-100 percent
poll_interval_seconds = 120 # 30-3600 seconds after a successful reading
retry_interval_seconds = 15 # 5-300 seconds after an error
notifications_enabled = true
log_level = "info" # error, warn, info, debug, or trace

# Optional tray color-map overrides. Uncomment the table and any values to activate.
# Thresholds are inclusive upper bounds; red_max_percent must be below yellow_max_percent.
# Colors use #RRGGBB notation.
# [battery_color_map]
# red_max_percent = 15
# yellow_max_percent = 30
# red = "#d23030"
# yellow = "#ebbe23"
# green = "#2daa4b"
```

Settings are stored per-user and do not require administrator privileges.
Volty watches the file for changes, and **Refresh Now** also reloads it.
Invalid settings are reported while the last valid configuration stays active.
The default tray color map matches the green/yellow/red ranges above; uncomment
`[battery_color_map]` to override its cutoffs or colors. Any omitted values
retain their defaults. The red and yellow cutoffs can be set from 0–99 and
1–100 percent respectively, and must be ordered so red is lower than yellow.
Colors must be six-digit hexadecimal RGB values such as `"#d23030"`.
Logs append to `%APPDATA%\Volty\volty.log`; choose **Open Log File** in the tray
menu to view them. The default `info` level records lifecycle and errors,
`debug` adds successful battery readings, and `trace` adds low-level event
details. Changes to `log_level` take effect when the config is reloaded.

Choose **Diagnose / Report an Issue** in the tray menu to open a native Windows
window with recent application events, the active configuration, and HID
interfaces visible to hidapi. The report includes device IDs, usage
information, and available manufacturer/product names, but omits device paths
and serial numbers. Review the contents before sharing; nothing is uploaded
automatically. **Copy Log** copies the report to the clipboard, while
**Open Issue** starts a GitHub issue draft. A local copy is saved under
`%APPDATA%\Volty\reports`.

## HID diagnostics

The `volty` executable also has explicit, on-demand HID++ diagnostics. These
commands run before the tray app's single-instance check; launching `volty`
without a `hid` subcommand still starts the normal tray app.
For the Python/hidapi probe that inspired this workflow, see
[the Logitech HID++ diagnostic notes](./docs/logitech.md).
For the first worked backend example—verifying a Logitech mouse or adding
support for a different HID++ battery feature—see
[Adding a Logitech mouse](./docs/adding-mice.md).
For the generic HID battery-monitoring goal, architecture, and profile
packaging options, see
[Generic HID battery-level support](./docs/hid-battery-support.md).

```powershell
volty hid inspect
volty hid inspect --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002
volty hid features --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002 --device-index 0x01
volty hid run --profile .\battery.toml --command battery-status --device-index 0x01 --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002
volty hid raw --device-index 0x01 --feature-index 0x06 --function-index 1 --report-type long --params 00,00,00,00,00,00,00,00,00,00,00,00,00,00,00,00 --allow-state-changing --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002
```

`inspect` prints matching interfaces' VID/PID, usage page/usage, interface
number, bus, product name, and hidapi path. The path is shown only by this
explicit diagnostic; serial numbers are never printed. hidapi does not expose
report lengths here, so Volty does not infer or display them. Selectors accept
`--vid`, `--pid`, `--usage-page`, `--usage`, `--interface`, and `--path`.
Commands that open a device require exactly one interface match: if a selector
is missing or ambiguous, use `inspect` to narrow it. Do not confuse the USB
interface number with the HID++ `--device-index`, which addresses a paired
device behind a receiver.

`features` queries the device's Root/FeatureSet mappings and prints each
logical feature ID alongside its dynamically assigned runtime index, type, and
version. Profiles refer to logical IDs; runtime indexes can vary by device and
must not be copied into a profile. On one tested receiver, logical Battery
feature `0x1004` resolved to runtime index `0x06`, version `0x03`, and the
status decoder reported 78%; that mapping is an observation, not a constant.

Profiles are TOML. For example, this read-only Unified Battery command uses
logical feature ID `0x1004`; the engine resolves the runtime index at execution
time:

```toml
[profile]
schema_version = 1
name = "battery-diagnostics"
protocol = "hidpp20"

[commands.battery-status]
description = "Read Unified Battery status"
access = "read_only"
operation = "feature_call"
feature_id = 0x1004
function_index = 1
report = "long"
request_params = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
timeout_ms = 2000
decoder = "unified_battery_status"
```

This profile is included at
[`crates/volty-hid/examples/unified-battery.toml`](./crates/volty-hid/examples/unified-battery.toml).
Run it with `volty hid run --profile .\crates\volty-hid\examples\unified-battery.toml --command battery-status --device-index 0x01` and an interface selector if needed. The command reports
the typed decoded result and all raw request/response frames, including feature
lookup traffic; a successful write without a matching response is not reported
as a successful command. State-changing profile commands are rejected unless
`--allow-state-changing` is explicitly provided.

`raw` accepts only exact HID++ report payload sizes: 3 parameter bytes for a
short report or 16 for a long report. It requires
`--allow-state-changing` because arbitrary raw requests cannot be safely
classified as read-only from their bytes alone, and it prints a warning before
transmission. Prefer named, validated profile commands. Inspect the target,
feature, device index, and requested operation before authorizing it.

Interface usage combinations and report behavior are device-specific. For
example, one tested `046D:C547` receiver exchanged valid 20-byte HID++ reports
on usage `FF00/0002`, while `FF00/0001` accepted a 7-byte write without a
matching response. This observation is not a universal rule for other
receivers or devices; select the interface that responds correctly on the
particular system. CLI output is local and is not uploaded. Review any
diagnostic output before sharing it. To save a local transcript in PowerShell,
pipe a command to `Tee-Object .\volty-hid.log`; inspect the file before sharing
it because diagnostic interface paths can identify local devices.

## Building and testing

Prerequisites: Windows 10/11 and the stable `x86_64-pc-windows-msvc` Rust
toolchain.

```powershell
cargo build --release --package volty-tray
cargo test --workspace
```

## Releases

Push a version tag such as `v0.1.0` to run the Windows release workflow. It
tests the workspace, verifies the tag against the `volty-tray` package version,
and publishes a standalone `x86_64-pc-windows-msvc` executable with a SHA-256
checksum to the GitHub Release. No installer or sidecar runtime files are
required.

Protocol-frame unit tests use mock byte buffers and do not require a connected
mouse. Device communication and tray behavior should be verified on Windows
with compatible Logitech Lightspeed hardware.

## License

The tray application is MIT-licensed. The HID crate is dual-licensed under MIT
or Apache-2.0.
