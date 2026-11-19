// The phone view: features, their stages and agents, read from gerdr's
// `--phone` server. Read-only; it polls while shown.

const main = document.getElementById("main");
const titleEl = document.getElementById("title");
const back = document.getElementById("back");
const seen = document.getElementById("seen");
const offline = document.getElementById("offline");

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
  return { title: d.label, back: href("f", p, s), html: `<pre class="term">${text}</pre>`, pane: true };
}

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
async function render(fresh) {
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
    if (v.html !== shown) {
      const y = window.scrollY;
      main.innerHTML = v.html;
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

window.addEventListener("hashchange", () => render(true));
document.addEventListener("visibilitychange", () => { if (!document.hidden) render(false); });
render(true);
