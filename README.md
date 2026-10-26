# Volty (`volty-rs`)

<p align="center">
  <img src="docs/icon.png" alt="Volty logo" width="220">
</p>

<p align="center">
  <strong>A Windows system-tray battery monitor for supported HID peripherals, written in Rust.</strong>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#building-and-testing">Building</a> •
  <a href="#license">License</a>
</p>

---

## Current support

Volty is a Windows system-tray battery monitor. The current backend supports
Logitech HID++ 2.0 interfaces and the standard Unified Battery and legacy
Battery Status features. Other vendors and generic HID battery reports are
not currently supported. Hardware compatibility is best-effort and has been
tested on a limited set of Logitech hardware.

> **Project status:** Volty is a vibe-coded hobby project. It is experimental,
> has limited hardware testing, and is not production-ready or affiliated with
> Logitech. Treat device support and battery readings as best-effort.

---

## ✨ Features

* **HID++ Battery Monitoring:** Polls supported Logitech interfaces and paired-device indexes, uses each device's HID++ name when available, and summarizes the lowest reported battery percentage in the tray icon.
* **Configurable Color Coding:** The default tray map is green above 30%, yellow from 16–30%, and red at 15% or below. Thresholds and colors can be overridden in the TOML configuration.
* **Threshold Alerts:** One-time native Windows toast notification per discharge cycle when battery hits warning thresholds.
* **One-Click Diagnostics:** A native Diagnose window shows recent app logs and HID details, with buttons to copy the log or open a GitHub issue draft.
* **Retry After Read Errors:** Polling resumes after errors using the configurable retry interval.
* **Standalone Executable:** Releases provide a single `.exe` with no installer. Optional Start on Boot uses the current user's Windows Run setting.

---

## 🏗️ Architecture

`volty-rs` is structured as a modular Cargo workspace, keeping hardware protocol handling strictly separated from UI rendering:

```text
volty-rs/
├── crates/
│   ├── volty-hid/             # Zero-UI, zero-Win32 protocol crate
│   │   ├── protocols/         # Built-in TOML command profiles
│   │   │   └── logitech-hidpp/
│   │   │       └── unified-battery.toml
│   │   ├── build.rs           # Embeds the profile directory at compile time
│   │   ├── src/
│   │   │   ├── lib.rs         # Public API and optional HID device access
│   │   │   ├── protocol.rs    # Protocol/model traits and support catalog
│   │   │   ├── logitech_hidpp.rs # HID++ 2.0 transport and battery backend
│   │   │   ├── battery.rs     # Normalized battery status and decoding
│   │   │   ├── command.rs     # Validated profile command execution
│   │   │   └── profile.rs     # TOML command profiles and response decoders
│   │   └── Cargo.toml
│   │
│   └── volty-tray/            # Windows binary application
│       ├── build.rs           # Embeds the Windows application manifest and icon
│       ├── volty.manifest
│       ├── assets/
│       │   └── volty.ico      # Windows application icon
│       ├── src/
│       │   ├── main.rs        # CLI dispatch and single-instance guard
│       │   ├── hid_cli.rs     # On-demand HID++ profile/device diagnostics
│       │   ├── app.rs         # Polling state machine and worker
│       │   ├── config.rs      # Per-user TOML settings
│       │   ├── tray.rs        # Dynamic icon generation & context menu
│       │   ├── about.rs       # About window
│       │   ├── diagnose.rs    # Native diagnostic window and clipboard actions
│       │   ├── native_ui.rs   # Shared native Windows UI helpers
│       │   ├── report.rs      # Local diagnostic report generation
│       │   ├── logging.rs     # Level-filtered local log file
│       │   └── notify.rs      # Windows toast dispatcher
│       └── Cargo.toml
│
└── Cargo.toml                 # Workspace manifest
```

### Runtime data flow

