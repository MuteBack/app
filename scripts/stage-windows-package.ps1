[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$DistDir
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")

if (-not [System.IO.Path]::IsPathRooted($DistDir)) {
    $DistDir = Get-RepoPath $DistDir
}

$DistDir = [System.IO.Path]::GetFullPath($DistDir)
Write-Host "Staging Windows package into $DistDir"

$bundleRoots = @(
    (Get-RepoPath "src-tauri" "target"),
    (Get-RepoPath "target")
)

$bundleDirs = foreach ($bundleRoot in $bundleRoots) {
    if (Test-Path -LiteralPath $bundleRoot -PathType Container) {
        Get-ChildItem -LiteralPath $bundleRoot -Directory -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -eq "nsis" -and $_.Parent.Name -eq "bundle" } |
            ForEach-Object { $_.FullName }
    }
}

$bundleDirs = @($bundleDirs) | Where-Object { $_ } | Select-Object -Unique

if ($bundleDirs.Count -eq 0) {
    Write-Host "No NSIS bundle directories were found under target roots."
    Write-Host "Bundle outputs found under target directories:"
    Get-ChildItem -Path $bundleRoots -Recurse -File -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match "[\\/](bundle)[\\/]" } |
        Select-Object FullName, Length |
        Format-Table -AutoSize
    throw "No NSIS bundle directories were found."
}

Write-Host "NSIS bundle directories:"
$bundleDirs | ForEach-Object { Write-Host " - $_" }

$installers = foreach ($bundleDir in $bundleDirs) {
    Get-ChildItem -LiteralPath $bundleDir -Filter "*.exe" -File -ErrorAction SilentlyContinue
}

$installers = @($installers) | Sort-Object LastWriteTimeUtc -Descending

if ($installers.Count -eq 0) {
    Write-Host "No .exe installers were found in NSIS bundle directories."
    Write-Host "Bundle outputs found under target directories:"
    Get-ChildItem -Path $bundleRoots -Recurse -File -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match "[\\/](bundle)[\\/]" } |
        Select-Object FullName, Length |
        Format-Table -AutoSize
    throw "No Windows installer was produced."
}

New-Item -ItemType Directory -Path $DistDir -Force | Out-Null
Get-ChildItem -LiteralPath $DistDir -File -ErrorAction SilentlyContinue | Remove-Item -Force

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

$packageFiles = @(Get-ChildItem -LiteralPath $DistDir -File)

if (-not ($packageFiles | Where-Object { $_.Extension -ieq ".exe" })) {
    throw "No .exe installer found in $DistDir"
}

if (-not (Test-Path -LiteralPath $checksumsPath -PathType Leaf)) {
    throw "Missing SHA256SUMS.txt in $DistDir"
}

Write-Host "Staged Windows package files:"
$packageFiles | Format-Table -AutoSize
