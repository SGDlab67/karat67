// In-browser account gate.
//
// The verdict comes from the karat67 crate compiled to WebAssembly, not from
// JavaScript. This file only moves bytes: build or decode the indexed payload,
// optionally fetch the account over JSON-RPC, hand both to the wasm export,
// draw what it returns. See src/wasm.rs for why the fetch happens here.

import init, {
  shape_json,
  specs_json,
  check_account_json,
} from "./pkg/karat67.js";

const ENDPOINT_KEY = "karat67.rpc_endpoint";
// Free, keyless, and allows browser requests. The Solana Foundation endpoint
// answers 403 to any request carrying a web Origin, so it cannot serve here.
const PUBLIC_RPC = "https://solana-rpc.publicnode.com";

const el = {
  form: document.getElementById("gate-form"),
  account: document.getElementById("account"),
  indexed: document.getElementById("indexed"),
  indexedSlot: document.getElementById("indexed-slot"),
  rpc: document.getElementById("rpc"),
  endpointState: document.getElementById("endpoint-state"),
  clearEndpoint: document.getElementById("clear-endpoint"),
  run: document.getElementById("run-gate"),
  out: document.getElementById("gate-out"),
  advanced: document.getElementById("advanced"),
  livePicker: document.getElementById("live-picker"),
  preset: document.getElementById("preset"),
  damage: document.getElementById("damage"),
  idlLabel: document.getElementById("idl-label"),
  idlCount: document.getElementById("idl-count"),
  idlBar: document.getElementById("bar-idl"),
  idlDisc: document.getElementById("idl-disc"),
  wroteCount: document.getElementById("wrote-count"),
  wroteBar: document.getElementById("bar-wrote"),
  void: document.getElementById("bytefield-void"),
};

let wasmReady = false;

// ---------------------------------------------------------------- registry

// Served by the wasm module from crate::checks::specs, not copied here. A
// hand-kept copy drifts the moment a program is registered, and then the page
// shows a length the gate never checked against. Empty until init() resolves,
// so every reader below runs after that.
let SPECS = [];

const specOf = (name) => SPECS.find((s) => s.type === name);

/** The spec whose discriminator the payload carries, or null. */
function findSpec(bytes) {
  if (bytes.length < 8) return null;
  return SPECS.find((s) => s.disc.every((b, i) => bytes[i] === b)) ?? null;
}

/** len bytes, spec's discriminator at offset 0. Zeros elsewhere: shape reads
 *  the discriminator and the length, never the field values. */
function payload(spec, len) {
  const bytes = new Uint8Array(len);
  if (spec && len >= 8) bytes.set(spec.disc, 0);
  return bytes;
}

// The four constructed cases. `spec` is the length the payload should have
// had, used when the payload carries no discriminator to resolve.
const CASES = {
  healthy: {
    name: "healthy Obligation",
    get spec() {
      return specOf("Obligation");
    },
    bytes: () => payload(specOf("Obligation"), 3344),
  },
  empty: {
    name: "empty payload",
    get spec() {
      return specOf("Obligation");
    },
    bytes: () => new Uint8Array(0),
  },
  truncated: {
    name: "truncated write",
    get spec() {
      return specOf("Obligation");
    },
    bytes: () => payload(specOf("Obligation"), 24),
  },
  // 424 bytes is not unique to VirtualPool: TransferHookPool is the same
  // length with a different discriminator. A length-only check would accept
  // this as VirtualPool; the gate names TransferHookPool. Mirrors the Rust
  // test same_424_length_does_not_make_transfer_hook_pool_a_virtual_pool.
  collision: {
    name: "right length, TransferHookPool",
    get spec() {
      return specOf("VirtualPool");
    },
    bytes: () => {
      const bytes = new Uint8Array(424);
      bytes.set([237, 219, 184, 23, 42, 189, 169, 35]);
      return bytes;
    },
  },
};

