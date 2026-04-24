# VAD And Windows Ducking Strategy

## Purpose

This document narrows two of the biggest technical decisions for the Windows-first MVP:

- which voice activity detection approach to use
- how to lower system audio in a way that is useful and realistic

The goal is not to choose the most advanced option on paper. The goal is to choose the option that gives us the best chance of shipping a stable Windows v1 in Rust while keeping the core portable for macOS later.

## Decision Summary

Recommended path:

- ship `push-to-talk` and `auto mode`
- implement a pluggable `VadEngine` interface in shared Rust code
- use `WebRTC VAD + RMS energy gate + hold-state hysteresis` as the first auto-mode engine
- use Windows device-level ducking first for internal builds
- keep a second ducking backend for session-level ducking as the intended upgrade path

This gives us:

- a small and fast first implementation
- low CPU usage
- predictable real-time behavior
- a clean fallback if auto mode is not good enough yet
- a path to better Windows behavior without rewriting the app

## VAD Requirements

The VAD layer is not trying to transcribe speech. It only needs to answer:

- is the user probably speaking right now
- should ducking start
- should ducking stay active
- should we move from `Talking` to `Hold`

For the MVP, the VAD engine should be:

- local only
- low latency
- stable on long-running desktop sessions
- lightweight enough for a tray app
- usable from Rust on Windows now
- portable enough to reuse on macOS later

## VAD Option 1: WebRTC VAD

### What it is

WebRTC VAD is a classic real-time speech detector widely used in communication systems. In Rust, it can be integrated through a wrapper around `libfvad` or a similar native binding.

### Pros

- very fast and low CPU
- small dependency footprint
- good fit for short audio frames
- deterministic behavior
- easy to run fully offline
- simpler packaging than a model runtime

### Cons

- weaker in noisy real-world desktop conditions than stronger model-based VADs
- may false-trigger on keyboard noise, breathing, or speaker bleed
- gives less rich output than a probability-based model
- usually needs tighter frame-size and sample-rate handling

### Best use

- first production attempt for auto mode
- default engine for headset-first Windows usage
- base engine combined with additional heuristics

## VAD Option 2: Silero VAD via ONNX Runtime

### What it is

Silero VAD is a small pretrained model that can run locally with ONNX Runtime.

### Pros

- generally stronger speech detection quality than classic rule-based VAD
- better tolerance for varied speakers and noisy conditions
- probability-style output is easier to combine with hysteresis
- still lightweight enough for local inference

### Cons

- more packaging complexity
- larger runtime surface area
- more moving parts in a tray app installer and updater
- more work to debug distribution issues on Windows
- more engineering effort for a cross-platform desktop app MVP

### Best use

- second-stage upgrade if WebRTC VAD is not accurate enough
- experimental or "enhanced auto mode" backend
- future option once the core app behavior is stable

## VAD Option 3: Mic Energy Only

### What it is

Use RMS or peak microphone energy without a real speech detector.

### Pros

- easiest possible implementation
- almost no dependency cost
- useful as a supplemental signal

### Cons

- not reliable enough as the primary detector
- too sensitive to keyboard noise and environmental sounds
- not realistic for a production auto mode

### Best use

- supporting signal only
- start threshold guard
- noise-floor calibration

## Recommended VAD Strategy

For Windows v1, the best realistic choice is:

- primary engine: `WebRTC VAD`
- supporting signal: `RMS energy gate`
- session logic: `Talking -> Hold -> Restore`
- explicit override: global hotkey

This is a hybrid strategy, not a single detector.

### Why this is the best first choice

- push-to-talk gives us a reliable fallback from day one
- WebRTC VAD keeps the implementation small and responsive
- RMS gating helps reject obviously invalid frames
- hold-state logic solves the "thinking pause" problem better than trying to perfectly detect intent
- a `VadEngine` trait lets us swap to Silero later if testing proves we need it

## Suggested VAD Pipeline

1. Capture microphone input.
2. Convert to mono `16 kHz` PCM for the detector path.
3. Maintain a rolling noise floor and RMS level.
4. Drop frames that are clearly below the energy floor.
5. Run WebRTC VAD on short frames.
6. Smooth the result over a short window.
7. Emit one of:
   - `SpeechStartCandidate`
   - `SpeechContinue`
   - `SpeechStopCandidate`
8. Let the session state machine decide whether to duck, hold, or restore.

### Suggested defaults

- frame size: `20 ms`
- sample rate for detector path: `16 kHz`
- speech start confirmation: `200 to 300 ms`
- stop candidate: `300 to 600 ms` of non-speech
- hold timeout: `4 to 6 s`

## Rust Abstraction For VAD

The shared core should not know which detector is underneath.

Example shape:

```rust
pub trait VadEngine {
    fn reset(&mut self);
    fn process_frame(&mut self, pcm_mono_16khz: &[i16]) -> VadDecision;
}

pub enum VadDecision {
    Silence,
    MaybeSpeech,
    Speech,
}
```

Then a higher-level coordinator combines:

- VAD result
- RMS level
- hotkey state
- timers

## Windows Audio Capture Options

### Option 1: `cpal`

Use `cpal` as the Rust-level microphone capture abstraction.

Pros:

- cross-platform API
- works with input devices and callback streams
- helps preserve the Windows-first but macOS-aware structure

Cons:

- less direct control than going straight to platform-specific APIs everywhere
- still requires careful device-change handling

Recommended use:

- use `cpal` for microphone capture in shared app code
- isolate Windows-specific ducking in a separate adapter

