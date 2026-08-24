[CmdletBinding()]
param(
    [ValidatePattern('^[A-Za-z0-9_]+-pc-windows-msvc$')]
    [string]$TargetTriple = "x86_64-pc-windows-msvc",

    [ValidateSet("Debug", "Release")]
    [string]$Profile = "Release",

    [ValidatePattern('^[A-Za-z0-9._-]+$')]
    [string]$BuildNumber = "dev"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($env:OS -ne "Windows_NT") {
    throw "The MochiPort Windows sidecar must be built on Windows with the MSVC toolchain."
}

$appRoot = Split-Path -Parent $PSScriptRoot
$repoRoot = (Resolve-Path (Join-Path $appRoot "..\..")).Path
$manifestPath = Join-Path $repoRoot "Cargo.toml"
$targetRoot = Join-Path $repoRoot "target"
$profileDirectory = if ($Profile -eq "Release") { "release" } else { "debug" }
$sourceBinary = Join-Path $targetRoot "$TargetTriple\$profileDirectory\mochiport.exe"
$binaryDirectory = Join-Path $appRoot "src-tauri\binaries"
$sidecarBinary = Join-Path $binaryDirectory "mochiport-daemon-$TargetTriple.exe"

if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Root Cargo manifest was not found at $manifestPath"
}

$cargoArguments = @(
    "build",
    "--manifest-path", $manifestPath,
    "--target-dir", $targetRoot,
    "--target", $TargetTriple,
    "--bin", "mochiport",
    "--locked"
)
if ($Profile -eq "Release") {
    $cargoArguments += "--release"
}

$previousBuildNumber = $env:MOCHIPORT_DAEMON_BUILD_NUMBER
try {
    $env:MOCHIPORT_DAEMON_BUILD_NUMBER = $BuildNumber
    & cargo @cargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
}
finally {
    $env:MOCHIPORT_DAEMON_BUILD_NUMBER = $previousBuildNumber
}

if (-not (Test-Path -LiteralPath $sourceBinary -PathType Leaf)) {
    throw "Cargo completed without producing $sourceBinary"
}

New-Item -ItemType Directory -Path $binaryDirectory -Force | Out-Null
Copy-Item -LiteralPath $sourceBinary -Destination $sidecarBinary -Force

$hash = (Get-FileHash -LiteralPath $sidecarBinary -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host "Tauri sidecar: $sidecarBinary"
Write-Host "SHA256: $hash"
Write-Output $sidecarBinary
