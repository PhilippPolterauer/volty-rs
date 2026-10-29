# Generic HID battery-level support

Volty's goal is to monitor battery levels from HID peripherals across vendors
and protocols. HID describes how a device exposes reports; it does not mean
every device reports battery state in the same format. A battery reading may
come from a standardized HID report or a vendor-specific protocol, and each
format needs a supported discovery and decoding path.

**Current implementation:** two HID battery device families are registered in
`SUPPORTED_HID_DEVICES`: Logitech HID++ 2.0 and generic standard HID battery
reports. The generic path covers any HID interface whose report descriptor
declares a usable battery level (Generic Device Controls *Battery Strength*,
Battery System relative/absolute state of charge, or remaining and full-charge
capacity) and reads it through the Windows HID parser, showing the reading as
"HID Battery". A device that exposes its battery only through a vendor-defined
report still needs its own protocol adapter.

**Current use case:** The project owner's current primary use case is the
Logitech G502 X Superlight RGB; device support remains best-effort and hardware
validation is limited.

## Battery-reading flow

```text
HID peripheral
     |
     v
Enumerate HID interfaces
     |
     v
Resolve each interface against SUPPORTED_HID_DEVICES
     | no match: no device selected
     | multiple matches: ambiguity error
     | exactly one match
     |
     v
Selected device's protocol backend
     |
     v
Protocol-specific query and decoder
     |
     v
Normalized battery reading (BatteryStatus)
     |
     v
HidProvider -> BatteryProvider (DeviceId + DeviceLabel + BatteryStatus)
     |
     +-- Tray icon
     +-- Low-battery notifications
```

Protocol-specific code should stop at the decoder boundary. The rest of the
application should consume a normalized reading with a percentage only when
the device actually reports a percentage, along with any known charging state.
In the current code, `BatteryStatus` is that normalized representation.

The workspace has two user-facing paths today:

| Path | What it does | Current scope |
| --- | --- | --- |
| Tray polling | Queries a battery through the built-in API, then updates the icon and notifications. | Logitech HID++ Unified Battery (`0x1004`) and legacy Battery Status (`0x1000`), plus standard HID battery usages (Generic Device Controls *Battery Strength* and Battery System state of charge or capacity). |
| HID diagnostics CLI | Inspects interfaces and HID++ feature mappings for on-demand diagnostics. | HID++ 2.0 diagnostics; separate from tray polling. Standard HID battery descriptors are exercised through the tray path instead. |

On-demand diagnostics do not select a tray backend or add device support.
Tray polling is selected only by the device catalog and its registered
protocol backends.

## Protocol and device extension points

The HID crate separates interface-to-device matching from protocol behavior:

- `HidBatteryDevice` supplies a family name and returns its `BatteryProtocol`.
  Override `matches` when the default protocol-level interface match is too
  broad.
- `BatteryProtocol` supplies its name, interface matcher, ordered
  `device_indices`, and `query_battery`, which owns transport, response
  decoding, and conversion to `BatteryStatus`. Optional `query_device_name`
  metadata is a separate operation and must not delay a battery reading.
- `SUPPORTED_HID_DEVICES` is the single dispatch catalog used to select a device
  and its backend.

During tray polling, Volty enumerates HID interfaces and calls
`resolve_hid_device` for each one. The resolver checks the devices in
`SUPPORTED_HID_DEVICES`: no match selects no device, exactly one match selects that
device, and multiple matches produce an ambiguity error rather than letting
catalog order decide. `HidBatteryDevice::matches` defaults to the selected backend's
`supports_interface` method. The uniquely selected device supplies the backend
used for the query; Volty does not try every protocol or choose a backend
from diagnostic input.

`SUPPORTED_HID_DEVICES` currently has two entries. The Logitech HID++ 2.0
device matcher selects vendor ID `0x046d`, usage page `0xff00`, and usage
`0x0002`; after selection, runtime feature discovery and battery decoding
determine whether a supported battery feature is available. The generic HID
battery device matcher accepts only non-vendor usage pages whose report
descriptor declares a usable battery field; it probes and caches the parsed
layout per interface, and an interface without a readable battery level does
not match. The two matchers are non-overlapping: the generic matcher never
accepts usage page `0xff00` or any other vendor-defined page, while the
Logitech matcher requires the Logitech vendor ID and usage page `0xff00`.

Polling keeps every successful interface/device-index reading, so one matching
interface is a candidate rather than proof that it reports a supported battery.
The tray lists each readable device and uses the lowest reported percentage for
its battery icon.

Successful readings are forwarded to notifications immediately instead of
waiting for every candidate slot to finish scanning. Each HID device is
identified as `hid:{stable key}#{slot}`. The stable key comes from
`stable_interface_keys` (VID/PID, interface number, usage page/usage and a hash
of the serial number when the device reports one) and deliberately excludes the
Windows interface path, which changes when USB re-enumerates. Interfaces that
are indistinguishable (for example two identical receivers without serials) get
a deterministic `~N` suffix ordered by path. Volty stores the last successful
device-index order per stable key in
`%LOCALAPPDATA%\Volty\device-state.sqlite3` and tries those indexes first on
later polls. This cache only changes discovery order: unreadable or missing
state falls back to the full index scan, and successful readings remain usable
while the rest of discovery continues. Monitor flags and low-battery alert
de-duplication use the same id, so they survive re-enumeration.

