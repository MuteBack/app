[CmdletBinding()]
param(
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$VendorDir = Join-Path $RepoRoot "assets\vendor"
$OnnxRuntimeVersion = "1.24.2"
$OnnxRuntimeFolder = "onnxruntime-win-x64-$OnnxRuntimeVersion"

New-Item -ItemType Directory -Path $VendorDir -Force | Out-Null

$Assets = @(
    @{
        Name = "Silero VAD ONNX"
        Url = "https://raw.githubusercontent.com/snakers4/silero-vad/master/src/silero_vad/data/silero_vad.onnx"
        Path = Join-Path $VendorDir "silero_vad.onnx"
        Sha256 = "1A153A22F4509E292A94E67D6F9B85E8DEB25B4988682B7E174C65279D8788E3"
    },
    @{
        Name = "WeSpeaker ECAPA speaker embedding ONNX"
        Url = "https://huggingface.co/Wespeaker/wespeaker-ecapa-tdnn512-LM/resolve/main/voxceleb_ECAPA512_LM.onnx?download=true"
        Path = Join-Path $VendorDir "voxceleb_ECAPA512_LM.onnx"
        Sha256 = "D71B85D9B48058EF68004F04F1B78ACEBEFB9DFCF542E19B976A12A5AD1F10B0"
    },
    @{
        Name = "ONNX Runtime Windows x64"
        Url = "https://github.com/microsoft/onnxruntime/releases/download/v$OnnxRuntimeVersion/onnxruntime-win-x64-$OnnxRuntimeVersion.zip"
        Path = Join-Path $VendorDir "onnxruntime-win-x64-$OnnxRuntimeVersion.zip"
        Sha256 = "8E3E9C826375352E29CB2614FE44F3D7A4B0FF7B8028AD7A456AF9D949A7E8B0"
    }
)

function Get-Sha256([string]$Path) {
    (Get-FileHash -Path $Path -Algorithm SHA256).Hash.ToUpperInvariant()
}

function Test-AssetHash($Asset) {
    if (-not (Test-Path -LiteralPath $Asset.Path)) {
        return $false
    }

    return (Get-Sha256 $Asset.Path) -eq $Asset.Sha256
}

function Save-Asset($Asset) {
    if (-not $Force -and (Test-AssetHash $Asset)) {
        Write-Host "OK   $($Asset.Name)"
        return
    }

    $targetDir = Split-Path -Parent $Asset.Path
    New-Item -ItemType Directory -Path $targetDir -Force | Out-Null

    $tempPath = "$($Asset.Path).download"
    if (Test-Path -LiteralPath $tempPath) {
        Remove-Item -LiteralPath $tempPath -Force
    }

    Write-Host "GET  $($Asset.Name)"
    Invoke-WebRequest -Uri $Asset.Url -OutFile $tempPath -UseBasicParsing

    $hash = Get-Sha256 $tempPath
    if ($hash -ne $Asset.Sha256) {
        Remove-Item -LiteralPath $tempPath -Force
        throw "Hash mismatch for $($Asset.Name). Expected $($Asset.Sha256), got $hash."
    }

    Move-Item -LiteralPath $tempPath -Destination $Asset.Path -Force
    Write-Host "OK   $($Asset.Name)"
}

function Expand-OnnxRuntime {
    $zipPath = Join-Path $VendorDir "onnxruntime-win-x64-$OnnxRuntimeVersion.zip"
    $extractRoot = Join-Path $VendorDir $OnnxRuntimeFolder
    $dllPath = Join-Path $extractRoot "$OnnxRuntimeFolder\lib\onnxruntime.dll"

    if (-not $Force -and (Test-Path -LiteralPath $dllPath)) {
        Write-Host "OK   ONNX Runtime extracted"
        return
    }

    Write-Host "ZIP  ONNX Runtime"
    New-Item -ItemType Directory -Path $extractRoot -Force | Out-Null
    Expand-Archive -LiteralPath $zipPath -DestinationPath $extractRoot -Force

    if (-not (Test-Path -LiteralPath $dllPath)) {
        throw "ONNX Runtime extraction did not produce $dllPath."
    }

    Write-Host "OK   ONNX Runtime extracted"
}

foreach ($asset in $Assets) {
    Save-Asset $asset
}

Expand-OnnxRuntime

Write-Host "Assets are ready in $VendorDir"
