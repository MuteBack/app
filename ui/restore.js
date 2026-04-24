const { invoke } = window.__TAURI__.core;

const restoreButton = document.querySelector("#restore-sound");

restoreButton.addEventListener("click", async () => {
  restoreButton.disabled = true;
  restoreButton.textContent = "Restoring";

  try {
    await invoke("request_restore");
  } finally {
    restoreButton.disabled = false;
    restoreButton.textContent = "Restore Sound";
  }
});

async function hideIfStale() {
  const [settings, status] = await Promise.all([
    invoke("get_settings"),
    invoke("get_runtime_status"),
  ]);

  if (!settings.manualRestore || !status.ducked) {
    await invoke("set_restore_prompt_visible", { visible: false });
  }
}

await hideIfStale();
setInterval(hideIfStale, 1000);
