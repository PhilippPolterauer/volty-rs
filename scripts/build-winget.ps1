#Requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    [ValidatePattern('^[\w.-]+/[\w.-]+$')]
    [string]$Repository = 'PhilippPolterauer/volty-rs'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$executable = Join-Path $root 'target/package-managed/x86_64-pc-windows-msvc/release/volty.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Managed release executable is missing. Run scripts/build-msix.ps1 to build it."
}
$binaryVersion = (Get-Item -LiteralPath $executable).VersionInfo.ProductVersion
if ($binaryVersion -ne $Version) {
    throw "Managed executable version '$binaryVersion' does not match WinGet version '$Version'."
}

$packageId = 'PhilippPolterauer.Volty'
$releaseUrl = "https://github.com/$Repository/releases"
$dist = Join-Path $root 'dist'
$manifestDir = Join-Path $dist "winget/manifests/p/PhilippPolterauer/Volty/$Version"
$installerName = "Volty-v$Version-winget-x64.zip"
$installer = Join-Path $dist $installerName
$manifestArchive = Join-Path $dist "Volty-v$Version-winget-manifests.zip"

New-Item -ItemType Directory -Path $manifestDir -Force | Out-Null
Compress-Archive -LiteralPath $executable -DestinationPath $installer -Force
$hash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
$header = @"
PackageIdentifier: $packageId
PackageVersion: '$Version'
"@

Set-Content -LiteralPath (Join-Path $manifestDir "$packageId.yaml") -Encoding utf8 -Value @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.1.12.0.schema.json
$header
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.12.0
"@

Set-Content -LiteralPath (Join-Path $manifestDir "$packageId.installer.yaml") -Encoding utf8 -Value @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.1.12.0.schema.json
$header
MinimumOSVersion: 10.0.19041.0
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
- RelativeFilePath: volty.exe
  PortableCommandAlias: volty
Dependencies:
  PackageDependencies:
  - PackageIdentifier: Microsoft.VCRedist.2015+.x64
Installers:
- Architecture: x64
  InstallerUrl: $releaseUrl/download/v$Version/$installerName
  InstallerSha256: $hash
ManifestType: installer
ManifestVersion: 1.12.0
"@

Set-Content -LiteralPath (Join-Path $manifestDir "$packageId.locale.en-US.yaml") -Encoding utf8 -Value @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.1.12.0.schema.json
$header
PackageLocale: en-US
Publisher: Philipp Polterauer
PublisherUrl: https://github.com/PhilippPolterauer
PackageName: Volty
PackageUrl: https://github.com/$Repository
PublisherSupportUrl: https://github.com/$Repository/issues
License: MIT
LicenseUrl: https://github.com/$Repository/blob/v$Version/LICENSE
ShortDescription: Monitors battery levels for supported mice, headsets, and gamepads in the Windows notification area.
Moniker: volty
ReleaseNotesUrl: $releaseUrl/tag/v$Version
ManifestType: defaultLocale
ManifestVersion: 1.12.0
"@

$manifests = Get-ChildItem -LiteralPath $manifestDir -Filter '*.yaml' -File
Compress-Archive -LiteralPath $manifests.FullName -DestinationPath $manifestArchive -Force
Write-Host "WinGet installer: $installer"
Write-Host "WinGet submission manifests: $manifestDir"
