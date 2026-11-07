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
cargo audit
```

The audit reports informational notices such as unmaintained crates and
unsound APIs, but only known vulnerabilities fail the check. Some
platform-specific dependencies are included in `Cargo.lock` even when they
are not built for Windows.

Protocol frame tests use mock byte buffers and do not need connected hardware.
Device communication, Windows HID parsing, Bluetooth discovery, and tray
behavior need Windows and compatible hardware. A successful diagnostic command
alone does not establish that the tray polls a device; verify it through the
normal tray path.

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
