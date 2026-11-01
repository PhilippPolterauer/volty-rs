# Hardware capture

A hardware capture is a local, machine-readable record of what Volty observed
from a HID device or a Bluetooth battery source. It separates evidence from
expectations: a bug report can say what a device should show, while the capture
contains the interface or device metadata, protocol frames, raw property
values, extracted values, and normalized reading that Volty actually saw. The
same records can be replayed locally as regression fixtures for protocol and
decoder tests.

Captures are read-only evidence. Volty does not upload them, and creating one
does not change device settings, pairing, or configuration. The current primary
use case is the Logitech G502 X Superlight RGB; Bluetooth recording is
available only in opt-in `bluetooth` builds, and capture support overall is
still experimental with limited hardware validation.

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
- **Generic HID layout:** battery-related report-descriptor capabilities, the
  values extracted from them, and the charging or discharging state bits, not
  the full input report.
- **Bluetooth devices:** a capture-local `bt-N` identifier, the Windows surface
  that exposed the device (classic, PnP, or BLE), and the privacy-filtered
  friendly name.
- **Bluetooth property evidence:** the raw Windows battery-property byte before
  range validation, together with the property name and type.
- **Bluetooth GATT evidence:** the raw GATT Battery Level payload before length
  or range validation, together with the service and characteristic UUIDs.
- **Normalized readings:** the source label, reported level when the device
  truly reports a percentage, charging or full state, unknown state, and
  timestamp.
- **Events and markers:** capture start and stop, discovery or read errors, and
  annotations attached to the capture.

HID++ captures are allowlisted to read-only queries. The exporter does not send
state-changing requests just to enrich a capture, and it does not record input,
button, or keystroke traffic. Bluetooth recording only reads Windows device
properties and the standard GATT Battery Service; it does not pair devices,
open an HFP/RFCOMM connection, or perform a broad Bluetooth scan.

## What is deliberately not recorded

A capture must not contain any of the following:

- Windows device paths or raw interface paths
- Device serial numbers
- Windows usernames or computer names
- Bluetooth addresses, pairing keys, or opaque Windows Bluetooth device ids
- Keystrokes, button events, or other user input
- Full generic HID input reports; only battery-related descriptor details and
  extracted values are kept
- Custom HID++ device names, which are excluded by default

The capture file is created locally and is never uploaded automatically. It
records observations only; it does not change device settings or run commands.

## Generate a capture from the tray

1. Right-click the Volty tray icon, select **Show Advanced Options**, then
   choose **Save Hardware Capture** for a capture alone or
   **Diagnose / Report an Issue** for a text report plus a capture.
2. Volty saves the timestamped `.volty.jsonl` hardware capture under
   `%APPDATA%\Volty\captures`. The tray status line shows the saved file name,
   and the log records the full path. A Windows notification appears; click its
   body or **Open Capture** button to open the saved file. Diagnose also saves a
   text report under `%APPDATA%\Volty\reports` and shows both exact paths.
   Bluetooth evidence is included only when Volty was built with the opt-in
   `bluetooth` feature.
3. Review the capture before sharing it, then attach it to a **Hardware report**
   issue if you are reporting device support or battery behavior.

If the capture file is not created, check the tray status or log for the error;
for Diagnose, the window also shows the failure.

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
- `--strict` omits model-level manufacturer, product, and Bluetooth friendly
  names, keeping only numeric identifiers and protocol evidence.
- Bluetooth records are added only when the executable was built with the tray
  `bluetooth` feature. Default builds produce HID-only captures but can still
  replay Bluetooth records from someone else's capture.

Inspect or replay a captured file without opening a device:

```text
volty capture inspect .\g502.volty.jsonl
volty capture replay .\g502.volty.jsonl
```

- `inspect` validates the schema and prints the same bounded summary as a fresh
  snapshot.
- `replay` decodes recorded HID++ exchanges, generic HID layouts, and Bluetooth
  property or GATT evidence through the same parsers the tray uses. It checks
  that the normalized readings can be reproduced from the recorded evidence.
  State-changing requests are rejected, and an exchange that does not match its
  response fails replay.

Replay is local-only and does not write another capture. A replay success means
Volty can reproduce the recorded decoding result, not that the device behaves
that way in every firmware or connection mode.

## Review before sharing

Open the `.volty.jsonl` file in a text editor. It is JSON Lines: one JSON object
per line, so it can be read without special tooling. Check for strings you do not
want to make public, including paths, serial numbers, usernames, computer names,
Bluetooth addresses, device names, and anything else specific to your setup.

The format is designed to exclude paths, serial numbers, usernames, computer
names, and Bluetooth addresses. Bluetooth devices are represented by
capture-local `bt-N` ids rather than addresses or opaque Windows device ids.
Still review the actual file before attaching it to a public issue. If you want
to omit model-level manufacturer, product, and Bluetooth friendly strings as
well, generate the capture with `volty capture --strict`; the tray export uses
the standard profile. Strict mode is an extra privacy option, not a substitute
for reading the file. If you are not comfortable sharing the contents, leave
the capture out of the report and describe the concern instead.

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

## Limitations

A capture records one system's observed behavior. It is not a guarantee that
every device of the same model behaves identically. Firmware, connection type,
and Windows support can affect the reading. Bluetooth capture is available only
in opt-in builds; only one Bluetooth Classic headset has been validated so far.
The BLE GATT capture path has not yet been tested on real hardware.

For the record layout and replay details, see the
[capture format reference](capture-format.md).
