# Hardware capture

A hardware capture is a local, machine-readable record of what Volty observed
from a HID device. It separates evidence from expectations: a bug report can
say what a device should show, while the capture contains the interface
metadata, protocol frames, extracted values, and normalized reading that Volty
actually saw. The same records are intended to become replayable regression
fixtures for protocol and decoder tests.

Captures are read-only evidence. Volty does not upload them, and creating one
does not change device settings, pairing, or configuration. The current primary
use case is the Logitech G502 X Superlight RGB; capture support is still
experimental and hardware validation remains limited.

## What is recorded

A capture can contain these kinds of records:

- **Interface metadata:** HID details such as vendor and product IDs, bus type,
  usage page and usage, and interface number.
- **Backend match:** which catalog device and protocol backend matched the
  interface, and whether resolution succeeded or was ambiguous.
- **HID++ feature table:** logical feature IDs with the runtime indexes, types,
  and versions reported by the device.
- **HID++ exchanges:** raw request and response frames, limited to an allowlist
  of read-only queries such as feature discovery and battery status.
- **Generic HID layout:** battery-related report-descriptor capabilities and the
  values extracted from them, not the full input report.
- **Normalized readings:** the source label, reported level when the device
  truly reports a percentage, charging or full state, unknown state, and
  timestamp.
- **Events and markers:** capture start and stop, discovery or read errors, and
  annotations attached to the capture.

HID++ captures are allowlisted to read-only queries. The exporter does not send
state-changing requests just to enrich a capture, and it does not record input,
button, or keystroke traffic.

## What is deliberately not recorded

A capture must not contain any of the following:

- Windows device paths or raw interface paths
- Device serial numbers
- Windows usernames or computer names
- Bluetooth addresses or pairing keys
- Keystrokes, button events, or other user input
- Full generic HID input reports; only battery-related descriptor details and
  extracted values are kept
- Custom HID++ device names, which are excluded by default

The capture file is created locally and is never uploaded automatically. Treat
it as untrusted input when reviewing or sharing it: it is evidence of observed
behavior, not executable instructions.

## Generate a capture from the tray

1. Right-click the Volty tray icon and choose **Diagnose / Report an Issue**.
2. Volty saves the text diagnostic report under `%APPDATA%\Volty\reports` and a
   timestamped `.volty.jsonl` hardware capture under
   `%APPDATA%\Volty\captures`. The Diagnose window shows the exact paths.
3. Review the capture before sharing it, then attach it to a **Hardware report**
   issue if you are reporting device support or battery behavior.

If the capture file is not created, include the text report and the error shown
by the Diagnose window in your issue.

## Command-line capture

`volty capture` writes the same read-only snapshot from a console:

```text
volty capture
volty capture --output .\g502.volty.jsonl
volty capture --strict
```

- `volty capture` writes a timestamped file under `%APPDATA%\Volty\captures` and
  prints its path plus a summary.
- `--output <path>` writes to a specific file instead.
- `--strict` omits model-level manufacturer and product strings, keeping only
  numeric identifiers and protocol evidence.

The following commands are planned and are not available in the current
release. Do not rely on them until they appear in a release or the CLI help
output.

```text
volty capture --duration 60
volty capture inspect <capture.volty.jsonl>
volty capture replay <capture.volty.jsonl>
```

- `--duration` is intended to record a session for a bounded period.
- `inspect` is intended to summarize and schema-check a capture without opening
  a device.
- `replay` is intended to feed recorded HID++ frames and generic layouts through
  the decoders without contacting hardware.

The exact syntax and output may change before these commands ship.

## Review before sharing

Open the `.volty.jsonl` file in a text editor. It is JSON Lines: one JSON object
per line, so it can be read without special tooling. Check for strings you do not
want to make public, including paths, serial numbers, usernames, computer names,
Bluetooth addresses, device names, and anything else specific to your setup.

The format is designed to exclude paths, serial numbers, usernames, computer
names, and Bluetooth addresses, but review the actual file before attaching it
to a public issue. If you want to omit model-level manufacturer and product
strings as well, generate the capture with `volty capture --strict`; the tray
export uses the standard profile. Strict mode is an extra privacy option, not a
substitute for reading the file. Do not hand-edit a capture that you want
maintainers to replay, because a modified file is no longer evidence. Remove the
file from the report and describe the concern instead.

## Submit a capture

Create an issue from the **Hardware report** template and attach the
`.volty.jsonl` file by dragging it into a comment on the issue after submitting
the form. Include:

- the device model
- the connection type (USB cable, Lightspeed receiver, Bluetooth, or other)
- the Volty version and Windows version
- what Volty showed and what you expected, or the reading from vendor software

GitHub issues are public. Attach only a capture you have reviewed and are
comfortable sharing. Nothing is uploaded automatically by Volty.

## How maintainers use captures

Captures are untrusted input. Maintainers validate the schema and bounds before
using a record, reject malformed or unknown records rather than guessing, and
never treat strings from a capture as commands or paths to open.

A capture supports two kinds of follow-up:

- **Frame replay:** run allowlisted HID++ exchanges through the same framing and
  decoding code used by the tray, and compare the semantic result (feature,
  status, level, charging state, or error).
- **Layout replay:** run recorded generic HID descriptor capabilities and
  extracted values through the generic battery layout and scaling code.

Replay checks the decoder, not the device. A successful replay means Volty
reproduces the recorded outcome; it does not prove that every device with the
same IDs behaves the same way.

Captures in the test corpus can be marked with one of these states:

| State | Meaning |
| --- | --- |
| `observed` | Recorded from hardware, but not independently confirmed or turned into an expectation. |
| `regression` | An expected outcome is encoded in a test; later changes must keep producing it. |
| `verified` | Corroborated by an external oracle, such as vendor software, an independent measurement, or repeat observations. |
| `broken` | A known failure kept for diagnosis and to prevent accidental promotion. |

A capture is never promoted to `verified` automatically. That requires an
external oracle or independent evidence beyond the capture itself.

## Format sketch

Captures use the `volty.capture/v1` schema and UTF-8 JSON Lines. The `header`
record identifies the schema version; the `footer` record closes a complete
capture. Records appear in this order when present:

| Record | Purpose |
| --- | --- |
| `header` | Capture metadata and schema version `volty.capture/v1`. |
| `interface` | HID interface metadata, without paths or serial numbers. |
| `match` | Catalog and backend match result for the interface. |
| `hidpp_features` | HID++ feature table. |
| `hid_layout` | Generic HID battery descriptor capabilities. |
| `hidpp_exchange` | One allowlisted read-only HID++ request/response exchange. |
| `hid_sample` | One extracted generic HID battery sample. |
| `reading` | One normalized device reading. |
| `event` | Capture lifecycle, discovery, or read error. |
| `marker` | Annotation or phase marker. |
| `footer` | End-of-capture summary and completion status. |

Not every capture contains every record. HID++ devices produce
`hidpp_features` and `hidpp_exchange`; generic HID devices produce `hid_layout`
and `hid_sample`. `reading`, `event`, and `marker` can appear more than once.
A parser must reject a different schema version and must not guess at records
it does not understand.

## Limitations

A capture records observed behavior on one system, with one firmware and one
connection path. It is not a certification that a device model is supported,
and it does not override the limited hardware validation described in the
README. Firmware updates, receiver slots, USB versus Bluetooth connections, and
Windows HID parsing can all change what a device exposes.

The in-memory flight recorder and the full replay corpus are future work. The
capture CLI and replay tooling are planned, not part of the current slice. Until
those exist, captures are reviewed locally and replayed by maintainers with
development tooling.
