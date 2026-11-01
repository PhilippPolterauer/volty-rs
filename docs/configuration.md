# Configuration and local data

Volty stores settings per Windows user and does not require administrator
privileges. Use **Show Advanced Options → Open Configuration** to create or
open `%APPDATA%\Volty\config.toml`. Volty watches the file for changes, and
**Discover Devices Now** also reloads it. If a new configuration is invalid,
Volty reports the error and keeps the last valid settings active.

```toml
low_battery_thresholds = [20, 10, 5] # 1-100 percent; normalized and deduplicated
low_battery_cooldown_minutes = 120 # repeat interval after the lowest threshold; 0 disables repeats
poll_interval_seconds = 120 # 10-3600 seconds after a successful reading
retry_interval_seconds = 15 # 5-300 seconds after an error
battery_eta_window_minutes = 10 # 1-180 minutes of discharge history
stale_after_minutes = 15 # 1-1440 minutes without a fresh reading before marking stale
notifications_enabled = true
log_level = "info" # error, warn, info, debug, or trace

# Optional tray color-map overrides. Thresholds are inclusive upper bounds.
# red_max_percent must be below yellow_max_percent; colors use #RRGGBB.
# [battery_color_map]
# red_max_percent = 15
# yellow_max_percent = 30
# red = "#d23030"
# yellow = "#ebbe23"
# green = "#2daa4b"
```

The old singular `low_battery_threshold` key is not accepted; replace it with
`low_battery_thresholds`. Thresholds are constrained to 1–100, deduplicated,
and sorted from highest to lowest. Each threshold fires once per discharge
cycle. After the lowest threshold fires, the alert can repeat after each
configured cooldown; `0` disables repeats.

The default tray colors are green above 30%, yellow from 16–30%, and red at 15%
or below. Red and yellow cutoffs can be overridden (red: 0–99, yellow: 1–100)
and must remain ordered. Omitted colors and thresholds retain their defaults.

## Battery estimates and stale readings

Battery ETA appears after a full configured history window of discharging
readings. It is hidden while charging or when the recent trend is unreliable.
Volty also keeps a per-device long-run discharge rate based only on time the app
was running and watching the device. After restart this may provide an ETA once
at least an hour and two percentage points of discharge have been observed;
the recent-window estimate takes over when available.

Tray labels show last-seen age after one minute and mark a reading `stale` after
`stale_after_minutes` without a fresh reading. Labels refresh about once a
minute, so a sleeping or disconnected device is not presented as live.

## Logs

Choose **Show Advanced Options → Open Log File** to open the current daily log
under `%APPDATA%\Volty`. Files are named `volty-YYYY-MM-DD.log`, rotate at 5 MiB
with one `.1` backup, and are retained for seven days. The default `info` level
records lifecycle events and errors; `debug` adds successful battery readings;
`trace` adds low-level details. Changes to `log_level` take effect when the
configuration reloads.

## Battery history

Monitored-device samples are stored locally in
`%LOCALAPPDATA%\Volty\battery-history.sqlite3`. Samples are appended when a
reading changes or at least five minutes have passed since its previous sample.
Entries older than 30 days are pruned automatically. Recording failures are
logged and do not interrupt polling. The database is never uploaded.

Run `volty history` while the tray app is running to summarize the database; it
does not open devices.

```powershell
volty history
volty history --days 30
volty history --device receiver
volty history --csv > battery-history.csv
```

The default view covers seven days and reports sample count, first and last
timestamps (UTC), latest/minimum/maximum percentage, and an estimated discharge
rate when enough decreasing readings are available. `--device` filters by a
case-insensitive substring of device identity or name; `--database <path>` reads
a different database. CSV columns are
`timestamp_utc,identity,name,percent,charging,voltage_mv`; unreported percent or
voltage values are empty.
