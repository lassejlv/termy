param(
    [string]$Version,
    [ValidateSet("x64", "arm64")]
    [string]$Arch,
    [string]$Target,
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Get-CargoPackageVersion {
    param([string]$CargoTomlPath)

    $inPackage = $false
    foreach ($line in Get-Content $CargoTomlPath) {
        if ($line -match '^\s*\[package\]\s*$') {
            $inPackage = $true
            continue
        }

        if ($inPackage -and $line -match '^\s*\[') {
            break
        }

        if ($inPackage -and $line -match '^\s*version\s*=\s*"([^"]+)"\s*$') {
            return $Matches[1]
        }
    }

    throw "Could not determine package version from $CargoTomlPath"
}

function Resolve-IsccPath {
    $iscc = Get-Command iscc -ErrorAction SilentlyContinue
    if ($iscc) {
        return $iscc.Source
    }

    $registryKeys = @(
        "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
        "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 5_is1",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 5_is1"
    )

    foreach ($key in $registryKeys) {
        try {
            $item = Get-ItemProperty -Path $key -ErrorAction Stop
            if ($item.InstallLocation) {
                $candidate = Join-Path $item.InstallLocation "ISCC.exe"
                if (Test-Path $candidate) {
                    return $candidate
                }
            }
            if ($item.DisplayIcon) {
                $iconPath = ($item.DisplayIcon -split ',')[0].Trim('"')
                if ($iconPath -and (Test-Path $iconPath)) {
                    return $iconPath
                }
            }
        } catch {
            # Continue to fallback locations.
        }
    }

    $candidates = New-Object System.Collections.Generic.List[string]
    if (${env:ProgramFiles(x86)}) {
        $candidates.Add((Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"))
        $candidates.Add((Join-Path ${env:ProgramFiles(x86)} "Inno Setup 5\ISCC.exe"))
    }
    if ($env:ProgramFiles) {
        $candidates.Add((Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe"))
        $candidates.Add((Join-Path $env:ProgramFiles "Inno Setup 5\ISCC.exe"))
    }
    if ($env:LOCALAPPDATA) {
        $candidates.Add((Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"))
    }

    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) {
            return $candidate
        }
    }

    throw "ISCC.exe not found. Install Inno Setup and ensure 'iscc' is on PATH."
}

function Get-DefaultTarget {
    $hostLine = (& rustc -vV | Select-String '^host:\s+').ToString()
    if (-not $hostLine) {
        return "x86_64-pc-windows-msvc"
    }

    $rustHostTriple = $hostLine -replace '^host:\s+', ''
    if ($rustHostTriple -like '*-pc-windows-msvc') {
        return $rustHostTriple
    }

    return "x86_64-pc-windows-msvc"
}

function Arch-FromTarget {
    param([string]$TargetTriple)

    if ($TargetTriple -like 'x86_64-*') { return "x64" }
    if ($TargetTriple -like 'aarch64-*') { return "arm64" }
    throw "Cannot infer architecture from target '$TargetTriple'. Set -Arch explicitly."
}

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $repoRoot

$cargoToml = Join-Path $repoRoot "crates\desktop_app\Cargo.toml"
$issPath = Join-Path $repoRoot "scripts\installer\termy.iss"
$iconPath = Join-Path $repoRoot "assets\termy.ico"

if (-not (Test-Path $cargoToml)) {
    throw "Desktop app Cargo.toml not found at $cargoToml"
}

if (-not (Test-Path $issPath)) {
    throw "Inno Setup script not found at $issPath"
}

if (-not (Test-Path $iconPath)) {
    throw "Windows installer icon not found at $iconPath. Generate it with scripts/generate-icon.sh."
}

if (-not $Version) {
    $Version = Get-CargoPackageVersion -CargoTomlPath $cargoToml
}

# Release builds pass the tag (v1.2.3). The output file keeps it so the asset
# name matches the tag; the version Windows shows has no "v".
$appVersion = $Version -replace '^v', ''

if (-not $Target) {
    $Target = Get-DefaultTarget
}

if (-not $Target.EndsWith("-pc-windows-msvc")) {
    throw "Windows installer currently supports MSVC targets only. Got: $Target"
}

if (-not $Arch) {
    $Arch = Arch-FromTarget -TargetTriple $Target
}

$exePath = Join-Path $repoRoot "target\$Target\release\termy.exe"
$cliExePath = Join-Path $repoRoot "target\$Target\release\termy-cli.exe"

if (-not $NoBuild) {
    Write-Host "Building termy.exe and termy-cli.exe for target '$Target'..."
    & cargo build --release --target $Target -p termy -p termy_cli
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
}

if (-not (Test-Path $exePath)) {
    throw "Expected binary not found at $exePath"
}
if (-not (Test-Path $cliExePath)) {
    throw "Expected CLI binary not found at $cliExePath"
}

# The WinUI tab strip needs termy.manifest in termy.exe and the WinUI 2
# runtime the desktop crate's build script stages beside it.
$releaseDir = Split-Path -Parent $exePath
foreach ($winuiFile in @("Microsoft.UI.Xaml.dll", "resources.pri", "termy.manifest")) {
    if (-not (Test-Path (Join-Path $releaseDir $winuiFile))) {
        throw "Expected $winuiFile beside termy.exe; rebuild termy for target '$Target'"
    }
}
& (Join-Path $PSScriptRoot "windows\embed-manifest.ps1") -ExePath $exePath

$isccPath = Resolve-IsccPath
Write-Host "Using ISCC at $isccPath"
Write-Host "Packaging Termy $appVersion ($Arch)..."

# Optional: release CI drops the ConPTY NuGet binaries beside termy.exe.
foreach ($conptyFile in @("conpty.dll", "OpenConsole.exe")) {
    if (Test-Path (Join-Path $releaseDir $conptyFile)) {
        Write-Host "Bundling $conptyFile"
    } else {
        Write-Host "No $conptyFile beside termy.exe; the installer relies on the system ConPTY"
    }
}

& $isccPath `
    "/DMyAppVersion=$appVersion" `
    "/DMyOutputVersion=$Version" `
    "/DMyArch=$Arch" `
    "/DMyTarget=$Target" `
    "/DMyExeName=termy.exe" `
    "/DMyCliExeName=termy-cli.exe" `
    $issPath

if ($LASTEXITCODE -ne 0) {
    throw "ISCC failed with exit code $LASTEXITCODE"
}

$outputFile = Join-Path $repoRoot "target\dist\Termy-$Version-windows-$Arch-Setup.exe"
if (-not (Test-Path $outputFile)) {
    throw "Installer build finished, but expected output was not found: $outputFile"
}

Write-Host "Done: $outputFile"
