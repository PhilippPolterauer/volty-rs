# First backend example: Logitech HID++ mouse

This is Volty's first backend example: it covers diagnosing a Logitech HID++
mouse and deciding whether it already works or needs a code change. For the
generic HID battery-level goal and backend architecture, see
[Generic HID battery-level support](./hid-battery-support.md).

## What is supported today

The tray polls Logitech HID++ interfaces and reads these standard battery
features:

| Feature ID | Feature | Tray behavior |
| --- | --- | --- |
| `0x1004` | Unified Battery | Reads capabilities and battery status; uses the reported percentage only when the device advertises percentage support. |
| `0x1000` | Legacy Battery Status | Used when Unified Battery is not present. Its level is treated as device-specific, not as a percentage. |

Feature runtime indexes are discovered from Root for each query; do not
hard-code an index shown by one device. If a mouse implements either standard
feature correctly, it should work without model-specific code.

The tray queries its built-in battery features directly. It keeps each
successful interface/device-index reading and shows every readable device;
on-demand diagnostics do not select or register tray backends.

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

## 2. Inspect feature mappings and verify tray behavior

Use the on-demand feature command to check which battery features the device
advertises and what runtime indexes Root assigns:

```powershell
cargo run --release --package volty-tray -- hid features --device-index 0x01 --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002
```

Look for logical feature `0x1004` or `0x1000`. The tray already queries both:
Unified Battery uses its reported percentage only when supported, while the
legacy raw level is never treated as a percentage. Then verify the reading in
the tray; the diagnostic feature listing alone does not read battery status.

## 3. Decide whether code changes are needed

### The mouse has `0x1004` or `0x1000`

No model-specific battery change should be necessary. Connect the mouse or its
receiver and let the tray poll. It scans detected Logitech interfaces and
device indexes `0xFF`, `1`–`6`, retaining each successful battery response.

If the tray does not show a reading, check that it is using the same interface
and paired-device slot shown by the feature listing. Diagnostic output can
contain local device details; review it before sharing.

### The battery is exposed through another feature or encoding

Use feature discovery and a read-only hardware query to establish the logical
feature ID, function, request parameters, response fields, and whether the
level is genuinely a percentage. Record observed request/response frames for
tests, but do not treat a standalone diagnostic result as tray support.

To make a non-standard battery feature available in the tray:

1. If the mouse uses a new transport or wire protocol, add a separate protocol
   implementation module and implement `BatteryProtocol`. Do not put a second
   vendor's framing in `logitech_hidpp.rs`.
2. Implement `MouseModel` for the mouse family, returning the appropriate
   protocol. Add the model to `SUPPORTED_MICE` in
   `crates/volty-hid/src/protocol.rs`; this is the dispatch catalog. There is
   no separate protocol registry. Keep matchers specific because overlapping
   matches are reported as ambiguous and are not polled.
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

Implement `query_device_name` only when the protocol exposes a useful display
name. The poller requests it after delivering the first battery reading and
caches the result, including a missing name or lookup failure, for that active
device connection. A battery-query failure invalidates the cache so a
replacement device in the same receiver slot can be named.

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

Use read-only diagnostics when inspecting hardware. Avoid arbitrary `hid raw`
requests and state-changing commands when testing battery support.
