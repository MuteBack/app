const demoSteps = [
  { title: "Start once", duration: 1700 },
  { title: "Runs quietly", duration: 1700 },
  { title: "Audio ducks", duration: 2300 },
  { title: "Sound returns", duration: 1700 },
];

document.querySelectorAll("[data-demo]").forEach((demo) => {
  const caption = demo.querySelector("[data-demo-caption]");
  const volume = demo.querySelector("[data-demo-volume]");
  const dots = Array.from(demo.querySelectorAll("[data-demo-dot]"));
  const prev = demo.querySelector("[data-demo-prev]");
  const next = demo.querySelector("[data-demo-next]");
  let step = 0;
  let auto = true;
  let timerId;

  function render() {
    demo.dataset.step = String(step);
    if (caption) {
      caption.textContent = demoSteps[step].title;
    }
    if (volume) {
      volume.textContent = step === 2 ? "10%" : "100%";
    }
    dots.forEach((dot, index) => {
      const selected = index === step;
      dot.setAttribute("aria-pressed", String(selected));
    });
  }

  function queueNextStep() {
    window.clearTimeout(timerId);
    if (!auto) return;
    timerId = window.setTimeout(() => {
      step = (step + 1) % demoSteps.length;
      render();
      queueNextStep();
    }, demoSteps[step].duration);
  }

  function setStep(nextStep, shouldStopAuto = true) {
    auto = !shouldStopAuto;
    step = (nextStep + demoSteps.length) % demoSteps.length;
    render();
    queueNextStep();
  }

  prev?.addEventListener("click", () => setStep(step - 1));
  next?.addEventListener("click", () => setStep(step + 1));
  dots.forEach((dot, index) => {
    dot.addEventListener("click", () => setStep(index));
  });

  render();
  queueNextStep();
});
