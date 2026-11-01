# Capture format and replay

This technical reference describes the `.volty.jsonl` capture format and how
Volty uses captures in tests. For instructions on creating, reviewing, and
sharing a capture, see [Hardware captures](hardware-capture.md).

## Replay behavior

`volty capture replay` checks that the recorded evidence can reproduce the
normalized reading. It can replay:

- **HID++ exchanges:** allowlisted requests and responses through the same frame
  parsing and battery decoding used by the app.
- **Generic HID samples:** recorded descriptor capabilities and extracted
  values through the generic battery scaling and state mapping.
- **Bluetooth values:** raw Windows battery-property bytes or GATT payloads
  through the production decoders.

Replay validates decoding, not the physical device. A successful replay does
not prove that all devices with the same model or IDs behave identically. Missing,
malformed, or out-of-range evidence can cause replay to fail.

## Record layout

Captures use the `volty.capture/v2` schema and UTF-8 JSON Lines. Each line is a
JSON object. A `header` record identifies the schema version and a `footer`
record closes a complete capture.

Normalized battery readings retain percentage provenance in the `percentage`
field: `{"kind":"exact","value":N}` is reported by the source,
`{"kind":"estimated","value":N,"method":"..."}` is an estimate, and
`{"kind":"unavailable"}` means no percentage is available. The separate
`level` and `raw_level` fields retain the coarse bucket and raw protocol value.
Capture summaries prefix estimates with `~`.

| Record | Purpose |
| --- | --- |
| `header` | Capture metadata and schema version. |
| `interface` | HID interface metadata, without paths or serial numbers. |
| `match` | Device and backend match result for the interface. |
| `bluetooth_device` | Capture-local Bluetooth device ID, kind, and filtered name. |
| `bluetooth_property` | Raw Windows battery-property value. |
| `bluetooth_gatt` | Raw GATT Battery Level payload and UUIDs. |
| `hidpp_features` | HID++ feature table. |
| `hid_layout` | Generic HID battery descriptor capabilities. |
| `hidpp_exchange` | One allowlisted read-only HID++ request/response exchange. |
| `hid_sample` | Extracted generic HID battery sample and state bits. |
| `reading` | One normalized device reading. |
| `event` | Capture lifecycle, discovery, or read error. |
| `marker` | Annotation or phase marker. |
| `footer` | End-of-capture summary and completion status. |

Not every capture contains every record. HID++ captures include feature and
exchange records; generic HID captures include layout and sample records;
Bluetooth captures include a device record and property or GATT evidence.
Bluetooth devices use capture-local `bt-N` identifiers rather than Windows
device IDs or Bluetooth addresses.

## Test corpus guidance

Fixtures in `tests/hardware` exercise captured and synthetic evidence. Captures
in the test corpus may be classified as:

| State | Meaning |
| --- | --- |
| `observed` | Recorded from hardware, but not independently confirmed or encoded as an expected result. |
| `regression` | Expected behavior is covered by a test. |
| `verified` | Corroborated by independent evidence, such as vendor software or repeated measurements. |
| `broken` | A known failure retained for diagnosis. |

A capture is not promoted to `verified` by replay alone. Before adding a real
capture to the test corpus, remove private information and confirm that the
captured behavior should become a regression expectation.
