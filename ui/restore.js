const { invoke } = window.__TAURI__.core;

const restoreButton = document.querySelector("#restore-sound");
const restoreLabel = document.querySelector("#restore-label");
let dragStart = null;
let dragged = false;

restoreButton.addEventListener("pointerdown", (event) => {
  if (event.button !== 0) {
    return;
  }

  dragStart = {
    pointerId: event.pointerId,
    x: event.clientX,
    y: event.clientY,
  };
  dragged = false;
  restoreButton.setPointerCapture(event.pointerId);
});

restoreButton.addEventListener("pointermove", async (event) => {
  if (!dragStart || dragStart.pointerId !== event.pointerId || dragged) {
    return;
  }

  const distance = Math.hypot(event.clientX - dragStart.x, event.clientY - dragStart.y);
  if (distance < 4) {
    return;
  }

  dragged = true;
  dragStart = null;

  try {
    await invoke("start_restore_prompt_drag");
  } catch (error) {
    console.error(error);
  }
});

function finishPointerDrag() {
  dragStart = null;
  setTimeout(() => {
    dragged = false;
  }, 350);
}

restoreButton.addEventListener("pointerup", finishPointerDrag);
restoreButton.addEventListener("pointercancel", finishPointerDrag);

restoreButton.addEventListener("click", async () => {
  if (dragged) {
    dragged = false;
    return;
  }

  restoreButton.disabled = true;
  restoreLabel.textContent = "Restoring";

  try {
    await invoke("request_restore");
  } finally {
    restoreButton.disabled = false;
    restoreLabel.textContent = "Restore Sound";
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
