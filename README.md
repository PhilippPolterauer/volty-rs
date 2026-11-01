# Volty (`volty-rs`)

<p align="center">
  <img src="docs/icon.png" alt="Volty logo" width="180">
</p>

<p align="center">
  <strong>A Windows system-tray battery monitor for supported HID devices, written in Rust.</strong>
</p>

<p align="center">
  <a href="https://github.com/PhilippPolterauer/volty-rs/actions/workflows/ci.yml"><img src="https://github.com/PhilippPolterauer/volty-rs/actions/workflows/ci.yml/badge.svg" alt="CI status"></a>
  <a href="https://github.com/PhilippPolterauer/volty-rs/actions/workflows/docs.yml"><img src="https://github.com/PhilippPolterauer/volty-rs/actions/workflows/docs.yml/badge.svg" alt="GitHub Pages status"></a>
  <a href="https://github.com/PhilippPolterauer/volty-rs/releases/latest"><img src="https://img.shields.io/github/v/release/PhilippPolterauer/volty-rs" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT license"></a>
</p>

Your mouse battery, always one glance away. Volty sits in the Windows
notification area, shows the live charge level on the tray icon, and warns you
before your device dies. It reads supported Logitech HID++ and standard HID
battery reports on Windows 10/11, and includes experimental Bluetooth battery
monitoring by default. The Baseus Bass BS2 Lite is validated; other Bluetooth
device coverage remains limited.

<p align="center">
  <img src="docs/hero.png" alt="Volty tray icon with battery tooltip, right-click menu, and low-battery notification" width="100%">
</p>

> **Project status:** Volty is a hobby project with limited hardware testing. It
> is not production-ready or affiliated with Logitech. Device support and
> readings are best-effort.

## Get started

Download the latest
[Windows release](https://github.com/PhilippPolterauer/volty-rs/releases/latest)
and run the standalone `.exe`. The native dashboard uses the Microsoft
WebView2 Runtime; if it is unavailable, Volty opens the dashboard in your
default browser instead.

**[Read the Volty documentation](https://philipppolterauer.github.io/volty-rs/)**
for installation and usage, supported devices, configuration, troubleshooting,
and development guides.

## Highlights

- **Live tray icon:** battery level and charging state at a glance; hover for
  device name, percentage, and status.
- **Right-click menu:** per-device status, refresh, configuration, log file,
  diagnostics, and start on boot.
- **Low-battery alerts:** native Windows notifications before you run dry.
- **Left-click dashboard:** local battery history and estimated remaining time
  in a native WebView2 window (browser fallback). Nothing is uploaded.

<p align="center">
  <img src="docs/dashboard.png" alt="Volty dashboard showing 47% battery, 5d 13h remaining, drain rate and history chart" width="720">
</p>

- **Customizable:** per-device controls and tray display options.
- **Diagnostics:** read-only hardware captures with local inspection and
  replay; nothing is uploaded automatically.
- **Modular Rust workspace:** separate battery, HID, Bluetooth, capture, and
  tray crates.

Device support is best-effort beyond the hardware listed in the
[device support guide](https://philipppolterauer.github.io/volty-rs/device-support.html).
Bluetooth is enabled in default builds but remains experimental with limited
hardware validation. See the
[Bluetooth validation notes](https://philipppolterauer.github.io/volty-rs/bluetooth-headphone-battery-support.html)
for current limitations.

## Build and test

Windows 10/11 and Rust 1.99 or newer are required to build the tray app.

```powershell
cargo build --release --package volty-tray --locked
cargo test --workspace --all-features --locked
```

See the [development guide](https://philipppolterauer.github.io/volty-rs/development.html)
for CI checks and hardware-testing notes.

## Project links

- [Documentation site](https://philipppolterauer.github.io/volty-rs/)
- [Releases](https://github.com/PhilippPolterauer/volty-rs/releases)
- [Changelog](CHANGELOG.md)
- [License](LICENSE)
