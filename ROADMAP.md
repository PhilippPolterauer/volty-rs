# Roadmap ideas

These are candidate directions, not release commitments. The priority is to
make Volty easy to understand, trustworthy about device support, and useful
over time.

## 1. Make first run effortless

- Explain where to find Volty if Windows hides its tray icon.
- Show what the icon, tooltip, and tray menu mean.
- Make it clear when no supported device is available and how to check
  monitoring settings.

## 2. Broaden verified device support

- Test more mice, headsets, and gamepads, and record the connection and battery
  source used for each result.
- Keep “implemented” support distinct from hardware-validated devices.
- Use privacy-reviewed captures to investigate unsupported devices.

## 3. Add useful battery-data insights

Build on the local history dashboard with per-device trends and summaries,
such as:

- Weekly or monthly high, low, and typical battery levels.
- Observed drain rate and charging trends, when the device reports enough data.
- Sample coverage and gaps, so a chart or estimate is not mistaken for a
  complete record.
- Clear labels separating measured readings from estimates, with an indication
  of confidence or available evidence.

## 4. Make settings easier to discover

- Consider a small settings experience for alert thresholds and percentage
  overlays that currently require editing `config.toml`.
- Keep tray icon style and theme controls close to the tray, where they are
  already easy to reach.
- Explain the effect of each setting without exposing implementation details.

## 5. Keep diagnostics clear and private

- Make it easy to understand why a device is not reporting a battery level.
- Keep diagnostic captures local and reviewable before users share them.
- Continue to distinguish missing, stale, estimated, and current readings.

## Guiding principles

- Keep battery history and diagnostics local by default.
- Do not imply support or precision beyond what Windows and the device report.
- Prefer small, understandable improvements to a large settings surface.