// What the stand-in indexer stored, given the bytes it read.
const DAMAGE = {
  none: { name: "none", apply: (b) => b },
  flip: {
    name: "one byte flipped at offset 100",
    apply: (b) => {
      const c = b.slice();
      c[Math.min(100, c.length - 1)] ^= 0xff;
      return c;
    },
  },
  truncate: {
    name: "cut 24 bytes short",
    apply: (b) => b.slice(0, Math.max(0, b.length - 24)),
  },
  empty: { name: "an empty row", apply: () => new Uint8Array(0) },
};

const selectedCase = () =>
  document.querySelector('input[name="case"]:checked')?.value ?? "healthy";

// ---------------------------------------------------------------- rendering

const ICON = { Pass: "PASS", Fail: "FAIL", Skipped: "SKIP" };
const CLS = { Pass: "line-pass", Fail: "line-fail", Skipped: "line-muted" };

function esc(text) {
  return String(text).replace(
    /[&<>]/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c],
  );
}

function line(cls, text) {
  return `<code class="${cls}">${esc(text)}</code>`;
}

function write(lines) {
  el.out.innerHTML = lines.join("");
}

/** Reconcile details are JSON. Same wording as `describe` in the CLI;
 *  anything that does not parse prints raw. */
function describe(detail) {
  let v;
  try {
    v = JSON.parse(detail);
  } catch {
    return [detail];
  }
  if (typeof v?.first_diff_offset !== "number") return [detail];
  const lines =
    v.reason === "slot lag"
      ? [
          `bytes differ at offset ${v.first_diff_offset}, indexer ${v.lag} slots behind`,
          "inside the slot-lag window: recheck, do not trust yet",
        ]
      : [
          `bytes differ from chain at offset ${v.first_diff_offset}`,
          `indexed ${v.indexed_len} bytes, on-chain ${v.onchain_len} bytes`,
        ];
  lines.push(`chain read at slot ${v.context_slot}`);
  return lines;
}

function renderResults(results, header, notes = []) {
  const lines = [line("line-head", header)];
  for (const note of notes) lines.push(line("line-muted", note));
  if (notes.length) lines.push(line("line-muted", ""));

  for (const r of results) {
    lines.push(
      line(
        CLS[r.status] ?? "line-muted",
        `${ICON[r.status] ?? "????"}  ${r.check}`,
      ),
    );
    for (const d of r.detail ? describe(r.detail) : []) {
      lines.push(line("line-muted", `      ${d}`));
    }
  }

  // Mirrors karat67::gate::all_pass: non-empty and every status Pass.
  // Skipped is not Pass, so a lag-tolerated run still exits 1.
  const ok = results.length > 0 && results.every((r) => r.status === "Pass");
  const failed = results.some((r) => r.status === "Fail");
  lines.push(line("line-muted", ""));
  if (ok) {
    lines.push(line("line-exit-ok", "exit 0  every check passed"));
  } else if (failed) {
    lines.push(line("line-exit-bad", "exit 1  a check failed"));
  } else {
    // Nothing failed; a check could not run. Still exit 1, but it is not the
    // same signal as corruption and is not coloured like it.
    lines.push(line("line-exit-skip", "exit 1  a check could not run"));
  }
  write(lines);
}

function renderError(message, hint) {
  const lines = [line("line-fail", `error  ${message}`)];
  if (hint) lines.push(line("line-muted", `       ${hint}`));
  write(lines);
}

// --------------------------------------------------------------- byte field

const bytes = (n) => `${n} bytes`;

/** Whether the written length satisfies the spec. `floor` types pass at or
 *  above the registered length; fixed types pass only exactly on it. */
function passesLength(wrote, declared, floor) {
  if (declared === null) return false;
  return floor ? wrote >= declared : wrote === declared;
}

