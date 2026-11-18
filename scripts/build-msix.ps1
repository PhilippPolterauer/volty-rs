#Requires -Version 5.1
<#
.SYNOPSIS
    Builds a Microsoft Store MSIX upload (.msixupload) for Volty.

.DESCRIPTION
    Compiles volty-tray without self-updates, stages it with the Store manifest and tile
    assets, packs one .msix per architecture, bundles them, and wraps the bundle
    in a .msixupload for Partner Center.

    Requires the Windows 10/11 SDK (MakeAppx.exe, 10.0.17763.0 or newer) and a
    Rust toolchain with the MSVC target installed.

.PARAMETER Architecture
    Package architecture. Volty ships x64 only.

.PARAMETER Configuration
    Cargo profile to package. Defaults to Release.

.PARAMETER Version
    Exact MSIX version override (up to four parts). Defaults to the volty-tray
    version with the major incremented and a zero revision: 0.15.0 becomes
    1.15.0.0, because Store package versions cannot start at 0.

.PARAMETER OutputDirectory
    Artifact directory. Defaults to dist/msix in the repository root.

.PARAMETER AssetsDirectory
    Optional folder with Square44x44Logo.png, Square150x150Logo.png, and
    StoreLogo.png. Without it, assets are generated from the tray icon.

.PARAMETER SkipBuild
    Reuse the existing build from target/package-managed instead of compiling.

.PARAMETER SelfSign
    Creates a throwaway code-signing certificate, signs the packages, and writes
    a .cer next to them for sideload testing. Store uploads do not need signing.

.PARAMETER CertificatePath
    Existing .pfx used to sign packages for sideload testing.

.PARAMETER CertificatePassword
    Password for -CertificatePath, or for the exported self-signed .pfx.

.PARAMETER KeepStaging
    Keeps the staging and package folders instead of deleting them.

.EXAMPLE
    pwsh ./scripts/build-msix.ps1

.EXAMPLE
    pwsh ./scripts/build-msix.ps1 -SelfSign

.EXAMPLE
    pwsh ./scripts/build-msix.ps1 -Version 2.0.0.0
#>
[CmdletBinding()]
param(
    [ValidateSet('x64')]
    [string[]]$Architecture = @('x64'),

    [ValidateSet('Release', 'Debug')]
    [string]$Configuration = 'Release',

    [string]$Version,

    [string]$OutputDirectory,

    [string]$AssetsDirectory,

    [switch]$SkipBuild,

    [switch]$SelfSign,

    [string]$CertificatePath,

    [string]$CertificatePassword,

    [switch]$KeepStaging
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$manifestTemplate = Join-Path $repoRoot 'packaging\msix\AppxManifest.xml'
$defaultIcon = Join-Path $repoRoot 'crates\volty-tray\assets\volty-favicon.png'
$devCertificatePassword = 'volty-dev'

$targetTriples = @{
    x64 = 'x86_64-pc-windows-msvc'
}

$architectures = @($Architecture | Select-Object -Unique)

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Resolve-FullPath {
    param([string]$Path, [string]$Base)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $Base $Path))
}

function ConvertTo-MsixVersion {
    param([string]$Value)

    $core = ($Value -split '[-+]')[0]
    $parts = $core.Split('.')
    if ($parts.Count -lt 2 -or $parts.Count -gt 4) {
        throw "Version '$Value' must have two to four numeric parts."
    }

    $numbers = @()
    foreach ($part in $parts) {
        if ($part -notmatch '^\d+$') {
            throw "Version '$Value' contains a non-numeric part '$part'."
        }
        $number = [int]$part
        if ($number -gt 65535) {
            throw "Version part '$part' exceeds the MSIX limit of 65535."
        }
        $numbers += $number
    }

    while ($numbers.Count -lt 4) {
        $numbers += 0
    }

    if ($numbers[0] -eq 0) {
        throw "MSIX major version must be at least 1 (got '$Value')."
    }
    if ($numbers[3] -ne 0) {
        throw "The Microsoft Store requires the fourth MSIX version part to be 0 (got '$Value')."
    }

    return ($numbers -join '.')
}

