# Device support

Volty supports Logitech HID++ 2.0 interfaces and standard HID battery reports
on Windows 10/11. Listed devices have been tested on hardware; other devices
that use supported protocols may work on a best-effort basis, but are not
verified until a reading and its capture replay are confirmed.

| Device or family | Connection | Battery backend | Tested functionality | Status |
| --- | --- | --- | --- | --- |
| Logitech G502 X Superlight RGB | USB cable or Lightspeed receiver | Logitech HID++ 2.0 Unified Battery (`0x1004`) | Percentage, charging/discharging state, HID++ capture and offline replay | Verified |
| Logitech HID++ devices exposing only Battery Voltage (`0x1001`) | Any HID++ 2.0 transport | Battery Voltage fallback | Estimated percentage from millivolts; raw voltage retained and replayed | Experimental; synthetic tests only, not hardware-validated |
| Baseus Bass BS2 Lite | Bluetooth Classic (HFP/A2DP) | Windows PnP battery property (`DEVPKEY_Device_BatteryLevel`) | Percentage, property capture and offline replay; charging state unavailable | Verified, opt-in Bluetooth build |
| Keychron K8 Pro | USB cable or Bluetooth | None exposed to Windows | No standard HID battery, BLE GATT Battery Service, or usable vendor command | Not supported |
| Flydigi controllers on the vendor protocol (VID `0x37d7`, e.g. Vader 5 Pro) | USB cable, vendor HID interface | Flydigi `5A A5 01` info report (state and 20 % level step in byte 11, per SDL's open-source driver) | Percentage in 20 % steps, charging/charged state | Experimental; synthetic tests only, not hardware-validated |
| Flydigi Vader 4 Pro (wired USB) | USB HID / XInput | No standard HID battery usage; it uses the older `04b4:2412` protocol, whose battery field is undocumented, and XInput reports only wired/full | No battery percentage or charging state observed | Investigating; in DInput mode the `05 EC` info reply (interface 2, usage `0xffa0`) identifies the device but contains no battery field |
| Generic HID battery devices | Any HID transport | Standard HID battery usages | Percentage, charging state when available, layout capture and offline replay | Experimental; no named device verified |

**Verified** means a reading was observed on real hardware and reproduced from
a hardware capture. **Experimental** means the code path is implemented and
covered by synthetic fixtures, but no named hardware has been verified.
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

The project's current primary use case is the Logitech G502 X Superlight RGB.
Hardware testing is limited; Volty is a hobby project and is not production-ready
or affiliated with Logitech.
