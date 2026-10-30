# Bluetooth headphone battery support

> **Status:** The tray Bluetooth integration is opt-in and disabled in default
> builds. Hardware validation is limited to one Bluetooth Classic headset; the
> BLE GATT path has not been tested on real hardware. Do not treat Bluetooth
> support as universal. To opt in locally, run
> `cargo run --release --package volty-tray --features bluetooth`.

## Bluetooth scope

Volty reads battery percentages from connected Bluetooth devices when Windows
supplies the `System.Devices.BatteryLife` property. It checks both the
connected-device properties and present Bluetooth PnP device nodes, since
Windows can expose that property through either surface. For connected
Hands-Free Profile (HFP) devices, it also checks the RFCOMM service metadata
for the same property without opening an RFCOMM connection. It reads connected
Windows-paired Bluetooth Low Energy (BLE) devices through the standardized GATT
Battery Service and Battery Level characteristic:

- Battery Service: `0x180F`
- Battery Level characteristic: `0x2A19`

The Windows property and GATT backend do not infer percentages from advertising
data. Only values from 0 through 100 are accepted; the Windows property's
unknown sentinel (101) is treated as unavailable. Charging state remains
unknown because neither source reports it.

Volty does not initiate pairing, connect paired devices, or perform broad
Bluetooth scans. A paired device whose Windows connection status is not
`Connected` is skipped, even if Windows still exposes a stale battery
property for its PnP device node. Each asynchronous Windows Runtime operation
has a five-second timeout, and the paired-device pass has a conservative
30-second overall budget. If that budget is exhausted, already collected
readings are retained, remaining paired devices are skipped, and an explicit
warning is recorded. A device without an available Windows battery property
or GATT service is skipped; failures are logged without suppressing valid
readings from other devices or the HID backend. If Windows reports multiple
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

## Validated hardware

| Device | Transport | Result |
| --- | --- | --- |
| Baseus Bass BS2 Lite | Bluetooth Classic (HFP/A2DP) | 100%, Windows PnP battery property |

The reading comes from the headset's Hands-Free AG PnP node through the
`System.Devices.BatteryLife` property, exposed to SetupAPI as
`DEVPKEY_Device_BatteryLevel`
(`{104EA319-6EE2-4701-BD47-8DDBF425BBE5}, 2`). For this headset, neither the
WinRT connected-device property nor the HFP RFCOMM service metadata returned a
value, so the SetupAPI PnP phase is required in practice. An earlier build read
the wrong property key
(`{49cd1f76-5626-4B17-A4E8-18B4AA1A2213}, 10`), which silently produced no
readings. The corrected key was verified on live hardware.

Windows reports no charging state for this source, so the headset is shown with
a percentage and unknown charging state. This validation does not cover BLE
GATT, disconnect/reconnect behavior, low-battery notifications, or a second
headset model.

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
budget; that is a discovery strategy, not a protocol. The validated PnP reading
meets the criteria for a trustworthy Windows reading with a stable device id.
Default enablement still waits for broader coverage, especially hardware
validation of the BLE GATT path and a second headset model.

## Runtime behavior

### Device discovery

- Volty checks connected Bluetooth devices and present Bluetooth PnP nodes for
  the Windows battery-life property, and enumerates Windows-paired Bluetooth
  LE devices. PnP nodes are ignored unless Windows reports the device as
  connected. It does not pair devices or perform a broad Bluetooth discovery
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

- `volty-bluetooth` uses Windows Runtime Bluetooth/GATT APIs and reads GATT
  only while Windows reports the device as connected. Every asynchronous
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

## Hardware capture and replay

When the tray is built with the `bluetooth` feature, `volty capture` and the
Diagnose action add Bluetooth records to the same `.volty.jsonl` file used for
HID captures:

- `bluetooth_device` identifies each observed device with a capture-local
  `bt-N` id and records whether Windows exposed it as classic, PnP, or BLE.
- `bluetooth_property` keeps the raw Windows property byte, property name, and
  type before validation.
- `bluetooth_gatt` keeps the raw Battery Level payload plus the service and
  characteristic UUIDs.
- `reading` records the normalized result with `device_index` set to `null`.

Bluetooth addresses and opaque Windows device ids are never written. Friendly
names follow the capture privacy profile and are omitted by `--strict`.

`volty capture replay` works without a Bluetooth backend and without hardware:
it re-runs the recorded property or GATT bytes through the same decoders used
by the tray and fails if the normalized reading does not match. A capture made
by a Bluetooth-enabled build can therefore be verified in a default build.
Replay reproduces what Windows exposed on the captured machine; it does not
prove support for other headsets or transports.

See [Hardware capture](./hardware-capture.md) for the record format, privacy
rules, and the capture submission workflow.

## Verification and hardware validation

### Automated tests

- Battery Level decoding accepts valid values from 0 through 100 and rejects
  malformed or out-of-range values.
- Existing HID++ parsing, polling, notification, and tray behavior remains
  unchanged.
- Bluetooth capture mapping replaces opaque device ids with capture-local
  `bt-N` ids, honors both privacy profiles, and never serializes a Windows
  device id or Bluetooth address.
- Property and GATT replay re-decode synthetic evidence to the recorded
  reading, reject mismatches and unknown sentinels, and require a matching
  device record.

Validated so far:

- The Baseus Bass BS2 Lite appears through the Windows PnP battery property
  with a valid percentage.
- The Bluetooth scan completes without warnings for that headset, and the
  reading carries the stable PnP instance id as its device identity.
- A Bluetooth-enabled build records the headset as `bt-1` with a raw
  `bluetooth_property` value and a reading whose `device_index` is `null`.
  `volty capture replay` reproduces the 100% reading in both Bluetooth-enabled
  and default builds.
- The generated capture contains no Bluetooth address, opaque PnP instance id,
  or device path; only the capture-local `bt-1` id and the privacy-filtered
  friendly name appear.
- Diagnose output includes Bluetooth scan counts and reading status without
  Bluetooth addresses or device paths.

Still required:

- A live BLE GATT capture from a headset exposing Battery Service, replayed
  without hardware.
- A tested BLE headset exposing Battery Service appears in the device list
  with its reported percentage.
- Removing or disconnecting a validated headset does not hang the UI or stop
  HID monitoring; it is found again after reconnect/refresh.
- With both a supported HID peripheral and headset attached, both appear and
  the tray icon uses the lowest available percentage.
- Low-battery notification behavior works for the headset without duplicate
  alerts across polls.
- A second Bluetooth Classic headset confirms that the PnP property path is
  not specific to one device.

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
