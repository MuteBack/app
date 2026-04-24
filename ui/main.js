const { invoke } = window.__TAURI__.core;

const controls = {
  homePage: document.querySelector("#home-page"),
  settingsPage: document.querySelector("#settings-page"),
  openSettings: document.querySelector("#open-settings"),
  closeSettings: document.querySelector("#close-settings"),
  appEnabled: document.querySelector("#app-enabled"),
  statusDot: document.querySelector("#status-dot"),
  level: document.querySelector("#duck-level"),
  levelValue: document.querySelector("#duck-level-value"),
  restoreModeLabel: document.querySelector("#restore-mode-label"),
  voiceMatchLabel: document.querySelector("#voice-match-label"),
  transitionButtons: [...document.querySelectorAll("[data-transition]")],
  manualRestore: document.querySelector("#manual-restore"),
  voiceMatchEnabled: document.querySelector("#voice-match-enabled"),
  recordVoice: document.querySelector("#record-voice"),
  resetVoice: document.querySelector("#reset-voice"),
  phraseStep: document.querySelector("#phrase-step"),
  phraseText: document.querySelector("#phrase-text"),
  voiceProgress: document.querySelector("#voice-progress"),
  duckFade: document.querySelector("#duck-fade-ms"),
  restoreFade: document.querySelector("#restore-fade-ms"),
  timingGrid: document.querySelector("#timing-grid"),
  previewFill: document.querySelector("#preview-fill"),
  saveStatus: document.querySelector("#save-status"),
};

const phrases = [
  "Today I want my music to move out of the way when I speak.",
  "This app should listen for my voice and ignore background speech.",
  "When I finish talking, I can choose when the sound comes back.",
];

let settings = await invoke("get_settings");
let enrollment = await invoke("get_voice_enrollment");
let isRecording = false;
let runtimeStatus = await invoke("get_runtime_status");

function render() {
  controls.appEnabled.checked = settings.enabled;
  controls.level.value = settings.duckLevelPercent;
  controls.levelValue.value = `${settings.duckLevelPercent}%`;
  controls.duckFade.value = settings.duckFadeMs;
  controls.restoreFade.value = settings.restoreFadeMs;
  controls.manualRestore.checked = settings.manualRestore;
  controls.voiceMatchEnabled.checked = settings.voiceMatchEnabled;
  controls.restoreModeLabel.textContent = settings.manualRestore ? "Manual" : "Automatic";
  controls.voiceMatchLabel.textContent = settings.voiceMatchEnabled ? "On" : "Off";
  document.body.dataset.enabled = settings.enabled;
  controls.timingGrid.dataset.disabled = settings.transition === "instant";
  controls.previewFill.style.width = `${Math.max(settings.duckLevelPercent, 2)}%`;

  for (const button of controls.transitionButtons) {
    button.dataset.active = button.dataset.transition === settings.transition;
  }

  renderRuntime();
  renderEnrollment();
}

async function save(nextSettings) {
  settings = nextSettings;
  render();
  controls.saveStatus.textContent = "Saving";

  try {
    settings = await invoke("update_settings", { input: settings });
    controls.saveStatus.textContent = "Saved";
    render();
  } catch (error) {
    controls.saveStatus.textContent = "Error";
    console.error(error);
  }
}

async function refreshRuntimeStatus() {
  try {
    runtimeStatus = await invoke("get_runtime_status");
    renderRuntime();
  } catch (error) {
    controls.statusDot.dataset.state = "stopped";
    controls.statusDot.title = "Status unavailable";
    console.error(error);
  }
}

function renderRuntime() {
  const state = runtimeStatus.ducked
    ? "ducked"
    : runtimeStatus.running
      ? "running"
      : runtimeStatus.enabled
        ? "stopped"
        : "disabled";

  controls.statusDot.dataset.state = state;
  controls.statusDot.title = statusTitle(state);
}

function statusTitle(state) {
  switch (state) {
    case "running":
      return "MuteBack is active";
    case "ducked":
      return "Background audio is lowered";
    case "disabled":
      return "MuteBack is disabled";
    default:
      return "MuteBack is stopped";
  }
}

async function openSettings() {
  controls.homePage.hidden = true;
  controls.settingsPage.hidden = false;
  await invoke("set_main_view", { view: "settings" });
}

