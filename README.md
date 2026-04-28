# MuteBack MVP

MuteBack is a Rust desktop app that lowers background audio while the user is speaking, then restores audio when the speaking session is truly over.

The first target is Windows. The design should stay cross-platform so we can add a macOS version without rewriting the core logic.

Project scripts live in [scripts](scripts) and are written for PowerShell Core. Use `pwsh` on Windows, Linux, and macOS.

Start here:

- [MVP Design](docs/MVP.md)
- [VAD And Ducking Options](docs/VAD_AND_DUCKING.md)

## Try The Console Prototype

The current console prototype is Windows-only.

- It listens to the default microphone.
- It uses `Silero VAD` on a resampled `16 kHz` mono stream.
- It lowers the default Windows output device volume while you speak.
- It restores the previous volume after you stop speaking.
- Press `Enter` to stop and restore the volume.

Run it with:

- `pwsh ./scripts/run.ps1 -Console`

Default settings:

- smooth fade down to `10%` of the previous volume
- `180 ms` fade down
- `260 ms` restore fade

Useful script commands:

- `pwsh ./scripts/download-assets.ps1`
- `pwsh ./scripts/ci-check.ps1`
- `pwsh ./scripts/bump-version.ps1 0.1.2`
- `pwsh ./scripts/package-windows.ps1`

After the first build, you can also run the compiled binary directly:

- `.\target\debug\muteback.exe`

The local setup expects the Silero model, WeSpeaker speaker embedding model, and ONNX Runtime files in [assets/vendor](assets/vendor). `scripts/run.ps1`, `scripts/ci-check.ps1`, and `scripts/package-windows.ps1` set the required environment variables and PATH entries for you.

On a fresh checkout, download the local model/runtime assets with:

- `pwsh ./scripts/download-assets.ps1`

The run, check, and package scripts also call this downloader automatically if a required asset is missing.

## Try The MuteBack GUI

Tauri supports tray apps on Windows and macOS. The GUI is now the main app entry point: it starts the background microphone/VAD/ducking runtime by default and keeps running from the tray.

Run it with:

- `pwsh ./scripts/run.ps1`

Current GUI status:

- tray icon with `Open MuteBack` and `Quit`
- close button hides the window instead of quitting
- starts the background ducking runtime by default
- compact main screen with only app status, enable toggle, and settings button
- larger settings page for ducking level, smooth/instant transition, fade timings, manual restore, and voice onboarding
- manual restore button appears in a separate always-on-top OS window when audio is currently lowered
- optional voice onboarding flow with three local recording samples
- onboarding records short local PCM samples through the WebView, embeds them with the local WeSpeaker ECAPA ONNX model, and stores the derived voice profile plus local sample audio for playback/removal
- optional speaker verification gates ducking session start when a voice profile is enrolled and Voice Match is enabled
- settings persistence now uses `tauri-plugin-store` with one-time migration from the legacy `settings.json` file
- updater plugin is scaffolded and exposed through a backend check command, but remains inactive until release endpoints/signing are configured

Current limitations:

- no push-to-talk yet
- endpoint-level ducking only
- still using default microphone and default output device only
