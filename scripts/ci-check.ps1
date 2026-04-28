[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

Ensure-MuteBackNativeAssets
Set-MuteBackNativeEnv

function Get-TomlPackageVersion([string]$Path) {
    $content = [System.IO.File]::ReadAllText($Path)
    $match = [regex]::Match($content, '(?m)^version\s*=\s*"([^"]+)"')
    if (-not $match.Success) {
        throw "Could not find package version in $Path."
    }

    $match.Groups[1].Value
}

function Assert-VersionSync {
    $rootVersion = Get-TomlPackageVersion (Get-RepoPath "Cargo.toml")
    $tauriCrateVersion = Get-TomlPackageVersion (Get-RepoPath "src-tauri" "Cargo.toml")
    $tauriConfig = Get-Content -LiteralPath (Get-RepoPath "src-tauri" "tauri.conf.json") -Raw | ConvertFrom-Json
    $tauriConfigVersion = $tauriConfig.version

    if ($rootVersion -ne $tauriCrateVersion -or $rootVersion -ne $tauriConfigVersion) {
        throw "Version mismatch: Cargo.toml=$rootVersion, src-tauri/Cargo.toml=$tauriCrateVersion, src-tauri/tauri.conf.json=$tauriConfigVersion. Run scripts/bump-version.ps1."
    }

    Write-Host "Versions are in sync: $rootVersion"
}

Assert-VersionSync

Invoke-CheckedCommand cargo check
Invoke-CheckedCommand cargo check --manifest-path (Get-RepoPath "src-tauri" "Cargo.toml")
