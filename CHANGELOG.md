# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Split `volty-hid` (`interface`, `hid`, `device`), the Bluetooth backend (`budget`, `winrt`, `classic`, `setupapi`, `ble`) and the About/Diagnose windows into focused modules; no function in non-test code exceeds 100 lines.
- Added a Windows CI workflow (fmt, clippy `-D warnings`, tests, Bluetooth build); releases are now gated on the same checks.
- Unified crate licensing and shared dependency versions via the workspace manifest; internal crates are `publish = false`.
- Added unit tests for tray menu logic, notification latches, local state, and extracted Win32 layout helpers.
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
