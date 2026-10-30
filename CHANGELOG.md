# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Logs now use one file per local day (`volty-YYYY-MM-DD.log`) and remove daily files older than seven days. Each daily file still rotates at 5 MiB with one `.1` backup, and an existing `volty.log` is moved into the current day's file.

### Fixed

- Bluetooth battery readings now require Windows to report the device as connected. Paired-but-disconnected devices no longer keep reporting a stale PnP battery percentage or remain in the tracked-device list.

## [0.3.1] - 2026-10-29

### Changed

- The tray hover tooltip now shows one compact line per device with its name, battery percentage, and ETA when one is available, instead of falling back to summary text when the full menu labels do not fit. Charge-level wording is kept in the expanded menu only.

### Fixed

- Fixed a crash on the second Bluetooth scan: Windows Runtime was initialized and torn down for every scan while the Windows bindings cache activation factories process-wide. It now stays initialized for each scanning thread's lifetime.

## [0.3.0] - 2026-10-29

### Added

- Added `volty capture` for read-only hardware snapshots, with `--output` and `--strict` (privacy profile) options. The tray Diagnose action writes the same `.volty.jsonl` format.
- Hardware captures can now include Bluetooth evidence: capture-local `bt-N` device records plus raw Windows battery-property and GATT Battery Level payloads. `volty capture replay` re-decodes that evidence without hardware in default builds. Captures never contain Bluetooth addresses or opaque Windows device ids; recording requires a tray build with the opt-in `bluetooth` feature.
- Added `volty capture inspect` and `volty capture replay` for offline schema checks and decoder/reading verification, plus synthetic and reviewed hardware fixtures under `tests/hardware`.
- Generic HID capture samples now include charging and discharging state bits so layout replay can reproduce the normalized battery state without full input reports.
- Snapshot capture now uses previously discovered HID++ slots when available, avoiding a full multi-slot probe on the common path.
- Documented the hardware capture format and workflow, and added a **Hardware report** issue template for device-specific battery behavior.
- The Diagnose window now also saves a redacted `.volty.jsonl` diagnostic capture for local review; captures are not uploaded automatically.

### Changed

- Added a tested-device table to the README with each device's connection, battery backend, verified functionality, and support status. The table tracks verified, experimental, and known-unsupported hardware and is the place to add newly tested devices.
- `volty-bluetooth` now keeps the pure property and GATT decoders available by default and gates the Windows backend behind its default `windows-backend` feature. The tray enables that backend only with its opt-in `bluetooth` feature, so hardware capture replay works in default builds.
- Corrected the HID support documentation to describe generic HID battery support as implemented, name the Logitech G502 X Superlight RGB as the current primary use case, and remove stale "Logitech-only" claims. No new hardware validation is claimed.
- Hardened CI: Clippy, tests, and builds run with `--locked`, tests use `--all-features`, a Rust 1.89.0 MSRV job checks the workspace, and an Ubuntu job runs `cargo audit --deny warnings` against `Cargo.lock`.
- **Breaking:** introduced the `BatteryProvider` trait in `volty-battery`. A provider yields readings keyed by a stable `DeviceId` plus a `DeviceLabel` and `BatteryStatus`; the tray polls a list of providers (`HidProvider`, and `BluetoothProvider` with the `bluetooth` feature) instead of hard-coding the HID query and a Bluetooth branch.
- **Breaking:** `BatteryStatus.source` is now a plain `&'static str` label set by each backend; the transport-specific `BatterySource` enum is removed from `volty-battery`.
- **Breaking:** renamed the HID catalog vocabulary to be device-neutral: `MouseModel` → `HidBatteryDevice`, `SUPPORTED_MICE` → `SUPPORTED_HID_DEVICES`, `resolve_mouse_model` → `resolve_hid_device`, `AmbiguousMouseModel` → `AmbiguousHidDevice`, `LogitechHidppMouse` → `LogitechHidppDevice`.
- Extracted a `DeviceSession` from the tray `App`: the poller, latest/pending readings, discovered devices and ETA state now live behind one service that the UI consumes.
- `volty-hid` command/profile code (`volty hid` diagnostics, TOML profiles) is behind the new `profile` feature, so protocol-only consumers no longer pull in `toml`. The tray enables it.
- Documented the provider model, stable identity, the Bluetooth promotion criterion (one trustworthy Windows reading with a stable id) and the Windows-only platform scope in the README and `docs/`.
- Reuse one HID context for device opens instead of re-enumerating all HID devices per query.
- Persist recent-device order once per session and run SQLite in WAL mode, removing a synced write per poll.
- Skip redundant tray icon, tooltip and menu updates when nothing changed.
- Track a long-run discharge rate per device (percent lost over the time Volty was running and watching it, excluding downtime, gaps, charging and level increases), persist it in the local state, and use it for the battery ETA right after startup until the recent-window estimate is available.
- Track log file size in memory instead of querying file metadata on every log line.
- The tray loop now blocks on a Win32 event plus the message queue (`MsgWaitForMultipleObjects`) instead of sleeping and polling; poll results and config-file changes wake it. Idle wakeups drop to zero.
- **Breaking:** removed the `ui_poll_interval_ms` config key (the loop is event-driven); remove it from `config.toml`, as unknown keys are a config error.
- Added generic HID battery support: any HID interface whose report descriptor declares a battery level (Generic Device Controls *Battery Strength*, or Battery System relative/absolute state of charge, or remaining ÷ full-charge capacity) is now read through the Windows HID parser and shown as "HID Battery", with charging state when the Battery System page reports it. Vendor-defined pages are skipped, probe results are cached per interface and dropped when the device disappears.
- HID++ polls cache each slot's battery feature index and unified-battery capabilities, so a steady-state poll is one transaction instead of three or four; a failed cached read rediscovers the layout. Discovery of an empty slot now gives up after 1 s instead of 2 s.
- Trace-level strings are no longer formatted unless the log level is `trace`; as a result, trace lines from HID discovery no longer appear in the diagnostics window at lower log levels.

