# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added Windows Bluetooth LE battery monitoring and device discovery in the tray app.
- Added SQLite-backed device state and prioritized HID polling.
- Added a design plan for Bluetooth headphone battery support and extended the battery protocol abstractions toward that work.

### Changed

- Consolidated battery management in the shared `volty-battery` crate, removing duplicate protocol implementations and unused build scripts.
- Refactored battery notification and configuration handling.
- Removed the standalone diagnostic command executor; mouse battery queries stay in protocol backends.
- Clarified HID battery support and project documentation.

## [0.1.0] - 2026-10-26

### Added

- Initial portable Windows system-tray battery monitor with Logitech HID++ 2.0 support.
- Dynamic percentage tray icon, configurable battery thresholds and color coding, and threshold notifications.
- Native diagnostics window with HID details, log copying, and GitHub issue draft support.
- Sleep and reconnect handling for supported wireless devices.
