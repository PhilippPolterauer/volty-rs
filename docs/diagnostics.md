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
again without hardware. Bluetooth evidence is recorded only by a build with
the opt-in `bluetooth` feature; default builds can still inspect and replay
Bluetooth records.

## HID diagnostics

The executable has explicit, on-demand HID++ diagnostics. They run before the
tray app's single-instance check; launching without a `hid` subcommand starts
the normal tray app.

```powershell
volty hid inspect
volty hid inspect --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002
volty hid features --vid 0x046d --pid 0xc547 --usage-page 0xff00 --usage 0x0002 --device-index 0x01
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

For a worked Logitech example, see [Adding a Logitech HID++ device](adding-mice.md).
For a battery support report, use the
[Hardware report issue template](https://github.com/PhilippPolterauer/volty-rs/issues/new?template=hardware-report.yml)
and attach a reviewed capture.
