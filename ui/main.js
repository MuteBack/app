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

const enrollmentCapture = {
  startThreshold: 0.018,
  continueThreshold: 0.011,
  minSpeechMs: 1100,
  endSilenceMs: 850,
  startTimeoutMs: 6000,
  maxMs: 10000,
};

let settings = await invoke("get_settings");
let enrollment = await invoke("get_voice_enrollment");
let isRecording = false;
let runtimeStatus = await invoke("get_runtime_status");

function render() {
  const enrollmentComplete = Boolean(enrollment.profile);
  controls.appEnabled.checked = settings.enabled;
  controls.level.value = settings.duckLevelPercent;
  controls.levelValue.value = `${settings.duckLevelPercent}%`;
  controls.duckFade.value = settings.duckFadeMs;
  controls.restoreFade.value = settings.restoreFadeMs;
  controls.manualRestore.checked = settings.manualRestore;
  controls.voiceMatchEnabled.disabled = !enrollmentComplete;
  controls.voiceMatchEnabled.checked = enrollmentComplete && settings.voiceMatchEnabled;
  controls.restoreModeLabel.textContent = settings.manualRestore ? "Manual" : "Automatic";
  controls.voiceMatchLabel.textContent = enrollmentComplete && settings.voiceMatchEnabled ? "On" : "Off";
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
  await invoke("set_main_view", { view: "settings" });
  controls.homePage.hidden = true;
  controls.settingsPage.hidden = false;
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
    voiceMatchEnabled: Boolean(enrollment.profile) && controls.voiceMatchEnabled.checked,
    duckFadeMs: Number(controls.duckFade.value),
    restoreFadeMs: Number(controls.restoreFade.value),
    ...overrides,
  };
}

function renderEnrollment() {
  const sampleCount = enrollment.samples.length;
  const required = enrollment.requiredSamples;
  const currentIndex = Math.min(sampleCount, phrases.length - 1);
  const isComplete = Boolean(enrollment.profile);

  controls.phraseStep.textContent = isComplete
    ? "Enrollment complete"
    : `Sample ${sampleCount + 1} of ${required}`;
  controls.phraseText.textContent = isComplete
    ? "Voice match is ready."
    : phrases[currentIndex];
  controls.recordVoice.disabled = isRecording || isComplete;
  controls.resetVoice.disabled = sampleCount === 0 || isRecording;
  controls.recordVoice.textContent = isRecording ? "Recording" : "Record Sample";
  controls.voiceProgress.textContent = isComplete
    ? `${sampleCount}/${required} samples recorded · threshold ${enrollment.profile.threshold.toFixed(2)}`
    : `${sampleCount}/${required} samples recorded`;
}

async function recordVoiceSample() {
  const AudioContextClass = window.AudioContext || window.webkitAudioContext;
  if (!navigator.mediaDevices?.getUserMedia || !AudioContextClass) {
    controls.saveStatus.textContent = "Mic unsupported";
    return;
  }

  isRecording = true;
  renderEnrollment();
  controls.saveStatus.textContent = "Listening";

  const startedAt = performance.now();
  const captured = await capturePcmSampleUntilSilence(AudioContextClass, (status) => {
    controls.saveStatus.textContent = status;
  });
  const durationMs = Math.round(performance.now() - startedAt);

  controls.saveStatus.textContent = "Embedding";
  enrollment = await invoke("add_voice_sample", {
    input: {
      phraseIndex: enrollment.samples.length,
      durationMs,
      sampleRate: captured.sampleRate,
      samples: captured.samples,
    },
  });

  isRecording = false;
  controls.saveStatus.textContent = "Saved";
  render();
}

async function capturePcmSampleUntilSilence(AudioContextClass, onStatus) {
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: false,
    },
  });
  const audioContext = new AudioContextClass();
  const source = audioContext.createMediaStreamSource(stream);
  const processor = audioContext.createScriptProcessor(4096, 1, 1);
  const monitorGain = audioContext.createGain();
  const sampleRate = Math.round(audioContext.sampleRate);
  const chunks = [];
  const preSpeechChunks = [];
  const startedAt = performance.now();
  let speechStartedAt = null;
  let lastSpeechAt = null;
  let settled = false;
  let stopTimer = null;

  monitorGain.gain.value = 0;

  const cleanup = async () => {
    if (stopTimer) {
      clearInterval(stopTimer);
    }
    processor.disconnect();
    source.disconnect();
    monitorGain.disconnect();
    for (const track of stream.getTracks()) {
      track.stop();
    }
    await audioContext.close();
  };

  processor.onaudioprocess = (event) => {
    const now = performance.now();
    const input = event.inputBuffer.getChannelData(0);
    const chunk = new Float32Array(input);
    const rms = normalizedRms(chunk);

    if (!speechStartedAt) {
      preSpeechChunks.push(chunk);
      if (preSpeechChunks.length > 6) {
        preSpeechChunks.shift();
      }
    }

    if (!speechStartedAt && rms >= enrollmentCapture.startThreshold) {
      speechStartedAt = now;
      lastSpeechAt = now;
      chunks.push(...preSpeechChunks);
      preSpeechChunks.length = 0;
      chunks.push(chunk);
      onStatus("Recording");
    } else if (speechStartedAt && rms >= enrollmentCapture.continueThreshold) {
      lastSpeechAt = now;
      chunks.push(chunk);
    } else if (speechStartedAt) {
      chunks.push(chunk);
    } else if (!speechStartedAt) {
      onStatus("Listening");
    }
  };

  source.connect(processor);
  processor.connect(monitorGain);
  monitorGain.connect(audioContext.destination);

  await new Promise((resolve, reject) => {
    const stop = (callback) => {
      if (settled) {
        return;
      }
      settled = true;
      callback();
    };

    stopTimer = setInterval(() => {
      const now = performance.now();
      const elapsed = now - startedAt;

      if (!speechStartedAt && elapsed >= enrollmentCapture.startTimeoutMs) {
        stop(() => reject(new Error("No speech detected")));
        return;
      }

      if (
        speechStartedAt &&
        now - speechStartedAt >= enrollmentCapture.minSpeechMs &&
        lastSpeechAt &&
        now - lastSpeechAt >= enrollmentCapture.endSilenceMs
      ) {
        onStatus("Finishing");
        stop(resolve);
        return;
      }

      if (elapsed >= enrollmentCapture.maxMs) {
        stop(resolve);
      }
    }, 80);
  }).finally(cleanup);

  const totalLength = chunks.reduce((total, chunk) => total + chunk.length, 0);
  const samples = new Array(totalLength);
  let cursor = 0;
  for (const chunk of chunks) {
    for (const sample of chunk) {
      samples[cursor] = sample;
      cursor += 1;
    }
  }

  return {
    sampleRate,
    samples,
  };
}

function normalizedRms(samples) {
  if (samples.length === 0) {
    return 0;
  }

  let sum = 0;
  for (const sample of samples) {
    sum += sample * sample;
  }

  return Math.sqrt(sum / samples.length);
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
  if (!enrollment.profile) {
    controls.voiceMatchEnabled.checked = false;
    return;
  }
  save(settingsFromControls());
});

controls.recordVoice.addEventListener("click", async () => {
  try {
    await recordVoiceSample();
  } catch (error) {
    isRecording = false;
    controls.saveStatus.textContent =
      error?.message === "No speech detected" ? "No speech" : "Mic error";
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
