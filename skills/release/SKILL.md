---
name: volty-tray-icon
description: Use when changing Volty's Windows tray battery icon, percentage digits, battery colors, or light/dark theme contrast.
---

# Volty tray icon

Use this skill when modifying the battery or percentage icon shown in the Windows system tray.

## Implementation

- `crates/volty-tray/src/tray/icon.rs` renders the 16 px and 32 px RGBA icons.
- `crates/volty-tray/src/tray.rs` reads the Windows system theme and selects the icon renderer.
- Battery fill colors come from `BatteryColorMap`; keep them consistent across both icon styles.
- Battery outlines should contrast with the taskbar: dark in light mode, light in dark mode. The system mode is read from `SystemUsesLightTheme`.
- Percentage icons contain digits only. Keep them legible, centered by their visible pixel bounds, and sized proportionally at both supported resolutions.
- Preserve transparent backgrounds and supersampled antialiasing.

## Changes and verification

Update the renderer and its tests together. Cover both themes, both supported sizes, color-map boundaries, and percentage alignment when relevant.

Run:

```powershell
cargo fmt --all -- --check
cargo test -p volty-tray --bin volty tray::icon::tests
```
