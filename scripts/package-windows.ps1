[CmdletBinding()]
param(
    [string]$Bundles = "nsis",
    [switch]$SkipTauriCliInstall
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

if (-not $IsWindows) {
    throw "Windows packaging must run on Windows because the NSIS installer target is Windows-only."
}

Ensure-MuteBackNativeAssets
Set-MuteBackNativeEnv

if (-not $SkipTauriCliInstall) {
    & cargo tauri --version *> $null
    if ($LASTEXITCODE -ne 0) {
        Invoke-CheckedCommand cargo install tauri-cli --version "^2" --locked
    }
}

Invoke-CheckedCommand cargo tauri build --bundles $Bundles

$bundleDir = Get-RepoPath "src-tauri" "target" "release" "bundle" "nsis"
$checksumsPath = Join-Path $bundleDir "SHA256SUMS.txt"
$installers = Get-ChildItem -LiteralPath $bundleDir -Filter "*.exe"

if ($installers.Count -eq 0) {
    throw "No Windows installer was produced in $bundleDir."
}

$checksums = $installers | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $($_.Name)"
}

[System.IO.File]::WriteAllLines($checksumsPath, $checksums, [System.Text.Encoding]::ASCII)
Write-Host "Wrote $checksumsPath"