```text
HID interface enumeration
        |
        v
resolve_mouse_model -> SUPPORTED_MICE catalog
        | no match: skip interface
        | ambiguous: report an error
        | unique model
        v
model.protocol() -> BatteryProtocol
        |
        v
protocol-specific transport, feature discovery, and decoding
        |
        v
BatteryStatus -> tray state, icon, and notifications
```

`volty-hid` owns protocol behavior and presents readings through the common
`BatteryStatus` type. `BatteryProtocol` implements interface matching,
device-address iteration, and the protocol-specific battery query.
`MouseModel` associates a device family with a backend. The `SUPPORTED_MICE`
catalog is the dispatch source: an interface must match exactly one model;
no match is skipped and overlapping matches are an error. The current catalog
contains one Logitech HID++ 2.0 model. That backend then discovers battery
features at runtime. Polling retains every successful interface/device-index
reading. The tray lists each readable device and uses the lowest reported
percentage for its battery icon.

`volty-tray` owns polling, configuration, tray presentation, and
notifications. Its `build.rs` embeds `volty.manifest` in Windows builds.
Separately, `volty-hid/build.rs` embeds every TOML profile below `protocols/`
in the executable. `volty hid profiles` lists these built-ins; `volty hid run`
can select one by ID or load a custom profile from a path. Profiles are
diagnostic command descriptions only: the tray does not load them or use them
for backend selection. The current profile runner supports HID++ 2.0 only.

Adding a device that shares an existing wire protocol requires a specific
`MouseModel` matcher and a catalog entry. `MouseModel` supplies the family
name, backend, and interface match (`matches` can override the default backend
match). Add it to `SUPPORTED_MICE`; that is the only tray dispatch catalog.

A new wire protocol needs a new `BatteryProtocol`; a new encoding on an
existing protocol can extend that backend. Its `supports_interface`
identifies candidate interfaces, `device_indices` returns addresses to try,
and `query_battery` performs discovery/transport/decoding and returns a
normalized `BatteryStatus`. Set `level_percent` only when the device really
reports a percentage. Add or update a `MouseModel` and register it in
`SUPPORTED_MICE` when the interface needs a new or more specific match. The
existing tray code consumes `BatteryStatus`, so a new backend does not need a
tray/UI trait or a protocol-specific icon.

If an existing model already matches the interface and its backend already
reads the device's battery feature, no new trait implementation is needed.
A profile file alone never registers tray support. See
[Generic HID battery-level support](./docs/hid-battery-support.md) for the
extension flow and current limitations.

## Quick start

Run the application on Windows from the workspace root:

```powershell
cargo run --release --package volty-tray
```

The first battery query is made at startup; subsequent polling occurs every two
minutes by default. Volty runs a single tray-app instance. The tray lists
all devices with readable battery status; its icon reflects the lowest reported
percentage. Use the menu to refresh, open configuration, enable startup, or
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
For the first worked backend example—verifying a Logitech mouse or adding
support for a different HID++ battery feature—see
[Adding a Logitech mouse](./docs/adding-mice.md).
For the generic HID battery-monitoring goal, architecture, and built-in or
custom profiles, see
[Generic HID battery-level support](./docs/hid-battery-support.md).

```powershell
volty hid inspect
volty hid inspect --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002
volty hid features --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002 --device-index 0x01
volty hid profiles
volty hid run --profile builtin:logitech-hidpp/unified-battery --command battery-status --device-index 0x01
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

The built-in read-only Unified Battery profile uses logical feature ID
`0x1004`; the engine resolves its runtime index for each device. It is embedded
in the executable, so no profile file needs to be copied or saved. Use
`volty hid profiles` to list embedded profiles. Custom TOML profiles can still
be loaded with `--profile PATH`.

Profile runs report decoded output and all raw request/response frames,
including feature lookup traffic; a successful write without a matching
response is not reported as a successful command. State-changing profile
commands are rejected unless `--allow-state-changing` is explicitly provided.

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
