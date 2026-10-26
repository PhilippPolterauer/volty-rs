# Volty (`volty-rs`)

<p align="center">
  <img src="assets/banner.png" alt="Volty Banner" width="600"/>
</p>

<p align="center">
  <strong>A zero-bloat, native Windows system-tray battery monitor for Logitech Lightspeed mice written in pure Rust.</strong>
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

Modern gaming mice like the **Logitech G502 X / G502 X Plus** feature state-of-the-art Lightspeed wireless hardware, yet monitoring their battery level on Windows often forces users to run background suites like **Logitech G HUB**—consuming hundreds of megabytes of RAM and running unnecessary background telemetries.

Standard Windows battery APIs (like ACPI) only report system battery status for laptops and fail to pick up proprietary USB HID peripherals.

**Volty** solves this cleanly. It communicates directly over raw USB HID using Logitech’s native **HID++ 2.0 protocol**, rendering a dynamic numeric percentage directly in your taskbar tray with zero fluff.

### Core Philosophy

* **⚡ Low Idle Overhead:** Written in Rust with a timer-driven device worker. On the local Windows MSVC run, idle CPU time was 0.062 seconds over 20 seconds; private working set was 1.32 MB and total working set was 11.23 MB. Shared system pages account for the difference. Memory varies with Windows and system state, so only the private working set was verified below 10 MB on this host.
* **🔒 Zero Bloat:** No Electron, no heavy frameworks, no telemetry daemons, and no G HUB dependency.
* **🛡️ Low-Level Precision:** Talks straight to the Lightspeed USB dongle using raw HID++ 2.0 frame parsing.
* **🔔 Smart Notifications:** Stays out of your way and fires a native Windows toast notification only when your battery crosses critical thresholds (e.g., 20%).

---

## ✨ Features

* **Dynamic Tray Icon:** High-DPI rasterized numeric percentage rendered live inside the Windows system tray.
* **Adaptive Color Coding:** 
  * 🟢 **Green:** $> 30\%$
  * 🟡 **Yellow:** $16\% - 30\%$
  * 🔴 **Red:** $\le 15\%$ (Low Battery)
* **Threshold Alerts:** One-time native Windows toast notification per discharge cycle when battery hits warning thresholds.
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
│   │   │   ├── protocol.rs    # HID++ frame construction and errors
│   │   │   └── battery.rs     # UnifiedBattery and legacy status parsing
│   │   └── Cargo.toml
│   │
│   └── volty-tray/            # Windows binary application
│       ├── src/
│       │   ├── main.rs        # Single-instance guard
│       │   ├── app.rs         # Polling state machine and worker
│       │   ├── tray.rs        # Dynamic icon generation & context menu
│       │   └── notify.rs      # Windows toast dispatcher
│       └── Cargo.toml
│
└── Cargo.toml                 # Workspace manifest
```

The HID crate handles Logitech HID++ communication and battery-state decoding.
The Windows tray application polls it on a background worker and updates the
tray and low-battery notifications without blocking the UI.

## Quick start

Run the application on Windows from the workspace root:

```powershell
cargo run --release --package volty-tray
```

The first battery query is made at startup; subsequent polling occurs every two
minutes. Use the tray menu to refresh or quit.

Low-battery alerts default to 20% and fire once per discharge cycle. To change
the threshold, edit `%APPDATA%\Volty\config.ini` and set
`low_battery_threshold=20` to a value from 1 to 100.

## Building and testing

Prerequisites: Windows 10/11 and the stable `x86_64-pc-windows-msvc` Rust
toolchain.

```powershell
cargo build --release --package volty-tray
cargo test --workspace
```

Protocol-frame unit tests use mock byte buffers and do not require a connected
mouse. Device communication and tray behavior should be verified on Windows
with compatible Logitech Lightspeed hardware.

## License

The workspace packages declare the MIT license.
