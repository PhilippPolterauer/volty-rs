# Device support

Volty supports Logitech HID++ 2.0 interfaces, standard HID battery reports,
and XInput gamepad battery levels on Windows 10/11. Listed devices have been
tested on hardware; replayable captures are also confirmed where the backend
provides them. Other devices that use supported protocols may work on a
best-effort basis but are not verified.

| Device or family | Connection | Battery backend | Tested functionality | Status |
| --- | --- | --- | --- | --- |
| Logitech G502 X Superlight RGB | USB cable or Lightspeed receiver | Logitech HID++ 2.0 Unified Battery (`0x1004`) | Percentage, charging/discharging state, HID++ capture and offline replay | Verified |
| Logitech HID++ devices exposing only Battery Voltage (`0x1001`) | Any HID++ 2.0 transport | Battery Voltage fallback | Estimated percentage from millivolts; raw voltage retained and replayed | Experimental; synthetic tests only, not hardware-validated |
| Baseus Bass BS2 Lite | Bluetooth Classic (HFP/A2DP) | Windows PnP battery property (`DEVPKEY_Device_BatteryLevel`) | Percentage, property capture and offline replay; charging state unavailable | Verified, opt-in Bluetooth build |
| Keychron K8 Pro | USB cable or Bluetooth | None exposed to Windows | No standard HID battery, BLE GATT Battery Service, or usable vendor command | Not supported |
| Flydigi controllers on the vendor protocol (VID `0x37d7`, e.g. Vader 5 Pro) | USB cable, vendor HID interface | Flydigi `5A A5 01` info report (state and 20 % level step in byte 11, per SDL's open-source driver) | Percentage in 20 % steps, charging/charged state | Experimental; synthetic tests only, not hardware-validated |
| Flydigi Vader 4 Pro (DInput mode) | USB cable or 2.4 GHz receiver | Candidate battery byte in the `05 EC` device-info reply on interface 2 (`0xffa0`) | Reply observed, but its battery byte was not correlated with charge state | Investigating; do not treat the field as a reading yet |
| Flydigi Vader 4 Pro (XInput mode) | USB cable | Flydigi XInput vendor command `A5 10` via Space Station's Xbox filter driver | Space Station service log shows device-info replies with battery byte `0x02` (2/5 bars, approximately 40%); status nibble `0` means discharging | Protocol and reply observed on hardware; Volty has no XInput vendor transport yet |
| Xbox One wireless controller | Xbox Wireless Adapter | Windows XInput `XInputGetBatteryInformation` | Alkaline/NiMH buckets mapped to marked percentage estimates (empty 0%, low 20%, medium 50%, full 90%); no charging state | Verified by XInput provider hardware smoke test; estimates drive the tray icon/color but not ETA, history, or numeric low-battery alerts |
| Generic HID battery devices | Any HID transport | Standard HID battery usages | Percentage, charging state when available, layout capture and offline replay | Experimental; no named device verified |

**Verified** means a reading was observed on real hardware and reproduced from
a hardware capture when the backend provides a replayable capture. **Experimental**
means the code path is implemented and covered by synthetic fixtures, but no
named hardware has been verified.
**Investigating** means current host-readable interfaces did not expose a
usable battery reading; it does not rule out an undocumented device-specific
protocol.
**Not supported** means testing found no non-invasive host-readable battery
source.

Generic HID support covers interfaces whose report descriptor declares a
standard battery level (Generic Device Controls *Battery Strength* or the
Battery System page). A device exposing battery only through a vendor-defined
report is not detected unless its protocol is implemented. Bluetooth support is
disabled in default builds; only one Bluetooth Classic headset has been
validated, and BLE GATT has not been tested on real hardware. See
[Bluetooth headphone battery support](bluetooth-headphone-battery-support.md).

For a new device, collect and review a hardware capture, then open a
[Hardware report](https://github.com/PhilippPolterauer/volty-rs/issues/new?template=hardware-report.yml).
Describe the connection, expected behavior, and what Volty showed. See
[Hardware captures](hardware-capture.md) for privacy and submission guidance.
For raw HID protocol work, use the separate opt-in
[HID report trace](hardware-capture.md#raw-hid-report-traces); it does not
record USB traffic sent by another application.

On 2026-11-01, a live Vader 4 Pro USB session enumerated as XInput device
`045e:028e`. Microsoft's
[XINPUT_BATTERY_INFORMATION documentation](https://learn.microsoft.com/en-us/windows/win32/api/xinput/ns-xinput-xinput_battery_information)
says battery level is valid only for wireless devices; the XInput API is not the
source of Space Station's wired battery reading. Space Station's service log
shows it sending vendor command `A5 10` and receiving an info reply embedded in
the XInput input report. In that reply, byte 23 is packed as status in the high
nibble and level in the low nibble: status `0` is discharging, status `1` is
charging, and level `0..5` represents 20% steps. The logged `0x02` therefore
means approximately 40% and discharging; an initial `0x00` is also observed
immediately after connection and should be treated as not-yet-read rather than
empty. The 10-second Volty HID trace saw only 14-byte gamepad reports because
this vendor channel is accessed through Flydigi's Xbox filter driver; Volty
does not yet implement that transport.

The [independent protocol analysis](https://github.com/uiltonlopes/flydigi-space-station-mac/blob/main/docs/protocol.md)
documents the same `A5 10` reply layout, while its [Space Station 4 analysis](https://github.com/uiltonlopes/flydigi-space-station-mac/blob/main/docs/spacestation4-analysis.md)
identifies the XInput transport as Flydigi's Xbox filter driver and
`XboxComm64.dll`. The response bytes above were observed in the local Space
Station service log; they were not captured from a USB bus trace.

The public [Vader 4 Pro Reader](https://github.com/dantmnf/Vader4ProReader)
prototype supports DInput over cable or dongle but lists battery reporting as
unavailable. The [DInput device report](https://github.com/EinTim23/hiddriver360/issues/43)
identifies four interfaces as `04b4:2412`. Volty's passive `05 EC` observation
did not correlate its candidate battery byte with charge state. The remaining
implementation question is how Volty can access the normal-mode XInput vendor
channel without depending on Flydigi's installed filter driver.

The project's current primary use case is the Logitech G502 X Superlight RGB.
Hardware testing is limited; Volty is a hobby project and is not production-ready
or affiliated with Logitech.
