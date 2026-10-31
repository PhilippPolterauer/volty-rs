# Reddit announcement — Volty v0.4.0

**Subreddit:** r/vibecoding

## Title

I built a Windows tray battery monitor in Rust — v0.4.0 adds history, stale detection, and needs weird hardware testers

## Body

Hey r/vibecoding,

I built **Volty**, a small Windows system-tray battery monitor in Rust. It started
because I wanted one clean battery readout for my Logitech mouse without opening
Logitech's software. v0.4.0 is out.

**What it does**

- HID++ battery monitoring for supported Logitech devices, generic HID battery
  support, and opt-in Bluetooth
- Multi-threshold alerts at 20/10/5 by default, once per discharge cycle, with an
  optional repeat cooldown
- A local SQLite history database and a `volty history` command with table and
  CSV output
- Battery-glyph or numeric tray icon, with an optional pinned device
- Last-seen ages and `stale` markers, so a sleeping or disconnected mouse stops
  looking alive
- One standalone `.exe`, MIT licensed, no installer, no telemetry, no uploads

**What's still rough**

- Hardware validation is thin. The Logitech G502 X Superlight RGB is the only
  fully verified HID++ device.
- The new Battery Voltage (`0x1001`) fallback is implemented and replayed from a
  synthetic fixture, but it has not been tested on a real voltage-only device.
- Bluetooth BLE GATT is implemented but not hardware-validated.
- Windows 10/11 only. No installer, no Mac, no Linux.

This is a hobby project, partly built with AI agents and reviewed by me. Anything
I couldn't validate on real hardware is labeled experimental in the README and
changelog rather than presented as working.

**Feedback wanted**

If you have a Logitech device that only exposes HID++ Battery Voltage (`0x1001`),
or a BLE peripheral with a standard battery service, I'd love a hardware capture.
There's a **Hardware report** issue template, and captures are local files you can
inspect before attaching.

General feedback on the tray behavior, history output, or alert defaults is also
welcome.

Repo: https://github.com/PhilippPolterauer/volty-rs
Release: https://github.com/PhilippPolterauer/volty-rs/releases/tag/v0.4.0

**Disclosure:** this is my hobby project. No monetization, MIT licensed, and I'm
not affiliated with Logitech.
