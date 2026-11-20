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
// An agent: a dot that blinks while it works.
const chip = (a, text) => {
  const cls = STATUS[a.status] || a.status;
  return `<span class="chip ${esc(cls)}"><span class="sdot${cls === "working" ? " blink" : ""}"></span>${esc(text)}</span>`;
};
const chips = (agents) => agents.length
  ? `<div class="chips">${agents.map((a) => chip(a, `${a.label}: ${a.status}`)).join("")}</div>`
  : "";
const href = (...parts) => "#/" + parts.map(encodeURIComponent).join("/");

// Each stage's mark, as gerdr's rail reads it; from the stages alone when
// gerdr couldn't be asked.
const marks = (f) => f.card?.marks || f.stages.map((s) => s.done ? "done" : s.now ? "current" : "later");

// The stage dots of a sidebar card: the big one blinks while it runs.
function dots(f) {
  const big = f.card ? f.card.big : f.stages.findIndex((s) => s.now);
  return `<span class="dots">${marks(f).map((m, i) => {
    const run = i === big && (f.card ? f.card.running : toneOf(f) === "working") ? " blink" : "";
    return `<span class="dot ${esc(m)}${i === big ? " big" : ""}${run}" title="${esc(f.stages[i]?.label)}"></span>`;
  }).join("")}</span>`;
}

const toneOf = (f) => f.card?.tone ||
  (f.waits ? "waits" : f.agents.some((a) => a.status === "working") ? "working" : "idle");
const waitsBox = (w, now) => w
  ? `<div class="waits"><b>${esc(w.title)}</b> · ${esc(w.next)} · ${ago(w.since, now)}</div>` : "";

function card(f, now) {
  return `<a class="card tone-${esc(toneOf(f))}" href="${href("f", f.project, f.slug)}">
    <div class="top"><span class="name">${esc(f.title)}</span><span class="ago">${ago(f.updated, now)}</span></div>
    <div class="meta">${esc(f.projectName)}</div>
    <div class="status">${dots(f)}<span class="line">${esc(f.card?.line || f.stage)}</span></div>
    ${waitsBox(f.waits, now)}
    ${chips(f.agents)}
  </a>`;
}

// Notifications: the service worker shows what gerdr pushes. iOS has
// push only once the page is on the home screen.
const sw = "serviceWorker" in navigator ? navigator.serviceWorker.register("/sw.js").catch(() => null) : Promise.resolve(null);
navigator.serviceWorker?.addEventListener("message", (e) => { if (e.data?.go) location.hash = e.data.go; });

async function pushSub() {
  const reg = await sw;
  return reg?.pushManager ? reg.pushManager.getSubscription() : undefined;
}

async function pushCard() {
  let sub = await pushSub().catch(() => undefined);
  // gerdr may have turned this phone's notifications off: then it's off here too.
  if (sub && !(await get("/api/push?e=" + encodeURIComponent(sub.endpoint)).catch(() => ({}))).on) sub = null;
  let html = `<h2>Notifications</h2><div class="card">`;
  if (sub === undefined) {
    html += `<div class="quiet small">This browser can't get notifications here. On an iPhone, add gerdr to the home screen and open it from there.</div>`;
  } else if (Notification.permission === "denied") {
    html += `<div class="quiet small">Notifications are blocked for this page. Allow them in the phone's settings.</div>`;
  } else if (sub) {
    html += `<div class="small">On. While you're away from the desk, agents that need you, stage files ready for review and CI results come here.</div>
      <div class="buttons"><button data-push="test">Send a test</button><button data-push="off">Turn off</button></div>`;
  } else {
    html += `<div class="small quiet">Get a notification when an agent needs you or a stage file is ready, while you're away from the desk.</div>
      <div class="buttons"><button class="primary" data-push="on">Turn on</button></div>`;
  }
  return html + `</div>`;
}

const keyBytes = (b64) => Uint8Array.from(atob(b64.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));

