# Microsoft Store (MSIX)

`scripts/build-msix.ps1` builds the Microsoft Store upload for Volty: it
compiles `volty-tray` with Bluetooth enabled and the self-updater omitted,
stages it with `packaging/msix/AppxManifest.xml`, packs a
`.msix` per architecture, bundles them, and wraps the bundle in a
`.msixupload` for Partner Center.

The manifest already contains the identity from Partner Center
(Apps and games > volty-rs > Product management > Product identity):

| Field | Value |
| --- | --- |
| Package/Identity/Name | `philipp.polterauer.volty-rs` |
| Package/Identity/Publisher | `CN=631AAAE1-ED5D-48D1-A4F8-A9BF5B008645` |
| Package/Properties/PublisherDisplayName | `philipp.polterauer` |
| Package/Properties/DisplayName | `volty-rs` |
| Store ID | `9NP6XLQPMD78` |

## Requirements

- Windows 10/11 with the Windows SDK 10.0.17763.0 or newer (`MakeAppx.exe`).
- Rust with the MSVC target for every packaged architecture
  (`rustup target add x86_64-pc-windows-msvc`).

## Build the Store upload

```powershell
pwsh ./scripts/build-msix.ps1
```

Artifacts land in `dist/msix`:

- `Volty_<version>_x64.msixupload` - upload this to Partner Center.
- `Volty_<version>_x64.msixbundle` - the bundle inside the upload.
- `packages/Volty_<version>_x64.msix` - the single-architecture package.

The tagged release workflow also attaches the `.msixupload` and `.msixbundle`
to the corresponding GitHub release for submission to Partner Center.

If Partner Center shows the package as **null Bytes** or **Paused**, the upload
stalled; that is not a package validation failure. Delete the failed package,
refresh the submission, and upload `Volty_<version>_x64.msixbundle` (or the
`.msix`) directly. Partner Center accepts all three formats, and the direct
files skip the upload archive. Use a clean Edge window without extensions or a
VPN if it keeps stalling.

The script derives the Store version from the `volty-tray` package version. Store
package versions cannot start at 0, so the major is incremented and the revision
stays 0: `0.15.0` becomes `1.15.0.0`, and `1.0.0` will become `2.0.0.0`. That
keeps Store versions strictly increasing. Override it with an exact version when
needed:

```powershell
pwsh ./scripts/build-msix.ps1 -Version 2.0.0.0
```

Only x64 is packaged. `volty-tray` enables its SQLite dependency for
`x86_64-pc-windows-msvc` only, so an ARM64 package needs that target enabled
first.

Every Store submission must use a higher version than the previously published
package, and the fourth part must stay `0`. The Store rejects lower versions.

## Upload to Partner Center

1. Open **Apps and games > volty-rs > New submission**.
2. On **Packages**, upload the `.msixupload` file.
3. Complete the listing, age rating, and pricing steps, then submit.

The Store re-signs MSIX and AppX packages during publishing, so Store uploads do
not need a code-signing certificate. If Partner Center rejects the upload with
"your developer account isn't authorized to submit apps that use the
runFullTrust capability", request that authorization from Developer Support; it
belongs to the account, not the package.

## Restricted capability justification

The submission form asks why the package needs `runFullTrust`. Paste this
exactly as it is; the field is plain text, so markdown would show up literally.

