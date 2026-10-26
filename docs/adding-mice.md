# First backend example: Logitech HID++ mouse

This is Volty's first backend example: it covers diagnosing a Logitech HID++
mouse and deciding whether it already works or needs a code change. For the
generic HID battery-level goal, component model, profile format, and packaging
options, see [Generic HID battery-level support](./hid-battery-support.md).

## What is supported today

The tray polls Logitech HID++ interfaces and reads these standard battery
features:

| Feature ID | Feature | Tray behavior |
| --- | --- | --- |
| `0x1004` | Unified Battery | Reads capabilities and battery status; uses the reported percentage only when the device advertises percentage support. |
| `0x1000` | Legacy Battery Status | Used when Unified Battery is not present. Its level is treated as device-specific, not as a percentage. |

Feature runtime indexes are discovered from Root for each query; do not
hard-code an index shown by one device. If a mouse implements either standard
feature correctly, it should work without adding a model-specific profile or
changing Rust code.

TOML profiles are currently a diagnostic and testing facility for
`volty hid run`. The tray does **not** load profiles. A profile that reads or
decodes a new feature is not, by itself, enough to make that feature appear in
the tray. Also, the tray currently returns the first successful battery
reading it finds while scanning detected interfaces and device indexes
`0xFF`, `1`–`6`; it does not show a separate reading for every mouse paired to
a receiver.

## 1. Identify the HID++ interface and device index

From the repository root, list candidate interfaces:

```powershell
cargo run --release --package volty-tray -- hid inspect --vid 0x046d
```

Choose an interface that exposes a Logitech vendor HID collection. Use its
VID, PID, usage page, usage, and interface number to narrow later commands to
exactly one interface. Do not confuse the USB interface number with the
HID++ device index:

- `0xFF` addresses a directly connected device or receiver endpoint.
- `1`–`6` address paired-device slots behind a receiver.

Discover the features for the mouse's device index (replace the example PID,
usage, and index with the values for your setup):

```powershell
cargo run --release --package volty-tray -- hid features --device-index 0x01 --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002
```

If the selector matches more than one interface, add `--interface` using the
interface number reported by `inspect`. Look for logical feature `0x1004` or
`0x1000`; the output also shows the runtime index and version assigned on that
device.

## 2. Test Unified Battery with the read-only profile

The included profile tests Unified Battery (`0x1004`, function 1) and decodes
its status:

```powershell
cargo run --release --package volty-tray -- hid run --profile .\crates\volty-hid\examples\unified-battery.toml --command battery-status --device-index 0x01 --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002
```

Use the correct device index and interface selectors from step 1. A successful
run prints the decoded result and raw request/response frames. The profile
engine resolves the feature's runtime index when it runs the command.

This example only tests `0x1004`. If discovery shows only legacy Battery
Status (`0x1000`), the tray already has a built-in reader for it; test the
mouse in the tray after confirming that it responds to the standard feature.
Do not interpret the legacy raw level as a percentage.

## 3. Decide whether code changes are needed

### The mouse has `0x1004` or `0x1000`

No model-specific battery change should be necessary. Connect the mouse or its
receiver and let the tray poll. It scans detected Logitech interfaces and
device indexes `0xFF`, `1`–`6`, stopping at the first successful battery
response.

If the profile succeeds but the tray does not show a reading, check that the
tray is using the same interface and paired-device slot that responded in the
diagnostic. Include the `features` output and the profile's TX/RX result when
investigating a discrepancy. Diagnostic output can contain local device
details; review it before sharing.

### The battery is exposed through another feature or encoding

First use a read-only profile to establish the logical feature ID, function,
request parameters, response fields, and whether the level is genuinely a
percentage. The profile engine supports raw data and common integer, flag,
enum, and Unified Battery decoders. Keep a successful profile as a reproducible
protocol example, but remember it does not change tray polling.

To make a non-standard battery feature available in the tray:

1. If the mouse uses a new transport or wire protocol, add a separate protocol
   implementation module and implement `BatteryProtocol`. Do not put a second
   vendor's framing in `logitech_hidpp.rs`.
2. Implement `MouseModel` for the mouse family, returning the appropriate
   protocol. Add both implementations to the explicit protocol and mouse
   registries in `crates/volty-hid/src/protocol.rs`.
3. For another Logitech HID++ feature, keep feature discovery and request
   handling within the HID++ backend, then parse its response into the shared
   `BatteryStatus` representation in
   [`crates/volty-hid/src/battery.rs`](../crates/volty-hid/src/battery.rs).
   Set `level_percent` only when the device's protocol guarantees that value
   is a percentage; preserve unknown status values rather than guessing.
4. Add unit tests using captured response frames, including malformed and
   unknown values. Keep hardware tests read-only.
5. Run the workspace checks below and verify the new mouse in the tray.

`BatteryProtocol` returns the normalized battery model consumed by the tray;
protocol-specific frames and errors should not leak into tray code. The
registry is the only place that should make a new mouse family available to
polling.

## Validate changes

From the repository root:

```powershell
cargo fmt --check
cargo test --workspace --locked
cargo check --workspace --all-targets --locked
```

Then build and verify the tray with the target mouse or receiver on Windows:

```powershell
cargo build --release --package volty-tray --locked
```

Use named read-only profile commands for diagnosis. Avoid arbitrary `hid raw`
requests and state-changing commands when testing battery support.
