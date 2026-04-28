[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

Ensure-MuteBackNativeAssets
Set-MuteBackNativeEnv

Invoke-CheckedCommand cargo check
Invoke-CheckedCommand cargo check --manifest-path (Get-RepoPath "src-tauri" "Cargo.toml")
