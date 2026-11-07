# Supported devices and limits

Volty reads battery data exposed through supported Windows device interfaces.
The entries below describe specific tested devices and connections; they do not
guarantee that every device using the same protocol will behave identically.

## Tested devices

| Device | Connection | Battery source | Result |
| --- | --- | --- | --- |
| Logitech G502 X Superlight RGB | USB cable or Lightspeed receiver | Logitech HID++ 2.0 Unified Battery | Percentage, charging state, capture and replay |
| Logitech G502 X PLUS (slot 1) | Connection not recorded | HID battery report | Live battery reading observed in tray |
| Baseus Bass BS2 Lite | Bluetooth Classic (HFP/A2DP) | Windows battery property | Percentage; charging state unavailable |
| Xiaomi J18 headphones | Bluetooth (profile not recorded) | Windows Bluetooth battery discovery | Percentage observed; exact source not recorded |
| Xbox One wireless controller | Xbox Wireless Adapter | Windows XInput | Coarse battery estimate; no charging state |

These are hardware observations, not a compatibility list for all devices in a
product family. XInput levels are estimates: Windows reports empty, low,
medium, or full rather than an exact percentage.

## Implemented battery sources

Only the devices in the table above have named hardware validation.

- **Logitech HID++ 2.0:** Unified Battery and legacy Battery Status features.
- **Standard HID:** battery fields declared in a report descriptor, including
  Generic Device Controls Battery Strength and Battery System values.
- **XInput:** coarse battery levels for connected gamepads.
- **Bluetooth:** Windows battery properties and the standard GATT Battery
  Service for connected devices. Bluetooth support is enabled in the default
  tray build, but readings depend on the interfaces Windows exposes.

Bluetooth hardware validation is limited to the two headphone models above.
The BLE GATT path has not been validated on hardware. Volty does not pair
devices, perform broad Bluetooth scans, or open a second audio-profile
connection.

## Limits and reporting

Devices that expose battery data only through an unsupported vendor report
cannot be read until a matching backend is implemented. A device may show a
battery level in its vendor software or Windows settings while exposing no
source Volty can use.

The Keychron K8 Pro was not readable in the tested USB and Bluetooth
configurations: Windows did not expose a standard battery report, BLE Battery
Service, or usable vendor command.

To report a device, save and review a local [hardware capture](hardware-capture.md),
then attach it to a
[Hardware report](https://github.com/PhilippPolterauer/volty-rs/issues/new?template=hardware-report.yml).
For implementation details, see [HID support and adding devices](hid-battery-support.md).
