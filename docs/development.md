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

## Native UI preview

Use PowerShell 7 from the workspace root. The script builds incrementally with
the `ui-preview` Cargo profile, then opens the requested page in the native
Volty window with a disposable fixture. The first build prepares the preview
profile's dependency cache.

```powershell
./scripts/ui-preview.ps1 History
./scripts/ui-preview.ps1 Manage -Theme Light -NoBuild
./scripts/ui-preview.ps1 Cleanup -Narrow -NoBuild -Screenshot "$env:TEMP/volty-cleanup.png"
./scripts/ui-preview.ps1 Settings -Size 1024x760 -NoBuild
./scripts/ui-preview.ps1 InvalidSettings -Theme Light -NoBuild
./scripts/ui-preview.ps1 History -Empty -NoBuild
./scripts/ui-preview.ps1 -Stop
```

Rerun the command after a Rust or layout change. Use `-NoBuild` to switch
scenarios with the already-built executable. `-Screenshot` waits for the
requested content and saves the native window as PNG. `-Size` takes logical
pixels. It sets the dashboard client area or the Settings outer window.
`-Narrow` selects 700 × 560.

The launcher restarts the preview executable for this checkout. Volty's
installed tray, dashboard, and Settings can stay open. Every launch starts
with fresh sample history, live device states, and preferences. Cleanup,
device controls, and Settings saves use the disposable profile. The startup
checkbox and test-notification command simulate their effects for UI testing.

On the Windows development machine, the first preview build took about
38 seconds. Subsequent Rust edits rebuilt in about 4 seconds, and the
tested `-NoBuild` scenarios reached their capture-ready state in
0.25–0.61 seconds.

For a visual change, rebuild and capture the affected scenario. For fixture
or isolation changes, also run the focused tests. Run the workspace checks
above before handing off a set of changes.

```powershell
cargo test -p volty-tray --features ui-preview --locked dashboard::preview
```

### Manage data

Manage data uses the native dashboard shell, navigation, palette, typography, and DPI scaling. It shares Battery history’s device icons, series colors, battery indicator, table cell layout, and row highlighting. The device picker now uses the same inventory and selected identity as the other pages.

Settings and Manage data share the responsive settings row, text field, and toggle. Manage data also reuses the dashboard’s background workers, refresh signals, and confirmation dialog. The dialog now shows the retention period, affected counts and device names, and the permanence warning. Cleanup rechecks the affected device identities, reading counts, and time bounds inside the deletion transaction.

The new page-specific pieces are the searchable, sortable inventory table, device detail and cleanup layouts, and overview counters. The diagnostic list uses the supplied file icon and the existing file deletion operations. Inactive-device cleanup removes history. Forget device also clears stored monitoring, recording, and tray preferences.

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
