# Troubleshooting and diagnostics

## Diagnose and report an issue

Choose **Show Advanced Options → Diagnose / Report an Issue** to open the
Windows Diagnose window. It shows recent app events, active configuration,
Bluetooth read status when that feature is enabled, and HID interfaces visible
to hidapi. It saves a text report under `%APPDATA%\Volty\reports` and a
machine-readable capture under `%APPDATA%\Volty\captures`.

The report includes HID IDs, usage information, and available manufacturer or
product names, but omits HID paths, serial numbers, and Bluetooth IDs or
addresses. Review both files before sharing; names and log messages may still
identify your setup. Nothing is uploaded automatically. **Copy Log** copies the
report, and **Open Issue** starts a GitHub issue draft.

To save only a capture, choose **Show Advanced Options → Save Hardware
Capture**. The tray status shows its file name and a Windows notification opens
the file. See [Hardware captures](hardware-capture.md) for its contents,
privacy properties, review steps, and submission workflow.

## Battery history dashboard

Generate a self-contained dashboard from the locally stored battery history:

```powershell
volty dashboard
volty dashboard --days 14
volty dashboard --window --days 14
volty dashboard --output .\battery-dashboard.html --no-open
volty dashboard --database .\battery-history.sqlite3
```

By default, the dashboard covers the last seven days, writes
`%TEMP%\volty-dashboard.html`, and opens it in the default browser. Use
`--no-open` to only write the file, or `--window` to show it in a native
WebView2 window. The Microsoft WebView2 Runtime is required for the native
window; if it is unavailable, Volty writes the HTML file and opens it in the
default browser. Left-click the Volty tray icon to open the same native window.
All data stays local; nothing is uploaded.

## Command-line capture

From a PowerShell prompt, use the release executable or run through Cargo from
the repository root:

```powershell
volty capture
volty capture --strict
volty capture --output .\capture.volty.jsonl
volty capture inspect .\capture.volty.jsonl
volty capture replay .\capture.volty.jsonl
```

`--strict` omits model strings. `inspect` validates a saved capture and
`replay` checks whether its recorded HID or Bluetooth evidence can be decoded
again without hardware. Bluetooth evidence is recorded when the `bluetooth`
feature is enabled (the default for the tray); builds made with
`--no-default-features` can still inspect and replay Bluetooth records.

## HID diagnostics

The executable has explicit, on-demand HID++ diagnostics. They run before the
tray app's single-instance check; launching without a `hid` subcommand starts
the normal tray app.

```powershell
volty hid inspect
volty hid inspect --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002
volty hid features --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002 --device-index 0x01
volty hid sniff --vid 0x04b4 --pid 0x2412 --interface 2 --duration-seconds 15 --output .\vader4pro.hidtrace.jsonl --include-input
volty hid trace .\vader4pro.hidtrace.jsonl
```

`inspect` lists matching interface IDs, usage page/usage, interface number,
bus, product name, and hidapi path. Paths appear only in this explicit command;
serial numbers are never printed. Selectors include `--vid`, `--pid`,
`--usage-page`, `--usage`, `--interface`, and `--path`. Commands opening a device
require exactly one interface match. Use `inspect` to narrow an ambiguous
selector. The USB interface number is not the HID++ `--device-index`, which
addresses a paired device behind a receiver.

`features` queries Root/FeatureSet and lists logical feature IDs with each
device's dynamically assigned runtime index, type, and version. Do not copy a
runtime index from one device to another. `volty hid raw` exists for exceptional
diagnostics but requires `--allow-state-changing` because arbitrary raw requests
cannot safely be classified as read-only. Prefer `inspect` and `features`.
Review diagnostic output before sharing it; interface paths can identify local
devices.

`sniff` is a separate, passive report-level trace for protocol investigation.
It requires `--include-input` because raw reports can contain buttons or
keystrokes, runs for at most 120 seconds, and refuses to overwrite a file.
`trace` validates the JSONL file and summarizes its report IDs and sizes. The
trace omits interface paths and serial numbers but must still be reviewed
before sharing. It sees input reports available to Volty, not USB traffic or
output/feature requests made by another application; see
[raw HID report traces](hardware-capture.md#raw-hid-report-traces).

For a worked Logitech example, see [Adding a Logitech HID++ device](adding-mice.md).
For a battery support report, use the
[Hardware report issue template](https://github.com/PhilippPolterauer/volty-rs/issues/new?template=hardware-report.yml)
and attach a reviewed capture.
