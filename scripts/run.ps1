[CmdletBinding()]
param(
    [switch]$Console,
    [switch]$Release,

    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$AppArgs
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

Ensure-MuteBackNativeAssets
Set-MuteBackNativeEnv

$cargoArgs = @("run")
if (-not $Console) {
    $cargoArgs += "--manifest-path"
    $cargoArgs += (Get-RepoPath "src-tauri" "Cargo.toml")
}
if ($Release) {
    $cargoArgs += "--release"
}
if ($AppArgs.Count -gt 0) {
    $cargoArgs += "--"
    $cargoArgs += $AppArgs
}

Invoke-CheckedCommand cargo @cargoArgs
