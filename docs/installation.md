# Install and run

## Download a release

Download the latest
[Volty release](https://github.com/PhilippPolterauer/volty-rs/releases/latest).
For a standard 64-bit Windows PC, choose
`Volty-vX.Y.Z-x86_64-pc-windows-msvc.exe`. The matching `.sha256` file contains
the executable's SHA-256 checksum. Volty is a standalone executable; no
installer or additional runtime file is needed.

Save the executable where you want to keep it, then double-click it. Volty
starts in the Windows notification area. On first launch, choose whether to
start Volty automatically when you sign in. This setting applies to the current
Windows user, does not require administrator privileges, and can be changed
later from the tray menu. If enabled, Windows starts Volty from the executable's
current location, so keep the file there.

## Use the tray app

Volty monitors supported devices while it runs. The icon shows the lowest
reported battery level by default; you can pin a device or choose a numeric
percentage icon. Hover over the icon for a compact line per device, including
its name, percentage, estimated remaining time when available, and last-seen age.
A reading is marked `stale` after the configured stale period.

![Volty tray icon showing battery status](tray.png)

Low-battery alerts are Windows notifications. They are enabled by default and
fire at 20%, 10%, and 5% once per discharge cycle. Their thresholds and repeat
cooldown are configurable.

![Volty low-battery notification](notification.png)

Right-click the icon for the compact menu: manage **Monitored Devices**, toggle
**Start on Boot**, open **About Volty**, or quit. **Show Advanced Options**
reveals manual discovery, tray customization, configuration and log access,
hardware capture, diagnostics, and reset. The advanced-menu choice is saved
locally; **Reset Volty** returns to the compact menu.

![Volty tray menu](context.png)

Unchecking a device in **Monitored Devices** excludes it from tray status,
icon percentage, low-battery alerts, and battery history. Unlisted devices are
monitored by default.

## Tray display

Under **Show Advanced Options → Tray Display**, choose:

- **Battery** (default): a battery glyph filled to the percentage.
- **Percentage**: the number drawn using a built-in pixel font. The icon shows
  the number without a percent sign; unknown levels use the neutral battery
  icon.

The same submenu can pin the icon to a discovered device. **Lowest battery**
restores the default. If a pinned device is missing or unchecked, Volty falls
back to the lowest monitored percentage and reports **Pinned device
unavailable**. The pin remains saved and takes effect again if the device
returns.

Display mode and pin are stored in
`%LOCALAPPDATA%\Volty\device-state.sqlite3`, not in `config.toml`.

## Reset Volty

**Reset Volty** asks for confirmation, deletes `%APPDATA%\Volty\config.toml` and
`%LOCALAPPDATA%\Volty\device-state.sqlite3`, disables **Start on Boot**, and
clears the first-run startup preference. Volty then reinitializes with fresh
defaults, including the compact tray menu. The startup prompt appears again on
the next launch. Logs and the battery history database are retained.
