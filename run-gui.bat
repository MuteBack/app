@echo off
setlocal
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul
set "SCRIPT_DIR=%~dp0"
set "ORT_LIB_DIR=%SCRIPT_DIR%assets\vendor\onnxruntime-win-x64-1.24.2\onnxruntime-win-x64-1.24.2\lib"
set "SILERO_MODEL_PATH=%SCRIPT_DIR%assets\vendor\silero_vad.onnx"
set "SPEAKER_MODEL_PATH=%SCRIPT_DIR%assets\vendor\voxceleb_ECAPA512_LM.onnx"
set "ORT_LIB_LOCATION=%ORT_LIB_DIR%"
set "ORT_PREFER_DYNAMIC_LINK=1"
set "ORT_SKIP_DOWNLOAD=1"
set "PATH=%ORT_LIB_DIR%;%USERPROFILE%\.cargo\bin;%PATH%"
if not exist "%SILERO_MODEL_PATH%" (
    call "%SCRIPT_DIR%download-assets.bat" || exit /b 1
)
if not exist "%SPEAKER_MODEL_PATH%" (
    call "%SCRIPT_DIR%download-assets.bat" || exit /b 1
)
if not exist "%ORT_LIB_DIR%\onnxruntime.dll" (
    call "%SCRIPT_DIR%download-assets.bat" || exit /b 1
)
pushd "%SCRIPT_DIR%"
cargo run --manifest-path src-tauri\Cargo.toml
popd
