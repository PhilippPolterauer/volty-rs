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
| Flydigi Vader 4 Pro (XInput mode) | USB cable | Flydigi XInput vendor reply `A5 10` | 32-byte report observed on USB; battery byte `0x02` is approximately 40%, discharging | Investigated; no live Volty transport; integration work stopped |
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

The [Vader 4 Pro investigation findings](vader-4-pro-findings.md) document
hardware observations and Windows transport limits. In brief, the `A5 10`
reply and battery byte were confirmed in USB captures, but the normal XInput
and HID APIs do not expose the extended bytes. Volty has no live reader, and
integration work is stopped.

The [independent protocol analysis](https://github.com/uiltonlopes/flydigi-space-station-mac/blob/main/docs/protocol.md)
documents the same `A5 10` reply layout, while its [Space Station 4 analysis](https://github.com/uiltonlopes/flydigi-space-station-mac/blob/main/docs/spacestation4-analysis.md)
identifies the XInput transport as Flydigi's Xbox filter driver and
`XboxComm64.dll`. Local USB captures and exact observations are detailed in the
investigation findings.

The public [Vader 4 Pro Reader](https://github.com/dantmnf/Vader4ProReader)
prototype supports DInput over cable or dongle but lists battery reporting as
unavailable. The [DInput device report](https://github.com/EinTim23/hiddriver360/issues/43)
identifies four interfaces as `04b4:2412`. Volty's passive `05 EC` observation
did not correlate its candidate battery byte with charge state. The DInput
battery byte remains uncorrelated and is not reported as a reading.

The project's current primary use case is the Logitech G502 X Superlight RGB.
Hardware testing is limited; Volty is a hobby project and is not production-ready
or affiliated with Logitech.
