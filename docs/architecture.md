# Architecture

Volty is a modular Cargo workspace. Protocol handling is separated from tray
rendering and app behavior.

```text
volty-battery      Shared BatteryStatus, DeviceId, DeviceLabel, provider trait
volty-hid          HID interface matching, HID++ and generic HID backends
volty-bluetooth    Windows Bluetooth battery provider (enabled by default)
volty-tray         Windows tray app, XInput and HID polling, settings, history and UI
volty-capture      Versioned JSONL battery captures and raw HID trace schema
```

## Runtime flow

```text
HID interfaces -> supported HID catalog -> HID backend --------+
XInput controller battery API -> XInput provider ---------------+-> BatteryProvider
Windows Bluetooth properties/GATT -> Bluetooth backend --------+       |
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
overlapping matchers. Its current entries cover Logitech HID++ 2.0, Flydigi's
vendor protocol, and generic standard HID battery reports. The generic matcher
accepts only non-vendor usage pages and probes/caches a usable battery layout.

For protocol investigation, `volty-hid` also exposes report-level I/O and an
opt-in recording wrapper. `volty-capture` stores those observations in a
separate bounded trace schema, keeping raw button/keyboard data out of regular
battery captures. The CLI's passive trace records input reports only; it does
not sniff another process or the USB bus. See
[HID report traces](hardware-capture.md#raw-hid-report-traces).

HID identity does not depend on the Windows interface path, which can change
when USB re-enumerates. Stable interface keys use VID/PID, interface number,
usage page/usage, and (when present) a serial-number hash. The tray's device
state, monitoring preferences, and low-battery de-duplication use a stable
device identity plus HID++ slot.

`volty-bluetooth` is separate from the HID backend and remains experimental. It
is enabled by the tray crate's default features; build with
`--no-default-features` to exclude it. See the
[Bluetooth support notes](bluetooth-headphone-battery-support.md).

`volty-tray` owns polling, configuration, local state, notifications, and tray
presentation. A new provider implements `BatteryProvider`; new HID protocols
implement `BatteryProtocol` and register a specific `HidBatteryDevice` in the
catalog. Its XInput source reads coarse battery levels for connected gamepads
and maps them to marked percentage estimates (empty 0%, low 20%, medium 50%,
full 90%). Providers preserve whether a percentage was reported or estimated;
estimates can drive the tray icon and color, but do not feed ETA, discharge-rate
learning, percentage history, or numeric low-battery thresholds.
The XInput player slot is the best available device identity. See
[Generic HID battery support](hid-battery-support.md) and
[Adding a Logitech HID++ device](adding-mice.md).

The Windows tray crate centralizes executable-resource icon loading, native
window icons, the WebView window icon, and the dashboard favicon in
`app_icon.rs`. The local dashboard is organized as `dashboard/model.rs` for
history aggregation, `dashboard/render.rs` for HTML generation, and
`dashboard/window.rs` for the native WebView host; CLI orchestration remains in
`dashboard.rs`. Native dashboard data-management requests use a typed local
WebView IPC bridge and operate on Volty-owned history, preferences, and managed
diagnostic files; standalone HTML exports remain read-only.
