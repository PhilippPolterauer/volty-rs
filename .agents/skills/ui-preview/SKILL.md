---
name: ui-preview
description: Iterate on Volty's native Windows dashboard and Settings UI using isolated fixtures and PNG captures. Use when changing or reviewing Volty layouts, themes, dialogs, and validation states.
---

# Volty UI preview

Run [scripts/ui-preview.ps1](../../../scripts/ui-preview.ps1) from the Volty workspace root in PowerShell 7 on Windows. It builds the dedicated incremental Cargo profile and opens the real native Volty window, including its Settings page. The first build prepares the profile's dependency cache. Rerun the launcher after changing Rust or layout code. Use `-NoBuild` to switch views, themes, or sizes with the executable you just built.

```powershell
./scripts/ui-preview.ps1 History -Screenshot "$env:TEMP/volty-history.png"
./scripts/ui-preview.ps1 Manage -Theme Light -NoBuild
./scripts/ui-preview.ps1 Cleanup -Narrow -NoBuild -Screenshot "$env:TEMP/volty-cleanup.png"
./scripts/ui-preview.ps1 Settings -Size 1024x760 -NoBuild
./scripts/ui-preview.ps1 InvalidSettings -Theme Light -NoBuild
./scripts/ui-preview.ps1 History -Empty -NoBuild
```

- `Cleanup` opens Manage data with the actual delete confirmation displayed. `InvalidSettings` opens the native Settings form with an invalid alert threshold.
- `-Screenshot` waits for the selected scenario to be ready and saves the native window as a PNG. Inspect the image to verify the affected layout and interaction state.
- The default theme is dark. Use `-Theme Light` or `-Theme System` as needed. `-Narrow` selects 700 × 560 logical pixels. `-Size WIDTHxHEIGHT` sets the shared window's client area.
- Each launch starts with fresh fixture data and restarts this checkout's preview. The installed tray and its windows can stay open. Cleanup and Settings saves affect the disposable profile. Startup registration and test notifications are simulated.
- Use `./scripts/ui-preview.ps1 -Stop` when finished, or leave the preview open if the user wants to inspect it.

For a visual edit, capture the affected state before moving on. For fixture or isolation changes, run `cargo test -p volty-tray --features ui-preview --locked dashboard::preview`. See the [development guide](../../../docs/development.md#native-ui-preview) for the workflow and workspace checks.
