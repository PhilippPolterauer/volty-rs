# Talking to Logitech mice over HID++

This documents the read-only diagnostic flow in [`debug.py`](../debug.py). It
uses Logitech's HID++ 2.0 protocol through `hidapi` to query a Logitech USB
receiver and the devices paired to it. The script is a diagnostic example,
not a general HID++ implementation; which interfaces and features work depends
on the receiver and mouse.

## Run the diagnostic

On Windows, install [uv](https://docs.astral.sh/uv/) and run the script from
the repository root:

```powershell
uv run .\debug.py
```

The script declares Python 3.10+ and the `hidapi` dependency in its inline
metadata, which uv uses to run it. It defaults to Logitech VID `0x046D`,
product ID `0xC547`, and probes the receiver/direct index `0xFF` and slots 1
through 6. For a different product ID or a smaller slot scan:

```powershell
uv run .\debug.py --pid 0xC547 --slots 1
uv run .\debug.py --vid 0x046D --pid 0xC548 --slots 1,2 --timeout-ms 1000
uv run .\debug.py --slots 1 --log .\hidpp-log.txt
```

`--slots` accepts comma-separated values 1 through 6 and `ff` (also
`receiver`) for index `0xFF`. The default per-request timeout is 350 ms; the
script uses a fresh nonzero software ID for each request to avoid confusing
late replies with the current request. `--swid` selects the starting value
(1–15), and `--timeout-ms` must be at least 50.

The diagnostic opens only matching HID interfaces and sends HID++ queries. It
does call the HID write API to transmit them, but it does not send commands
that change settings, profiles, pairing, or firmware. It logs to the console
and a timestamped text file in the current directory by default. Logs include
the enumerated HID path and serial number; review and redact them before
sharing.

## Find the HID++ interface

The script enumerates HID interfaces matching the selected VID and PID, then
uses only interfaces with HID usage page `0xFF00` and usage `0x0001` or
`0x0002`:

| Usage | HID++ report | Total bytes | Parameter bytes |
| --- | --- | ---: | ---: |
| `0x0001` | Short, report ID `0x10` | 7 | 3 |
| `0x0002` | Long, report ID `0x11` | 20 | 16 |

It chooses the report ID and length from the interface usage; it does not
guess lengths for other collections. A matching VID/PID alone is not enough:
if there is no vendor collection, the script cannot send HID++ requests on
that interface. It opens each candidate interface separately and closes it
after probing.

## HID++ 2.0 request framing

Each report starts with this four-byte header, followed by parameters and
zero-padding to the report's fixed length:

```text
[report ID, device index, feature index, (function index << 4) | software ID, parameters...]
```

- **Report ID** selects the short or long HID report described above.
- **Device index** selects the receiver/direct endpoint (`0xFF`) or a paired
  device slot (1–6).
- **Feature index** is the device's runtime index for a feature. Feature
  discovery requests use the Root feature at index `0x00`.
- The fourth byte packs a four-bit function index in the high nibble and a
  four-bit software ID in the low nibble. The diagnostic uses software IDs
  1–15; zero is reserved for unsolicited events.
- **Parameters** are operation-specific. The remaining report bytes are
  zero-filled.

For each request, the script writes one complete report and reads until it
gets a response matching the device index, feature index, function, and
software ID, or the timeout expires. Unmatched packets are logged rather than
treated as the answer. HID++ errors use feature byte `0xFF`; the error reply
contains the failed feature index, original function/software-ID byte, and
error code.

## Discover features and read battery status

Feature IDs are logical IDs, not the indexes to use in later calls. The
diagnostic discovers each requested feature through the Root feature:

1. Send Root function 0 (`GetFeature`) with the two-byte feature ID and a
   feature-type parameter of `0x00`.
2. Read the response's runtime feature index, type, and version.
3. An index of `0x00` means that feature is unsupported on that device. A
   nonzero index is used for subsequent calls.

The script first uses Device Information (`0x0003`) as a read-only
framing/routing check for each selected device index. It then looks up these
battery-related features on slots that responded:

| Logical feature ID | Meaning in the diagnostic | Follow-up |
| --- | --- | --- |
| `0x1000` | Legacy Battery Status | Function 0, `GetBatteryLevelStatus` |
| `0x1001` | Battery Voltage | Feature lookup only |
| `0x1004` | Unified Battery | Function 1, `GetStatus` |

Battery requests use the discovered runtime feature index, not the logical ID.
The script logs raw response parameters and, when enough bytes are present,
the protocol fields: legacy level/next-level/status for `0x1000`, and
charge/level flags/charging status/external power for `0x1004`. The battery
voltage feature is discovered but not queried or decoded by this script.

## Read the output

The log labels the exchange and its outcome:

- `TX` shows the exact outgoing bytes; `TX FAILED` indicates a transport write
  error.
- `RX_MATCH` is a response matching the current request. `RX_ERROR` is a
  matching HID++ error response.
- `RX_UNMATCHED` is a packet that arrived but did not match the current
  request. Its device, feature, function, and software ID can help explain why.
- `TIMEOUT` means no matching response arrived before the configured timeout;
  a successful write alone does not establish that the mouse replied.
- `FEATURE RESULT` reports whether a feature was found and its runtime index,
  type, and version. `BATTERY RAW PARAMS` and `BATTERY ... decode` show the
  battery reply.

Common failure points are distinct: no matching interfaces usually means the
VID/PID is wrong or the receiver is not present; no HID++ collection means no
supported vendor usage was found; an open or write failure is a HID access or
interface problem; and a timeout means no matching protocol response was
received. If one usage times out while another responds, use the responding
collection on that hardware rather than assuming one report type works for all
receivers. The diagnostic's summary and raw `TX`/`RX` lines are intended to
help distinguish these cases.