### Fixed

- HID device identity no longer depends on the Windows interface path. Ids are now `hid:{stable key}#{slot}` where the key is derived from VID/PID, interface number, usage page/usage and a hash of the serial number (identical twins are split deterministically). The slot cache, monitor flags and low-battery alert de-duplication therefore survive USB re-enumeration instead of splitting.
- Bluetooth PnP battery reads now request the correct `DEVPKEY_Device_BatteryLevel` property (`{104EA319-6EE2-4701-BD47-8DDBF425BBE5}, 2`). The previous key never contained a value, so devices that expose `System.Devices.BatteryLife` only through a PnP node produced no reading. Validated with a Baseus Bass BS2 Lite headset; the opt-in Bluetooth feature remains off by default while BLE GATT and broader hardware coverage are unverified.

### Migration

- Local state schema version 2: the `recent_devices` table is keyed by `interface_key` instead of `interface_path`. On first start the old path-keyed table is dropped and existing `hid:` entries are removed from monitored-device flags and notified discharge cycles, so per-device monitor choices and alert latches for HID devices reset once. Bluetooth ids are unchanged.

## [0.2.1] - 2026-10-28

### Changed

- Refreshed the README: added CI/release/license badges, corrected the tray menu action name (**Discover Devices Now**), documented per-device monitoring, log rotation, MSRV, the CI check commands, and the release procedure.
- Introduced this changelog's release workflow: user-facing changes are recorded under `[Unreleased]` and moved under a version heading on release.

- Split `volty-hid` (`interface`, `hid`, `device`), the Bluetooth backend (`budget`, `winrt`, `classic`, `setupapi`, `ble`) and the About/Diagnose windows into focused modules; no function in non-test code exceeds 100 lines.
- Added a Windows CI workflow (fmt, clippy `-D warnings`, tests, Bluetooth build); releases are now gated on the same checks.
- Unified crate licensing and shared dependency versions via the workspace manifest; internal crates are `publish = false`.
- Added an MIT `LICENSE` file; `volty-hid` is now MIT-only like the rest of the workspace.
- Added unit tests for tray menu logic, notification latches, local state, and extracted Win32 layout helpers.
- Enabled workspace lints (`undocumented_unsafe_blocks`, `unsafe_op_in_unsafe_fn`, `dbg_macro`, `todo`, `unimplemented`) and documented the safety invariant of every `unsafe` block.

### Fixed

- Hardened Win32/SetupAPI FFI: clipboard memory and locks are released through RAII guards, wide strings with interior NULs are rejected, API-reported buffer sizes are bounded, and window-state/`SetStdHandle`/monitor-lookup error paths no longer lose or misreport errors.
- Split the tray application into focused modules (`app/*`, `tray/*`, shared `win32` helpers) and decomposed the monolithic event loop and device query.
- Bluetooth scans now share a single overall time budget across the classic, SetupAPI, and BLE phases; partial results are kept with a warning.
- The log file now rotates at 5 MiB, keeping one `.1` backup.
- Resolved all Clippy warnings and removed runtime `unwrap` panics in HID selection and report parsing.

## [0.2.0] - 2026-10-26

### Added

- Added Windows Bluetooth battery monitoring and device discovery, including support for compatible headphones.
- Added SQLite-backed monitored-device preferences and recent-device state with prioritized HID polling.
- Added a tray action to reset Volty configuration and local state.
- Added a configurable UI polling interval and sustained-run performance assessment.

### Changed

- Restored Ctrl+C handling when launched from a terminal while keeping Explorer launches console-free.
- Improved Bluetooth discovery, HFP service handling, and diagnostics.
- Consolidated battery readings in the shared `volty-battery` crate and refined HID++ battery support.
- Refined battery notification and configuration handling, About-window layout, and font rendering.
- Updated device-support and Bluetooth documentation.

## [0.1.0] - 2026-10-26

### Added

- Initial portable Windows system-tray battery monitor with Logitech HID++ 2.0 support.
- Dynamic percentage tray icon, configurable battery thresholds and color coding, and threshold notifications.
- Native diagnostics window with HID details, log copying, and GitHub issue draft support.
- Sleep and reconnect handling for supported wireless devices.