async function push(what) {
  if (what === "test") {
    const a = await post("/api/push-test", {});
    return say(a.error ? [{ text: a.error }] : a.said, !!a.error);
  }
  const reg = await sw;
  let sub = await reg.pushManager.getSubscription();
  if (what === "on") {
    if (await Notification.requestPermission() !== "granted") return say([{ text: "Notifications weren't allowed" }], true);
    const { key } = await get("/api/push");
    sub = sub || await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: keyBytes(key) });
  }
  if (!sub) return;
  const a = await post("/api/push", { subscription: sub.toJSON(), on: what === "on" });
  if (what === "off") await sub.unsubscribe();
  if (a.error) say([{ text: a.error }], true);
  else say([{ text: what === "on" ? "Notifications on" : "Notifications off", level: "success" }]);
  render(false, true);
}

// A note's tap on an agent: its feature's page of it.
async function go(pane) {
  const d = await get("/api/features");
  const f = d.features.find((f) => f.agents.some((a) => a.pane === pane));
  location.replace(f ? href("pane", f.project, f.slug, pane) : "#/");
  return { title: "gerdr", html: "" };
}

async function list() {
  const d = await get("/api/features");
  const waiting = d.features.filter((f) => f.waits);
  const rest = d.features.filter((f) => !f.waits);
  let html = d.problems.map((p) => `<p class="error">${esc(p)}</p>`).join("");
  if (!d.features.length) html += `<p class="quiet">No open features.</p>`;
  const group = (name, fs) => fs.length
    ? `<h2>${name} <span class="count">${fs.length}</span></h2>` + fs.map((f) => card(f, d.now)).join("") : "";
  html = `<a class="new-link" href="#/new">New feature</a>` + html;
  html += group("Waiting for you", waiting) + group("In progress", rest);
  html += await pushCard();
  return { title: "gerdr", html };
}

// The feature tab's rail: numbered nodes joined as the desktop joins them,
// the arch back to Build, and each stage's short line and agent.
const NODE_W = 82;
function rail(f, p, s) {
  const r = f.offer?.rail || {
    nodes: f.stages.map((s, i) => ({ label: s.label, line: "", mark: marks(f)[i] })),
    back: null,
  };
  const n = r.nodes;
  const mark = (i) => n[i]?.mark;
  const ran = (i) => ["ran", "paused"].includes(mark(i));
  const back = r.back;
  const joins = n.slice(1).map((_, i) => {
    // A stage with work sent back to it was passed all the same.
    const done = ["done", "stale"].includes(mark(i)) || !["later", undefined].includes(mark(i + 1));
    const once = (back && i >= back.to) || ran(i) || (ran(i + 1) && !(back && i + 1 === back.to));
    const cls = !done ? "" : once ? "once" : "done";
    const left = 12 + i * NODE_W + NODE_W / 2 + 18;
    return `<span class="join ${cls}" style="left:${left}px;width:${NODE_W - 36}px"></span>`;
  }).join("");
  const pulse = n.findIndex((x) => x.mark === "fixing") >= 0
    ? n.findIndex((x) => x.mark === "fixing") : n.findIndex((x) => x.mark === "current");
  const arch = back
    ? `<span class="back${[back.to, back.from].some((i) => mark(i) === "waiting") && mark(back.to) !== "fixing" ? " waits" : ""}"
        style="left:${12 + back.to * NODE_W + NODE_W / 2}px;width:${(back.from - back.to) * NODE_W}px"></span>` : "";
  const nodes = n.map((x, i) => {
    const disc = ["done", "stale", "ran"].includes(x.mark) ? "✓" : i + 1;
    const sel = i === f.page.stage ? " selected" : "";
    return `<a class="node ${esc(x.mark)}${i === pulse ? " pulse" : ""}${sel}" title="${esc(x.word || x.mark)}"
        href="${href("f", p, s, String(i))}">
      <div class="disc">${disc}</div>
      <div class="label">${esc(x.label)}</div>
      ${x.line ? `<div class="nline">${esc(x.line)}</div>` : ""}
      ${x.agent ? `<div class="agent">${esc(x.agent)}</div>` : ""}
    </a>`;
  }).join("");
  return `<div class="rail-wrap"><div class="rail">${joins}${arch}${nodes}</div></div>`;
}

