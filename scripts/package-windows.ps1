[CmdletBinding()]
param(
    [string]$Bundles = "nsis",
    [string]$DistDir,
    [switch]$SkipBuild,
    [switch]$SkipTauriCliInstall
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

$isWindowsPlatform = if (Get-Variable -Name IsWindows -ErrorAction SilentlyContinue) {
    $IsWindows
} else {
    $env:OS -eq "Windows_NT"
}

if (-not $isWindowsPlatform) {
    throw "Windows packaging must run on Windows because the NSIS installer target is Windows-only."
}

if (-not $DistDir) {
    $DistDir = Get-RepoPath "dist" "windows"
} elseif (-not [System.IO.Path]::IsPathRooted($DistDir)) {
    $DistDir = Get-RepoPath $DistDir
}

Write-Host "Windows package output directory: $DistDir"

function Test-TauriCliInstalled {
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & cargo tauri --version *> $null
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }

    $exitCode -eq 0
}

Ensure-MuteBackNativeAssets
Set-MuteBackNativeEnv

if (-not $SkipBuild) {
    if (-not $SkipTauriCliInstall) {
        if (-not (Test-TauriCliInstalled)) {
            Invoke-CheckedCommand cargo install tauri-cli --version "^2" --locked
        }
    }

    Invoke-CheckedCommand cargo tauri build --bundles $Bundles
}

$knownBundleDirs = @(
    (Get-RepoPath "src-tauri" "target" "release" "bundle" "nsis"),
    (Get-RepoPath "target" "release" "bundle" "nsis")
)

$bundleRoots = @(
    (Get-RepoPath "src-tauri" "target"),
    (Get-RepoPath "target")
)

$discoveredBundleDirs = foreach ($bundleRoot in $bundleRoots) {
    if (Test-Path -LiteralPath $bundleRoot) {
        Get-ChildItem -LiteralPath $bundleRoot -Directory -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -eq "nsis" -and $_.Parent.Name -eq "bundle" } |
            ForEach-Object { $_.FullName }
    }
}

$candidateBundleDirs = @($knownBundleDirs + $discoveredBundleDirs) |
    Where-Object { $_ } |
    Select-Object -Unique

$installers = @()
foreach ($bundleDir in $candidateBundleDirs) {
    if (Test-Path -LiteralPath $bundleDir) {
        $installers += Get-ChildItem -LiteralPath $bundleDir -Filter "*.exe" -File
    }
}

if ($installers.Count -eq 0) {
    $searched = $candidateBundleDirs -join ", "
    throw "No Windows installer was produced. Searched: $searched."
}

New-Item -ItemType Directory -Path $DistDir -Force | Out-Null
Get-ChildItem -LiteralPath $DistDir -File | Remove-Item -Force

$stagedInstallers = foreach ($installer in $installers) {
    $targetPath = Join-Path $DistDir $installer.Name
    Copy-Item -LiteralPath $installer.FullName -Destination $targetPath -Force
    Get-Item -LiteralPath $targetPath
}

$checksums = $stagedInstallers | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $($_.Name)"
}

$checksumsPath = Join-Path $DistDir "SHA256SUMS.txt"
[System.IO.File]::WriteAllLines($checksumsPath, $checksums, [System.Text.Encoding]::ASCII)
Write-Host "Staged Windows package files in $DistDir"
Write-Host "Wrote $checksumsPath"

Get-ChildItem -LiteralPath $DistDir -File | ForEach-Object {
    Write-Host " - $($_.FullName)"
}
