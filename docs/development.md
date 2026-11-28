# Build and test

## Requirements

Build the tray application on Windows 10/11 with the `x86_64-pc-windows-msvc`
Rust toolchain. The minimum supported Rust version is 1.99; stable Rust is
recommended. The release executable is written to `target\release\volty.exe`.

## Build and validate

From the workspace root:

```powershell
cargo build --release --package volty-tray --locked
cargo test --workspace --all-features --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

CI also builds the tray with its default Bluetooth feature, checks the
workspace on Rust 1.99.0, and audits dependencies:

```powershell
cargo build -p volty-tray --locked
cargo check --workspace --all-features --locked
cargo audit --deny warnings
```

The audit checks the shared lockfile and fails on vulnerabilities and advisory warnings for either platform.

Protocol frame tests use mock byte buffers and do not need connected hardware.
Device communication, Windows HID parsing, Bluetooth discovery, and tray
behavior need Windows and compatible hardware. A successful diagnostic command
alone does not establish that the tray polls a device; verify it through the
normal tray path.

## Linux desktop beta

On Debian or Ubuntu, install `pkg-config`, `libudev-dev`, and `libdbus-1-dev`.
Use a normal desktop account to check hardware access. The status command also
runs in a terminal without a graphical session.

```sh
cargo build -p volty-tray --locked
target/debug/volty status
target/debug/volty tray
target/debug/volty capture inspect tests/hardware/g502x-plus-unified.volty.jsonl
target/debug/volty capture replay tests/hardware/g502x-plus-unified.volty.jsonl
```

Run `cargo test --workspace --all-features --locked` with `dbus-daemon` and
`busctl` installed. The Linux desktop integration test starts a private bus
with a synthetic BlueZ headset and checks the CLI, running tray, history
database, and dashboard export. Ubuntu packaging CI also installs, upgrades,
and removes the built package. For physical device tests, follow the
[Linux capture procedure](hardware-capture.md#linux-snapshots).

## README visuals

The renderer test generates temporary PNG previews for the README hero and tray
icon comparison without launching the tray app:

```powershell
cargo test -p volty-tray generates_readme_graphics_from_the_icon_renderer -- --nocapture
```

To refresh the checked-in images after inspecting the previews, direct the same
test to `docs`:

```powershell
$env:VOLTY_VISUAL_OUTPUT_DIR = (Resolve-Path docs).Path
cargo test -p volty-tray generates_readme_graphics_from_the_icon_renderer -- --nocapture
Remove-Item Env:VOLTY_VISUAL_OUTPUT_DIR
```

The test uses the Windows Segoe UI font and the current dashboard preview.

## Hardware reports and contributions

When reporting a device, attach a reviewed `.volty.jsonl` capture to the
[Hardware report issue](https://github.com/PhilippPolterauer/volty-rs/issues/new?template=hardware-report.yml).
The [capture guide](hardware-capture.md) explains what is recorded and how to
review it. Device support is added to the documented support table only after a
real reading and offline replay are confirmed.

Use read-only diagnostics when inspecting hardware. Avoid arbitrary
`volty hid raw` requests and state-changing commands when validating battery
support. For passive report-level protocol discovery, use the finite,
explicitly opted-in `volty hid sniff` trace; it records raw input data and
cannot observe traffic sent by another process. For implementation details,
start with the [architecture guide](architecture.md) and
[HID support and adding devices](hid-battery-support.md). Bluetooth availability
and tested hardware are summarized in the [device support guide](device-support.md).

## Release

Update the workspace version, move `[Unreleased]` changelog entries under the
release version and date, then tag the commit `vX.Y.Z`. The Windows release
workflow checks the tag against the tray package version and publishes the
standalone executable and its SHA-256 checksum. The tray updater expects those
versioned asset names.
