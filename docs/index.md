# Volty

<p align="center" class="volty-logo">
  <img src="icon.png" alt="Volty-RS logo" width="220">
</p>

Volty is a Windows system-tray battery monitor for supported HID devices,
written in Rust. It reads supported Logitech HID++ and standard HID battery
reports. An experimental Bluetooth provider is available as an opt-in build
feature.

Volty is a hobby project with limited hardware validation. Device support and
battery readings are best-effort; Volty is not production-ready and is not
affiliated with Logitech.

## Start here

- [Install the latest release](installation.md)
- [See verified and experimental devices](device-support.md)
- [Configure Volty](configuration.md)
- [Troubleshoot or report a device](diagnostics.md)

## At a glance

- Windows 10/11, 64-bit x86; standalone executable, no installer.
- Battery tray icon, per-device monitoring, configurable low-battery alerts,
  estimated time remaining, and local history.
- Left-click the tray icon to open the local battery-history dashboard in a
  native WebView2 window, with a browser fallback; right-click for device stats
  and other actions.
- Hardware captures can be inspected and replayed locally; Volty never uploads
  diagnostics automatically.
- The default build monitors HID devices. Bluetooth support is experimental,
  opt-in, and has limited hardware validation.

## Technical reference

For project architecture, protocol support, and development information, see
the [technical reference](architecture.md) and its related pages in the sidebar.

Get the executable from the
[latest GitHub release](https://github.com/PhilippPolterauer/volty-rs/releases/latest).
For project changes, see the
[changelog](https://github.com/PhilippPolterauer/volty-rs/blob/main/CHANGELOG.md).
