[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidatePattern('^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$')]
    [string]$Version
)

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$Utf8NoBom = [System.Text.UTF8Encoding]::new($false)

function Get-RepoPath([string]$RelativePath) {
    Join-Path $RepoRoot $RelativePath
}

function Read-RepoText([string]$RelativePath) {
    [System.IO.File]::ReadAllText((Get-RepoPath $RelativePath))
}

function Write-RepoText([string]$RelativePath, [string]$Content) {
    [System.IO.File]::WriteAllText((Get-RepoPath $RelativePath), $Content, $Utf8NoBom)
}

function Update-ManifestVersion([string]$RelativePath) {
    $content = Read-RepoText $RelativePath
    $regex = [regex]::new('(?m)^version\s*=\s*"[^"]+"')
    if (-not $regex.IsMatch($content)) {
        throw "Could not find package version in $RelativePath."
    }

    $updated = $regex.Replace($content, "version = `"$Version`"", 1)
    Write-RepoText $RelativePath $updated
    Write-Host "Updated $RelativePath"
}

function Update-TauriConfigVersion([string]$RelativePath) {
    $content = Read-RepoText $RelativePath
    $regex = [regex]::new('(?m)^(\s*"version"\s*:\s*)"[^"]+"')
    if (-not $regex.IsMatch($content)) {
        throw "Could not find Tauri config version in $RelativePath."
    }

    $updated = $regex.Replace($content, ('$1"' + $Version + '"'), 1)
    Write-RepoText $RelativePath $updated
    Write-Host "Updated $RelativePath"
}

function Update-LockPackageVersion([string]$RelativePath, [string]$PackageName) {
    $content = Read-RepoText $RelativePath
    $escapedPackageName = [regex]::Escape($PackageName)
    $regex = [regex]::new("(?m)(\[\[package\]\]\r?\nname = `"$escapedPackageName`"\r?\nversion = `")[^`"]+(`")")
    if (-not $regex.IsMatch($content)) {
        throw "Could not find package '$PackageName' in $RelativePath."
    }

    $updated = $regex.Replace($content, ('${1}' + $Version + '${2}'), 1)
    Write-RepoText $RelativePath $updated
    Write-Host "Updated $RelativePath package $PackageName"
}

Update-ManifestVersion "Cargo.toml"
Update-ManifestVersion "src-tauri/Cargo.toml"
Update-TauriConfigVersion "src-tauri/tauri.conf.json"
Update-LockPackageVersion "Cargo.lock" "muteback"
Update-LockPackageVersion "src-tauri/Cargo.lock" "muteback"
Update-LockPackageVersion "src-tauri/Cargo.lock" "muteback-tauri"

Write-Host "Version bumped to $Version"
