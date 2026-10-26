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
| HID profile CLI | Validates and runs an explicitly selected command profile, returning raw frames and decoded output. | HID++ 2.0 only; diagnostic/test path, not used by the tray. |

The tray does not load TOML profiles. A profile that successfully reads a
battery value is useful evidence, but it does not automatically add that
device or protocol to tray polling.

## Protocol and mouse extension points

The HID crate separates interface-to-device matching from protocol behavior:

- `MouseModel` supplies a family name and returns its `BatteryProtocol`.
  Override `matches` when the default protocol-level interface match is too
  broad.
- `BatteryProtocol` supplies its name, interface matcher, ordered
  `device_indices`, and `query_battery`, which owns device discovery,
  transport, response decoding, and conversion to `BatteryStatus`.
- `SUPPORTED_MICE` is the single dispatch catalog used to select a model and
  its backend.

During tray polling, Volty enumerates HID interfaces and calls
`resolve_mouse_model` for each one. The resolver checks the models in
`SUPPORTED_MICE`: no match selects no model, exactly one match selects that
model, and multiple matches produce an ambiguity error rather than letting
catalog order decide. `MouseModel::matches` defaults to the selected backend's
`supports_interface` method. The uniquely selected model supplies the backend
used for the query; Volty does not try every protocol or choose a backend
from a profile.

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

## How profiles fit

A profile is a declarative command description, not a device driver. The
current TOML schema describes HID++ operations, including a logical feature
ID, operation, function, report format, request parameters, timeout, access
class, and response decoder. The executor resolves a logical feature ID to
the device's runtime index when it runs; that index is never a profile
constant.

The command path is:

```text
TOML file -> strict validation -> command executor -> selected protocol
          -> correlated response -> decoder -> result and raw frames
```

Profiles are selected explicitly for the diagnostic CLI, either by built-in
profile ID or custom file path, along with a device/interface selection. They
do not identify a mouse model, dispatch the tray to a protocol backend, or
tell the tray which command to run. The current profile commands and runner are
HID++ 2.0-specific: `volty hid run` opens an HID++ session, so profile files
do not select a protocol. Supporting diagnostic commands for a different
protocol would require protocol-specific command types and a matching
transport, not just a profile field.

See [Adding a Logitech mouse](./adding-mice.md) for the current backend's
worked example: interface selection, feature discovery, read-only profile
testing, and the Rust changes required for a new battery encoding.

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
   protocol, a profile file alone cannot provide that transport.
4. **Normalize the result.** Convert the response to the shared battery model
   while retaining unknown/sentinel values instead of guessing.
5. **Connect it to the tray deliberately.** Add a `MouseModel` to
   `SUPPORTED_MICE` that selects the backend. A successful diagnostic profile
   run alone does not add tray support.
6. **Test both layers.** Unit-test parsing with recorded frames, then validate
   with read-only hardware queries and the actual tray path.

The first implementation supports the Logitech backend only. In particular,
there is not yet a generic parser for standard HID battery reports; adding
another HID device requires implementing its discovery and report/protocol
adapter before it can be monitored by the tray.

## Built-in and custom profiles

The `volty-hid` crate includes built-in protocol profiles from
[`crates/volty-hid/protocols/`](../crates/volty-hid/protocols/). Its build
script embeds every `.toml` file in that directory tree into the executable,
so releases need no sidecar profile files. A profile's built-in ID is its
relative path without the `.toml` extension; for example,
[`logitech-hidpp/unified-battery`](../crates/volty-hid/protocols/logitech-hidpp/unified-battery.toml).
Run `volty hid profiles` to list IDs, then use the `builtin:` prefix to select
one:

```powershell
.\volty.exe hid profiles
.\volty.exe hid run --profile builtin:logitech-hidpp/unified-battery --command battery-status --device-index 0x01
```

Custom profiles remain supported and are read from the specified path at
runtime. They use the same validated TOML schema and do not need to be copied
into the executable:

```powershell
.\volty.exe hid run --profile .\battery.toml --command battery-status --device-index 0x01
```

The README's [HID diagnostics section](../README.md#hid-diagnostics) explains
how to list and run built-in profiles. Built-in and custom profiles are
explicit diagnostic CLI inputs only: the tray does not load them, and they do
not select or register its backend. Tray support is selected through
`SUPPORTED_MICE` in code.

## Validation boundaries

Profile validation, frame construction, response correlation, and decoders
can be tested without hardware. A read-only profile run verifies that a
specific command works on a selected interface and device. Tray support must
then be tested through its own polling path; a successful profile run alone
does not prove that the tray consumes that command.
