// The phone view: features, their stages and agents, read from gerdr's
// `--phone` server, which it polls while shown. Its presses go back there:
// a stage page's buttons, findings' decisions and send-backs to gerdr,
// lines and keys to an agent's pane.

const main = document.getElementById("main");
const titleEl = document.getElementById("title");
const back = document.getElementById("back");
const seen = document.getElementById("seen");
const offline = document.getElementById("offline");
const note = document.getElementById("note");
const bar = document.getElementById("bar");

// Colours an agent pane's output; plain text if the library can't load.
let ansi = null;
import("https://cdn.jsdelivr.net/npm/ansi_up@6.0.2/ansi_up.js")
  .then((m) => { ansi = new m.AnsiUp(); ansi.use_classes = false; })
  .catch(() => {});

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) =>
  ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const q = (o) => new URLSearchParams(o).toString();

function ago(at, now) {
  const s = Math.max(0, now - at);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

let lastOk = 0;
function showSeen() {
  seen.textContent = lastOk ? `updated ${ago(lastOk / 1000, Date.now() / 1000)}` : "";
}
setInterval(showSeen, 15000);

async function get(url) {
  let res;
  try {
    res = await fetch(url, { cache: "no-store" });
  } catch {
    offline.textContent = "Can't reach gerdr. Is Tailscale on, and the phone view running?";
    offline.hidden = false;
    throw new Error("offline");
  }
  const body = await res.json().catch(() => ({ error: `Bad answer (${res.status})` }));
  if (!res.ok) throw new Error(body.error || `Error ${res.status}`);
  offline.hidden = true;
  lastOk = Date.now();
  showSeen();
  return body;
}

// Say what gerdr said, for a few seconds.
let noteTimer = null;
function say(lines, bad) {
  if (!lines.length) return;
  note.innerHTML = lines.map((l) => `<div class="${esc(l.level || (bad ? "error" : ""))}">${esc(l.text)}</div>`).join("");
  note.hidden = false;
  clearTimeout(noteTimer);
  noteTimer = setTimeout(() => { note.hidden = true; }, 6000);
}

async function post(path, body) {
  let res;
  try {
    res = await fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Gerdr": "phone" },
      body: JSON.stringify(body),
    });
  } catch {
    return { error: "Can't reach gerdr" };
  }
  return res.json().catch(() => ({ error: `Bad answer (${res.status})` }));
}

// Ask gerdr to do it; a confirm it asks is asked here, and on yes sent
// again to be answered there. True when it went.
async function ask(path, body) {
  let answer = await post(path, body);
  if (answer.asks) {
    const a = answer.asks;
    if (!confirm(`${a.title}${a.body ? "\n\n" + a.body : ""}`)) return false;
    answer = await post(path, { ...body, confirm: true });
  }
  if (answer.error) { say([{ text: answer.error }], true); return false; }
  say(answer.said || []);
  if (!(answer.said || []).length) say([{ text: "Done", level: "success" }]);
  render(false, true);
  return true;
}

const STATUS = { "working": "working", "waiting for you": "waiting" };
const chips = (agents) => agents.length
  ? `<div class="chips">${agents.map((a) =>
      `<span class="chip ${STATUS[a.status] || ""}">${esc(a.label)}: ${esc(a.status)}</span>`).join("")}</div>`
  : "";
const rail = (stages) =>
  `<div class="rail">${stages.map((s) =>
    `<span class="${s.done ? "done" : s.now ? "now" : ""}" title="${esc(s.label)}"></span>`).join("")}</div>`;
const href = (...parts) => "#/" + parts.map(encodeURIComponent).join("/");

function card(f, now) {
  return `<a class="card" href="${href("f", f.project, f.slug)}">
    <div class="name">${esc(f.title)}</div>
    <div class="meta">${esc(f.projectName)} · ${esc(f.stage)}</div>
    ${rail(f.stages)}
    ${f.waits ? `<div class="waits">${esc(f.waits.title)} · ${esc(f.waits.next)} · ${ago(f.waits.since, now)}</div>` : ""}
    ${chips(f.agents)}
    ${f.last ? `<div class="meta">${esc(f.last)} · ${ago(f.updated, now)}</div>` : ""}
  </a>`;
}

async function list() {
  const d = await get("/api/features");
  const waiting = d.features.filter((f) => f.waits);
  const rest = d.features.filter((f) => !f.waits);
  let html = d.problems.map((p) => `<p class="error">${esc(p)}</p>`).join("");
  if (!d.features.length) html += `<p class="quiet">No open features.</p>`;
  if (waiting.length) html += `<h2>Waiting for you</h2>` + waiting.map((f) => card(f, d.now)).join("");
  if (rest.length) html += `<h2>In progress</h2>` + rest.map((f) => card(f, d.now)).join("");
  return { title: "gerdr", html };
}