function voidText(wrote, declared, type, floor = false) {
  if (declared === null) {
    return "Discriminator matches no registered account type, so there is no declared length to compare against.";
  }
  if (passesLength(wrote, declared, floor)) {
    return floor && wrote > declared
      ? `${wrote - declared} bytes above the ${declared}-byte minimum. ${type} carries variable-length fields, so this is a healthy length, not an overrun.`
      : "";
  }
  if (floor) {
    return `${declared - wrote} bytes below the ${declared} a ${type} needs to hold its fixed fields.`;
  }
  if (wrote > declared) {
    return `${wrote - declared} bytes beyond the ${declared} the ${type} discriminator claims.`;
  }
  const missing = declared - wrote;
  if (wrote === 0) {
    return `Nothing written: all ${missing} bytes missing. This is the class that hid for 158.91 hours behind green liveness.`;
  }
  const share = ((missing / declared) * 100).toFixed(1);
  return `${missing} bytes never written, ${share}% of the account.`;
}

/**
 * Draw the byte field for a payload. `fallback` supplies the declared length
 * when the payload carries no resolvable discriminator (an empty row has none).
 * `animate` runs the one motion moment: the lower bar growing to its width.
 */
function drawField(payloadBytes, fallback, animate) {
  // The fallback is only for a payload too short to carry a discriminator at
  // all, where the type is whatever the operator says the row was. A payload
  // that does carry one and matches nothing must draw as unknown: labelling it
  // with the type the visitor picked would show a full bar and no shortfall
  // next to a FAIL, which is the green-while-wrong reading this gate exists to
  // refuse.
  const carried = findSpec(payloadBytes);
  const spec = carried ?? (payloadBytes.length < 8 ? (fallback ?? null) : null);
  const wrote = payloadBytes.length;
  const declared = spec ? spec.len : null;
  const basis = Math.max(wrote, declared ?? 0, 1);

  // A variable-length type registers a floor, not a layout, so "declared" is
  // a minimum and anything at or above it is healthy. Drawing it as a target
  // would mark a valid long account red.
  const floor = spec?.lenRule === "atLeast";

  el.idlLabel.textContent = spec
    ? `IDL, ${spec.type}${floor ? ", minimum" : ""}`
    : "IDL, unknown type";
  el.idlCount.textContent = declared === null ? "unknown" : bytes(declared);
  el.idlBar.style.width = `${(((declared ?? 0) / basis) * 100).toFixed(2)}%`;
  el.idlDisc.hidden = declared === null;

  el.wroteCount.textContent = bytes(wrote);
  el.wroteBar.dataset.verdict = passesLength(wrote, declared, floor)
    ? "pass"
    : "fail";
  el.void.textContent = voidText(wrote, declared, spec?.type, floor);

  // 0.72% on the truncated case is a sliver next to a full bar. That contrast
  // is the point, so no minimum width is applied.
  const target = `${((wrote / basis) * 100).toFixed(2)}%`;
  if (!animate) {
    el.wroteBar.style.width = target;
    return;
  }
  el.wroteBar.style.width = "0%";
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      el.wroteBar.style.width = target;
    });
  });
}

/** Live case before a fetch: nothing to draw yet, and no verdict to imply. */
function pendingField() {
  el.idlLabel.textContent = "IDL, after the fetch";
  el.idlCount.textContent = "pending";
  el.idlBar.style.width = "0%";
  el.idlDisc.hidden = true;
  el.wroteCount.textContent = "pending";
  el.wroteBar.style.width = "0%";
  delete el.wroteBar.dataset.verdict;
  el.void.textContent = "";
}

// ---------------------------------------------------------------- endpoint

function readStoredEndpoint() {
  try {
    return localStorage.getItem(ENDPOINT_KEY) ?? "";
  } catch {
    // Private windows and blocked site data both throw here.
    return "";
  }
}

function storeEndpoint(value) {
  try {
    if (value) localStorage.setItem(ENDPOINT_KEY, value);
    else localStorage.removeItem(ENDPOINT_KEY);
  } catch {
    // Not fatal: the endpoint still works for this session.
  }
}

function refreshEndpointState() {
  const value = el.rpc.value.trim();
  if (!value) {
    el.endpointState.textContent = `${new URL(PUBLIC_RPC).host} (default)`;
    return;
  }
  let host = value;
  try {
    host = new URL(value).host;
  } catch {
    // Show the raw string; the fetch will surface a real parse error.
  }
  el.endpointState.textContent = host;
}

// ------------------------------------------------------------------- bytes

