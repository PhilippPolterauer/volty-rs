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
Interface discovery and protocol selection
     |
     +-- Generic HID battery reports (future backend)
     |
     +-- Logitech HID++ 2.0 (current backend)
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

The HID crate separates protocol behavior from mouse-family matching:

- `BatteryProtocol` describes interface matching, protocol-specific device
  addresses, and a query that returns the normalized `BatteryStatus`.
- `MouseModel` names a supported mouse family and selects its protocol.
- `SUPPORTED_PROTOCOLS` and `SUPPORTED_MICE` are the explicit registries used
  to discover implementations.

`logitech_hidpp.rs` is the first and currently only protocol backend. Its
family matcher recognizes Logitech HID++ interfaces; runtime feature
discovery then confirms whether a device exposes one of the supported battery
features. Additional protocols should add their own implementation module and
registry entry, while additional mouse families can reuse an existing
protocol. The tray consumes these traits rather than selecting a vendor
directly.

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

Profiles currently require an explicit file path and device/interface
selection. They do not identify a model, choose a protocol backend, or tell
the tray which profile to run. A future cross-protocol profile format would
need protocol-specific command types and adapters rather than assuming that
all HID devices speak HID++.

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
5. **Connect it to the tray deliberately.** Add the battery query to the
   built-in polling path, or implement a reviewed, read-only profile registry
   and matching policy. The CLI profile runner is not that registry.
6. **Test both layers.** Unit-test parsing with recorded frames, then validate
   with read-only hardware queries and the actual tray path.

The first implementation supports the Logitech backend only. In particular,
there is not yet a generic parser for standard HID battery reports; adding
another HID device requires implementing its discovery and report/protocol
adapter before it can be monitored by the tray.

## Shipping profile files with the executable

`volty hid run --profile FILE` reads the TOML file from that path at runtime.
The sample profile is in the repository at
[`crates/volty-hid/examples/unified-battery.toml`](../crates/volty-hid/examples/unified-battery.toml).
The current [release workflow](../.github/workflows/release.yml) publishes
only `volty.exe` and its SHA-256 checksum; it does **not** ship profile files.

There are two packaging approaches:

| Approach | Result | Cost |
| --- | --- | --- |
| Embed profiles in the executable | One standalone executable; profiles are compiled into the binary and selected by a built-in name. | Requires a rebuild and release to add or update a profile, plus CLI support for built-in names. |
| Ship a sidecar profile directory | An archive contains the executable and a `profiles\` tree; the CLI is given the profile path. | Profiles can be changed independently, but the release workflow must package the tree and the CLI currently requires an explicit path. |

For adding device profiles without recompiling, use the sidecar approach. A
future release archive could have this layout:

```text
Volty/
  volty.exe
  profiles/
    hidpp/
      unified-battery.toml
```

With today's CLI, the command would name the file explicitly:

```powershell
.\volty.exe hid run --profile .\profiles\hidpp\unified-battery.toml --command battery-status --device-index 0x01
```

This layout does not make profiles auto-discoverable and does not change tray
polling. To let the tray use sidecar profiles, Volty would additionally need
a validated registry, device/protocol matching metadata, and a constrained
read-only battery-command interface. It should not execute every profile
found beside the executable; state-changing commands must remain explicitly
authorized for interactive diagnostics.

## Validation boundaries

Profile validation, frame construction, response correlation, and decoders
can be tested without hardware. A read-only profile run verifies that a
specific command works on a selected interface and device. Tray support must
then be tested through its own polling path; a successful profile run alone
does not prove that the tray consumes that command.
