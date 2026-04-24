use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use muteback::config::AppConfig;
use muteback::runtime::{RuntimeEvent, RuntimeHandle};
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

const TRAY_ICON: tauri::image::Image<'_> = tauri::include_image!("./icons/tray.png");
const MAIN_HOME_SIZE: (f64, f64) = (360.0, 260.0);
const MAIN_SETTINGS_SIZE: (f64, f64) = (620.0, 720.0);
const RESTORE_WINDOW_LABEL: &str = "restore_prompt";
const RESTORE_WINDOW_SIZE: (f64, f64) = (220.0, 72.0);

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum TransitionMode {
    Smooth,
    Instant,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    enabled: bool,
    duck_level_percent: u8,
    transition: TransitionMode,
    manual_restore: bool,
    voice_match_enabled: bool,
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
            duck_fade_ms: config.duck_fade.as_millis() as u64,
            restore_fade_ms: config.restore_fade.as_millis() as u64,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoiceSampleInput {
    phrase_index: u8,
    duration_ms: u64,
    bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceSample {
    phrase_index: u8,
    duration_ms: u64,
    bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceEnrollment {
    required_samples: u8,
    samples: Vec<VoiceSample>,
}

impl Default for VoiceEnrollment {
    fn default() -> Self {
        Self {
            required_samples: 3,
            samples: Vec::new(),
        }
    }
}

impl VoiceEnrollment {
    fn is_complete(&self) -> bool {
        self.samples.len() >= self.required_samples as usize
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
    validate_settings(&input)?;

    {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings state is unavailable".to_string())?;
        *settings = input.clone();
    }

    sync_runtime(&input, &state, &app)?;
    update_restore_prompt(&app, &state)?;
    Ok(input)
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
fn add_voice_sample(
    input: VoiceSampleInput,
    state: tauri::State<'_, AppState>,
) -> Result<VoiceEnrollment, String> {
    if input.duration_ms < 500 {
        return Err("voice sample is too short".to_string());
    }

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
        bytes: input.bytes,
    });

    Ok(enrollment.clone())
}

#[tauri::command]
fn reset_voice_enrollment(state: tauri::State<'_, AppState>) -> Result<VoiceEnrollment, String> {
    let mut enrollment = state
        .voice_enrollment
        .lock()
        .map_err(|_| "voice enrollment state is unavailable".to_string())?;
    *enrollment = VoiceEnrollment::default();
    Ok(enrollment.clone())
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

fn validate_settings(settings: &Settings) -> Result<(), String> {
    if settings.duck_level_percent > 100 {
        return Err("ducking level must be between 0 and 100".to_string());
    }

    Ok(())
}

fn sync_runtime(settings: &Settings, state: &AppState, app: &AppHandle) -> Result<(), String> {
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
    }
}

fn start_or_update_runtime(
    settings: &Settings,
    state: &AppState,
    app: &AppHandle,
) -> Result<(), String> {
    let config = app_config_from_settings(settings);
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

            let _ = update_restore_prompt_from_shared(&app, &settings, &runtime_status);

            if stopped {
                break;
            }
        }
    });
}

fn app_config_from_settings(settings: &Settings) -> AppConfig {
    let mut config = AppConfig::default();
    config.ducking_level = settings.duck_level_percent as f32 / 100.0;
    config.smooth_ducking = settings.transition == TransitionMode::Smooth;
    config.manual_restore = settings.manual_restore;
    config.duck_fade = std::time::Duration::from_millis(settings.duck_fade_ms);
    config.restore_fade = std::time::Duration::from_millis(settings.restore_fade_ms);
    config
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

fn resize_main_window(
    window: &WebviewWindow,
    size: (f64, f64),
    resizable: bool,
) -> Result<(), String> {
    let logical_size = LogicalSize::new(size.0, size.1);

    window
        .set_resizable(resizable)
        .map_err(|error| error.to_string())?;
    window
        .set_min_size(Some(logical_size))
        .map_err(|error| error.to_string())?;
    window
        .set_size(logical_size)
        .map_err(|error| error.to_string())?;
    window.center().map_err(|error| error.to_string())?;

    Ok(())
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
        position_restore_window(&window).map_err(|error| error.to_string())?;
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

    TrayIconBuilder::new()
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
            get_runtime_status,
            get_voice_enrollment,
            add_voice_sample,
            reset_voice_enrollment,
            request_restore,
            set_main_view,
            set_restore_prompt_visible
        ])
        .setup(|app| {
            create_tray(app)?;
            create_restore_window(app)?;
            let state = app.state::<AppState>();
            let settings = state
                .settings
                .lock()
                .map(|settings| settings.clone())
                .map_err(|_| "settings state is unavailable")?;
            sync_runtime(&settings, &state, app.handle())?;
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
