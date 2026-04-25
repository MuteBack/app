const { invoke } = window.__TAURI__.core;

const restoreButton = document.querySelector("#restore-sound");
const dragRegion = document.querySelector("#restore-drag-region");

dragRegion.addEventListener("pointerdown", async (event) => {
  if (event.button !== 0) {
    return;
  }

  try {
    await invoke("start_restore_prompt_drag");
  } catch (error) {
    console.error(error);
  }
});

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
