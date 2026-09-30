(function () {
  const steps = document.querySelectorAll(".flow-step[data-step]");
  if (!steps.length) return;

  let index = 0;
  const order = Array.from(steps);

  function tick() {
    order.forEach((el, i) => {
      el.classList.toggle("is-active", i === index);
    });
    index = (index + 1) % order.length;
  }

  tick();
  setInterval(tick, 1400);

  document.querySelectorAll(".copy-btn[data-copy]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const id = btn.getAttribute("data-copy");
      const pre = document.getElementById(id);
      if (!pre) return;
      const text = pre.innerText;
      try {
        await navigator.clipboard.writeText(text);
        btn.textContent = "Copied";
        btn.classList.add("copied");
        setTimeout(() => {
          btn.textContent = "Copy workflow";
          btn.classList.remove("copied");
        }, 2000);
      } catch {
        btn.textContent = "Select & copy";
      }
    });
  });
})();