function showHome() {
  controls.settingsPage.hidden = true;
  controls.homePage.hidden = false;
}

async function closeSettings() {
  showHome();
  await invoke("set_main_view", { view: "home" });
}

function settingsFromControls(overrides = {}) {
  return {
    enabled: controls.appEnabled.checked,
    duckLevelPercent: Number(controls.level.value),
    transition: settings.transition,
    manualRestore: controls.manualRestore.checked,
    voiceMatchEnabled: controls.voiceMatchEnabled.checked,
    duckFadeMs: Number(controls.duckFade.value),
    restoreFadeMs: Number(controls.restoreFade.value),
    ...overrides,
  };
}

function renderEnrollment() {
  const sampleCount = enrollment.samples.length;
  const required = enrollment.requiredSamples;
  const currentIndex = Math.min(sampleCount, phrases.length - 1);
  const isComplete = sampleCount >= required;

  controls.phraseStep.textContent = isComplete
    ? "Enrollment complete"
    : `Sample ${sampleCount + 1} of ${required}`;
  controls.phraseText.textContent = isComplete
    ? "Voice enrollment is ready for a speaker verification model."
    : phrases[currentIndex];
  controls.recordVoice.disabled = isRecording || isComplete || !settings.voiceMatchEnabled;
  controls.resetVoice.disabled = sampleCount === 0 || isRecording;
  controls.recordVoice.textContent = isRecording ? "Recording" : "Record Sample";
  controls.voiceProgress.textContent = `${sampleCount}/${required} samples recorded`;
}

async function recordVoiceSample() {
  if (!navigator.mediaDevices?.getUserMedia || typeof MediaRecorder === "undefined") {
    controls.saveStatus.textContent = "Mic unsupported";
    return;
  }

  isRecording = true;
  renderEnrollment();
  controls.saveStatus.textContent = "Recording";

  const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  const recorder = new MediaRecorder(stream);
  const startedAt = performance.now();
  const chunks = [];

  recorder.addEventListener("dataavailable", (event) => {
    if (event.data.size > 0) {
      chunks.push(event.data);
    }
  });

  await new Promise((resolve) => {
    recorder.addEventListener("stop", resolve, { once: true });
    recorder.start();
    setTimeout(() => recorder.stop(), 2800);
  });

  for (const track of stream.getTracks()) {
    track.stop();
  }

  const bytes = chunks.reduce((total, chunk) => total + chunk.size, 0);
  const durationMs = Math.round(performance.now() - startedAt);

  enrollment = await invoke("add_voice_sample", {
    input: {
      phraseIndex: enrollment.samples.length,
      durationMs,
      bytes,
    },
  });

  isRecording = false;
  controls.saveStatus.textContent = "Saved";
  render();
}

controls.openSettings.addEventListener("click", () => {
  openSettings().catch(console.error);
});

controls.closeSettings.addEventListener("click", () => {
  closeSettings().catch(console.error);
});

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !controls.settingsPage.hidden) {
    closeSettings().catch(console.error);
  }
});

window.addEventListener("muteback:show-home", showHome);

controls.appEnabled.addEventListener("change", () => {
  save(settingsFromControls());
});

controls.level.addEventListener("input", () => {
  save(settingsFromControls());
});

controls.duckFade.addEventListener("change", () => {
  save(settingsFromControls());
});

controls.restoreFade.addEventListener("change", () => {
  save(settingsFromControls());
});

controls.manualRestore.addEventListener("change", () => {
  save(settingsFromControls());
});

controls.voiceMatchEnabled.addEventListener("change", () => {
  save(settingsFromControls());
});

controls.recordVoice.addEventListener("click", async () => {
  try {
    await recordVoiceSample();
  } catch (error) {
    isRecording = false;
    controls.saveStatus.textContent = "Mic error";
    renderEnrollment();
    console.error(error);
  }
});

controls.resetVoice.addEventListener("click", async () => {
  enrollment = await invoke("reset_voice_enrollment");
  controls.saveStatus.textContent = "Reset";
  render();
});

for (const button of controls.transitionButtons) {
  button.addEventListener("click", () => {
    save(settingsFromControls({ transition: button.dataset.transition }));
  });
}

render();
await refreshRuntimeStatus();
setInterval(refreshRuntimeStatus, 1000);