function decodeBase64(text) {
  const cleaned = text.replace(/\s+/g, "");
  if (cleaned === "") return new Uint8Array(0);
  // atob throws on invalid input, which is what we want: a payload we cannot
  // decode is a user error, not an empty account.
  const binary = atob(cleaned);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
  return out;
}

/**
 * getMultipleAccounts for one key.
 * Returns { contextSlot, data } where data is null when the account does not
 * exist on chain. Throws when the fetch itself fails.
 */
async function fetchAccount(endpoint, account) {
  const response = await fetch(endpoint, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "getMultipleAccounts",
      params: [[account], { encoding: "base64" }],
    }),
  });

  if (!response.ok) {
    throw new Error(`endpoint returned HTTP ${response.status}`);
  }

  const body = await response.json();
  if (body.error) {
    throw new Error(body.error.message ?? JSON.stringify(body.error));
  }

  const contextSlot = body?.result?.context?.slot;
  if (typeof contextSlot !== "number") {
    throw new Error("response missing result.context.slot");
  }

  const value = body?.result?.value?.[0] ?? null;
  return {
    contextSlot,
    data: value ? decodeBase64(value.data[0]) : null,
  };
}

/** The pasted payload, or null when the textarea is empty. Throws on bad base64. */
function customPayload() {
  const raw = el.indexed.value.trim();
  return raw === "" ? null : decodeBase64(raw);
}

// -------------------------------------------------------------- case change

function caseChanged(animate) {
  const id = selectedCase();
  el.livePicker.hidden = id !== "live";
  if (id === "live") {
    pendingField();
    write([
      line("line-muted", "Live case. Pick an account and what the indexer stored,"),
      line("line-muted", "then run the gate. Reads go from this tab to"),
      line("line-muted", `${el.endpointState.textContent}.`),
    ]);
    return;
  }

  let custom = null;
  try {
    custom = customPayload();
  } catch {
    custom = null;
  }

  const kase = CASES[id];
  const field = custom ?? kase.bytes();
  drawField(field, kase.spec, animate);

  const notes = custom
    ? [
        `Your pasted payload, ${bytes(custom.length)}. It replaces the selected case.`,
      ]
    : [
        `Loaded ${kase.name}, ${bytes(field.length)}, constructed in this page.`,
        "No chain account exists for it, so shape runs alone.",
      ];
  write([
    ...notes.map((n) => line("line-muted", n)),
    line("line-muted", ""),
    line("line-muted", "Run the gate to check it."),
  ]);
}

// --------------------------------------------------------------------- gate

async function runLive(endpoint) {
  const account = el.preset.value || el.account.value.trim();
  if (!account) {
    el.advanced.open = true;
    renderError(
      "the live case needs an account pubkey",
      "add one under your own payload and endpoint",
    );
    return;
  }

  // u64 crosses the wasm boundary as BigInt, never Number.
  const slotRaw = el.indexedSlot.value.trim();
  if (slotRaw !== "" && !/^\d+$/.test(slotRaw)) {
    renderError("the indexed write slot is not a whole number");
    return;
  }
  const indexedSlot = slotRaw === "" ? undefined : BigInt(slotRaw);

  let custom;
  try {
    custom = customPayload();
  } catch {
    renderError(
      "the indexed payload is not valid base64",
      "paste the account bytes as your index stored them, base64 encoded",
    );
    return;
  }

  write([line("line-muted", `reading ${account} ...`)]);
  const notes = [];
  let indexed = custom;
  let fallback = null;

  // Reconcile needs two independent sides: what the index stored and what the
  // chain holds. Comparing one read against itself would pass every time, the
  // green-while-blind verdict this gate exists to refuse. With no pasted
  // payload the page plays the indexer: a first read is the stored copy, the
  // chosen damage is applied to it, and a second read is the chain.
  if (!custom) {
    const stored = await fetchAccount(endpoint, account);
    if (stored.data === null) {
      renderError(
        `no account at ${account} as of context slot ${stored.contextSlot}`,
        "the gate needs bytes to shape; paste the row your indexer wrote",
      );
      return;
    }
    const damage = DAMAGE[el.damage.value] ?? DAMAGE.none;
    indexed = damage.apply(stored.data);
    fallback = findSpec(stored.data);
    notes.push(
      `Stored copy: read at slot ${stored.contextSlot}, ${bytes(stored.data.length)}, damage: ${damage.name}.`,
    );
  }

  const fetched = await fetchAccount(endpoint, account);
  drawField(indexed, fallback, true);
  if (!custom) {
    notes.push(`Chain: read again at slot ${fetched.contextSlot}.`);
  }
  if (indexedSlot === undefined) {
    notes.push("No write slot, so any byte difference fails, never skips as lag.");
  }

  const json = check_account_json(
    account,
    indexed,
    indexedSlot,
    fetched.data ?? undefined,
    BigInt(fetched.contextSlot),
    undefined,
  );
  renderResults(
    JSON.parse(json),
    `karat account ${account}, shape then reconcile`,
    notes,
  );
}