async function feature(p, s) {
  const f = await get(`/api/feature?${q({ p, s })}`);
  let html = `<div class="meta quiet">${esc(f.projectName)} · ${esc(f.stage)}</div>`;
  if (f.waits) html += `<div class="waits">${esc(f.waits.title)} · ${esc(f.waits.next)} · ${ago(f.waits.since, f.now)}</div>`;
  html += actions(f.offer);
  html += findings(f.findings);
  html += `<h2>Agents</h2>`;
  html += f.agents.length
    ? f.agents.map((a) => `<a class="row" href="${href("pane", p, s, a.pane)}">
        <span>${esc(a.label)}</span><span class="chip ${STATUS[a.status] || ""}">${esc(a.status)}</span></a>`).join("")
    : `<p class="quiet small">${esc(f.agentsError || "No agent running for it here.")}</p>`;
  html += `<h2>Stages</h2><ul class="stages">${f.stages.map((st) =>
    `<li class="${st.done ? "done" : st.now ? "now" : ""}">${esc(st.label)}</li>`).join("")}</ul>`;
  if (f.docs.length) {
    html += `<h2>Files</h2>` + f.docs.map((d) =>
      `<a class="row" href="${href("doc", p, s, d)}"><span>${esc(d)}</span><span class="quiet">›</span></a>`).join("");
  }
  html += `<h2>History</h2>`;
  html += f.history.length
    ? `<ul class="history">${f.history.map((e) =>
        `<li><time>${ago(e.at, f.now)}</time>${esc(e.what)}</li>`).join("")}</ul>`
    : `<p class="quiet small">Nothing yet.</p>`;
  return { title: f.title, back: "#/", html };
}

function actions(offer) {
  if (offer.error) return `<p class="quiet small">Actions need gerdr: ${esc(offer.error)}</p>`;
  if (offer.elsewhere) return `<p class="quiet small">${esc(offer.elsewhere)} writes it: act there.</p>`;
  let html = "";
  if (offer.buttons.length) {
    html += `<div class="buttons">${offer.buttons.map((b) =>
      `<button class="${b.primary ? "primary" : ""}" data-press="${esc(b.label)}">${esc(b.label)}</button>`).join("")}</div>`;
  }
  if (offer.sendBack) {
    html += `<details class="sendback"><summary>Send back with a comment</summary>
      <textarea id="backText" rows="4" placeholder="What should change"></textarea>
      <button data-sendback>Send back</button></details>`;
  }
  return html;
}

const SETTLED = { fix: "to fix", sent: "sent to fix", fixed: "fixed", posted: "on GitHub" };
function findings(list) {
  if (!list.length) return "";
  const open = list.filter((x) => x.state === "open").length;
  return `<h2>Findings${open ? ` · ${open} to decide` : ""}</h2>` + list.map((x) => {
    const moves = x.state === "open"
      ? `<button class="primary" data-decide="fix" data-id="${esc(x.id)}">Fix</button>
         <button data-decide="dismiss" data-id="${esc(x.id)}">Dismiss…</button>`
      : x.state === "fix" || x.state === "dismissed"
        ? `<button data-decide="reopen" data-id="${esc(x.id)}">Reopen</button>`
        : "";
    const state = x.state === "open" ? "" : (SETTLED[x.state] || x.state);
    return `<div class="card finding ${esc(x.state)}">
      <div class="meta"><span class="sev ${esc(x.severity)}">${esc(x.severity)}</span> ${esc(x.at)}${state ? ` · ${esc(state)}` : ""}</div>
      <div class="name">${esc(x.title)}</div>
      ${x.reason ? `<div class="meta">Dismissed: ${esc(x.reason)}</div>` : ""}
      <details><summary>Details</summary><p>${esc(x.body)}</p>${x.fix ? `<p><b>Fix:</b> ${esc(x.fix)}</p>` : ""}</details>
      ${moves ? `<div class="buttons">${moves}</div>` : ""}
    </div>`;
  }).join("");
}

async function doc(p, s, name) {
  const d = await get(`/api/doc?${q({ p, s, f: name })}`);
  return { title: name, back: href("f", p, s), html: `<article class="doc">${d.html}</article>`, keep: true };
}

async function pane(p, s, id) {
  const d = await get(`/api/pane?${q({ p, s, pane: id })}`);
  // TUIs pad lines to their width: on a phone those runs wrap into blank rows.
  const raw = d.text.replace(/\r\n/g, "\n").replace(/ {6,}/g, "   ")
    .replace(/([─━═▀▄])\1{29,}/g, (_, c) => c.repeat(30));
  const text = ansi ? ansi.ansi_to_html(raw) : esc(raw.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, ""));
  return { title: d.label, back: href("f", p, s), html: `<pre class="term">${text}</pre>`, pane: true,
    bar: `<form id="say"><input id="line" enterkeyhint="send" autocomplete="off" placeholder="Reply to the agent">
      <button>Send</button></form>
      <div class="keys">${KEYS.map(([k, l]) => `<button data-key="${k}">${l}</button>`).join("")}</div>` };
}

