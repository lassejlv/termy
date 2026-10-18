<#
.SYNOPSIS
Embeds termy.manifest into termy.exe.

.DESCRIPTION
GPUI links its own application manifest into termy.exe, and a second manifest
cannot be linked beside it. The WinUI tab strip (tab_style = native) needs
entries GPUI's lacks: XAML Islands require maxversiontested, and the
Microsoft.UI.Xaml classes need registration-free activation. The desktop
crate's build script writes the full manifest next to the exe as
termy.manifest; this replaces the exe's manifest resource with it using the
Windows SDK's mt.exe. Without this step Termy draws its own Windows 11 style
tabs instead.

Run it after every build of termy.exe (just build, just run and
scripts/build-setup.ps1 do).

.PARAMETER ExePath
Path to termy.exe. termy.manifest must sit beside it.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath
)

$ErrorActionPreference = "Stop"

function Resolve-MtPath {
    $kitsRoot = $null
    foreach ($key in @(
        "HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots"
    )) {
        $value = (Get-ItemProperty -Path $key -Name KitsRoot10 -ErrorAction SilentlyContinue).KitsRoot10
        if ($value) {
            $kitsRoot = $value
            break
        }
    }
    if (-not $kitsRoot) {
        throw "Windows 10/11 SDK not found (no KitsRoot10 in the registry)"
    }
    $hostArch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "arm64" } else { "x64" }
    $candidates = Get-ChildItem -Path (Join-Path $kitsRoot "bin") -Directory -Filter "10.*" |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName "$hostArch\mt.exe" } |
        Where-Object { Test-Path $_ }
    $mt = $candidates | Select-Object -First 1
    if (-not $mt) {
        throw "mt.exe not found under $kitsRoot\bin"
    }
    return $mt
}

$exe = (Resolve-Path $ExePath).Path
$manifest = Join-Path (Split-Path -Parent $exe) "termy.manifest"
if (-not (Test-Path $manifest)) {
    throw "termy.manifest not found beside $exe; build termy on Windows first"
}

$mt = Resolve-MtPath
& $mt -nologo -manifest $manifest "-outputresource:$exe;#1"
if ($LASTEXITCODE -ne 0) {
    throw "mt.exe failed with exit code $LASTEXITCODE"
}
Write-Host "Embedded termy.manifest into $exe"
