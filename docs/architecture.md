# Architecture

Volty is a modular Cargo workspace. Protocol handling is separated from tray
rendering and app behavior.

```text
volty-battery      Shared BatteryStatus, DeviceId, DeviceLabel, provider trait
volty-hid          HID interface matching, HID++ and generic HID backends
volty-bluetooth    Optional Windows Bluetooth battery provider
volty-capture      Versioned JSONL capture schema, reader, writer and replay
volty-tray         Windows tray app, polling, settings, history and UI
```

## Runtime flow

```text
HID interfaces -> supported HID catalog -> HID backend --+
                                                        +-> BatteryProvider
Windows Bluetooth properties/GATT -> Bluetooth backend +       |
                                                                v
                                           DeviceSession -> tray, ETA,
                                           history and alerts
```

`volty-battery` defines transport-neutral `BatteryStatus`, stable `DeviceId`,
and `BatteryProvider`. Providers yield normalized readings; tray code does not
see HID++ opcodes, device indexes, interface paths, or Bluetooth transport
details.

`volty-hid` owns HID interface matching and protocol behavior. The single
`SUPPORTED_HID_DEVICES` catalog resolves each interface to no device, exactly
one device, or an ambiguity error; catalog order never selects between
overlapping matchers. Its current entries cover Logitech HID++ 2.0 and generic
standard HID battery reports. The generic matcher accepts only non-vendor
usage pages and probes/caches a usable battery layout.

HID identity does not depend on the Windows interface path, which can change
when USB re-enumerates. Stable interface keys use VID/PID, interface number,
usage page/usage, and (when present) a serial-number hash. The tray's device
state, monitoring preferences, and low-battery de-duplication use a stable
device identity plus HID++ slot.

`volty-bluetooth` is separate from the HID backend and remains experimental. It
is compiled into the tray only with `--features bluetooth`; see the
[Bluetooth support notes](bluetooth-headphone-battery-support.md).

`volty-tray` owns polling, configuration, local state, notifications, and tray
presentation. A new provider implements `BatteryProvider`; new HID protocols
implement `BatteryProtocol` and register a specific `HidBatteryDevice` in the
catalog. See [Generic HID battery support](hid-battery-support.md) and
[Adding a Logitech HID++ device](adding-mice.md).
