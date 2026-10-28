# Bluetooth headphone battery support

> **Status:** The tray Bluetooth integration is disabled by default because it
> is currently not working as intended and has not been tested. Do not rely on
> it. This document describes intended scope and implementation notes, not
> verified support. To opt in locally, run
> `cargo run --release --package volty-tray --features bluetooth`.

## Intended Bluetooth scope

Volty reads battery percentages from connected Bluetooth devices when Windows
supplies the `System.Devices.BatteryLife` property. It checks both the
connected-device properties and present Bluetooth PnP device nodes, since
Windows can expose that property through either surface. For connected
Hands-Free Profile (HFP) devices, it also checks the RFCOMM service metadata
for the same property without opening an RFCOMM connection. It reads
Windows-paired Bluetooth Low Energy (BLE) devices, including paired devices
that Windows does not report as connected, through the standardized GATT
Battery Service and Battery Level characteristic:

- Battery Service: `0x180F`
- Battery Level characteristic: `0x2A19`

The Windows property and GATT backend do not infer percentages from advertising
data. Only values from 0 through 100 are accepted; the Windows property's
unknown sentinel (101) is treated as unavailable. Charging state remains
unknown because neither source reports it.

Volty does not initiate pairing or perform broad Bluetooth scans. GATT reads
are attempted for paired devices even when Windows does not report an active
BLE connection; Windows must still be able to reach the device. Each
asynchronous Windows Runtime operation has a five-second timeout, and the
paired-device pass has a conservative 30-second overall budget. If that
budget is exhausted, already collected readings are retained, remaining
paired devices are skipped, and an explicit warning is recorded. A device
without an available Windows battery property or GATT service is skipped;
failures are logged without suppressing valid readings from other devices or
the HID backend. If Windows reports multiple
Battery Services or Battery Level characteristics for one device, Volty
reports an ambiguity rather than inventing left/right/case labels or combining
the values.

Classic Bluetooth Hands-Free Profile (HFP) battery indications and
manufacturer-specific protocols are separate follow-up work. They are not
interchangeable with GATT. Volty does not open a second HFP/RFCOMM connection:
Windows owns the active audio profile, and a competing profile connection may
fail or interfere with audio. A headphone can therefore show a battery
percentage in Windows while Volty has no non-invasive battery source for it.
Devices that expose neither the Windows battery property nor the standard GATT
service need a separately implemented protocol.

## Current architecture and integration boundary

The HID path enumerates interfaces, resolves each against
`SUPPORTED_HID_DEVICES`, and calls a `BatteryProtocol` using a `HidInterfaceInfo` and
device index. That contract is specifically for HID; a Bluetooth GATT device
should not be forced into it. Both sources meet at the `BatteryProvider` trait
in `volty-battery`, which yields a stable `DeviceId`, a `DeviceLabel` and a
`BatteryStatus`. See [`protocol.rs`](../crates/volty-hid/src/protocol.rs),
[`provider.rs`](../crates/volty-battery/src/provider.rs) and the polling flow
in [`app.rs`](../crates/volty-tray/src/app.rs).

The tray accepts multiple device rows, computes the lowest reported percentage
for its icon, and sends per-device low-battery notifications.

The implemented structure keeps the backends separate:

1. Keep `BatteryProtocol`, `HidBatteryDevice`, and `SUPPORTED_HID_DEVICES` HID-specific.
2. `volty-battery` owns the protocol-neutral normalized reading types and the
   provider trait. `volty-hid` retains HID++ decoding and returns the shared
   model.
3. `volty-bluetooth` reads the Windows battery-life property and owns GATT
   discovery and battery reads, exposed as `BluetoothProvider`. Keep Windows
   Bluetooth transport out of `volty-hid` and tray rendering. `hidapi` only
   handles HID and is not a GATT transport.
4. The tray polling worker polls every provider and merges successful
   readings. Device ids carry the source prefix (`hid:`, `bluetooth:`), so HID
   and Bluetooth notification state cannot collide.

Bluetooth stays behind the `bluetooth` feature. Its three probes (battery-life
property, SetupAPI, paired GATT) are merged by opaque device id under a shared
budget; that is a discovery strategy, not a protocol. It should only become a
default peer of HID++ once one Windows reading is trustworthy on its own and
produces a stable device id.

## Runtime behavior

### Device discovery

- Volty checks connected Bluetooth devices and present Bluetooth PnP nodes for
  the Windows battery-life property, and enumerates Windows-paired Bluetooth
  LE devices. It does not pair devices or perform a broad Bluetooth discovery
  scan.
- A device must expose GATT Battery Service `0x180F` and Battery Level
  characteristic `0x2A19`; classic HFP and vendor-specific battery reports are
  not read.

### Battery model

- The protocol-neutral reading types live in `volty-battery`; HID++ decoding
  remains in `volty-hid`.
- A valid one-byte Battery Level value from 0 through 100 is reported as a
  percentage. Charging state remains unknown because this characteristic does
  not provide that information.

### GATT reads

- `volty-bluetooth` uses Windows Runtime Bluetooth/GATT APIs. Every asynchronous
  operation has a five-second timeout and is cancelled if it expires. The
  paired-device pass also stops at its 30-second overall budget, retaining
  partial readings and warning when remaining devices are skipped.
- Missing services are skipped. Read errors are reported per device and do not
  suppress valid readings from other devices.
- Multiple matching services or characteristics are reported as ambiguous;
  values are not combined and components are not assigned invented labels.

### Polling and tray

- When built with the opt-in `bluetooth` feature, the tray poller queries HID
  and Bluetooth independently. It preserves successful readings if the other
  source fails, and keeps low-battery notification state keyed by
  source-specific device identity. Default builds poll HID only.
- Bluetooth names are shown without HID slot numbers. The existing tray list
  and lowest-percentage icon behavior are intended to apply to both sources.

## Diagnostics and privacy

- The Diagnose report identifies whether the Bluetooth integration is enabled
  in this build. Bluetooth scan counts and reading status are included only
  when the feature is enabled.
- **Discover Devices Now** triggers an immediate HID poll; it also polls
  Bluetooth only when the opt-in feature is enabled. Trace logs explain HID
  interface matches or skips and Bluetooth selector, Windows battery-property,
  and GATT Battery Service match results.
- Opaque Windows Bluetooth device IDs and addresses are retained only
  internally for stable notification identity; they are not written to logs
  or reports. HID interface paths are also excluded from discovery logs.

## Verification and hardware validation

### Automated tests

- Battery Level decoding accepts valid values from 0 through 100 and rejects
  malformed or out-of-range values.
- Existing HID++ parsing, polling, notification, and tray behavior remains
  unchanged.

Hardware verification is still required for each target headset:

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
headset models and confirming what Windows exposes for them. Any direct HFP
implementation must be explicitly tested for interaction with the Windows
audio profile before being enabled during routine polling. Add each follow-up
as a separate backend with its own tests and hardware validation; do not widen
the generic BLE claim to imply support for every Bluetooth headset.

## References

- [Windows GATT client documentation](https://learn.microsoft.com/en-us/windows/apps/develop/devices-sensors/gatt-client)
- [Windows Battery Level characteristic UUID](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.genericattributeprofile.gattcharacteristicuuids.batterylevel)
- [Bluetooth SIG Battery Service specification](https://www.bluetooth.com/specifications/specs/battery-service-1-0/)
