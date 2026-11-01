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

CI also builds the tray with the optional Bluetooth feature, checks the
workspace on Rust 1.99.0, and audits dependencies:

```powershell
cargo build -p volty-tray --features bluetooth --locked
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
start with the
[architecture guide](architecture.md) and the relevant HID or Bluetooth page.