// Keys for an agent's menus and prompts: herdr's name, and the label.
const KEYS = [["esc", "Esc"], ["1", "1"], ["2", "2"], ["3", "3"], ["Up", "↑"], ["Down", "↓"], ["Enter", "⏎"]];

// The view the address names, and how often it is read again.
function route() {
  const parts = location.hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
  switch (parts[0]) {
    case "f": return { load: () => feature(parts[1], parts[2]), every: 5000 };
    case "doc": return { load: () => doc(parts[1], parts[2], parts[3]), every: 0 };
    case "pane": return { load: () => pane(parts[1], parts[2], parts[3]), every: 2000 };
    default: return { load: list, every: 5000 };
  }
}

let timer = null;
let shown = "";
let barFor = null;
// Something typed on the page: a refresh would lose it.
const typing = () => [...main.querySelectorAll("textarea, input")].some((e) => e.value || e === document.activeElement);
async function render(fresh, force) {
  clearTimeout(timer);
  const r = route();
  const hash = location.hash;
  // A pane read follows the end unless the user scrolled up from it.
  const atEnd = window.innerHeight + window.scrollY >= document.body.scrollHeight - 40;
  try {
    const v = await r.load();
    if (hash !== location.hash) return;
    titleEl.textContent = v.title;
    back.hidden = !v.back;
    if (v.back) back.href = v.back;
    if (barFor !== hash) {
      bar.innerHTML = v.bar || "";
      bar.hidden = !v.bar;
      document.body.classList.toggle("with-bar", !!v.bar);
      barFor = hash;
    }
    if (v.html !== shown && (fresh || force || !typing())) {
      const y = window.scrollY;
      const open = [...main.querySelectorAll("details")].map((d) => d.open);
      main.innerHTML = v.html;
      if (!fresh) main.querySelectorAll("details").forEach((d, i) => { d.open = open[i] || false; });
      shown = v.html;
      if (v.pane && (fresh || atEnd)) window.scrollTo(0, document.body.scrollHeight);
      else if (!fresh) window.scrollTo(0, y);
    }
    if (fresh && !v.pane) window.scrollTo(0, 0);
  } catch (e) {
    if (hash !== location.hash) return;
    if (fresh || e.message !== "offline") {
      // Home-screen apps have no browser Back: always a way out.
      back.hidden = route().load === list;
      back.href = "#/";
      main.innerHTML = `<p class="error">${esc(e.message === "offline" ? "Couldn't load." : e.message)}</p>`;
      shown = "";
    }
  }
  if (r.every && !document.hidden) timer = setTimeout(() => render(false), r.every);
}

// The route's feature and pane: `#/f/<p>/<s>` or `#/pane/<p>/<s>/<pane>`.
const here = () => location.hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);

document.addEventListener("click", async (e) => {
  const b = e.target.closest("button[data-press], button[data-decide], button[data-sendback], button[data-key]");
  if (!b || b.disabled) return;
  const [, p, s, pane] = here();
  b.disabled = true;
  try {
    if (b.dataset.press) {
      const label = b.dataset.press;
      if (confirm(`${label}: ${titleEl.textContent}?`)) await ask("/api/press", { p, s, label });
    } else if (b.dataset.decide) {
      const decision = b.dataset.decide;
      let reason = "";
      if (decision === "dismiss") {
        reason = prompt("Why dismiss it?") || "";
        if (!reason.trim()) return;
      }
      await ask("/api/decide", { p, s, id: b.dataset.id, decision, reason });
    } else if (b.dataset.sendback !== undefined) {
      const box = document.getElementById("backText");
      if (!box.value.trim()) { say([{ text: "Say what should change" }], true); return; }
      if (await ask("/api/send-back", { p, s, text: box.value })) box.value = "";
    } else if (b.dataset.key) {
      const a = await post("/api/agent", { p, s, pane, key: b.dataset.key });
      if (a.error) say([{ text: a.error }], true);
      setTimeout(() => render(false, true), 400);
    }
  } finally {
    b.disabled = false;
  }
});

document.addEventListener("submit", async (e) => {
  if (e.target.id !== "say") return;
  e.preventDefault();
  const line = document.getElementById("line");
  if (!line.value.trim()) return;
  const [, p, s, pane] = here();
  const a = await post("/api/agent", { p, s, pane, text: line.value });
  if (a.error) say([{ text: a.error }], true);
  else line.value = "";
  setTimeout(() => render(false, true), 600);
});

window.addEventListener("hashchange", () => render(true));
document.addEventListener("visibilitychange", () => { if (!document.hidden) render(false); });
render(true);
