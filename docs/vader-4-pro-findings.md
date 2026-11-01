# Flydigi Vader 4 Pro battery investigation

## Decision

Stop work on integrating the Vader 4 Pro for now. Volty does not have a live
battery reader for it. The XInput reply format is understood, but Windows does
not expose the complete reply through the ordinary XInput or HID APIs. Reaching
it requires a lower-level driver path, and the mechanism that makes the
controller produce the reply is not yet understood.

No device-specific Windows service, kernel driver, or tray-provider integration
is being pursued. A pure decoder for captured `A5 10` replies exists in
`volty-hid`, but it is not connected to a live transport.

## Confirmed protocol

The controller in its normal mode enumerates as XInput device `045e:028e`.
`XInputGetBatteryInformation` reports a wired device; Microsoft documents the
battery level as meaningful only for wireless controllers, so that API does
not provide this controller's battery reading
([API documentation](https://learn.microsoft.com/en-us/windows/win32/api/xinput/ns-xinput-xinput_battery_information)).

Space Station's service log contains 32-byte controller reports with the
Flydigi response beginning at offset 14. For example:

```text
00-14-00-00-00-00-20-01-FC-00-14-FF-E8-FF-A5-10-69-43-8C-93-38-55-69-02-02-00-06-00-00-00-95-00
```

In this report, byte 14 is `A5`, byte 15 is response ID `10`, and byte 23 is
the packed battery value. `0x02` was observed and represents two of five 20%
steps (approximately 40%), discharging. The high nibble encodes state (`0`
discharging, `1` charging, `2` charged); the low nibble is a level from zero to
five. A zero value was observed at connection and should be treated as
not-ready, not empty. The layout is also described in the
[independent protocol analysis](https://github.com/uiltonlopes/flydigi-space-station-mac/blob/main/docs/protocol.md).

## USB captures

USBPcap captures made on 2026-11-01 confirmed that the extended reports travel
on the USB bus as 32-byte interrupt-IN (`0x81`) transfers. The gamepad data is
at the start of each report and the Flydigi response is at offset 14. The
captures contained no `A5` command going out from the host.

During the capture with Space Station active, its traffic included interrupt
OUT (`0x05`) rumble packets:

```text
00 08 00 AB BA 00 00 00
00 08 00 BA AB 00 00 00
00 08 00 AB BA 00 00 00
00 08 00 00 00 00 00 00
```

Those packets correspond to XInput motor values `0xAB` and `0xBA`. The info
replies appeared after controller re-initialization activity, including
descriptor reads, `SET_CONFIGURATION`, XInput capability reads, and LED
packets. This is a timing correlation, not proof of the trigger; no host-side
`A5` command was visible.

A separate capture with Space Station stopped sent the same rumble sequence
three times using `XInputSetState`. It produced the rumble OUT packets but no
`A5` replies. Therefore, the rumble sequence alone is not sufficient to trigger
the response.

## Windows access limits

- The regular HID `IG_00` path yielded 14-byte reports.
- XInput and the `xusb22` `IOCTL_XUSB_GET_STATE` exposed standard gamepad state,
  not the bytes after the XInput payload.
- Static inspection of Flydigi's `XboxComm64.dll` identified a filter-manager
  port (`\XboxFilterPort`) for receiving messages and `DeviceIoControl` calls
  on `\\.\XboxFilter` for writes/readiness.
- From an unelevated process, `\\.\XboxFilter` could be opened, but its tested
  IOCTLs failed with `ERROR_GEN_FAILURE`; connecting to `\XboxFilterPort`
  failed with `E_ACCESSDENIED`.
- USBPcap can capture the raw USB transfer with its capture driver active, but
  it is a separate driver dependency and an elevated capture workflow, not a
  shipped Volty reader.

Thus, no zero-dependency user-mode route to the extended report was found. A
custom kernel adapter could potentially expose the raw report, but would need a
Windows driver project, installation/elevation, and signed distribution. That
would be a substantial and higher-risk addition, not a lightweight,
dependency-free user-mode adapter.

## Capture artifacts and scope

The captures remain local and are not included in the repository:

- `C:\Users\phili\vader-pcap-123917\` — Space Station active; contains the
  extended reports.
- `C:\Users\phili\vader-pcap-125129-magic\` — Space Station stopped; rumble
  control capture with no `A5` replies.

This investigation did not validate the DInput battery candidate, determine
what triggers the `A5` response, or implement a live reader. Revisit only if a
safe, maintainable Windows transport becomes available and Vader 4 Pro support
is prioritized again.
