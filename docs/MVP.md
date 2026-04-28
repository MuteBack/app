# MVP Design

## Goal

Build a small Rust desktop app that helps people talk to AI tools, meeting apps, and voice-enabled software without manually pausing music every time they speak.

When the user starts speaking, the app should duck background audio. When the user is done, the app should restore the audio smoothly.

## Product Direction

- Start with Windows only.
- Keep the core architecture portable so a macOS version can be added later.
- Avoid per-app integrations in the MVP.
- Use generic signals instead of custom rules for specific apps.
- Prioritize reliability and low friction over advanced automation.

## Problem We Are Solving

Windows has a built-in communications ducking feature, but it is not enough for this use case.

- It usually reacts to recognized communication sessions, not simply to microphone speech.
- It is not designed around modern workflows like talking to AI assistants in browsers or desktop apps.
- It does not give enough control over how long audio should stay ducked during thinking pauses.

The MVP should solve the more common user need:

"When I speak, reduce distracting audio. When I am really done, restore it."

For the first release, we should be careful not to promise more than we can reliably deliver. The most realistic initial target is a Windows user who is using headphones or a headset while speaking to AI or voice-enabled apps.

## Main Product Decision

We should not try to detect "the user is done talking" from one signal only.

Instead, the MVP should combine multiple generic indicators:

- microphone activity
- voice activity detection (VAD)
- user intent signals such as a hotkey or tray toggle
- silence duration

This avoids brittle logic and works across many apps without custom integrations.

## Interaction Model

The MVP should support two modes.

### 1. Push-to-talk mode

This is the most reliable mode.

- User holds a global hotkey to enter speaking mode.
- While active, background audio is ducked.
- When the key is released, audio should remain ducked for a short grace period.
- If the user does not continue speaking, audio is restored.

### 2. Auto mode

This is the more convenient mode.

- The app listens to the selected or default microphone.
- VAD and mic activity are used to detect likely speech.
- Background audio is ducked when speech is detected.
- The app keeps audio ducked briefly after speech ends to account for thinking pauses.

## State Machine

The core logic should be modeled as a simple state machine:

1. `Idle`
2. `Talking`
3. `Hold`

### `Idle`

- Normal audio level
- Waiting for speech or explicit user intent

### `Talking`

- Background audio is ducked
- Speech is actively detected, or user intent says the session is active

### `Hold`

- Background audio remains ducked
- Speech has paused, but the user may continue
- This state prevents the app from restoring audio too aggressively

This `Hold` state is important because silence does not always mean the user is finished. The user may still be thinking, reading, or waiting before continuing.

## End-of-Speaking Strategy

We should not try to know the exact stop moment. We should infer it using confidence from several signals.

### Strong signs that the session is ending

- push-to-talk key released
- tray toggle turned off
- long silence window is exceeded

### Weak signs that the session may be ending

- short silence only

The system should react differently to these signals:

- strong intent signals can end the session quickly
- silence alone should move the app into `Hold` first

## Recommended MVP Behavior

- If the hotkey is active, duck immediately.
- Else if VAD detects speech for a short stable window, duck.
- While speech continues, stay in `Talking`.
- When speech stops, move to `Hold`.
- If speech resumes during `Hold`, go back to `Talking`.
- If the user explicitly ends the session, restore audio quickly.
- If silence continues long enough, restore audio smoothly.

## Initial Timing Defaults

These values should be configurable, but good MVP defaults are:

- speech start confirmation: 200 to 300 ms
- brief pause tolerance: 1.0 to 1.5 s
- hold timeout: 4 to 6 s
- restore fade: 300 to 800 ms

These values are intentionally conservative so the app feels stable instead of jumpy.

## Realistic First-Release Assumptions

The first version should explicitly optimize for the setup most likely to work well:

- Windows desktop or laptop
- default microphone selected by the user
- headphones or a headset preferred
- local processing only

Speaker playback through open laptop speakers should be treated as best-effort in the first release, because microphone bleed can reduce VAD accuracy and create false triggers.

## MVP Scope

### Include

- Rust desktop app
- Windows-first implementation
- tray app
- global hotkey
- microphone capture
- voice activity detection
- ducking of the default output device
- smooth fade down and fade up
- configurable hold timeout
- configuration designed so macOS support can be added later
- safe audio restore on app exit or failure

### Exclude for now

- per-app integrations for every communication tool
- app-specific stop detection
- AI-based intent prediction
- advanced audio routing
- full streaming or creator mixer workflows
- open-speaker reliability as a first-release promise

## Platform Strategy

### Phase 1: Windows

Use Windows as the first shipping target because:

- the original use case is on Windows
- the audio APIs are capable enough for an MVP
- it is easier to validate product behavior before supporting multiple OSes

### Phase 2: macOS

