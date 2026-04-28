[CmdletBinding()]
param(
    [string]$Version = "3.12",
    [int]$ChocolateyAttempts = 3
)

$ErrorActionPreference = "Stop"
if (Get-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue) {
    $PSNativeCommandUseErrorActionPreference = $false
}

function Find-Makensis {
    $command = Get-Command makensis.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    function Join-OptionalPath {
        param(
            [string]$BasePath,
            [string]$ChildPath
        )

        if ([string]::IsNullOrWhiteSpace($BasePath)) {
            return $null
        }

        Join-Path $BasePath $ChildPath
    }

    $candidateDirs = @(
        (Join-OptionalPath ${env:ProgramFiles(x86)} "NSIS"),
        (Join-OptionalPath $env:ProgramFiles "NSIS"),
        (Join-OptionalPath $env:ChocolateyInstall "bin"),
        (Join-OptionalPath $env:ChocolateyInstall "lib/nsis.install/tools")
    ) | Where-Object { $_ }

    foreach ($candidateDir in $candidateDirs) {
        $candidatePath = Join-Path $candidateDir "makensis.exe"
        if (Test-Path -LiteralPath $candidatePath -PathType Leaf) {
            return $candidatePath
        }
    }

    return $null
}

function Add-MakensisPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$MakensisPath
    )

    $makensisDir = Split-Path -Parent $MakensisPath
    $separator = [System.IO.Path]::PathSeparator
    $pathEntries = @($env:PATH -split [regex]::Escape($separator)) | Where-Object { $_ }

    if ($pathEntries -notcontains $makensisDir) {
        $env:PATH = "$makensisDir$separator$env:PATH"
    }

    if ($env:GITHUB_PATH) {
        $makensisDir | Out-File -FilePath $env:GITHUB_PATH -Append -Encoding utf8
    }

    Write-Host "Using makensis at $MakensisPath"
    & $MakensisPath /VERSION
}

$makensisPath = Find-Makensis
if ($makensisPath) {
    Add-MakensisPath $makensisPath
    return
}

if (Get-Command choco.exe -ErrorAction SilentlyContinue) {
    for ($attempt = 1; $attempt -le $ChocolateyAttempts; $attempt++) {
        Write-Host "Installing NSIS with Chocolatey, attempt $attempt of $ChocolateyAttempts..."
        & choco install nsis -y --no-progress
        $chocoExitCode = $LASTEXITCODE

        $makensisPath = Find-Makensis
        if ($makensisPath) {
            if ($chocoExitCode -ne 0) {
                Write-Warning "Chocolatey exited with $chocoExitCode, but makensis.exe was installed."
            }

            Add-MakensisPath $makensisPath
            return
        }

        Write-Warning "Chocolatey NSIS install failed with exit code $chocoExitCode."
        if ($attempt -lt $ChocolateyAttempts) {
            Start-Sleep -Seconds ([Math]::Min(60, 10 * $attempt))
        }
    }
} else {
    Write-Warning "Chocolatey is not available on PATH."
}

$downloadRoot = if ($env:RUNNER_TOOL_CACHE) {
    Join-Path $env:RUNNER_TOOL_CACHE "nsis"
} else {
    Join-Path ([System.IO.Path]::GetTempPath()) "muteback-nsis"
}

$zipPath = if ($env:RUNNER_TEMP) {
    Join-Path $env:RUNNER_TEMP "nsis-$Version.zip"
} else {
    Join-Path ([System.IO.Path]::GetTempPath()) "nsis-$Version.zip"
}

$cachedMakensisPath = if (Test-Path -LiteralPath $downloadRoot -PathType Container) {
    Get-ChildItem -LiteralPath $downloadRoot -Recurse -Filter makensis.exe -File |
        Where-Object { $_.FullName -match [regex]::Escape("nsis-$Version") } |
        Select-Object -First 1 -ExpandProperty FullName
}

if ($cachedMakensisPath) {
    Add-MakensisPath $cachedMakensisPath
    return
}

$downloadUrls = @(
    "https://downloads.sourceforge.net/project/nsis/NSIS%203/$Version/nsis-$Version.zip",
    "https://sourceforge.net/projects/nsis/files/NSIS%203/$Version/nsis-$Version.zip/download"
)

New-Item -ItemType Directory -Path $downloadRoot -Force | Out-Null

$downloaded = $false
foreach ($downloadUrl in $downloadUrls) {
    try {
        Write-Host "Downloading portable NSIS $Version from $downloadUrl"
        Invoke-WebRequest -Uri $downloadUrl -OutFile $zipPath -MaximumRedirection 10
        $downloaded = $true
        break
    } catch {
        Write-Warning "NSIS download failed from $downloadUrl`: $($_.Exception.Message)"
    }
}

if (-not $downloaded) {
    throw "Unable to download portable NSIS $Version."
}

Expand-Archive -LiteralPath $zipPath -DestinationPath $downloadRoot -Force

$makensisPath = Get-ChildItem -LiteralPath $downloadRoot -Recurse -Filter makensis.exe -File |
    Where-Object { $_.FullName -match [regex]::Escape("nsis-$Version") } |
    Select-Object -First 1 -ExpandProperty FullName

if (-not $makensisPath) {
    throw "Portable NSIS $Version was downloaded, but makensis.exe was not found under $downloadRoot."
}

Add-MakensisPath $makensisPath
