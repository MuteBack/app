$ErrorActionPreference = "Stop"
if (Get-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$script:RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$script:OnnxRuntimeVersion = "1.24.2"
$script:OnnxRuntimeFolder = "onnxruntime-win-x64-$script:OnnxRuntimeVersion"

function Join-PathParts {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Parts
    )

    [System.IO.Path]::Combine($Parts)
}

function Get-RepoPath {
    param(
        [Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)]
        [string[]]$Parts
    )

    Join-PathParts (@($script:RepoRoot) + $Parts)
}

function Get-VendorDir {
    Get-RepoPath "assets" "vendor"
}

function Get-OnnxRuntimeLibDir {
    Get-RepoPath "assets" "vendor" $script:OnnxRuntimeFolder $script:OnnxRuntimeFolder "lib"
}

function Get-SileroModelPath {
    Get-RepoPath "assets" "vendor" "silero_vad.onnx"
}

function Get-SpeakerModelPath {
    Get-RepoPath "assets" "vendor" "voxceleb_ECAPA512_LM.onnx"
}

function Get-OnnxRuntimeDllPath {
    Join-Path (Get-OnnxRuntimeLibDir) "onnxruntime.dll"
}

function Add-PathEntry {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }

    $separator = [System.IO.Path]::PathSeparator
    $entries = @($env:PATH -split [regex]::Escape($separator)) | Where-Object { $_ }
    if ($entries -notcontains $Path) {
        $env:PATH = "$Path$separator$env:PATH"
    }
}

function Set-MuteBackNativeEnv {
    $ortLibDir = Get-OnnxRuntimeLibDir

    $env:SILERO_MODEL_PATH = Get-SileroModelPath
    $env:SPEAKER_MODEL_PATH = Get-SpeakerModelPath
    $env:ORT_LIB_LOCATION = $ortLibDir
    $env:ORT_DYLIB_PATH = Get-OnnxRuntimeDllPath
    $env:ORT_PREFER_DYNAMIC_LINK = "1"
    $env:ORT_SKIP_DOWNLOAD = "1"

    Add-PathEntry $ortLibDir
}

function Test-MuteBackNativeAssets {
    (Test-Path -LiteralPath (Get-SileroModelPath)) -and
    (Test-Path -LiteralPath (Get-SpeakerModelPath)) -and
    (Test-Path -LiteralPath (Get-OnnxRuntimeDllPath))
}

function Ensure-MuteBackNativeAssets {
    if (Test-MuteBackNativeAssets) {
        return
    }

    & (Get-RepoPath "scripts" "download-assets.ps1")
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

function Invoke-CheckedCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Command,

        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]]$Arguments
    )

    Write-Host "+ $Command $($Arguments -join ' ')"
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}