The shared logic should already be structured so macOS can reuse:

- the state machine
- timing rules
- VAD pipeline
- config model
- ducking behavior abstractions

Platform-specific code should be isolated behind interfaces so the core speaking-session logic remains shared.

## Production Risks And Open Decisions

These are the main holes in the current plan that we should treat as explicit product risks.

### 1. Ducking target policy is still underspecified

We say "duck the default output device," but that may be too blunt in real use.

Risks:

- it may lower everything, including audio the user still wants to hear
- it may behave badly in meetings if remote voices are lowered too
- it may not match the user's mental model of "lower music, not everything"

Decision needed:

- for v1, do we intentionally duck all output on the default device
- or do we delay launch until we can target selected audio sessions

The most realistic stop point is to ship v1 with device-level ducking, but document that clearly as a limitation.

### 2. Speech detection quality is the biggest product risk

The current plan says "use VAD and mic activity," but production quality will depend on difficult real-world conditions:

- keyboard noise
- breathing and mouth noises
- quiet speakers
- accented or non-English speech
- music leaking from speakers back into the microphone

Decision needed:

- which VAD engine we trust for v1
- whether we require a calibration flow
- whether we position the first release as headset-first

If we do not set clear boundaries here, we risk shipping something that feels random.

### 3. We need explicit fail-safe behavior

The document says we will duck and restore audio, but it does not yet define what happens if:

- the app crashes
- the machine sleeps or wakes
- the audio device changes
- headphones are unplugged
- the default microphone changes

Production requirement:

- the app must never leave the system stuck in a ducked state

This means device change handling, crash-safe restore, and defensive cleanup are not optional polish. They are core behavior.

### 4. Permissions, trust, and privacy need to be part of the product spec

A background app that continuously inspects microphone input can feel invasive even if all processing is local.

We should be explicit that:

- audio is processed locally for VAD
- raw microphone audio is not uploaded
- the app asks only for the permissions it needs

This matters for user trust and for future macOS support, where permissions and OS prompts are more visible.

### 5. We do not yet have measurable release criteria

The current definition of success is directionally good, but still subjective.

For a real shipping decision, we should add measurable thresholds such as:

- maximum speech-to-duck latency
- acceptable false-trigger rate
- acceptable early-restore rate
- CPU usage target while idle and while monitoring

Without these, it will be hard to know whether we have an MVP or only a promising prototype.

## Proposed Architecture

Split the project into two layers.

### Shared core

Platform-independent Rust code for:

- session state machine
- VAD orchestration
- timing and hold behavior
- configuration
- fade logic
- event handling

### Platform adapters

Platform-specific modules for:

- microphone capture
- global hotkeys
- system tray integration
- output volume ducking

The Windows adapter comes first. The macOS adapter should be planned from the beginning but implemented later.

## Design Principles

- Default to simple behavior that feels trustworthy.
- Do not require users to configure every target app.
- Keep the first version generic.
- Bias toward not restoring audio too early.
- Make explicit user intent stronger than guessed intent.
- Keep cross-platform boundaries clean from the start.
- Prefer a reliable headset-first experience over a broad but inconsistent first release.
- Never leave the user stuck with lowered audio after an error.
- Treat privacy and local-only processing as a product feature, not a footnote.

## Recommended Realistic Stop Point

We should define a stop point that is ambitious enough to be useful but narrow enough to ship.

The most realistic first production candidate is:

- Windows only
- headset or headphones recommended
- tray app with hotkey mode and auto mode
- local VAD only
- device-level ducking on the default output device
- smooth restore after hold timeout
- safe restore on exit, crash, or device changes

We should not block the first release on perfect open-speaker detection, per-app ducking, or complex AI-driven intent modeling.

## First Implementation Milestones

1. Create the Rust app skeleton and shared core types.
2. Implement the speaking-session state machine.
3. Choose and validate a Windows VAD approach with recorded real-world usage samples.
4. Add Windows microphone capture and device-level audio ducking.
5. Add global hotkey support.
6. Add a tray app with simple on/off mode switching.
7. Implement safe restore behavior for exit, crash recovery, and device changes.
8. Tune timing and fade behavior through real usage.
9. Refactor platform boundaries where needed before adding macOS.

## Working Definition Of Success

The MVP is successful if a Windows user can:

- keep music or background audio playing
- start talking to an AI or voice-enabled app
- have the background audio duck automatically or via hotkey
- pause briefly without audio restoring too early
- finish speaking and have the audio return smoothly

If this feels reliable in daily use, the foundation is good enough to expand toward macOS and more advanced controls.

For a production-ready Windows v1, we should also be able to say:

- the app does not frequently false-trigger in normal desktop use
- the app does not leave audio ducked after failure cases
- the app behaves predictably across common device changes
- the app is transparent about local microphone processing
