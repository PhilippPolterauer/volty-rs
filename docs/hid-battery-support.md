# Generic HID battery-level support

Volty's goal is to monitor battery levels from HID peripherals across vendors
and protocols. HID describes how a device exposes reports; it does not mean
every device reports battery state in the same format. A battery reading may
come from a standardized HID report or a vendor-specific protocol, and each
format needs a supported discovery and decoding path.

**Current implementation:** Logitech HID++ 2.0 is the first supported
backend. Generic HID battery-report discovery and other vendor protocols are
not implemented yet. The Logitech flow is the first example, not the limit of
the intended product or documentation.

## Battery-reading flow

```text
HID peripheral
     |
     v
Enumerate HID interfaces
     |
     v
Resolve each interface against SUPPORTED_MICE
     | no match: no model selected
     | multiple matches: ambiguity error
     | exactly one match
     |
     v
Selected model's protocol backend
     |
     v
Protocol-specific query and decoder
     |
     v
Normalized battery reading
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
| Tray polling | Queries a battery through the built-in API, then updates the icon and notifications. | Logitech HID++ Unified Battery (`0x1004`) and legacy Battery Status (`0x1000`). |
| HID diagnostics CLI | Inspects interfaces and HID++ feature mappings for on-demand diagnostics. | HID++ 2.0 diagnostics; separate from tray polling. |

On-demand diagnostics do not select a tray backend or add device support.
Tray polling is selected only by the model catalog and its registered
protocol backends.

## Protocol and mouse extension points

The HID crate separates interface-to-device matching from protocol behavior:

- `MouseModel` supplies a family name and returns its `BatteryProtocol`.
  Override `matches` when the default protocol-level interface match is too
  broad.
- `BatteryProtocol` supplies its name, interface matcher, ordered
  `device_indices`, and `query_battery`, which owns transport, response
  decoding, and conversion to `BatteryStatus`. Optional `query_device_name`
  metadata is a separate operation and must not delay a battery reading.
- `SUPPORTED_MICE` is the single dispatch catalog used to select a model and
  its backend.

During tray polling, Volty enumerates HID interfaces and calls
`resolve_mouse_model` for each one. The resolver checks the models in
`SUPPORTED_MICE`: no match selects no model, exactly one match selects that
model, and multiple matches produce an ambiguity error rather than letting
catalog order decide. `MouseModel::matches` defaults to the selected backend's
`supports_interface` method. The uniquely selected model supplies the backend
used for the query; Volty does not try every protocol or choose a backend
from diagnostic input.

Today the only registered model is the Logitech HID++ 2.0 mouse. Its backend
matches vendor ID `0x046d` and usage page `0xff00`. After selection, runtime
feature discovery and battery decoding determine whether a supported battery
feature is available. Polling keeps every successful interface/device-index
reading, so one matching interface is a candidate rather than proof that it
reports a supported battery. The tray lists each readable device and uses the
lowest reported percentage for its battery icon.

Successful readings are forwarded to notifications immediately instead of
waiting for every candidate slot to finish scanning. Volty stores the last
successful device-index order per HID interface in
`%LOCALAPPDATA%\Volty\device-state.sqlite3` and tries those indexes first on
later polls. This cache only changes discovery order: unreadable or missing
state falls back to the full index scan, and successful readings remain usable
while the rest of discovery continues.

Protocol names are queried only after the battery scan has delivered its
readings. Each interface/device-index name (including a missing name or lookup
failure) is cached for the active connection, so repeated polls do not issue
name requests. A failed battery query invalidates that entry to allow a
replacement device in the same receiver slot to be named.

To get a device into tray polling:

1. If the existing matcher selects it and the existing backend already
   decodes its battery feature, no new trait implementation is needed.
2. If the wire protocol is already supported but the device/family needs its
   own interface match, implement `MouseModel`. Return the existing backend
   from `protocol()` and add the model to `SUPPORTED_MICE`.
3. If a new battery feature or encoding uses an existing wire protocol,
   extend that backend's `query_battery` and decoding path. Add or update a
   `MouseModel` only if the interface does not already resolve to that backend.
4. For a new wire protocol, implement `BatteryProtocol`: identify compatible
   interfaces, list addresses to try, perform the protocol-specific query,
   and return a normalized `BatteryStatus`. Then implement `MouseModel` to
   select that backend and register the model in `SUPPORTED_MICE`.
5. Keep model matchers mutually exclusive for every interface. If multiple
   models match, resolution fails with an ambiguity error rather than choosing
   a backend based on catalog order.
6. Only `level_percent` should drive the numeric tray icon and percentage
   notifications. Leave it `None` when the device reports only a raw level or
   unknown value; the normalized battery state is still available for status
   text.

No additional tray/UI trait is required: once `query_battery` returns a valid
`BatteryStatus`, the existing app updates the status text, icon when a true
percentage is available, and charging/full notification state.

`logitech_hidpp.rs` is currently the only protocol backend. The tray consumes
the protocol and model traits rather than selecting a vendor directly.

## How HID diagnostics fit

The `volty hid` CLI provides on-demand HID++ inspection and diagnostics. It is
separate from the tray poller: diagnostic commands do not identify a mouse
model, register a backend, or change which battery query the tray runs.

See [Adding a Logitech mouse](./adding-mice.md) for the current backend's
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
5. **Connect it to the tray deliberately.** Add a `MouseModel` to
   `SUPPORTED_MICE` that selects the backend. A successful standalone
   diagnostic query alone does not add tray support.
6. **Test both layers.** Unit-test parsing with recorded frames, then validate
   with read-only hardware queries and the actual tray path.

The first implementation supports the Logitech backend only. In particular,
there is not yet a generic parser for standard HID battery reports; adding
another HID device requires implementing its discovery and report/protocol
adapter before it can be monitored by the tray.

## Validation

Frame construction, response correlation, and battery decoders can be tested
without hardware. On-demand diagnostics can verify an interface or feature
mapping, but tray support must be tested through its own polling path; a
successful standalone diagnostic query does not prove that the tray consumes
that reading.
