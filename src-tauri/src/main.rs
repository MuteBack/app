#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use muteback::config::{AppConfig, SpeakerProfile};
use muteback::runtime::{list_input_devices, RuntimeEvent, RuntimeHandle};
use muteback::speaker::{
    build_voice_profile, resample_f32_to_i16, OnnxSpeakerEmbeddingEngine, SpeakerEmbeddingEngine,
};
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    path::BaseDirectory,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_store::StoreExt;

const TRAY_ICON: tauri::image::Image<'_> = tauri::include_image!("./icons/tray.png");
const TRAY_ID: &str = "main-tray";
const MAIN_HOME_SIZE: (f64, f64) = (280.0, 190.0);
const MAIN_SETTINGS_SIZE: (f64, f64) = (322.0, 660.0);
const MAIN_SETTINGS_MIN_SIZE: (f64, f64) = (322.0, 630.0);
const RESTORE_WINDOW_LABEL: &str = "restore_prompt";
const RESTORE_WINDOW_SIZE: (f64, f64) = (216.0, 52.0);
const MAIN_RESIZE_STEPS: u32 = 14;
const MAIN_RESIZE_FRAME_MS: u64 = 12;
const SETTINGS_STORE_PATH: &str = "settings.store.json";
const SETTINGS_STORE_KEY: &str = "settings";
const SILERO_RESOURCE_PATH: &str = "assets/vendor/silero_vad.onnx";
const SPEAKER_RESOURCE_PATH: &str = "assets/vendor/voxceleb_ECAPA512_LM.onnx";
const ONNX_RUNTIME_RESOURCE_PATH: &str = "assets/vendor/onnxruntime.dll";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum TransitionMode {
    Smooth,
    Instant,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    enabled: bool,
    duck_level_percent: u8,
    transition: TransitionMode,
    manual_restore: bool,
    voice_match_enabled: bool,
    microphone_id: Option<String>,
    duck_fade_ms: u64,
    restore_fade_ms: u64,
}