function Get-VoltyVersion {
    param([string]$Override)

    if ($Override) {
        return ConvertTo-MsixVersion -Value $Override
    }

    $metadataJson = & cargo metadata --locked --no-deps --format-version 1 --manifest-path (Join-Path $repoRoot 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) {
        throw 'cargo metadata failed; fix the workspace before packaging.'
    }

    $metadata = $metadataJson | ConvertFrom-Json
    $package = $metadata.packages | Where-Object { $_.name -eq 'volty-tray' } | Select-Object -First 1
    if (-not $package) {
        throw 'Could not find the volty-tray package in cargo metadata.'
    }

    $appVersion = ($package.version -split '[-+]')[0]
    if ($appVersion -notmatch '^(\d+)\.(\d+)\.(\d+)$') {
        throw "Cannot map volty-tray version '$($package.version)' to a Store package version."
    }

    # Store versions cannot start at 0. Bumping the major keeps versions
    # monotonic when the app eventually reaches 1.0.0 (0.15.0 -> 1.15.0.0,
    # 1.0.0 -> 2.0.0.0). Pass -Version to set the exact Store version instead.
    $major = [int]$Matches[1] + 1
    $minor = [int]$Matches[2]
    $build = [int]$Matches[3]

    return (ConvertTo-MsixVersion -Value "$major.$minor.$build.0")
}

function Get-WindowsSdkTools {
    $roots = @(
        "${env:ProgramFiles(x86)}\Windows Kits\10\bin",
        "${env:ProgramFiles}\Windows Kits\10\bin"
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }

    $candidates = @()
    foreach ($root in $roots) {
        $candidates += Get-ChildItem -LiteralPath $root -Directory |
            Where-Object { $_.Name -match '^10\.\d+\.\d+\.\d+$' } |
            ForEach-Object {
                [pscustomobject]@{
                    Version = [version]$_.Name
                    Bin     = Join-Path $_.FullName 'x64'
                }
            }
    }

    $sdk = $candidates |
        Sort-Object -Property Version -Descending |
        Where-Object {
            $_.Version -ge [version]'10.0.17763.0' -and
            (Test-Path -LiteralPath (Join-Path $_.Bin 'makeappx.exe'))
        } |
        Select-Object -First 1

    if (-not $sdk) {
        throw 'MakeAppx.exe was not found. Install the Windows 10/11 SDK (10.0.17763.0 or newer) and run this script again.'
    }

    return [pscustomobject]@{
        Version       = $sdk.Version
        MakeAppx      = Join-Path $sdk.Bin 'makeappx.exe'
        SignTool      = Join-Path $sdk.Bin 'signtool.exe'
        BinDirectory  = $sdk.Bin
    }
}

function New-MsixAssets {
    param(
        [string]$SourceIcon,
        [string]$Destination
    )

    if (-not (Test-Path -LiteralPath $SourceIcon)) {
        throw "Asset source '$SourceIcon' does not exist."
    }

    Add-Type -AssemblyName System.Drawing
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null

    $logos = [ordered]@{
        'Square44x44Logo.png'   = 44
        'Square150x150Logo.png' = 150
        'StoreLogo.png'         = 50
    }

    $source = [System.Drawing.Image]::FromFile($SourceIcon)
    try {
        foreach ($logo in $logos.GetEnumerator()) {
            $bitmap = [System.Drawing.Bitmap]::new(
                $logo.Value,
                $logo.Value,
                [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
            )
            $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
            try {
                $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
                $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
                $graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
                $graphics.DrawImage($source, 0, 0, $logo.Value, $logo.Value)
            }
            finally {
                $graphics.Dispose()
            }

            $bitmap.Save((Join-Path $Destination $logo.Key), [System.Drawing.Imaging.ImageFormat]::Png)
            $bitmap.Dispose()
        }
    }
    finally {
        $source.Dispose()
    }
}

function Copy-MsixAssets {
    param(
        [string]$Source,
        [string]$Destination
    )

    $required = @('Square44x44Logo.png', 'Square150x150Logo.png', 'StoreLogo.png')
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null

    foreach ($name in $required) {
        $file = Join-Path $Source $name
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
            throw "Asset '$name' is missing from '$Source'."
        }
        Copy-Item -LiteralPath $file -Destination (Join-Path $Destination $name)
    }
}

