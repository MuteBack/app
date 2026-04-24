# MuteBack MVP

MuteBack is a Rust desktop app that lowers background audio while the user is speaking, then restores audio when the speaking session is truly over.

The first target is Windows. The design should stay cross-platform so we can add a macOS version without rewriting the core logic.

Start here:

- [MVP Design](C:\Users\marci\Documents\New project\docs\MVP.md)
- [VAD And Ducking Options](C:\Users\marci\Documents\New project\docs\VAD_AND_DUCKING.md)

## Try The Console Prototype

The current console prototype is Windows-only.

- It listens to the default microphone.
- It uses `Silero VAD` on a resampled `16 kHz` mono stream.
- It lowers the default Windows output device volume while you speak.
- It restores the previous volume after you stop speaking.
- Press `Enter` to stop and restore the volume.

Run it with:

- `.\run-console.bat`

Default settings:

- smooth fade down to `10%` of the previous volume
- `180 ms` fade down
- `260 ms` restore fade

Useful test commands:

- `.\run-console.bat --transition instant`
- `.\run-console.bat --duck-level 0`
- `.\run-console.bat --restore-mode manual`
- `.\run-console.bat --duck-level 20 --duck-fade-ms 300 --restore-fade-ms 450`

After the first build, you can also run the compiled binary directly:

- `.\target\debug\muteback.exe`

The local Windows setup expects the Silero model and ONNX Runtime files in [assets/vendor](C:\Users\marci\Documents\New project\assets\vendor). `run-console.bat` sets the required environment variables and PATH entries for you.

## Try The MuteBack GUI

Tauri supports tray apps on Windows and macOS. The GUI is now the main app entry point: it starts the background microphone/VAD/ducking runtime by default and keeps running from the tray.

Run it with:

- `.\run-gui.bat`

Current GUI status:

- tray icon with `Open MuteBack` and `Quit`
- close button hides the window instead of quitting
- starts the background ducking runtime by default
- compact main screen with only app status, enable toggle, and settings button
- larger settings page for ducking level, smooth/instant transition, fade timings, manual restore, and voice onboarding
- manual restore button appears in a separate always-on-top OS window when audio is currently lowered
- optional voice onboarding flow with three local recording samples
- onboarding currently records through the WebView, then discards raw audio and stores only sample metadata in memory
- speaker verification is not implemented yet; voice onboarding is preparing the UI/data path

Current limitations:

- no push-to-talk yet
- endpoint-level ducking only
- still using default microphone and default output device only