// A feature's history, folded: its last event in sight, the rest behind it.
function history(list, now) {
  if (!list.length) return `<h2>History</h2><p class="quiet small">Nothing yet.</p>`;
  const item = (e) => `<li><time>${ago(e.at, now)}</time>${esc(e.what)}</li>`;
  return `<details class="fold history-fold"><summary><h2>History <span class="count">${list.length}</span></h2>
      <div class="last small"><span class="quiet">${ago(list[0].at, now)}</span> · ${esc(list[0].what)}</div></summary>
    <ul class="history">${list.map(item).join("")}</ul></details>`;
}

const stripAnsi = (t) => String(t ?? "").replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
const toned = (l) => l ? `<span class="t-${esc(l.tone)}">${esc(l.text)}</span>` : "";
const dotOf = (d) => d ? `<span class="sdot d-${esc(d)}${d === "working" ? " blink" : ""}"></span>` : "";
// A list of things to do, ticked as they are done; those at work blink.
const ticks = (items, running = []) => `<ul class="ticks">${items.map(([done, text], i) =>
  `<li class="${done ? "done" : ""}${running.includes(i) ? " at" : ""}">
    <span class="tick${running.includes(i) ? " blink" : ""}">${done ? "✓" : ""}</span><span>${esc(text)}</span></li>`).join("")}</ul>`;
const meter = (done, total, cls = "") => total
  ? `<div class="bar ${cls}"><span style="width:${Math.round(100 * done / total)}%"></span></div>` : "";