function Get-DevCertificate {
    param([string]$Publisher)

    $existing = Get-ChildItem 'Cert:\CurrentUser\My' |
        Where-Object {
            $_.Subject -eq $Publisher -and
            $_.HasPrivateKey -and
            $_.NotAfter -gt (Get-Date).AddDays(30)
        } |
        Sort-Object -Property NotAfter -Descending |
        Select-Object -First 1

    if ($existing) {
        return $existing
    }

    if (-not (Get-Command New-SelfSignedCertificate -ErrorAction SilentlyContinue)) {
        throw 'New-SelfSignedCertificate is unavailable. Pass -CertificatePath with an existing .pfx instead.'
    }

    return New-SelfSignedCertificate `
        -Type Custom `
        -Subject $Publisher `
        -KeyUsage DigitalSignature `
        -FriendlyName 'Volty MSIX development' `
        -CertStoreLocation 'Cert:\CurrentUser\My' `
        -NotAfter (Get-Date).AddYears(1) `
        -TextExtension @(
            '2.5.29.37={text}1.3.6.1.5.5.7.3.3',
            '2.5.29.19={text}'
        )
}

function Invoke-SignTool {
    param(
        [string]$SignTool,
        [string]$FilePath,
        [string]$PfxPath,
        [string]$Password,
        [string]$Thumbprint
    )

    if ($Thumbprint) {
        $arguments = @('sign', '/fd', 'SHA256', '/sha1', $Thumbprint, '/v', $FilePath)
    }
    else {
        $arguments = @('sign', '/fd', 'SHA256', '/f', $PfxPath)
        if ($Password) {
            $arguments += @('/p', $Password)
        }
        $arguments += @('/v', $FilePath)
    }

    & $SignTool @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "SignTool failed for '$FilePath'."
    }
}

# --- Build -----------------------------------------------------------------

Write-Step "Volty MSIX build for $($architectures -join ', ')"

$msixVersion = Get-VoltyVersion -Override $Version
Write-Host "    Package version: $msixVersion"

if (-not $OutputDirectory) {
    $OutputDirectory = 'dist\msix'
}
$outputRoot = Resolve-FullPath -Path $OutputDirectory -Base $repoRoot
$packagesDir = Join-Path $outputRoot 'packages'
$stagingRoot = Join-Path $outputRoot "staging-$([guid]::NewGuid())"
$managedTarget = Join-Path $repoRoot 'target\package-managed'

New-Item -ItemType Directory -Path $packagesDir -Force | Out-Null

foreach ($arch in $architectures) {
    if (-not $targetTriples.ContainsKey($arch)) {
        throw "Unsupported architecture '$arch'."
    }

    $triple = $targetTriples[$arch]
    $stage = Join-Path $stagingRoot $arch

    if (-not $SkipBuild) {
        Write-Step "Compiling volty-tray for $triple ($Configuration)"
        $cargoArguments = @(
            'build', '--locked', '--manifest-path', (Join-Path $repoRoot 'Cargo.toml'),
            '--package', 'volty-tray', '--target', $triple, '--target-dir', $managedTarget,
            '--no-default-features', '--features', 'bluetooth'
        )
        if ($Configuration -eq 'Release') {
            $cargoArguments += '--release'
        }
        & cargo @cargoArguments
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed for $triple."
        }
    }

    $profileDirectory = $Configuration.ToLowerInvariant()
    $executable = Join-Path $managedTarget "$triple\$profileDirectory\volty.exe"
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "Expected binary '$executable' was not produced. Run without -SkipBuild."
    }

    New-Item -ItemType Directory -Path $stage -Force | Out-Null

    Copy-Item -LiteralPath $executable -Destination (Join-Path $stage 'Volty.exe')

    $assetStage = Join-Path $stage 'Assets'
    if ($AssetsDirectory) {
        $assetSource = Resolve-FullPath -Path $AssetsDirectory -Base $repoRoot
        Copy-MsixAssets -Source $assetSource -Destination $assetStage
    }
    else {
        New-MsixAssets -SourceIcon $defaultIcon -Destination $assetStage
    }

    $manifest = (Get-Content -LiteralPath $manifestTemplate -Raw)
    $manifest = $manifest.Replace('__MSIX_VERSION__', $msixVersion).Replace('__MSIX_ARCHITECTURE__', $arch)
    [System.IO.File]::WriteAllText(
        (Join-Path $stage 'AppxManifest.xml'),
        $manifest,
        [System.Text.UTF8Encoding]::new($false)
    )

    Write-Step "Staged $arch payload"
}

$sdk = Get-WindowsSdkTools
Write-Host "    Windows SDK $($sdk.Version)"

# --- Pack ------------------------------------------------------------------

$packages = @()
foreach ($arch in $architectures) {
    $stage = Join-Path $stagingRoot $arch
    $package = Join-Path $packagesDir ("Volty_{0}_{1}.msix" -f $msixVersion, $arch)

    Write-Step "Packing $([System.IO.Path]::GetFileName($package))"
    & $sdk.MakeAppx pack /o /h SHA256 /d $stage /p $package
    if ($LASTEXITCODE -ne 0) {
        throw "MakeAppx pack failed for $arch."
    }

    $packages += $package
}

# --- Sign (sideload only) --------------------------------------------------

$signing = $SelfSign -or $CertificatePath
$certificateThumbprint = $null
$pfxPath = $null
$plainPassword = $null

if ($signing) {
    if ($CertificatePath) {
        $pfxPath = Resolve-FullPath -Path $CertificatePath -Base $repoRoot
        if (-not (Test-Path -LiteralPath $pfxPath -PathType Leaf)) {
            throw "Certificate '$pfxPath' does not exist."
        }
        $plainPassword = $CertificatePassword
        if (-not $plainPassword) {
            $plainPassword = $env:MSIX_CERTIFICATE_PASSWORD
        }
        Write-Step 'Signing with the provided certificate'
    }
    else {
        Write-Step 'Creating a self-signed development certificate'
        $identity = [xml](Get-Content -LiteralPath $manifestTemplate -Raw)
        $publisher = $identity.Package.Identity.Publisher
        $certificate = Get-DevCertificate -Publisher $publisher
        $certificateThumbprint = $certificate.Thumbprint

        $plainPassword = $CertificatePassword
        if (-not $plainPassword) {
            $plainPassword = $devCertificatePassword
        }

        $pfxPath = Join-Path $outputRoot 'Volty-msix-dev.pfx'
        $cerPath = Join-Path $outputRoot 'Volty-msix-dev.cer'
        $securePassword = ConvertTo-SecureString -String $plainPassword -AsPlainText -Force
        Export-PfxCertificate -Cert $certificate -FilePath $pfxPath -Password $securePassword | Out-Null
        Export-Certificate -Cert $certificate -FilePath $cerPath | Out-Null
        Write-Host "    Certificate: $pfxPath (password: $plainPassword)"
        Write-Host "    Trust it for sideloading: Import-Certificate -FilePath '$cerPath' -CertStoreLocation Cert:\LocalMachine\TrustedPeople"
    }

    if (-not (Test-Path -LiteralPath $sdk.SignTool)) {
        throw "SignTool.exe was not found next to MakeAppx.exe in '$($sdk.BinDirectory)'."
    }

    foreach ($package in $packages) {
        Write-Step "Signing $([System.IO.Path]::GetFileName($package))"
        Invoke-SignTool -SignTool $sdk.SignTool -FilePath $package -PfxPath $pfxPath -Password $plainPassword -Thumbprint $certificateThumbprint
    }
}

# --- Bundle ----------------------------------------------------------------

$bundleName = 'Volty_{0}_{1}.msixbundle' -f $msixVersion, ($architectures -join '_')
$bundle = Join-Path $outputRoot $bundleName
$mappingPath = Join-Path $stagingRoot 'bundle-map.txt'
$mapping = @('[Files]') + @($packages | ForEach-Object {
    '"{0}" "{1}"' -f $_, (Split-Path -Leaf $_)
})
Set-Content -LiteralPath $mappingPath -Value $mapping -Encoding Unicode

Write-Step "Bundling $bundleName"
& $sdk.MakeAppx bundle /o /bv $msixVersion /f $mappingPath /p $bundle
if ($LASTEXITCODE -ne 0) {
    throw 'MakeAppx bundle failed.'
}

if ($signing) {
    Write-Step "Signing $bundleName"
    Invoke-SignTool -SignTool $sdk.SignTool -FilePath $bundle -PfxPath $pfxPath -Password $plainPassword -Thumbprint $certificateThumbprint
}

# --- Store upload ----------------------------------------------------------

$upload = Join-Path $outputRoot ('Volty_{0}_{1}.msixupload' -f $msixVersion, ($architectures -join '_'))
if (Test-Path -LiteralPath $upload) {
    Remove-Item -LiteralPath $upload -Force
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::Open($upload, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
        $archive,
        $bundle,
        $bundleName,
        [System.IO.Compression.CompressionLevel]::Optimal
    )
}
finally {
    $archive.Dispose()
}

if (-not $KeepStaging) {
    $resolvedStage = (Resolve-Path -LiteralPath $stagingRoot).Path
    if ((Split-Path -Parent $resolvedStage) -ne (Resolve-Path -LiteralPath $outputRoot).Path) {
        throw "Refusing to delete unexpected staging directory '$resolvedStage'."
    }
    Remove-Item -LiteralPath $resolvedStage -Recurse -Force
}

# --- Summary ---------------------------------------------------------------

Write-Step 'Artifacts'
$artifacts = @($packages + @($bundle, $upload))
if ($signing -and $pfxPath -and -not $CertificatePath) {
    $artifacts += $pfxPath
    $artifacts += (Join-Path $outputRoot 'Volty-msix-dev.cer')
}

foreach ($artifact in $artifacts) {
    $file = Get-Item -LiteralPath $artifact
    $hash = (Get-FileHash -LiteralPath $artifact -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host ("    {0}  {1:N0} bytes  sha256 {2}" -f $file.FullName, $file.Length, $hash)
}

Write-Host ''
Write-Host 'Upload the .msixupload file on Partner Center > volty-rs > New submission > Packages.' -ForegroundColor Green
Write-Host 'If the upload stalls at "null Bytes"/"Paused", delete it and upload the .msixbundle directly.' -ForegroundColor DarkGray
Write-Host 'The Store re-signs MSIX/AppX packages; signing is only needed for local sideloading.' -ForegroundColor DarkGray