```text
Capability: runFullTrust (restricted capability, Windows.Desktop)

volty-rs is a packaged Win32 desktop application, not a UWP app. It is a system
tray utility that shows battery levels for supported mice, headsets, and
gamepads in the Windows notification area. The manifest uses
EntryPoint="Windows.FullTrustApplication",
uap10:RuntimeBehavior="packagedClassicApp", and
uap10:TrustLevel="mediumIL", so runFullTrust is what lets the packaged process
start at all. Without it the app cannot launch.

The app depends on Win32 APIs that have no AppContainer or UWP equivalent:

- HID APIs (hidapi, over the Win32 HID interfaces) to read battery reports
  directly from supported peripherals.
- Windows Bluetooth LE (GATT) for headset battery levels, and XInput for
  controller battery levels.
- Win32 shell and window APIs for the notification area icon and its menu
  (Shell_NotifyIcon), window messages, per-monitor DPI, and toast
  notifications.
- Local configuration and battery history in a local SQLite database under the
  user's %APPDATA%.
- An embedded WebView2 control that renders the local battery dashboard.

Scope and risk: volty-rs runs only as the signed-in user at medium integrity,
exactly like a normal desktop app. It never requests elevation, and it installs
no drivers and no services. Start-up is an opt-in StartupTask that stays
disabled until the user enables it in Settings > Apps > Startup. The app does
not modify system files or settings and needs no administrator rights. HID and
Bluetooth access is read-only and limited to battery and status information.
Configuration and history stay on the device; no device or usage data is
collected or uploaded.

runFullTrust is the only restricted capability declared in the manifest. It
cannot be removed without removing the app's entire functionality, because
these Win32 capabilities are the product.
```

If the field has a length limit, this shorter version still covers everything a
reviewer checks (what the app is, why the capability is unavoidable, what it
does not do):

```text
volty-rs is a packaged Win32 system tray app that shows battery levels for
supported mice, headsets, and gamepads. The manifest uses
EntryPoint="Windows.FullTrustApplication" and
uap10:RuntimeBehavior="packagedClassicApp", so runFullTrust is required for the
process to launch. It needs Win32 APIs with no AppContainer equivalent: HID
(hidapi) for peripheral battery reports, Bluetooth LE (GATT) and XInput for
headset and controller battery levels, Shell_NotifyIcon and window messages for
the tray icon, a local SQLite database under %APPDATA% for configuration and
history, and WebView2 for the local dashboard. It runs as the signed-in user at
medium integrity, never elevates, installs no drivers or services, and reads
device data read-only without uploading anything. runFullTrust is the only
restricted capability and cannot be removed without removing all app
functionality.
```

## Sideload testing

`-SelfSign` creates (or reuses) a development certificate, signs the packages
and the bundle, and writes `Volty-msix-dev.pfx` and `Volty-msix-dev.cer` to
`dist/msix`. The default PFX password is `volty-dev`; override it with
`-CertificatePassword`.

```powershell
pwsh ./scripts/build-msix.ps1 -SelfSign
Import-Certificate -FilePath dist/msix/Volty-msix-dev.cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople
Add-AppxPackage dist/msix/Volty_1.16.0.0_x64.msixbundle
```

For a real certificate, pass `-CertificatePath <pfx>` and optionally
`-CertificatePassword`, or set `MSIX_CERTIFICATE_PASSWORD`.

## Package behavior

- **Runtime dependency:** Volty links `VCRUNTIME140.dll`, so the manifest
  declares the `Microsoft.VCLibs.140.00.UWPDesktop` framework package. Keep that
  dependency unless the build switches to a static C runtime.
- **Updates** are handled by the Store. The package build omits update checks,
  the tray update action, and the executable replacement helper.
- **Start-up:** the `HKCU` Run key does not work for packaged apps, so the
  manifest exposes a `windows.startupTask`. It starts disabled and users enable
  it in **Settings > Apps > Startup**.
- **Command line:** the `windows.appExecutionAlias` extension makes
  `volty.exe` available in a terminal.
- **WebView2:** the dashboard needs the Evergreen WebView2 Runtime. It ships
  with Windows 11 and current Windows 10 installations; the package cannot
  install it.
- **Data:** packaged Windows apps virtualize `%APPDATA%`, so configuration and
  history live under
  `%LOCALAPPDATA%\Packages\philipp.polterauer.volty-rs_azjq5m2u7g0\LocalCache\Roaming\Volty`.
  Existing standalone installations keep their own data.