// A section of a stage's page, as the desktop has it, in words.
function section(x, f, p, s) {
  switch (x.kind) {
    case "agent": {
      const a = f.agents.find((a) => a.label.startsWith(`${f.page.label} · `));
      const head = `<div class="top">${dotOf(x.dot)}<span class="name">${esc(x.name || "Agent")}</span>
        ${a ? `<span class="chev">›</span>` : ""}</div>
        <div class="meta">${esc(x.status)}</div>
        ${x.tail?.length ? `<pre class="tail">${esc(stripAnsi(x.tail.slice(-4).join("\n")))}</pre>` : ""}`;
      const cls = `card agent${x.waits ? " tone-waits" : x.dot === "working" ? " tone-working" : ""}`;
      return a ? `<a class="${cls}" href="${href("pane", p, s, a.pane)}">${head}</a>` : `<div class="${cls}">${head}</div>`;
    }
    case "file":
      return `<div class="fileline small"><b>${esc(x.name)}</b> · ${toned(x.state)}</div>`;
    case "doc":
      return `<details class="fold doc-fold" open><summary><h2>${esc(x.name)}</h2></summary>
        ${x.html ? `<article class="doc card">${x.html}</article>` : `<p class="error small">${esc(x.error)}</p>`}</details>`;
    case "lines":
      return `<h3>${esc(x.label)}</h3><ul class="lines">${x.lines.map((l) => `<li>${toned(l)}</li>`).join("")}</ul>`;
    case "heading":
      return `<h3>${esc(x.text)}</h3>`;
    case "milestones":
    {
      // The count, bar and state show; the milestones fold, as they're long.
      const head = `<div class="top"><span class="name">${esc(x.label)}</span>
          <span class="count">${x.done} of ${x.total}</span></div>
        ${meter(x.done, x.total, x.done === x.total ? "full" : "")}
        ${x.state ? `<div class="meta">${toned(x.state)}</div>` : ""}`;
      return x.items.length
        ? `<details class="card milestones"><summary>${head}</summary>${ticks(x.items, x.running)}</details>`
        : `<div class="card milestones">${head}</div>`;
    }
    case "round": {
      const asked = x.asked.length ? `<h3>${esc(x.askedCaption)}</h3><ul class="lines">${x.asked.map((a) =>
        `<li><b>${esc(a.id)}</b> ${esc(a.title)}${a.now ? ` · ${toned(a.now)}` : ""}
          ${a.note ? `<div class="quiet small">${esc(a.note)}</div>` : ""}</li>`).join("")}</ul>` : "";
      const made = x.fixes.filter(([d]) => d).length;
      const fixes = x.fixes.length ? `<h3>Fixes <span class="count">${made} of ${x.fixes.length}</span></h3>
        ${meter(made, x.fixes.length)}${ticks(x.fixes, x.running)}` : "";
      const changed = x.changed.length ? `<h3>Changed</h3><ul class="lines quiet">${x.changed.map((c) => `<li>${esc(c)}</li>`).join("")}</ul>` : "";
      return `<details class="card round"><summary>
          <div class="top">${dotOf(x.dot)}<span class="name">${esc(x.label)}</span>${toned(x.state)}</div>
          ${x.meta ? `<div class="meta">${esc(x.meta)}</div>` : ""}</summary>${asked}${fixes}${changed}</details>`;
    }
    case "finding": {
      const lead = x.dot ? dotOf(x.dot) : x.tag ? `<span class="sev ${esc(x.tag)}">${esc(x.tag)}</span>` : "";
      const more = [
        x.body && `<p>${esc(x.body)}</p>`,
        x.fix && `<p>${esc(x.fix)}</p>`,
        x.note && `<p class="quiet">${esc(x.note)}</p>`,
        x.comment && `<p><b>Your comment:</b> ${esc(x.comment)}</p>`,
        x.preview?.length && `<pre class="tail">${esc(stripAnsi(x.preview.join("\n")))}</pre>`,
      ].filter(Boolean).join("");
      const meta = [x.place, x.state, x.clock].filter(Boolean).map(esc).join(" · ");
      const body = `<div class="top">${lead}<span class="name">${esc(x.title)}</span>${x.made ? `<span class="t-ready small">made</span>` : ""}</div>
        ${meta ? `<div class="meta">${meta}</div>` : ""}`;
      return more
        ? `<details class="card finding${x.faded ? " faded" : ""}"><summary>${body}</summary>${more}</details>`
        : `<div class="card finding${x.faded ? " faded" : ""}">${body}</div>`;
    }
    case "review":
      return `<div class="card"><div class="top"><span class="name">${esc(x.label)}</span>
          <span class="count">${x.decided} of ${x.total} decided</span></div>
        ${meter(x.decided, x.total)}<div class="meta">${esc(x.words)}</div>
        ${x.notes.map((l) => `<div class="small">${toned(l)}</div>`).join("")}</div>`;
    case "ship": {
      const c = x.card;
      const head = c
        ? `<div class="top">${dotOf(c.dot)}<span class="name">${esc(x.head)}</span>${toned(c.state)}</div>
           ${c.meta ? `<div class="meta">${esc(c.meta)}</div>` : ""}
           ${[c.base, c.ci].filter(Boolean).map((l) => `<div class="small">${toned(l)}</div>`).join("")}`
        : `<h3>${esc(x.head)}</h3>`;
      const steps = x.steps.length ? `<ul class="ticks">${x.steps.map((st) =>
        `<li class="${st.done ? "done" : ""}${st.off ? " off" : ""}"><span class="tick${st.done ? "" : " box"}">${st.done || st.ticked ? "✓" : ""}</span>
          <span>${esc(st.label)}${st.detail ? `<div class="small">${toned(st.detail)}</div>` : ""}</span></li>`).join("")}</ul>` : "";
      const note = x.note ? `<div class="meta">${esc(x.note)}</div>` : "";
      return c ? `<div class="card">${head}${note}${steps}</div>` : head + note + steps;
    }
    case "recap": {
      const figs = x.figures.length ? `<div class="figures">${x.figures.map(([v, c]) =>
        `<div><b>${esc(v)}</b><span>${esc(c)}</span></div>`).join("")}</div>` : "";
      return `<div class="card recap">${figs}
        ${x.summary.map((t) => `<p>${esc(t)}</p>`).join("")}
        ${x.summaryNote ? `<div class="meta">${esc(x.summaryNote)}</div>` : ""}
        ${x.stages.length ? `<ul class="lines">${x.stages.map(([n, t]) => `<li><b>${esc(n)}</b> · ${esc(t)}</li>`).join("")}</ul>` : ""}
        ${x.lines.map((l) => `<div class="small">${toned(l)}</div>`).join("")}</div>`;
    }
    default: return "";
  }
}