Protocol names are queried only after the battery scan has delivered its
readings. Each interface/device-index name (including a missing name or lookup
failure) is cached for the active connection, so repeated polls do not issue
name requests. A failed battery query invalidates that entry to allow a
replacement device in the same receiver slot to be named.

To get a device into tray polling:

1. If the interface declares a standard HID battery usage, the generic matcher
   already selects it; no new trait implementation is needed as long as its
   report descriptor exposes a readable level.
2. If the existing Logitech matcher selects it and the existing backend already
   decodes its battery feature, no new trait implementation is needed.
3. If the wire protocol is already supported but the device/family needs its
   own interface match, implement `HidBatteryDevice`. Return the existing backend
   from `protocol()` and add the device to `SUPPORTED_HID_DEVICES`.
4. If a new battery feature or encoding uses an existing wire protocol,
   extend that backend's `query_battery` and decoding path. Add or update a
   `HidBatteryDevice` only if the interface does not already resolve to that backend.
5. For a new wire protocol, implement `BatteryProtocol`: identify compatible
   interfaces, list addresses to try, perform the protocol-specific query,
   and return a normalized `BatteryStatus`. Then implement `HidBatteryDevice` to
   select that backend and register it in `SUPPORTED_HID_DEVICES`.
6. Keep device matchers mutually exclusive for every interface. If multiple
   devices match, resolution fails with an ambiguity error rather than choosing
   a backend based on catalog order.
7. Only `level_percent` should drive the numeric tray icon and percentage
   notifications. Leave it `None` when the device reports only a raw level or
   unknown value; the normalized battery state is still available for status
   text.

No additional tray/UI trait is required: once `query_battery` returns a valid
`BatteryStatus`, the existing app updates the status text, icon when a true
percentage is available, and charging/full notification state.

`logitech_hidpp.rs` and `generic_battery.rs` are the two protocol backends. The
generic backend reads one directly addressed interface (`0xff`) through the
Windows HID parser and does not enumerate paired-device slots. The tray consumes
the protocol and device traits rather than selecting a vendor directly.

## How HID diagnostics fit

The `volty hid` CLI (the `profile` feature of `volty-hid`, enabled by the tray)
provides on-demand HID++ inspection and diagnostics. It is
separate from the tray poller: diagnostic commands do not identify a HID
device, register a backend, or change which battery query the tray runs. It
does not inspect standard HID battery descriptors; the generic path is
exercised through tray polling.

See [Adding a Logitech mouse](./adding-mice.md) for the Logitech backend's
worked example: interface selection, feature discovery, tray validation, and
the Rust changes required for a new battery encoding.

## Adding another HID battery source

For a new device, keep these responsibilities explicit:

1. **Discover the interface.** Identify the HID interface and how the target
   device is addressed. USB interface selection and a device's own paired-slot
   address are different concepts.
2. **Identify the protocol and battery fields.** Determine the request,
   response layout, units, valid range, charging states, and whether a reported
   level is actually a percentage. Do not infer percentages from arbitrary
   level codes.
3. **Add or reuse a protocol adapter.** The adapter owns framing, timeouts,
   response matching, and protocol errors. If the device uses a different
   protocol, implement its transport rather than relying on diagnostic input.
4. **Normalize the result.** Convert the response to the shared battery model
   while retaining unknown/sentinel values instead of guessing.
5. **Connect it to the tray deliberately.** Add a `HidBatteryDevice` to
   `SUPPORTED_HID_DEVICES` that selects the backend. A successful standalone
   diagnostic query alone does not add tray support.
6. **Test both layers.** Unit-test parsing with recorded frames, then validate
   with read-only hardware queries and the actual tray path.

The generic matcher already covers devices that declare a standard battery
usage, so those need no new protocol adapter. A device whose battery appears
only in a vendor-defined or otherwise non-standard report still needs its own
discovery and report/protocol adapter before the tray can monitor it. The
generic path scales a declared level from its logical range or uses remaining
and full-charge capacity; it does not guess a percentage from an opaque vendor
value.

## Validation

Frame construction, response correlation, and battery decoders can be tested
without hardware. Generic layout selection, logical-range scaling, capacity
ratios, and charging/discharging mapping are unit-tested in `generic_battery.rs`;
descriptor probing and report reads require Windows HID APIs and hardware.
On-demand diagnostics can verify a HID++ interface or feature mapping, but tray
support must be tested through its own polling path; a successful standalone
diagnostic query does not prove that the tray consumes that reading. The
current primary use case is the Logitech G502 X Superlight RGB, and hardware
validation remains limited; do not infer support for every model from one
working device.
