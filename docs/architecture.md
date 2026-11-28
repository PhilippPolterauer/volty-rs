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
overlapping matchers. The catalog includes device-specific backends and generic
standard HID battery reports. The generic matcher accepts only non-vendor usage
pages and probes/caches a usable battery layout.

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

`volty-bluetooth` is separate from the HID backend and is enabled by the tray
crate's default features. Battery availability depends on the data Windows
exposes for each device. Build with
`--no-default-features` to exclude it. See [device support](device-support.md)
for validated examples and limits.

`volty-tray` owns polling, configuration, local state, notifications, and tray
presentation. A new provider implements `BatteryProvider`; new HID protocols
implement `BatteryProtocol` and register a specific `HidBatteryDevice` in the
catalog. Its XInput source reads coarse battery levels for connected gamepads
and maps them to marked percentage estimates (empty 0%, low 20%, medium 50%,
full 90%). Providers preserve whether a percentage was reported or estimated;
estimates can drive the tray icon and color, but do not feed ETA, discharge-rate
learning, percentage history, or numeric low-battery thresholds.
The XInput player slot is the best available device identity. See
[HID support and adding devices](hid-battery-support.md).

The Windows tray crate centralizes application icons in `app_icon.rs`.
`dashboard/model.rs` aggregates history and `dashboard/render.rs` generates
standalone HTML exports. The native dashboard uses egui with the OpenGL
renderer and the Windows AccessKit adapter. Settings is a separate Win32
window with native controls. Both use the semantic colors in
`dashboard/design.rs` and retain the Windows title bar.

## Reactive desktop state

| State | Owner | Consumers |
| --- | --- | --- |
| Current devices and readings | Monitor's `DeviceSession` | Tray and dashboard |
| History and preferences | SQLite, `config.toml`, and Windows startup registration | Monitor, dashboard, and Settings |
| Prepared charts and statistics | Dashboard history worker | Dashboard renderer |
| Unsaved settings, selections, dialogs, and operation feedback | The window showing them | That window |

The tray process owns discovery and battery sampling independently of either
window. It publishes a small, versioned snapshot of detected devices and
readings through session-local shared memory. The dashboard event thread
reads it and replaces a single pending snapshot. A sequence counter lets
readers reject an update that overlaps a write. Closing the monitor clears
the snapshot. Battery percentages retain their exact or estimated status.
Pausing history recording still allows the monitor to publish live readings.

Windows device and resume messages request discovery through the existing
provider worker. Device-change bursts are coalesced. The providers continue
to use their configured polling intervals for readings that Windows cannot
push. A bounded history recorder owns the SQLite writer so writing a sample
cannot block tray message handling or device discovery. The recorder checks
the current recording preference before inserting and reports a full queue.
A saved preference wakes the tray process to reload its committed
state. Appearance changes reach the dashboard separately from history so
theme and battery colors can update during a query.

The dashboard has separate bounded request queues for history, management,
and CSV export. It allows one outstanding history read and coalesces
additional refreshes. A range change discards the superseded result; the
previously displayed data stays labeled while the replacement loads. CSV
export retains the displayed immutable snapshot. The history worker prepares
the chart's bounded set of points, charging bands, activity intervals, and
axis labels. Hover looks up original readings in memory, and the readings
table only lays out visible rows. History gaps and the period before evidence
appears are shown explicitly. Historical connectivity is inferred from
samples. The live dashboard uses the monitor's current observations for
device status.

Delete confirmations keep the preview cutoff and check the affected sample
and device counts inside the SQLite write transaction. Changed counts require
a new preview. Settings loads its form and performs saves and test
notifications on a background worker. The form tracks a draft and the last
successful submission independently, so a save response preserves edits made
while it was running. The color slider renders with the same icon routines as
the tray and uses cached preview inputs. The startup checkbox reads Windows
registration when Settings opens and requests a registration change when its
value is edited.

The desktop changes are concentrated in `dashboard/window/`,
`dashboard/settings_window.rs` and its `appearance.rs`, and
`dashboard/design.rs`. `dashboard/live.rs`, `app.rs`, `app/wake.rs`,
`app/history_recorder.rs`, and `tray.rs` connect the monitor and tray to the windows.
`dashboard/management.rs` and `history.rs` own history and preference
operations.

## Windows validation

The development host ran Windows build 26300 on an AMD Ryzen 7 5800X3D.
The comparison used the existing optimized executable from before this work
and an optimized build with the reactive desktop changes. Settings was
driven through Win32 control messages over all 101 slider values. The
dashboard used a synthetic SQLite fixture with 100,000 rows across four
devices. Working set and CPU time were sampled at the process level.

| Check | Baseline | Revised build |
| --- | ---: | ---: |
| Settings controls ready, two runs | 103–205 ms | 102–118 ms |
| Settings slider message processing, p95 | 1.16–1.27 ms | 1.16–1.24 ms |
| Settings working set, two runs | 17.9–18.2 MiB | 18.3 MiB |
| Dashboard native window available | 443 ms | 322 ms |
| Dashboard working set after three seconds | 216.5 MiB | 137.6 MiB |
| Dashboard CPU time over five seconds at rest | 0.00 s recorded | 0.00 s recorded |
| Optimized executable size | 12.41 MiB | 12.95 MiB |

A Windows UI Automation client found dashboard navigation, device entries,
metrics, and chart headings. A visible dashboard completed 30 scripted resize
calls in 287 ms. The Settings check changed the theme in an isolated profile,
verified the written configuration, and observed Save return to its disabled
state. A debug test loaded 100,000 readings and prepared their chart in 262 ms.
The same test exported that immutable snapshot while another connection
held an exclusive database lock. A Windows mapping test exercised publication,
readback, and rejection of malformed or partially written live snapshots.

These measurements are local smoke checks. Win32 slider timings stop before
display composition, and resize-call timing does not measure presented
frames. GPU usage, frame rate, Narrator speech, mixed-DPI monitor moves,
physical device disconnects, and sleep/resume still need manual verification.
Settings uses horizontal and vertical scrolling when the controls cannot fit.
Window positions and sizes still reset to defaults on launch. The tray
retains synchronous access to some small configuration and notification-state
operations. Settings commits its TOML, SQLite, and startup registration
through separate stores and reports partial failures.