// A stage's page: what it is for, how it stands, its sections as the
// desktop's page has them, and its files: the one to read shown, the
// others to open. AI review's findings can be decided here.
function stagePage(f, p, s) {
  const pg = f.page;
  let html = `<div class="page-head"><h2>${esc(pg.label)}${pg.own ? "" : ` <span class="count">· it is at ${esc(f.stage)}</span>`}</h2>
    ${pg.about ? `<div class="quiet small">${esc(pg.about)}</div>` : ""}
    ${pg.status ? `<div class="pstatus">${toned(pg.status)}</div>` : ""}</div>`;
  // The stage's agents first, as they're what's at work.
  const agents = pg.sections.filter((x) => x.kind === "agent");
  for (const x of agents) html += section(x, f, p, s);
  if (pg.own) html += actions(f.offer);
  let decided = false;
  for (const x of pg.sections.filter((x) => x.kind !== "agent")) {
    if (pg.decides && (x.kind === "finding" || x.kind === "heading")) {
      if (!decided) html += findings(f.findings);
      decided = true;
      continue;
    }
    html += section(x, f, p, s);
  }
  if (pg.decides && !decided) html += findings(f.findings);
  if (pg.files?.length) {
    html += `<h2>Files</h2><div class="group">` + pg.files.map((d) =>
      `<a class="row" href="${href("doc", p, s, d)}"><span>${esc(d)}</span><span class="chev">›</span></a>`).join("") + `</div>`;
  }
  return html;
}

