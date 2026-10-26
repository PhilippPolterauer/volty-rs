# Bluetooth headphone battery support plan

## Goal

Extend Volty to show battery readings reported by supported Bluetooth
headphones, while keeping its existing Logitech HID++ support intact.

This plan interprets "headphones that announce their power" as headphones that
make a battery reading available to Windows. The first implementation should
support the standardized Bluetooth Low Energy (BLE) Battery Service, not infer
battery from voice prompts or assume every Bluetooth headset exposes the same
information.

## Recommended MVP scope

Support paired BLE headphones that Windows can reach through GATT and that
expose:

- Battery Service: `0x180F`
- Battery Level characteristic: `0x2A19`

Discover the service and read its characteristic through Windows Bluetooth
APIs. Do not assume the percentage is present in the BLE advertising packet.
Treat a valid one-byte value from 0 through 100 as a percentage. Treat missing,
malformed, or out-of-range values as unavailable, and do not infer charging
state unless the device or another reliable API reports it.

The MVP should not initiate pairing or perform broad background scans. Start
with devices Windows already knows about and can currently reach; confirm the
exact enumeration behavior in the hardware prototype before committing to an
API shape.

Classic Bluetooth Hands-Free Profile (HFP) battery indications and
manufacturer-specific protocols are separate follow-up work. They are not
interchangeable with GATT, and the MVP should not claim support for headphones
that expose neither the standard GATT service nor an implemented vendor
protocol.

## Current architecture and integration boundary

The current HID path enumerates interfaces, resolves each against
`SUPPORTED_MICE`, and calls a `BatteryProtocol` using a `HidInterfaceInfo` and
device index. That contract is specifically for HID; a Bluetooth GATT device
should not be forced into it. See [`protocol.rs`](../crates/volty-hid/src/protocol.rs)
and the polling flow in [`app.rs`](../crates/volty-tray/src/app.rs).

The tray already accepts multiple device rows, computes the lowest reported
percentage for its icon, and sends per-device low-battery notifications. The
shared reading type currently lives in the HID crate, however, and the polling
identity is an HID interface path plus a device index. See
[`battery.rs`](../crates/volty-hid/src/battery.rs) and
[`app.rs`](../crates/volty-tray/src/app.rs).

Recommended structure:

1. Keep `BatteryProtocol`, `MouseModel`, and `SUPPORTED_MICE` HID-specific.
2. Extract the normalized battery model (`BatteryStatus`, `BatteryLevel`,
   `BatteryState`, and source metadata) into a small protocol-neutral
   `volty-battery` crate. Keep HID++ decoding in `volty-hid`; have both
   backends return the shared model.
3. Add a `volty-bluetooth` crate for Windows GATT discovery and battery reads.
   Keep Windows Bluetooth transport out of `volty-hid` and tray rendering.
   Use Windows Runtime Bluetooth/GATT APIs; `hidapi` only handles HID and is
   not a GATT transport.
4. Have the tray polling worker call the existing HID query and the new
   Bluetooth query, then merge their readings. Keep the orchestration simple:
   separate source-specific query functions are sufficient; a dynamic plugin
   registry is not needed for two backends.

## Implementation steps

### 1. Prove the Windows and device behavior

- Select at least one target headset and verify whether it exposes the BLE
  Battery Service while paired and connected.
- Prototype Windows device enumeration, GATT service discovery, characteristic
  reads, and behavior when the device is disconnected or the service is
  unavailable.
- Distinguish standard GATT support from classic HFP or vendor-specific
  reporting before setting the supported-device claim.
- Record Windows versions and device/firmware versions used for the test.

### 2. Create a shared battery model

- Add `volty-battery` to the workspace and move the normalized reading types
  out of `volty-hid/src/battery.rs`; leave protocol-specific HID++ parsers in
  `volty-hid`.
- Add a Bluetooth GATT source identifier to the normalized source metadata.
- Represent battery percentage as a percentage only when the source reports a
  valid value. Leave charging state unknown for Battery Service readings
  unless a reliable source supplies it.
- Update `volty-hid` and the tray to consume the shared types.

### 3. Implement the BLE GATT backend

- Add `volty-bluetooth` and its Windows-only Bluetooth/GATT dependencies.
- Enumerate eligible Windows-known Bluetooth devices and identify those
  reachable through GATT.
- Discover Battery Service `0x180F`, then read Battery Level `0x2A19`.
- Bound asynchronous operations with timeouts so a missing or sleeping headset
  cannot stall polling. Treat disconnects and stale GATT services as
  recoverable per-device failures.
- If a headset exposes multiple battery services, report separate components
  only when Windows provides a reliable way to identify them; do not invent
  left/right/case labels or silently combine ambiguous values.

### 4. Merge Bluetooth readings into polling and the tray

- Replace the HID-only polling result with a source-neutral batch that can
  contain readings and per-source warnings. A Bluetooth failure must not hide
  valid HID readings, and vice versa.
- Replace the HID-only `DeviceIdentity` (interface path plus index) with a
  source-tagged identity suitable for both backends. Use a stable opaque
  Windows device identity internally, together with a component identifier
  when needed; do not log raw Bluetooth addresses or device IDs.
- Keep notifications keyed per physical device/component so one headset does
  not inherit another device's low-battery notification state.
- Reuse the existing poll/retry intervals initially. Do not add Bluetooth
  settings until there is a demonstrated user need.
- Display Bluetooth device names without fake HID slot numbers. The existing
  tray list and lowest-percentage icon behavior should continue to work.

### 5. Update diagnostics and user-facing text

- Report the source and a useful failure reason, such as service unavailable,
  read timeout, or device disconnected.
- Include Bluetooth support/read status in diagnostics, but omit raw Bluetooth
  addresses and system device paths.
- Change mouse-only status text and product descriptions to device-neutral
  wording where applicable.
- Update the architecture and supported-device sections in
  [`README.md`](../README.md), and link this plan from the relevant battery
  support documentation.

## Tests and acceptance criteria

### Automated tests

- Battery Level decoding accepts valid values from 0 through 100 and rejects
  malformed or out-of-range values.
- GATT service/characteristic absence, access errors, timeouts, and disconnects
  are surfaced as Bluetooth-source failures rather than fabricated readings.
- Poll aggregation preserves successful readings when another backend fails,
  and reports an aggregate error only when no source provides a reading.
- Device identities keep notification state isolated between HID devices,
  Bluetooth devices, and distinct headphone components.
- Existing HID++ parsing, polling, notification, and tray behavior remains
  unchanged.

### Hardware acceptance

- A tested BLE headset exposing Battery Service appears in the device list
  with its reported percentage.
- Removing or disconnecting that headset does not hang the UI or stop HID
  monitoring; it is found again after reconnect/refresh.
- With both a supported HID peripheral and headset attached, both appear and
  the tray icon uses the lowest available percentage.
- Low-battery notification behavior works for the headset without duplicate
  alerts across polls.
- Diagnostics provide enough information to investigate failures without
  exposing Bluetooth addresses or device paths.

## Follow-up scope

Only pursue HFP or vendor-specific support after collecting concrete target
headset models and confirming what Windows exposes for them. Add each follow-up
as a separate backend with its own tests and hardware validation; do not widen
the generic BLE claim to imply support for every Bluetooth headset.

## References

- [Windows GATT client documentation](https://learn.microsoft.com/en-us/windows/apps/develop/devices-sensors/gatt-client)
- [Windows Battery Level characteristic UUID](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.genericattributeprofile.gattcharacteristicuuids.batterylevel)
- [Bluetooth SIG Battery Service specification](https://www.bluetooth.com/specifications/specs/battery-service-1-0/)
