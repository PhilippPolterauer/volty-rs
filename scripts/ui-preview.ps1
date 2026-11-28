#Requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('History', 'Manage', 'Cleanup', 'Settings', 'InvalidSettings')]
    [string] $View = 'History',
    [ValidateSet('Dark', 'Light', 'System')]
    [string] $Theme = 'Dark',
    [ValidatePattern('^\d+x\d+$')]
    [string] $Size,
    [switch] $Narrow,
    [switch] $Empty,
    [switch] $NoBuild,
    [switch] $Stop,
    [string] $Screenshot
)

$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Volty UI previews require Windows.' }
if ($Narrow -and $Size) { throw 'Choose either -Narrow or -Size.' }
if ($Empty -and $View -eq 'Cleanup') { throw 'The cleanup preview requires sample history.' }
if ($Size) {
    $width, $height = $Size.Split('x') | ForEach-Object { [int] $_ }
    if ($width -lt 660 -or $width -gt 3840 -or $height -lt 460 -or $height -gt 2160) {
        throw 'Size must be WIDTHxHEIGHT within 660x460 to 3840x2160.'
    }
}

$workspace = Split-Path -Parent $PSScriptRoot
$target = Join-Path $workspace 'target'
$previewDirectory = Join-Path $target 'ui-preview'
$executable = Join-Path $previewDirectory 'volty-ui-preview.exe'

function Stop-Preview {
    foreach ($preview in Get-Process -Name 'volty-ui-preview' -ErrorAction SilentlyContinue) {
        if ($preview.Path -ine $executable) { continue }
        if (-not $preview.HasExited -and $preview.CloseMainWindow()) {
            [void] $preview.WaitForExit(1800)
        }
        if (-not $preview.HasExited) {
            Stop-Process -InputObject $preview
            if (-not $preview.WaitForExit(3000)) { throw "Preview process $($preview.Id) did not stop." }
        }
    }
}

if ($Stop) {
    Stop-Preview
    return
}

if (-not $NoBuild) {
    $build = [Diagnostics.Stopwatch]::StartNew()
    & cargo build --manifest-path (Join-Path $workspace 'Cargo.toml') --target-dir $target --profile ui-preview --locked -p volty-tray --features ui-preview
    if ($LASTEXITCODE -ne 0) { throw "Preview build failed with exit code $LASTEXITCODE." }
    Write-Host "Build finished in $($build.Elapsed.TotalSeconds.ToString('0.00')) s."
    Stop-Preview
    New-Item -ItemType Directory -Path $previewDirectory -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $previewDirectory 'volty.exe') -Destination $executable -Force
} else {
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw 'Build the preview once before using -NoBuild.'
    }
    Stop-Preview
}

$viewName = $View.ToLowerInvariant()
if ($View -eq 'InvalidSettings') { $viewName = 'invalid-settings' }
if ($Narrow) { $Size = '700x560' }
$arguments = @('ui-preview', $viewName, '--theme', $Theme.ToLowerInvariant())
if ($Size) { $arguments += @('--size', $Size) }
if ($Empty) { $arguments += '--empty' }

$launch = [Diagnostics.Stopwatch]::StartNew()
$process = Start-Process -FilePath $executable -ArgumentList $arguments -WindowStyle Normal -PassThru
Write-Host "Opened $View preview (PID $($process.Id))."
if (-not $Screenshot) { return }

Add-Type -AssemblyName System.Drawing
if (-not ('VoltyUiPreviewCapture' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class VoltyUiPreviewCapture
{
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect { public int Left, Top, Right, Bottom; }

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool GetWindowRect(IntPtr window, out Rect bounds);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool PrintWindow(IntPtr window, IntPtr dc, uint flags);

    [DllImport("user32.dll")]
    public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
}
'@
}

$caption = if ($View -in @('Settings', 'InvalidSettings')) { ' Settings' } else { ' Dashboard' }
$ready = $false
while (-not $ready -and $launch.Elapsed.TotalSeconds -lt 12) {
    $process.Refresh()
    if ($process.HasExited) { throw "Preview exited with code $($process.ExitCode)." }
    $window = $process.MainWindowHandle
    $ready = $window -ne [IntPtr]::Zero -and $process.MainWindowTitle.StartsWith('Volty UI Preview') -and
        $process.MainWindowTitle.EndsWith($caption)
    if (-not $ready) { Start-Sleep -Milliseconds 30 }
}
if (-not $ready) { throw "Timed out waiting for the $View preview." }
Write-Host "Preview ready in $($launch.Elapsed.TotalMilliseconds.ToString('0')) ms."

$destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Screenshot)
$bounds = [VoltyUiPreviewCapture+Rect]::new()
$dpi = [VoltyUiPreviewCapture]::SetThreadDpiAwarenessContext([IntPtr] (-4))
try {
    if (-not [VoltyUiPreviewCapture]::GetWindowRect($window, [ref] $bounds)) {
        throw 'Could not get the preview window bounds.'
    }
} finally {
    if ($dpi -ne [IntPtr]::Zero) { [void] [VoltyUiPreviewCapture]::SetThreadDpiAwarenessContext($dpi) }
}
# Give the compositor time to present the frame that published the ready caption.
Start-Sleep -Milliseconds 50
$bitmap = [Drawing.Bitmap]::new($bounds.Right - $bounds.Left, $bounds.Bottom - $bounds.Top)
try {
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $dc = $graphics.GetHdc()
        try {
            if (-not [VoltyUiPreviewCapture]::PrintWindow($window, $dc, 2)) {
                throw 'Windows could not capture the preview window.'
            }
        } finally {
            $graphics.ReleaseHdc($dc)
        }
    } finally {
        $graphics.Dispose()
    }
    $bitmap.Save($destination, [Drawing.Imaging.ImageFormat]::Png)
} finally {
    $bitmap.Dispose()
}
Write-Output $destination