async function feature(p, s, stage) {
  const f = await get(`/api/feature?${q(stage === undefined ? { p, s } : { p, s, stage })}`);
  const tone = toneOf(f);
  let html = `<div class="meta quiet small">${esc(f.projectName)}</div>`;
  html += `<div class="status"><span class="line tone-${esc(tone)}">${esc(f.card?.line || f.stage)}</span></div>`;
  html += rail(f, p, s);
  html += waitsBox(f.waits, f.now);
  html += stagePage(f, p, s);
  if (f.agents.length) {
    html += `<h2>Agents <span class="count">${f.agents.length}</span></h2><div class="group">` +
      f.agents.map((a) => `<a class="row" href="${href("pane", p, s, a.pane)}">
        <span>${esc(a.label)}</span>${chip(a, a.status)}</a>`).join("") + `</div>`;
  } else {
    html += `<h2>Agents</h2><p class="quiet small">${esc(f.agentsError || "No agent running for it here.")}</p>`;
  }
  html += history(f.history, f.now);
  return { title: f.title, back: stage === undefined ? "#/" : href("f", p, s), html };
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

// A new feature: its project, title, the repositories it changes when the
// project has several, the agent its Discover runs on and what it is for.
// gerdr makes it and starts Discover; its page opens once it has.
let lastProject = "";
try { lastProject = localStorage.getItem("newProject") || ""; } catch {}
async function newForm() {
  const d = await get("/api/new");
  const ok = d.projects.filter((x) => !x.error);
  const pick = (ok.find((x) => x.id === lastProject) || ok[0])?.id;
  let html = d.error ? `<p class="error small">Starting a feature needs gerdr running: ${esc(d.error)}</p>` : "";
  if (!ok.length) {
    html += `<p class="quiet">No project can take a feature: each needs a workspace with a folder.</p>`;
    return { title: "New feature", back: "#/", html };
  }
  const opts = d.projects.map((x) => `<option value="${esc(x.id)}"${x.id === pick ? " selected" : ""}${x.error ? " disabled" : ""}>
    ${esc(x.name)}${x.error ? ` (${esc(x.error)})` : ""}</option>`).join("");
  const parts = ok.map((x) => {
    const repos = x.multi ? `<div class="checks"><div class="caption">Repositories it changes</div>
      ${x.repos.map((r) => `<label><input type="checkbox" name="repo" value="${esc(r)}">${esc(r)}</label>`).join("")}</div>` : "";
    const agents = x.agents?.agents || [];
    const agent = agents.length
      ? `<label>Discover runs on<select name="agent">${agents.map((a) =>
          `<option${a === x.agents.default ? " selected" : ""}>${esc(a)}</option>`).join("")}</select></label>`
      : `<p class="quiet small">${d.error ? "" : "No agent gerdr can start Discover on is installed."}</p>`;
    return `<div class="for" data-for="${esc(x.id)}"${x.id === pick ? "" : " hidden"}>${repos}${agent}</div>`;
  }).join("");
  html += `<form id="newf" class="form card">
    <label>Project<select name="project">${opts}</select></label>
    <label>Title<input name="title" autocomplete="off" enterkeyhint="next" placeholder="Bulk edit"></label>
    ${parts}
    <label>What it is for <span class="quiet">(optional)</span>
      <textarea name="idea" rows="5" placeholder="Discover's agent starts from this, not from its own question"></textarea></label>
    <div class="buttons"><button class="primary" ${d.error ? "disabled" : ""}>Start feature</button></div>
  </form>`;
  return { title: "New feature", back: "#/", html };
}

async function startFeature(form) {
  const f = form.elements;
  const p = f.project.value;
  const part = form.querySelector(`.for[data-for="${CSS.escape(p)}"]`);
  const title = f.title.value.trim();
  if (!title) return say([{ text: "Give it a title" }], true);
  const boxes = [...(part?.querySelectorAll("input[name=repo]") || [])];
  const repos = boxes.filter((b) => b.checked).map((b) => b.value);
  if (boxes.length && !repos.length) return say([{ text: "Pick a repository it changes" }], true);
  const agent = part?.querySelector("select[name=agent]")?.value || null;
  const button = form.querySelector("button");
  button.disabled = true;
  button.textContent = "Creating…";
  const a = await post("/api/new", { p, title, repos, idea: f.idea.value, agent });
  button.disabled = false;
  button.textContent = "Start feature";
  if (a.error) return say([{ text: a.error }], true);
  try { localStorage.setItem("newProject", p); } catch {}
  lastProject = p;
  say(a.said || []);
  form.reset();
  location.hash = href("f", p, a.slug);
}

document.addEventListener("change", (e) => {
  if (e.target.name !== "project" || e.target.form?.id !== "newf") return;
  e.target.form.querySelectorAll(".for").forEach((x) => { x.hidden = x.dataset.for !== e.target.value; });
});

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
    case "f": return { load: () => feature(parts[1], parts[2], parts[3]), every: 5000 };
    case "doc": return { load: () => doc(parts[1], parts[2], parts[3]), every: 0 };
    case "pane": return { load: () => pane(parts[1], parts[2], parts[3]), every: 2000 };
    case "go": return { load: () => go(parts[1]), every: 0 };
    case "new": return { load: newForm, every: 0 };
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
      const railX = main.querySelector(".rail-wrap")?.scrollLeft;
      main.innerHTML = v.html;
      if (!fresh) main.querySelectorAll("details").forEach((d, i) => { d.open = open[i] || false; });
      shown = v.html;
      // What blinks keeps its beat across a refresh.
      main.querySelectorAll(".blink").forEach((e) => { e.style.animationDelay = `-${Date.now() % 1600}ms`; });
      main.querySelectorAll(".pulse .disc").forEach((e) => { e.style.animationDelay = `-${Date.now() % 1800}ms`; });
      const wrap = main.querySelector(".rail-wrap");
      if (wrap && (fresh || railX === undefined)) {
        const at = wrap.querySelector(".node.selected") ||
          wrap.querySelector(".node.pulse, .node.waiting, .node.failed, .node.current");
        if (at) wrap.scrollLeft = at.offsetLeft - (wrap.clientWidth - at.offsetWidth) / 2;
      } else if (wrap) wrap.scrollLeft = railX;
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
  const b = e.target.closest("button[data-press], button[data-decide], button[data-sendback], button[data-key], button[data-push]");
  if (!b || b.disabled) return;
  const [, p, s, pane] = here();
  b.disabled = true;
  try {
    if (b.dataset.push) {
      await push(b.dataset.push).catch((err) => say([{ text: err.message }], true));
    } else if (b.dataset.press) {
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
  if (e.target.id === "newf") {
    e.preventDefault();
    return startFeature(e.target);
  }
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
