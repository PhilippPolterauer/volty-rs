# WinGet portable package

Tagged GitHub releases include `Volty-vX.Y.Z-winget-x64.zip` and
`Volty-vX.Y.Z-winget-manifests.zip`. The package contains `volty.exe` and exposes
the `volty` command. It uses the same updater-free build as MSIX, with upgrades
handled by WinGet.

## Build and validate locally

```powershell
pwsh ./scripts/build-msix.ps1
pwsh ./scripts/build-winget.ps1 -Version 0.16.0
winget validate ./dist/winget/manifests/p/PhilippPolterauer/Volty/0.16.0
```

## Submit to the WinGet community source

After the release goes live, submit the three generated manifests to
`manifests/p/PhilippPolterauer/Volty/X.Y.Z/` in the community repository.
Use the manifests from the published release so their installer URL and SHA-256
match the downloadable ZIP. [Microsoft’s submission guide](https://learn.microsoft.com/windows/package-manager/package/winget-validation)

Once the submission is merged and indexed:

```powershell
winget install --id PhilippPolterauer.Volty --exact --source winget
volty
```

Quit Volty from the tray before upgrading or uninstalling it through WinGet.