function runConstructed(id) {
  let custom;
  try {
    custom = customPayload();
  } catch {
    renderError(
      "the indexed payload is not valid base64",
      "paste the account bytes as your index stored them, base64 encoded",
    );
    return;
  }

  const kase = CASES[id];
  const indexed = custom ?? kase.bytes();
  drawField(indexed, kase.spec, false);

  const notes = [];
  if (custom) notes.push("Your pasted payload, not the selected case.");
  else notes.push(`Constructed in this page, not fetched from chain.`);
  notes.push("Shape alone. Select the live case to reconcile against chain.");

  // shape_json, not check_account_json: there is no chain account for a
  // constructed payload, so reconcile is not merely skipped, it cannot run.
  renderResults(
    JSON.parse(shape_json(indexed)),
    `karat shape, ${bytes(indexed.length)}`,
    notes,
  );
}

async function runGate(event) {
  event.preventDefault();
  if (!wasmReady) {
    renderError(
      "the WebAssembly module is still loading",
      "try again in a moment",
    );
    return;
  }

  const typed = el.rpc.value.trim();
  storeEndpoint(typed);
  refreshEndpointState();
  const endpoint = typed || PUBLIC_RPC;

  const id = selectedCase();
  el.run.disabled = true;
  try {
    if (id === "live") await runLive(endpoint);
    else runConstructed(id);
  } catch (error) {
    const message = error?.message ?? String(error);
    const looksLikeCors =
      message === "Failed to fetch" || /NetworkError/i.test(message);
    renderError(
      `the gate could not reach the endpoint: ${message}`,
      looksLikeCors
        ? "most providers block browser requests by default; check that yours allows CORS from this origin"
        : undefined,
    );
  } finally {
    el.run.disabled = false;
  }
}

// --------------------------------------------------------------------- copy

for (const button of document.querySelectorAll(".copy-btn")) {
  button.addEventListener("click", async () => {
    const source = document.getElementById(button.dataset.copy);
    if (!source) return;
    try {
      await navigator.clipboard.writeText(source.textContent.trim());
      const original = button.textContent;
      button.textContent = "Copied";
      setTimeout(() => {
        button.textContent = original;
      }, 1400);
    } catch {
      button.textContent = "Press Cmd+C";
    }
  });
}

// --------------------------------------------------------------------- boot

el.rpc.value = readStoredEndpoint();
refreshEndpointState();

el.form.addEventListener("submit", runGate);
el.rpc.addEventListener("input", refreshEndpointState);
el.indexed.addEventListener("change", () => caseChanged(true));
el.clearEndpoint.addEventListener("click", () => {
  el.rpc.value = "";
  storeEndpoint("");
  refreshEndpointState();
});
for (const radio of document.querySelectorAll('input[name="case"]')) {
  radio.addEventListener("change", () => caseChanged(true));
}

init()
  .then(() => {
    wasmReady = true;
    SPECS = JSON.parse(specs_json());
    caseChanged(true);
  })
  .catch((error) => {
    renderError(
      "could not load the WebAssembly module",
      error?.message ?? "reload the page",
    );
  });
