(function () {
  // Faithful offline simulation of `karat --json shape` for UserMetadata.
  // Pass: len 1032 · Fail: len 0 — same JSON + exit semantics as the CLI.
  const CASES = {
    pass: {
      banner: "=== Pass: well-formed UserMetadata ===",
      cmd: "karat --json shape --account-type UserMetadata --len 1032",
      json: {
        check: "shape(UserMetadata)",
        status: "Pass",
        detail: null,
      },
      exit: 0,
    },
    fail: {
      banner: "=== Fail: empty payload (silent corruption class) ===",
      cmd: "karat --json shape --account-type UserMetadata --len 0",
      json: {
        check: "shape",
        status: "Fail",
        detail:
          "empty payload: 0 bytes, every allocated account starts with an 8-byte discriminator",
      },
      exit: 1,
    },
  };

  const out = document.getElementById("demo-out");
  const buttons = document.querySelectorAll(".try-btn[data-case]");

  function escapeHtml(s) {
    return String(s)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  function formatCase(key) {
    const c = CASES[key];
    const json = JSON.stringify(c.json, null, 2);
    const exitClass = c.exit === 0 ? "line-exit-ok" : "line-exit-bad";
    const statusClass = c.json.status === "Pass" ? "line-pass" : "line-fail";
    return [
      `<span class="line-banner">${escapeHtml(c.banner)}</span>`,
      `<span class="line-muted">$ ${escapeHtml(c.cmd)}</span>`,
      "",
      escapeHtml(json)
        .replace(/"Pass"/g, `"<span class="${statusClass}">Pass</span>"`)
        .replace(/"Fail"/g, `"<span class="${statusClass}">Fail</span>"`),
      "",
      `<span class="${exitClass}">exit ${c.exit}</span>`,
    ].join("\n");
  }

  function render(keys) {
    const blocks = keys.map(formatCase);
    if (keys.length === 2) {
      blocks.push(
        "",
        `<span class="line-exit-ok">ACCEPTANCE OK: empty payload failed under green shape check</span>`,
        `<span class="line-muted"># Real binary: bash demo/worlds_fair.sh</span>`
      );
    }
    out.innerHTML = `<code>${blocks.join("\n\n")}</code>`;
  }

  function setActive(caseName) {
    buttons.forEach((btn) => {
      btn.classList.toggle("is-active", btn.getAttribute("data-case") === caseName);
    });
  }

  buttons.forEach((btn) => {
    btn.addEventListener("click", () => {
      const which = btn.getAttribute("data-case");
      setActive(which);
      if (which === "both") {
        render(["pass", "fail"]);
      } else {
        render([which]);
      }
    });
  });

  // Flow step highlight
  const steps = document.querySelectorAll(".flow-step[data-step]");
  if (steps.length) {
    let index = 0;
    const order = Array.from(steps);
    function tick() {
      order.forEach((el, i) => {
        el.classList.toggle("is-active", i === index);
      });
      index = (index + 1) % order.length;
    }
    tick();
    setInterval(tick, 1600);
  }

  // Copy buttons
  document.querySelectorAll(".copy-btn[data-copy]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const id = btn.getAttribute("data-copy");
      const pre = document.getElementById(id);
      if (!pre) return;
      const label = btn.textContent;
      try {
        await navigator.clipboard.writeText(pre.innerText);
        btn.textContent = "Copied";
        btn.classList.add("copied");
        setTimeout(() => {
          btn.textContent = label;
          btn.classList.remove("copied");
        }, 2000);
      } catch {
        btn.textContent = "Select & copy";
      }
    });
  });
})();