impl Default for Settings {
    fn default() -> Self {
        let config = AppConfig::default();

        Self {
            enabled: true,
            duck_level_percent: (config.normalized_ducking_level() * 100.0).round() as u8,
            transition: if config.smooth_ducking {
                TransitionMode::Smooth
            } else {
                TransitionMode::Instant
            },
            manual_restore: config.manual_restore,
            voice_match_enabled: false,
            microphone_id: None,
            duck_fade_ms: config.duck_fade.as_millis() as u64,
            restore_fade_ms: config.restore_fade.as_millis() as u64,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MicrophoneOption {
    id: String,
    name: String,
    is_default: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoiceSampleInput {
    phrase_index: u8,
    duration_ms: u64,
    sample_rate: u32,
    samples: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceSampleAudio {
    sample_rate: u32,
    samples: Vec<f32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceSample {
    phrase_index: u8,
    duration_ms: u64,
    bytes: u64,
    playable: bool,
    #[serde(skip)]
    embedding: Vec<f32>,
    #[serde(skip)]
    audio_sample_rate: u32,
    #[serde(skip)]
    audio_samples: Vec<i16>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceProfileSummary {
    model_id: String,
    threshold: f32,
    sample_count: u8,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceEnrollment {
    required_samples: u8,
    samples: Vec<VoiceSample>,
    profile: Option<VoiceProfileSummary>,
    #[serde(skip)]
    speaker_profile: Option<SpeakerProfile>,
}

impl Default for VoiceEnrollment {
    fn default() -> Self {
        Self {
            required_samples: 3,
            samples: Vec::new(),
            profile: None,
            speaker_profile: None,
        }
    }
}

impl VoiceEnrollment {
    fn is_complete(&self) -> bool {
        self.samples.len() >= self.required_samples as usize && self.speaker_profile.is_some()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredVoiceProfile {
    required_samples: u8,
    samples: Vec<StoredVoiceSample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<StoredSpeakerProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    embedding: Option<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    threshold: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredVoiceSample {
    phrase_index: u8,
    duration_ms: u64,
    bytes: u64,
    #[serde(default)]
    embedding: Vec<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audio_sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    audio_samples: Vec<i16>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredSpeakerProfile {
    embedding: Vec<f32>,
    threshold: f32,
    sample_rate: u32,
    model_id: String,
}

impl From<&VoiceSample> for StoredVoiceSample {
    fn from(sample: &VoiceSample) -> Self {
        Self {
            phrase_index: sample.phrase_index,
            duration_ms: sample.duration_ms,
            bytes: sample.bytes,
            embedding: sample.embedding.clone(),
            audio_sample_rate: sample.playable.then_some(sample.audio_sample_rate),
            audio_samples: sample.audio_samples.clone(),
        }
    }
}

impl From<StoredVoiceSample> for VoiceSample {
    fn from(sample: StoredVoiceSample) -> Self {
        let audio_sample_rate = sample.audio_sample_rate.unwrap_or(0);
        let playable = audio_sample_rate > 0 && !sample.audio_samples.is_empty();

        Self {
            phrase_index: sample.phrase_index,
            duration_ms: sample.duration_ms,
            bytes: sample.bytes,
            playable,
            embedding: sample.embedding,
            audio_sample_rate,
            audio_samples: sample.audio_samples,
        }
    }
}

impl From<&SpeakerProfile> for StoredSpeakerProfile {
    fn from(profile: &SpeakerProfile) -> Self {
        Self {
            embedding: profile.embedding.clone(),
            threshold: profile.threshold,
            sample_rate: profile.sample_rate,
            model_id: profile.model_id.clone(),
        }
    }
}

impl From<StoredSpeakerProfile> for SpeakerProfile {
    fn from(profile: StoredSpeakerProfile) -> Self {
        Self {
            embedding: profile.embedding,
            threshold: profile.threshold,
            sample_rate: profile.sample_rate,
            model_id: profile.model_id,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    enabled: bool,
    running: bool,
    ducked: bool,
    message: String,
    microphone: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCheckStatus {
    configured: bool,
    update_available: bool,
    version: Option<String>,
    message: String,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            enabled: true,
            running: false,
            ducked: false,
            message: "Starting".to_string(),
            microphone: None,
        }
    }
}

struct AppState {
    settings: Arc<Mutex<Settings>>,
    voice_enrollment: Mutex<VoiceEnrollment>,
    runtime: Mutex<Option<RuntimeHandle>>,
    runtime_status: Arc<Mutex<RuntimeStatus>>,
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> Result<Settings, String> {
    state
        .settings
        .lock()
        .map(|settings| settings.clone())
        .map_err(|_| "settings state is unavailable".to_string())
}

#[tauri::command]
fn update_settings(
    input: Settings,
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Settings, String> {
    let input = settings_available_to_state(input, &state)?;
    validate_settings(&input)?;

    let restart_runtime = {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings state is unavailable".to_string())?;
        let restart_runtime = settings.microphone_id != input.microphone_id;
        *settings = input.clone();
        restart_runtime
    };

    persist_settings(&app, &input)?;
    sync_runtime(&input, &state, &app, restart_runtime)?;
    update_restore_prompt(&app, &state)?;
    Ok(input)
}

#[tauri::command]
fn list_microphones() -> Result<Vec<MicrophoneOption>, String> {
    list_input_devices()
        .map_err(|error| error.to_string())
        .map(|devices| {
            devices
                .into_iter()
                .map(|device| MicrophoneOption {
                    id: device.id,
                    name: device.name,
                    is_default: device.is_default,
                })
                .collect()
        })
}

#[tauri::command]
fn get_runtime_status(state: tauri::State<'_, AppState>) -> Result<RuntimeStatus, String> {
    state
        .runtime_status
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "runtime status is unavailable".to_string())
}

#[tauri::command]
fn get_voice_enrollment(state: tauri::State<'_, AppState>) -> Result<VoiceEnrollment, String> {
    state
        .voice_enrollment
        .lock()
        .map(|enrollment| enrollment.clone())
        .map_err(|_| "voice enrollment state is unavailable".to_string())
}

#[tauri::command]
fn get_voice_sample_audio(
    index: u32,
    state: tauri::State<'_, AppState>,
) -> Result<VoiceSampleAudio, String> {
    let enrollment = state
        .voice_enrollment
        .lock()
        .map_err(|_| "voice enrollment state is unavailable".to_string())?;
    let sample = enrollment
        .samples
        .get(index as usize)
        .ok_or_else(|| "voice sample does not exist".to_string())?;

    if !sample.playable {
        return Err("voice sample audio is not available".to_string());
    }

    Ok(VoiceSampleAudio {
        sample_rate: sample.audio_sample_rate,
        samples: sample
            .audio_samples
            .iter()
            .map(|sample| (*sample as f32 / i16::MAX as f32).clamp(-1.0, 1.0))
            .collect(),
    })
}

#[tauri::command]
fn add_voice_sample(
    input: VoiceSampleInput,
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<VoiceEnrollment, String> {
    if input.duration_ms < 500 {
        return Err("voice sample is too short".to_string());
    }

    if input.samples.len() < input.sample_rate as usize / 2 {
        return Err("voice sample has too little audio".to_string());
    }

    let pcm = resample_f32_to_i16(&input.samples, input.sample_rate, 16_000);
    let mut embedder = OnnxSpeakerEmbeddingEngine::new().map_err(|error| error.to_string())?;
    let embedding = embedder.embed(&pcm).map_err(|error| error.to_string())?;
    let bytes = pcm.len() as u64 * std::mem::size_of::<i16>() as u64;

    let result = {
        let mut enrollment = state
            .voice_enrollment
            .lock()
            .map_err(|_| "voice enrollment state is unavailable".to_string())?;

        if enrollment.is_complete() {
            return Ok(enrollment.clone());
        }

        enrollment.samples.push(VoiceSample {
            phrase_index: input.phrase_index,
            duration_ms: input.duration_ms,
            bytes,
            playable: true,
            embedding,
            audio_sample_rate: 16_000,
            audio_samples: pcm,
        });
        normalize_voice_sample_order(&mut enrollment);
        refresh_voice_profile(&mut enrollment)?;

        enrollment.clone()
    };

    persist_voice_profile(&app, &result)?;
    let settings = settings_after_voice_enrollment_change(&app, &state, &result)?;
    sync_runtime(&settings, &state, &app, false)?;

    Ok(result)
}

#[tauri::command]
fn remove_voice_sample(
    index: u32,
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<VoiceEnrollment, String> {
    let result = {
        let mut enrollment = state
            .voice_enrollment
            .lock()
            .map_err(|_| "voice enrollment state is unavailable".to_string())?;
        let index = index as usize;
        if index >= enrollment.samples.len() {
            return Err("voice sample does not exist".to_string());
        }

        enrollment.samples.remove(index);
        normalize_voice_sample_order(&mut enrollment);
        refresh_voice_profile(&mut enrollment)?;
        enrollment.clone()
    };

    persist_voice_profile(&app, &result)?;
    let settings = settings_after_voice_enrollment_change(&app, &state, &result)?;
    sync_runtime(&settings, &state, &app, false)?;

    Ok(result)
}

#[tauri::command]
fn reset_voice_enrollment(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<VoiceEnrollment, String> {
    let mut enrollment = state
        .voice_enrollment
        .lock()
        .map_err(|_| "voice enrollment state is unavailable".to_string())?;
    *enrollment = VoiceEnrollment::default();
    let result = enrollment.clone();
    drop(enrollment);

    delete_voice_profile(&app)?;
    let settings = settings_after_voice_enrollment_change(&app, &state, &result)?;
    sync_runtime(&settings, &state, &app, false)?;

    Ok(result)
}

#[tauri::command]
fn request_restore(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let runtime = state
        .runtime
        .lock()
        .map_err(|_| "runtime state is unavailable".to_string())?;

    if let Some(runtime) = runtime.as_ref() {
        runtime
            .request_restore()
            .map_err(|error| error.to_string())?;
        update_runtime_message(&state, "Restore queued");
    }

    set_restore_prompt_visible_inner(&app, false)?;
    Ok(())
}

#[tauri::command]
fn set_main_view(view: String, window: WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Ok(());
    }

    match view.as_str() {
        "home" => resize_main_window(&window, MAIN_HOME_SIZE, false),
        "settings" => resize_main_window(&window, MAIN_SETTINGS_SIZE, true),
        _ => Err("unknown app view".to_string()),
    }
}

#[tauri::command]
fn set_restore_prompt_visible(app: AppHandle, visible: bool) -> Result<(), String> {
    set_restore_prompt_visible_inner(&app, visible)
}

#[tauri::command]
fn start_restore_prompt_drag(window: WebviewWindow) -> Result<(), String> {
    if window.label() != RESTORE_WINDOW_LABEL {
        return Ok(());
    }

    window.start_dragging().map_err(|error| error.to_string())
}

#[tauri::command]
fn check_for_updates() -> UpdateCheckStatus {
    UpdateCheckStatus {
        configured: false,
        update_available: false,
        version: None,
        message: "Updater is scaffolded but not configured yet.".to_string(),
    }
}

fn validate_settings(settings: &Settings) -> Result<(), String> {
    if settings.duck_level_percent > 100 {
        return Err("ducking level must be between 0 and 100".to_string());
    }

    Ok(())
}

fn settings_available_to_state(
    mut settings: Settings,
    _state: &AppState,
) -> Result<Settings, String> {
    settings.voice_match_enabled = false;

    Ok(settings)
}

fn settings_after_voice_enrollment_change(
    app: &AppHandle,
    state: &AppState,
    enrollment: &VoiceEnrollment,
) -> Result<Settings, String> {
    let mut settings = state
        .settings
        .lock()
        .map_err(|_| "settings state is unavailable".to_string())?;

    if !enrollment.is_complete() && settings.voice_match_enabled {
        settings.voice_match_enabled = false;
        persist_settings(app, &settings)?;
    }

    Ok(settings.clone())
}

fn normalize_saved_settings(app: &AppHandle, state: &AppState) -> Result<Settings, String> {
    let settings = state
        .settings
        .lock()
        .map(|settings| settings.clone())
        .map_err(|_| "settings state is unavailable".to_string())?;
    let normalized = settings_available_to_state(settings.clone(), state)?;

    if normalized.voice_match_enabled != settings.voice_match_enabled {
        let mut saved_settings = state
            .settings
            .lock()
            .map_err(|_| "settings state is unavailable".to_string())?;
        *saved_settings = normalized.clone();
        persist_settings(app, &normalized)?;
    }

    Ok(normalized)
}

fn sync_runtime(
    settings: &Settings,
    state: &AppState,
    app: &AppHandle,
    restart_runtime: bool,
) -> Result<(), String> {
    if restart_runtime {
        stop_runtime(state);
    }

    if settings.enabled {
        start_or_update_runtime(settings, state, app)
    } else {
        stop_runtime(state);
        set_runtime_status(state, |status| {
            status.enabled = false;
            status.running = false;
            status.ducked = false;
            status.message = "Disabled".to_string();
            status.microphone = None;
        });
        Ok(())
    }?;

    update_tray_icon(app, state)
}

fn start_or_update_runtime(
    settings: &Settings,
    state: &AppState,
    app: &AppHandle,
) -> Result<(), String> {
    let config = app_config_from_settings(settings, state);
    let mut runtime = state
        .runtime
        .lock()
        .map_err(|_| "runtime state is unavailable".to_string())?;

    if let Some(runtime) = runtime.as_ref() {
        runtime
            .update_config(config)
            .map_err(|error| error.to_string())?;
        set_runtime_status(state, |status| {
            status.enabled = true;
            if status.running {
                status.message = "Listening".to_string();
            }
        });
        return Ok(());
    }

    let (event_tx, event_rx) = mpsc::channel();
    let handle = RuntimeHandle::start(config, event_tx).map_err(|error| error.to_string())?;
    *runtime = Some(handle);
    set_runtime_status(state, |status| {
        status.enabled = true;
        status.running = true;
        status.ducked = false;
        status.message = "Starting".to_string();
    });
    watch_runtime_events(
        app.clone(),
        Arc::clone(&state.settings),
        Arc::clone(&state.runtime_status),
        event_rx,
    );

    Ok(())
}

fn stop_runtime(state: &AppState) {
    let mut runtime = match state.runtime.lock() {
        Ok(runtime) => runtime,
        Err(_) => return,
    };

    if let Some(mut runtime) = runtime.take() {
        runtime.stop();
    }
}

fn watch_runtime_events(
    app: AppHandle,
    settings: Arc<Mutex<Settings>>,
    runtime_status: Arc<Mutex<RuntimeStatus>>,
    event_rx: mpsc::Receiver<RuntimeEvent>,
) {
    thread::spawn(move || {
        while let Ok(event) = event_rx.recv() {
            let stopped = matches!(event, RuntimeEvent::Stopped);

            {
                let mut status = match runtime_status.lock() {
                    Ok(status) => status,
                    Err(_) => break,
                };

                match event {
                    RuntimeEvent::Started(info) => {
                        status.running = true;
                        status.ducked = false;
                        status.message = "Listening".to_string();
                        status.microphone = Some(info.microphone);
                    }
                    RuntimeEvent::Ducked => {
                        status.running = true;
                        status.ducked = true;
                        status.message = "Ducking".to_string();
                    }
                    RuntimeEvent::Restored => {
                        status.running = true;
                        status.ducked = false;
                        status.message = "Listening".to_string();
                    }
                    RuntimeEvent::Warning(message) => {
                        status.message = message;
                    }
                    RuntimeEvent::Error(message) => {
                        status.running = false;
                        status.ducked = false;
                        status.message = message;
                    }
                    RuntimeEvent::Stopped => {
                        status.running = false;
                        status.ducked = false;
                        status.message = if status.enabled {
                            "Stopped".to_string()
                        } else {
                            "Disabled".to_string()
                        };
                    }
                }
            }

            let _ = update_tray_icon_from_shared(&app, &settings, &runtime_status);
            let _ = update_restore_prompt_from_shared(&app, &settings, &runtime_status);

            if stopped {
                break;
            }
        }
    });
}

fn app_config_from_settings(settings: &Settings, state: &AppState) -> AppConfig {
    let mut config = AppConfig::default();
    config.ducking_level = settings.duck_level_percent as f32 / 100.0;
    config.smooth_ducking = settings.transition == TransitionMode::Smooth;
    config.manual_restore = settings.manual_restore;
    config.voice_match_enabled = settings.voice_match_enabled;
    config.microphone_name = settings.microphone_id.clone();
    config.speaker_profile = if settings.voice_match_enabled {
        state
            .voice_enrollment
            .lock()
            .ok()
            .and_then(|enrollment| enrollment.speaker_profile.clone())
    } else {
        None
    };
    config.duck_fade = std::time::Duration::from_millis(settings.duck_fade_ms);
    config.restore_fade = std::time::Duration::from_millis(settings.restore_fade_ms);
    config
}

fn load_settings(app: &AppHandle) -> Result<Option<Settings>, String> {
    let store = app
        .store(SETTINGS_STORE_PATH)
        .map_err(|error| format!("failed to open settings store: {error}"))?;

    if let Some(value) = store.get(SETTINGS_STORE_KEY) {
        return serde_json::from_value::<Settings>(value.clone())
            .map(Some)
            .map_err(|error| format!("failed to parse settings from store: {error}"));
    }

    let legacy_settings = load_legacy_settings(app)?;
    if let Some(settings) = legacy_settings.as_ref() {
        persist_settings(app, settings)?;
    }

    Ok(legacy_settings)
}

fn persist_settings(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let store = app
        .store(SETTINGS_STORE_PATH)
        .map_err(|error| format!("failed to open settings store: {error}"))?;
    let value =
        serde_json::to_value(settings).map_err(|error| format!("failed to encode settings: {error}"))?;

    store.set(SETTINGS_STORE_KEY, value);
    store
        .save()
        .map_err(|error| format!("failed to save settings store: {error}"))
}

fn load_legacy_settings(app: &AppHandle) -> Result<Option<Settings>, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(path)
        .map_err(|error| format!("failed to read legacy settings file: {error}"))?;
    serde_json::from_str::<Settings>(&contents)
        .map(Some)
        .map_err(|error| format!("failed to parse legacy settings: {error}"))
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| format!("failed to locate app data directory: {error}"))?
        .join("settings.json"))
}

fn load_voice_profile(app: &AppHandle) -> Result<Option<VoiceEnrollment>, String> {
    for path in voice_profile_paths(app)? {
        if !path.exists() {
            continue;
        }

        let contents = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read voice profile: {error}"))?;
        let enrollment = parse_voice_profile(&contents)
            .map_err(|error| format!("failed to parse voice profile: {error}"))?;
        return Ok(Some(enrollment));
    }

    Ok(None)
}

fn parse_voice_profile(contents: &str) -> Result<VoiceEnrollment, String> {
    let stored =
        serde_json::from_str::<StoredVoiceProfile>(contents).map_err(|error| error.to_string())?;
    let required_samples = stored.required_samples.max(1);
    let speaker_profile = stored_speaker_profile(&stored);
    let samples = stored
        .samples
        .into_iter()
        .map(VoiceSample::from)
        .collect::<Vec<_>>();
    let profile = speaker_profile
        .as_ref()
        .map(|profile| voice_profile_summary(profile, samples.len()));
    let mut enrollment = VoiceEnrollment {
        required_samples,
        samples,
        profile,
        speaker_profile,
    };

    if enrollment.speaker_profile.is_none() {
        refresh_voice_profile(&mut enrollment)?;
    }

    Ok(enrollment)
}

fn persist_voice_profile(app: &AppHandle, enrollment: &VoiceEnrollment) -> Result<(), String> {
    let paths = voice_profile_paths(app)?;

    if enrollment.samples.is_empty() && enrollment.speaker_profile.is_none() {
        for path in paths {
            if path.exists() {
                fs::remove_file(path)
                    .map_err(|error| format!("failed to delete voice profile: {error}"))?;
            }
        }
        return Ok(());
    }

    let stored = StoredVoiceProfile {
        required_samples: enrollment.required_samples,
        samples: enrollment.samples.iter().map(StoredVoiceSample::from).collect(),
        profile: enrollment.speaker_profile.as_ref().map(StoredSpeakerProfile::from),
        embedding: None,
        threshold: None,
        sample_rate: None,
        model_id: None,
    };
    let contents = serde_json::to_string_pretty(&stored)
        .map_err(|error| format!("failed to encode voice profile: {error}"))?;

    for path in paths {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create voice profile directory: {error}"))?;
        }
        fs::write(path, &contents)
            .map_err(|error| format!("failed to save voice profile: {error}"))?;
    }

    Ok(())
}

fn stored_speaker_profile(stored: &StoredVoiceProfile) -> Option<SpeakerProfile> {
    if let Some(profile) = stored.profile.as_ref() {
        return Some(SpeakerProfile {
            embedding: profile.embedding.clone(),
            threshold: profile.threshold,
            sample_rate: profile.sample_rate,
            model_id: profile.model_id.clone(),
        });
    }

    let (Some(embedding), Some(threshold), Some(sample_rate), Some(model_id)) = (
        stored.embedding.as_ref(),
        stored.threshold,
        stored.sample_rate,
        stored.model_id.as_ref(),
    ) else {
        return None;
    };

    if embedding.is_empty() || model_id.is_empty() {
        return None;
    }

    Some(SpeakerProfile {
        embedding: embedding.clone(),
        threshold,
        sample_rate,
        model_id: model_id.clone(),
    })
}

fn refresh_voice_profile(enrollment: &mut VoiceEnrollment) -> Result<(), String> {
    if enrollment.samples.len() < enrollment.required_samples as usize {
        enrollment.profile = None;
        enrollment.speaker_profile = None;
        return Ok(());
    }

    let embeddings = enrollment
        .samples
        .iter()
        .map(|sample| sample.embedding.clone())
        .collect::<Vec<_>>();

    if embeddings.iter().any(Vec::is_empty) {
        enrollment.profile = None;
        enrollment.speaker_profile = None;
        return Ok(());
    }

    let profile = build_voice_profile(&embeddings).map_err(|error| error.to_string())?;
    enrollment.profile = Some(voice_profile_summary(&profile, enrollment.samples.len()));
    enrollment.speaker_profile = Some(profile);
    Ok(())
}

fn normalize_voice_sample_order(enrollment: &mut VoiceEnrollment) {
    for (index, sample) in enrollment.samples.iter_mut().enumerate() {
        sample.phrase_index = index as u8;
    }
}

fn voice_profile_summary(profile: &SpeakerProfile, sample_count: usize) -> VoiceProfileSummary {
    VoiceProfileSummary {
        model_id: profile.model_id.clone(),
        threshold: profile.threshold,
        sample_count: sample_count as u8,
    }
}

fn delete_voice_profile(app: &AppHandle) -> Result<(), String> {
    for path in voice_profile_paths(app)? {
        if path.exists() {
            fs::remove_file(path)
                .map_err(|error| format!("failed to delete voice profile: {error}"))?;
        }
    }

    Ok(())
}

fn voice_profile_paths(app: &AppHandle) -> Result<Vec<PathBuf>, String> {
    let resolver = app.path();
    let mut paths = Vec::new();
    push_voice_profile_path(
        &mut paths,
        resolver
            .app_data_dir()
            .map_err(|error| format!("failed to locate app data directory: {error}"))?,
    );

    if let Ok(path) = resolver.app_local_data_dir() {
        push_voice_profile_path(&mut paths, path);
    }
    if let Ok(path) = resolver.app_config_dir() {
        push_voice_profile_path(&mut paths, path);
    }

    Ok(paths)
}

fn push_voice_profile_path(paths: &mut Vec<PathBuf>, directory: PathBuf) {
    let path = directory.join("voice-profile.json");
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn configure_bundled_asset_env(app: &AppHandle) {
    set_env_from_resource(app, "SILERO_MODEL_PATH", SILERO_RESOURCE_PATH);
    set_env_from_resource(app, "SPEAKER_MODEL_PATH", SPEAKER_RESOURCE_PATH);
    set_env_from_resource(app, "ORT_DYLIB_PATH", ONNX_RUNTIME_RESOURCE_PATH);
}

fn set_env_from_resource(app: &AppHandle, key: &str, resource_path: &str) {
    if std::env::var_os(key).is_some() {
        return;
    }

    if let Some(path) = bundled_resource_path(app, resource_path).filter(|path| path.exists()) {
        std::env::set_var(key, path);
    }
}

fn bundled_resource_path(app: &AppHandle, resource_path: &str) -> Option<PathBuf> {
    app.path()
        .resolve(resource_path, BaseDirectory::Resource)
        .ok()
}

fn update_runtime_message(state: &AppState, message: &str) {
    set_runtime_status(state, |status| {
        status.message = message.to_string();
    });
}

fn set_runtime_status(state: &AppState, update: impl FnOnce(&mut RuntimeStatus)) {
    if let Ok(mut status) = state.runtime_status.lock() {
        update(&mut status);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayDot {
    Active,
    Inactive,
    Disabled,
}

impl TrayDot {
    fn color(self) -> [u8; 3] {
        match self {
            Self::Active => [34, 197, 94],
            Self::Inactive => [148, 163, 184],
            Self::Disabled => [239, 68, 68],
        }
    }
}

fn update_tray_icon(app: &AppHandle, state: &AppState) -> Result<(), String> {
    update_tray_icon_from_shared(app, &state.settings, &state.runtime_status)
}

fn update_tray_icon_from_shared(
    app: &AppHandle,
    settings: &Arc<Mutex<Settings>>,
    runtime_status: &Arc<Mutex<RuntimeStatus>>,
) -> Result<(), String> {
    let settings = settings
        .lock()
        .map(|settings| settings.clone())
        .map_err(|_| "settings state is unavailable".to_string())?;
    let status = runtime_status
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "runtime status is unavailable".to_string())?;

    update_tray_icon_inner(app, &settings, &status)
}

fn update_tray_icon_inner(
    app: &AppHandle,
    settings: &Settings,
    status: &RuntimeStatus,
) -> Result<(), String> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };

    let dot = if !settings.enabled || !status.enabled {
        TrayDot::Disabled
    } else if status.running {
        TrayDot::Active
    } else {
        TrayDot::Inactive
    };

    tray.set_icon(Some(tray_icon_with_dot(dot)))
        .map_err(|error| format!("failed to update tray icon: {error}"))?;
    let _ = tray.set_tooltip(Some(format!("MuteBack - {}", status.message)));

    Ok(())
}

fn tray_icon_with_dot(dot: TrayDot) -> tauri::image::Image<'static> {
    let width = TRAY_ICON.width();
    let height = TRAY_ICON.height();
    let mut rgba = TRAY_ICON.rgba().to_vec();
    draw_tray_status_dot(&mut rgba, width, height, dot.color());
    tauri::image::Image::new_owned(rgba, width, height)
}

fn draw_tray_status_dot(rgba: &mut [u8], width: u32, height: u32, color: [u8; 3]) {
    if width == 0 || height == 0 {
        return;
    }

    let min_side = width.min(height) as f64;
    let radius = (min_side * 0.17).round().max(3.0);
    let border = (min_side * 0.055).round().max(1.0);
    let padding = (min_side * 0.08).round().max(1.0);
    let outer_radius = radius + border;
    let center_x = width as f64 - padding - radius;
    let center_y = height as f64 - padding - radius;
    let min_x = (center_x - outer_radius - 1.0).floor().max(0.0) as u32;
    let min_y = (center_y - outer_radius - 1.0).floor().max(0.0) as u32;
    let max_x = (center_x + outer_radius + 1.0)
        .ceil()
        .min((width - 1) as f64) as u32;
    let max_y = (center_y + outer_radius + 1.0)
        .ceil()
        .min((height - 1) as f64) as u32;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f64 + 0.5 - center_x;
            let dy = y as f64 + 0.5 - center_y;
            let distance = (dx * dx + dy * dy).sqrt();
            let index = ((y * width + x) * 4) as usize;

            if distance <= outer_radius {
                blend_pixel(rgba, index, [248, 250, 252, 245]);
            }

            if distance <= radius {
                blend_pixel(rgba, index, [color[0], color[1], color[2], 255]);
            }
        }
    }
}

fn blend_pixel(rgba: &mut [u8], index: usize, src: [u8; 4]) {
    let src_alpha = src[3] as u16;
    let inverse_alpha = 255 - src_alpha;

    for channel in 0..3 {
        rgba[index + channel] =
            ((src[channel] as u16 * src_alpha + rgba[index + channel] as u16 * inverse_alpha)
                / 255) as u8;
    }

    rgba[index + 3] = (src_alpha + rgba[index + 3] as u16 * inverse_alpha / 255).min(255) as u8;
}

fn resize_main_window(
    window: &WebviewWindow,
    size: (f64, f64),
    resizable: bool,
) -> Result<(), String> {
    window
        .set_resizable(true)
        .map_err(|error| error.to_string())?;
    window
        .set_min_size(Some(LogicalSize::new(MAIN_HOME_SIZE.0, MAIN_HOME_SIZE.1)))
        .map_err(|error| error.to_string())?;

    animate_main_window_size(window, size)?;

    let min_size = if resizable {
        MAIN_SETTINGS_MIN_SIZE
    } else {
        size
    };
    window
        .set_min_size(Some(LogicalSize::new(min_size.0, min_size.1)))
        .map_err(|error| error.to_string())?;
    window
        .set_resizable(resizable)
        .map_err(|error| error.to_string())?;

    Ok(())
}

fn animate_main_window_size(window: &WebviewWindow, target: (f64, f64)) -> Result<(), String> {
    let current = current_logical_window_size(window)?;

    for step in 1..=MAIN_RESIZE_STEPS {
        let progress = step as f64 / MAIN_RESIZE_STEPS as f64;
        let eased = ease_in_out(progress);
        let width = interpolate(current.0, target.0, eased);
        let height = interpolate(current.1, target.1, eased);

        window
            .set_size(LogicalSize::new(width, height))
            .map_err(|error| error.to_string())?;
        window.center().map_err(|error| error.to_string())?;
        thread::sleep(Duration::from_millis(MAIN_RESIZE_FRAME_MS));
    }

    window
        .set_size(LogicalSize::new(target.0, target.1))
        .map_err(|error| error.to_string())?;
    window.center().map_err(|error| error.to_string())?;

    Ok(())
}

fn current_logical_window_size(window: &WebviewWindow) -> Result<(f64, f64), String> {
    let scale = window.scale_factor().map_err(|error| error.to_string())?;
    let size = window.inner_size().map_err(|error| error.to_string())?;

    Ok((size.width as f64 / scale, size.height as f64 / scale))
}

fn interpolate(start: f64, end: f64, progress: f64) -> f64 {
    start + (end - start) * progress
}

fn ease_in_out(progress: f64) -> f64 {
    let clamped = progress.clamp(0.0, 1.0);
    clamped * clamped * (3.0 - 2.0 * clamped)
}

fn update_restore_prompt(app: &AppHandle, state: &AppState) -> Result<(), String> {
    update_restore_prompt_from_shared(app, &state.settings, &state.runtime_status)
}

fn update_restore_prompt_from_shared(
    app: &AppHandle,
    settings: &Arc<Mutex<Settings>>,
    runtime_status: &Arc<Mutex<RuntimeStatus>>,
) -> Result<(), String> {
    let manual_restore = settings
        .lock()
        .map(|settings| settings.manual_restore)
        .unwrap_or(false);
    let ducked = runtime_status
        .lock()
        .map(|status| status.ducked)
        .unwrap_or(false);

    set_restore_prompt_visible_inner(app, manual_restore && ducked)
}

fn set_restore_prompt_visible_inner(app: &AppHandle, visible: bool) -> Result<(), String> {
    let Some(window) = app.get_webview_window(RESTORE_WINDOW_LABEL) else {
        return Ok(());
    };

    if visible {
        window
            .set_always_on_top(true)
            .map_err(|error| error.to_string())?;
        window.show().map_err(|error| error.to_string())?;
    } else {
        window.hide().map_err(|error| error.to_string())?;
    }

    Ok(())
}

fn position_restore_window(window: &WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()?.or(window.primary_monitor()?) else {
        return Ok(());
    };

    let work_area = monitor.work_area();
    let size = window.outer_size()?;
    let margin = 24;
    let x = work_area.position.x + work_area.size.width as i32 - size.width as i32 - margin;
    let y = work_area.position.y + work_area.size.height as i32 - size.height as i32 - margin;

    window.set_position(PhysicalPosition::new(
        x.max(work_area.position.x),
        y.max(work_area.position.y),
    ))
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = resize_main_window(&window, MAIN_HOME_SIZE, false);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.eval("window.dispatchEvent(new CustomEvent('muteback:show-home'))");
        let _ = window.set_focus();
    }
}

fn create_restore_window(app: &mut tauri::App) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(
        app,
        RESTORE_WINDOW_LABEL,
        WebviewUrl::App("restore.html".into()),
    )
    .title("Restore Sound")
    .inner_size(RESTORE_WINDOW_SIZE.0, RESTORE_WINDOW_SIZE.1)
    .min_inner_size(RESTORE_WINDOW_SIZE.0, RESTORE_WINDOW_SIZE.1)
    .max_inner_size(RESTORE_WINDOW_SIZE.0, RESTORE_WINDOW_SIZE.1)
    .resizable(false)
    .decorations(false)
    .transparent(false)
    .background_color(tauri::webview::Color(21, 25, 34, 255))
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .build()?;

    let _ = position_restore_window(&window);
    Ok(())
}

fn create_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open MuteBack", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(TRAY_ICON.clone())
        .tooltip("MuteBack")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            settings: Arc::new(Mutex::new(Settings::default())),
            voice_enrollment: Mutex::new(VoiceEnrollment::default()),
            runtime: Mutex::new(None),
            runtime_status: Arc::new(Mutex::new(RuntimeStatus::default())),
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            update_settings,
            check_for_updates,
            list_microphones,
            get_runtime_status,
            get_voice_enrollment,
            get_voice_sample_audio,
            add_voice_sample,
            remove_voice_sample,
            reset_voice_enrollment,
            request_restore,
            set_main_view,
            set_restore_prompt_visible,
            start_restore_prompt_drag
        ])
        .setup(|app| {
            configure_bundled_asset_env(app.handle());
            app.handle()
                .plugin(tauri_plugin_store::Builder::default().build())
                .map_err(|error| format!("failed to register store plugin: {error}"))?;
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())
                .map_err(|error| format!("failed to register updater plugin: {error}"))?;
            create_tray(app)?;
            create_restore_window(app)?;
            let state = app.state::<AppState>();
            if let Some(settings) = load_settings(app.handle())? {
                let mut saved_settings = state
                    .settings
                    .lock()
                    .map_err(|_| "settings state is unavailable")?;
                *saved_settings = settings;
            }
            if let Some(enrollment) = load_voice_profile(app.handle())? {
                persist_voice_profile(app.handle(), &enrollment)?;
                let mut voice_enrollment = state
                    .voice_enrollment
                    .lock()
                    .map_err(|_| "voice enrollment state is unavailable")?;
                *voice_enrollment = enrollment;
            }
            let settings = normalize_saved_settings(app.handle(), &state)?;
            sync_runtime(&settings, &state, app.handle(), false)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running MuteBack Tauri app");
}