### Option 2: direct WASAPI capture

Use Windows APIs directly for microphone capture.

Pros:

- maximum control on Windows
- easier to align every detail with the Windows backend

Cons:

- pushes platform-specific logic into the earliest versions of the codebase
- less reusable for macOS
- more implementation complexity up front

Recommended use:

- only if `cpal` proves too limiting in practice

## Windows Ducking Requirements

The Windows ducking layer should:

- reduce audio fast enough to feel immediate
- restore smoothly
- avoid getting stuck after failures
- preserve the user's original volume state
- tolerate default device changes

The ducking layer should be abstracted separately from VAD.

## Ducking Option 1: Device-Level Ducking

### What it is

Control the default output device volume directly through `IAudioEndpointVolume`.

### Pros

- easiest to implement first
- works regardless of which apps are producing audio
- simple mental model for internal testing
- useful as a fail-safe backend

### Cons

- lowers everything on the output device
- can also lower audio the user still wants to hear
- does not match the ideal product promise of "lower music, not everything"
- Microsoft explicitly recommends using session-level controls for shared-mode streams instead of endpoint volume when possible

### Best use

- first internal prototype
- emergency fallback backend
- narrow public alpha with clearly stated limitations

## Ducking Option 2: Session-Level Ducking

### What it is

Enumerate render sessions with `IAudioSessionManager2`, identify active sessions, and reduce their volume with `ISimpleAudioVolume`.

### Pros

- much closer to the desired user experience
- can preserve important audio paths if we choose not to duck them
- aligns better with how Windows models shared-mode application audio

### Cons

- more complex lifecycle management
- session discovery is not enough by itself; we also need notifications for newly created sessions
- we must track original session volumes and restore them correctly
- sessions can appear, disappear, or move with device changes
- exclusive-mode streams are outside normal shared-session control

### Best use

- intended public Windows v1 target once stable
- upgrade path after device-level prototype works

## Ducking Option 3: Use Only Windows Communication Ducking

### What it is

Rely on the system's built-in communications ducking behavior.

### Pros

- minimal implementation

### Cons

- does not solve the product problem well
- depends on Windows recognizing a communications session
- often will not trigger for browser-based AI voice workflows

### Best use

- none for the core product

## Recommended Windows Ducking Strategy

We should separate "what we ship first internally" from "what we want as the better product."

### Internal build recommendation

- start with device-level ducking
- validate VAD and state-machine behavior first
- use it to learn real-world timing and false-trigger behavior

### Public Windows v1 recommendation

- move to session-level ducking if implementation complexity stays manageable
- keep device-level ducking as a fallback or advanced compatibility mode

This sequence helps us avoid solving two hard problems at once.

## Proposed Rust Abstraction For Ducking

```rust
pub trait Ducker {
    fn duck(&mut self, level: f32) -> Result<(), DuckError>;
    fn restore(&mut self) -> Result<(), DuckError>;
    fn refresh_devices(&mut self) -> Result<(), DuckError>;
}
```

Possible implementations:

- `EndpointDucker`
- `SessionDucker`

## Recommended Architecture

The best current architecture is:

- `audio_input` layer
  - microphone capture
  - resampling / mono conversion
- `vad` layer
  - `VadEngine` trait
  - `WebRtcVadEngine`
  - future `SileroVadEngine`
- `session_logic` layer
  - state machine
  - smoothing
  - hold timers
  - explicit user intent
- `ducking` layer
  - `Ducker` trait
  - `EndpointDucker`
  - future `SessionDucker`
- `platform/windows`
  - hotkeys
  - tray integration
  - audio session enumeration

## Recommendation We Should Commit To

If we want to stay realistic and still move fast, the best decision is:

1. Build the Windows MVP around `push-to-talk` plus `auto mode`.
2. Use `cpal` for microphone input unless it blocks us.
3. Implement `WebRTC VAD + RMS gate + hold-state logic` as the first auto-mode engine.
4. Implement `EndpointDucker` first so we can validate the whole product loop.
5. Design the ducking interface so `SessionDucker` can replace it later without changing the state machine.
6. Revisit Silero only if real testing shows WebRTC VAD is not good enough.

## Open Questions To Resolve Soon

- Do we want public v1 to ship with device-level ducking, or only after session-level ducking works well enough
- Which sessions should be exempt later, if we move to session-level ducking
- Do we need a short microphone calibration step for noise floor detection
- Do we expose both `Compatibility` and `Smart` ducking modes in the UI, or keep only one mode at first

## Source Notes

Windows audio model references:

- Microsoft: [Endpoint Volume Controls](https://learn.microsoft.com/en-us/windows/win32/coreaudio/endpoint-volume-controls)
- Microsoft: [Session Volume Controls](https://learn.microsoft.com/en-us/windows/win32/coreaudio/session-volume-controls)
- Microsoft: [IAudioSessionEnumerator](https://learn.microsoft.com/en-us/windows/win32/api/audiopolicy/nn-audiopolicy-iaudiosessionenumerator)
- Microsoft: [IAudioSessionControl2::GetSessionIdentifier](https://learn.microsoft.com/en-us/windows/win32/api/audiopolicy/nf-audiopolicy-iaudiosessioncontrol2-getsessionidentifier)

Rust ecosystem references:

- docs.rs: [cpal](https://docs.rs/cpal/latest/cpal/)
- docs.rs: [onnxruntime](https://docs.rs/onnxruntime/latest/onnxruntime/)

VAD references:

- GitHub: [Silero VAD](https://github.com/snakers4/silero-vad)
