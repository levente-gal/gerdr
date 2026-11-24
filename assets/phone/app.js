// The phone view: features, their stages and agents, read from gerdr's
// `--phone` server, which it polls while shown. Its presses go back there:
// a new feature, a stage page's buttons and send-backs to gerdr,
// lines and keys to an agent's pane.

const main = document.getElementById("main");
const titleEl = document.getElementById("title");
const back = document.getElementById("back");
const seen = document.getElementById("seen");
const work = document.getElementById("work");
const sweep = document.getElementById("sweep");
const offline = document.getElementById("offline");
const note = document.getElementById("note");
const bar = document.getElementById("bar");

// Colours an agent pane's output. Served from here: no outside script
// runs on a page that can type to agents.
import { AnsiUp } from "/ansi_up.js";

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) =>
  ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const q = (o) => new URLSearchParams(o).toString();

// As the desktop says it (clock.rs): "5m ago", "3h ago", "yesterday".
function ago(at, now) {
  const s = Math.max(0, now - at);
  const DAY = 86400;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < DAY) return `${Math.floor(s / 3600)}h ago`;
  if (s < 2 * DAY) return "yesterday";
  if (s < 7 * DAY) return `${Math.floor(s / DAY)}d ago`;
  if (s < 35 * DAY) return `${Math.floor(s / (7 * DAY))}w ago`;
  return `${Math.floor(s / (30 * DAY))}mo ago`;
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

// Say what gerdr said, for a few seconds: a warning longer, an error
// longest, as the desktop's toasts. A tap puts it away.
let noteTimer = null;
function say(lines, bad) {
  if (!lines.length) return;
  const level = (l) => l.level || (bad ? "error" : "");
  note.innerHTML = lines.map((l) => `<div class="${esc(level(l))}">${esc(l.text)}</div>`).join("");
  note.hidden = false;
  clearTimeout(noteTimer);
  const worst = lines.some((l) => level(l) === "error") ? 10000 : lines.some((l) => level(l) === "warning") ? 7000 : 6000;
  noteTimer = setTimeout(() => { note.hidden = true; }, worst);
}
note.addEventListener("click", () => { note.hidden = true; });

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
    if (!(await sheetAsk({ title: a.title, body: a.body, go: a.go || "Go" }))) return false;
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
// The stage it is at: gerdr's card says, else its file.
const atStage = (f) => f.card ? f.stages[f.card.big]?.label ?? f.stage : f.stage;

// What it waits for, from another stage's page: that stage, what to do and
// since when, a tap away. Its own page says it in full.
function waitsLink(f, p, s) {
  const w = f.waits;
  const at = f.card ? f.card.big : f.stages.findIndex((x) => x.now);
  if (!w || at < 0 || at === f.page.stage) return "";
  return `<a class="waits" href="${href("f", p, s, String(at))}"><span><b>${esc(f.stages[at].label)} waits</b> · ${esc(w.next)} · ${ago(w.since, f.now)}</span><span class="chev">›</span></a>`;
}

// A sidebar card: its line says what it waits for, its page says more;
// only agents at work or waiting show.
function card(f, now) {
  return `<a class="card tone-${esc(toneOf(f))}" href="${href("f", f.project, f.slug)}">
    <div class="top"><span class="name">${esc(f.title)}</span><span class="ago">${ago(f.updated, now)}</span></div>
    <div class="meta">${esc(f.projectName)}</div>
    <div class="status">${dots(f)}<span class="line">${esc(f.card?.line || f.stage)}</span></div>
    ${chips(f.agents.filter((a) => !["idle", "unknown"].includes(a.status)))}
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
  // Asked before any wait: iOS allows it only straight from the tap.
  if (what === "on" && await Notification.requestPermission() !== "granted") {
    return say([{ text: "Notifications weren't allowed" }], true);
  }
  const reg = await sw;
  let sub = await reg.pushManager.getSubscription();
  if (what === "on") {
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
  const w = d.agents.find((w) => w.agents.some((a) => a.pane === pane));
  location.replace(f ? href("pane", f.project, f.slug, pane) : w ? href("agent", w.project, pane) : "#/");
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
  html += group("Waiting for you", waiting) + group("In progress", rest);
  html += d.agents.map(workspaceAgents).join("");
  html += `<a class="new-link" href="#/new">New feature</a>`;
  html += await pushCard();
  return { title: "gerdr", html };
}

// A project's agents in workspaces of no feature: those waiting first,
// then those at work.
const RANK = { "waiting for you": 0, "working": 1 };
function workspaceAgents(w) {
  const agents = [...w.agents].sort((a, b) => (RANK[a.status] ?? 2) - (RANK[b.status] ?? 2));
  return `<h2>Agents in ${esc(w.projectName)} <span class="count">${agents.length}</span></h2><div class="group">` +
    agents.map((a) => `<a class="row" href="${href("agent", w.project, a.pane)}">
      <span>${esc(a.label)}</span>${chip(a, a.status)}</a>`).join("") + `</div>`;
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
    ? `<span class="back${[back.to, back.from].some((i) => mark(i) === "waiting") && mark(back.to) !== "fixing" ? " held" : ""}"
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

const HISTORY_SHOWN = 50;
// A feature's history, folded: its last event in sight, the rest behind it.
function history(list, now) {
  if (!list.length) return `<h2>History</h2><p class="quiet small">Nothing yet.</p>`;
  const item = (e) => `<li><time>${ago(e.at, now)}</time>${esc(e.what)}</li>`;
  // The latest: hundreds would be drawn again at every read.
  const earlier = list.length - HISTORY_SHOWN;
  return `<details class="fold history-fold"><summary><h2>History <span class="count">${list.length}</span></h2>
      <div class="last small"><span class="quiet">${ago(list[0].at, now)}</span> · ${esc(list[0].what)}</div></summary>
    <ul class="history">${list.slice(0, HISTORY_SHOWN).map(item).join("")}</ul>
    ${earlier > 0 ? `<p class="quiet small">${earlier} earlier not shown here</p>` : ""}</details>`;
}

const stripAnsi = (t) => String(t ?? "").replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
const toned = (l) => l ? `<span class="t-${esc(l.tone)}">${esc(l.text)}</span>` : "";
// A dot's state, in words, for those who don't see its colour.
const DOT_WORD = { blocked: "waiting", done: "done", failed: "failed", working: "working", idle: "idle" };
const dotOf = (d) => d
  ? `<span class="sdot d-${esc(d)}${d === "working" ? " blink" : ""}" role="img" aria-label="${esc(DOT_WORD[d] || d)}"></span>` : "";
// A list of things to do, ticked as they are done; those at work blink.
const ticks = (items, running = []) => `<ul class="ticks">${items.map(([done, text], i) =>
  `<li class="${done ? "done" : ""}${running.includes(i) ? " at" : ""}">
    <span class="tick${running.includes(i) ? " blink" : ""}">${done ? "✓" : ""}</span><span>${esc(text)}</span></li>`).join("")}</ul>`;
const meter = (done, total, cls = "") => total
  ? `<div class="meter ${cls}"><span style="width:${Math.round(100 * done / total)}%"></span></div>` : "";

// A button of gerdr's page, pressed by what it does. One whose desk opens
// a field asks for its text first; a quick one (picks, marks, one gerdr
// confirms itself) presses without a confirm here. A pair's chosen one
// (Pass, Fail) shows in its tone. None while another machine writes it.
let writable = true;
// One that is off with a reason (Mark passed) looks off but presses at
// once, as on the desktop: gerdr says what is left.
const btn = (b, cls = "") => {
  if (!writable) return "";
  const dots = b.asks && !b.label.endsWith("…") ? "…" : "";
  const asks = b.asks ? ` data-asks="${esc(b.asks)}" data-value="${esc(b.value || "")}"` : "";
  const held = b.off && b.why;
  const more = [b.primary && "primary", b.tone, (b.quick || held) && "quiet-press", held && "held", cls].filter(Boolean).join(" ");
  const state = held ? ` title="${esc(b.why)}" aria-disabled="true"` : b.off ? " disabled" : "";
  return `<button class="${esc(more)}" data-press="${esc(b.id)}"${asks}${state}>${esc(b.label)}${dots}</button>`;
};
const row = (bs, cls = "") => writable && bs?.length ? `<div class="buttons">${bs.map((b) => btn(b, cls)).join("")}</div>` : "";

// A section of a stage's page, as the desktop has it, in words.
function section(x, f, p, s) {
  switch (x.kind) {
    case "agent": {
      // The pane of this card's own agent, while herdr has it.
      const a = x.pane && f.agents.find((a) => a.pane === x.pane);
      const head = `<div class="top">${dotOf(x.dot)}<span class="name">${esc(x.name || "Agent")}</span>
        ${a ? `<span class="chev">›</span>` : ""}</div>
        <div class="meta">${esc(x.status)}</div>
        ${x.tail?.length ? `<pre class="tail">${esc(stripAnsi(x.tail.slice(-4).join("\n")))}</pre>` : ""}`;
      const cls = `card agent${x.waits ? " tone-waits" : x.dot === "working" ? " tone-working" : ""}`;
      const top = a ? `<a class="open" href="${href("pane", p, s, a.pane)}">${head}</a>` : head;
      // The agent its next start runs on is lit; its model, after them.
      const picks = (x.picks || []).map((b) => btn({ ...b, primary: false }, `pick${b.primary ? " lit" : ""}`));
      if (x.model) picks.push(btn(x.model, "pick"));
      return `<div class="${cls}">${top}${row(x.buttons)}${writable && picks.length ? `<div class="buttons picks">${picks.join("")}</div>` : ""}</div>`;
    }
    case "file":
      return `<div class="fileline small"><b>${esc(x.name)}</b> · ${toned(x.state)}</div>${row(x.buttons)}`;
    case "doc":
      return `<details class="fold doc-fold" open><summary><h2>${esc(x.name)}</h2></summary>
        ${x.html ? `<article class="doc card">${x.html}</article>` : `<p class="error small">${esc(x.error)}</p>`}</details>`;
    case "lines":
      return `<h3>${esc(x.label)}</h3><ul class="lines">${x.lines.map((l) => `<li>${toned(l)}</li>`).join("")}</ul>${row(x.buttons)}`;
    case "heading":
      return `<h3>${esc(x.text)}</h3>`;
    case "milestones":
    {
      // The count, bar and state show; the milestones fold, as they're long.
      const head = `<div class="top"><span class="name">${esc(x.label)}</span>
          <span class="count">${x.done} of ${x.total}</span></div>
        ${meter(x.done, x.total, x.done === x.total ? "full" : "")}
        ${x.state ? `<div class="meta">${toned(x.state)}</div>` : ""}`;
      return (x.items.length
        ? `<details class="card milestones"><summary>${head}</summary>${ticks(x.items, x.running)}</details>`
        : `<div class="card milestones">${head}</div>`) + row(x.buttons);
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
          ${x.meta ? `<div class="meta">${esc(x.meta)}</div>` : ""}</summary>${asked}${fixes}${changed}</details>${row(x.buttons)}`;
    }
    case "finding": {
      const lead = x.dot ? dotOf(x.dot) : x.tag ? `<span class="sev ${esc(x.tag)}">${esc(x.tag)}</span>` : "";
      // An issue of one line, its title cut to the desktop's width: the
      // phone's lines wrap, so the line is the title.
      const [only] = x.steps?.length === 1 && !x.steps[0][0] ? x.steps : [];
      const whole = only && x.title.endsWith("…") && only[1].startsWith(x.title.slice(0, -1).trimEnd());
      const title = whole ? only[1] : x.title;
      // A criterion's steps in full: its Given, When and Then.
      const steps = !whole && x.steps?.length && `<dl class="steps">${x.steps.map(([w, t]) =>
        `<div>${w ? `<dt>${esc(w)}</dt>` : ""}<dd>${esc(t)}</dd></div>`).join("")}</dl>`;
      // A file's place by its name on the card, its path unfolded.
      const path = x.place?.includes("/") ? x.place : "";
      const more = [
        path && `<p class="quiet small">${esc(path)}</p>`,
        steps,
        x.body && `<p>${esc(x.body)}</p>`,
        x.fix && `<p>${esc(x.fix)}</p>`,
        x.note && `<p class="quiet">${esc(x.note)}</p>`,
        x.comment && `<p><b>Your comment:</b> ${esc(x.comment)}</p>`,
        x.preview?.length && `<pre class="tail">${esc(stripAnsi(x.preview.join("\n")))}</pre>`,
      ].filter(Boolean).join("");
      const meta = [path ? path.split("/").pop() : x.place, x.state, x.clock].filter(Boolean).map(esc).join(" · ");
      const body = `<div class="top">${lead}<span class="name">${esc(title)}</span>${x.made ? `<span class="t-ready small">made</span>` : ""}</div>
        ${meta ? `<div class="meta">${meta}</div>` : ""}`;
      // A comment's buttons edit what it says: it starts from that.
      const said = (x.commentButtons || []).map((b) => ({ ...b, value: b.value || x.comment }));
      const acts = row(x.buttons) + row(said);
      return more
        ? `<div class="card finding${x.faded ? " faded" : ""}"><details class="in"><summary>${body}</summary>${more}</details>${acts}</div>`
        : `<div class="card finding${x.faded ? " faded" : ""}">${body}${acts}</div>`;
    }
    case "review": {
      // As the desktop's: a pill per severity, what is left of it (all
      // decided: ✓), and a bar of what was wanted, then what was refused.
      const pills = (x.counts || []).map(([word, , open]) =>
        `<span class="pill ${esc(word)}${open ? "" : " faded"}">${open ? `${open} ${esc(word)}` : `${esc(word)} ✓`}</span>`).join("");
      const pct = (n) => x.total ? Math.round(100 * Math.min(n, x.total) / x.total) : 0;
      const bar = x.total ? `<div class="meter"><span class="wanted" style="width:${pct(x.decided - x.refused)}%"></span><span class="refused" style="width:${pct(x.refused)}%"></span></div>` : "";
      return `<div class="card"><div class="top"><span class="name">${esc(x.label)}</span></div>
        ${pills ? `<div class="pills">${pills}</div>` : ""}${bar}<div class="meta">${esc(x.words)}</div>
        ${x.notes.map((l) => `<div class="small">${toned(l)}</div>`).join("")}</div>`;
    }
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

const fold = (label, n, inner) =>
  `<details class="fold"><summary><h3>${esc(label)} <span class="count">${n}</span></h3></summary>${inner}</details>`;
// What settled findings are, by their lit button: Passed, Done, else Decided.
const SETTLED = { Pass: "Passed", Done: "Done" };
const settledWord = (xs) => [...new Set(xs.map((x) =>
  SETTLED[x.buttons?.find((b) => b.tone === "passed")?.label] || "Decided"))]
  .map((w, i) => i ? w.toLowerCase() : w).join(" or ");

// A stage's page: what it is for, how it stands, its rare buttons in a
// menu at its top right, its sections as the desktop's page has them,
// each with its buttons, and its latest files: the fix plan and evidence
// folded open there, the others to open.
function stagePage(f, p, s) {
  const pg = f.page;
  writable = !f.offer.elsewhere;
  const menu = writable && f.offer.menu?.length ? `<details class="menu"><summary aria-label="More">⋯</summary>
    <div class="menu-list">${f.offer.menu.map((b) => btn(b)).join("")}</div></details>` : "";
  const at = f.card ? f.card.big : f.stages.findIndex((x) => x.now);
  const where = pg.own || at < 0 ? ""
    : `<div class="quiet small">The feature is at <a href="${href("f", p, s, String(at))}">${esc(atStage(f))}</a></div>`;
  let html = `<div class="page-head">${menu}<h2>${esc(pg.label)}</h2>${where}
    ${pg.about ? `<div class="quiet small">${esc(pg.about)}</div>` : ""}
    ${pg.status ? `<div class="pstatus">${toned(pg.status)}</div>` : ""}</div>`;
  // The stage's agents first, as they're what's at work.
  const agents = pg.sections.filter((x) => x.kind === "agent");
  for (const x of agents) html += section(x, f, p, s);
  // Adding an issue goes under the criteria it joins.
  const rest = pg.sections.filter((x) => x.kind !== "agent");
  const last = rest.map((x) => !!x.steps).lastIndexOf(true);
  const adds = (b) => last >= 0 && b.id === "AddComment";
  html += actions({ ...f.offer, buttons: (f.offer.buttons || []).filter((b) => !adds(b)) });
  // Settled findings fold under the open ones, earlier fix rounds under
  // the latest: what is still to do stays in sight.
  let settled = [], rounds = [], addHere = false;
  const flush = () => {
    if (settled.length) html += fold(settledWord(settled), settled.length, settled.map((x) => section(x, f, p, s)).join(""));
    // Named for what they are: "Fix round 17" folds under Earlier fix rounds.
    const kind = rounds[0]?.label.replace(/\s*\d+$/, "").toLowerCase();
    if (rounds.length) html += fold(`Earlier ${kind}s`, rounds.length, rounds.map((x) => section(x, f, p, s)).join(""));
    if (addHere) html += row((f.offer.buttons || []).filter(adds));
    settled = []; rounds = []; addHere = false;
  };
  // A heading is a lines section with no lines: one over nothing is left
  // out. Over fix rounds, "History" says what they are: the feature's own
  // History is further down.
  const heading = (x) => x?.kind === "lines" && !x.lines.length && !x.buttons?.length;
  const bare = (i) => heading(rest[i]) && (!rest[i + 1] || ["lines", "heading"].includes(rest[i + 1].kind));
  rest.forEach((x, i) => {
    if (bare(i)) return;
    if (heading(x) && x.label === "History" && rest[i + 1]?.kind === "round") html += `<h3>Fix rounds</h3>`;
    else if (x.kind === "finding" && x.faded) settled.push(x);
    else if (x.kind === "round" && rest[i - 1]?.kind === "round") rounds.push(x);
    else html += section(x, f, p, s);
    if (i === last) addHere = true;
    if (rest[i + 1]?.kind !== x.kind) flush();
  });
  // Latest files, one list: those read here unfold in place, the others
  // open on their own page.
  if (pg.files?.length) {
    html += `<h2>Latest files</h2><div class="group">` + pg.files.map((d) => d.html || d.error
      ? `<details class="file-row"${d.open ? " open" : ""}><summary class="row"><span>${esc(d.name)}</span><span class="mark"></span></summary>
          ${d.html ? `<article class="doc">${d.html}</article>` : `<p class="error small">${esc(d.error)}</p>`}</details>`
      : `<a class="row" href="${href("doc", p, s, d.name)}"><span>${esc(d.name)}</span><span class="chev">›</span></a>`).join("") + `</div>`;
  }
  return html;
}

async function feature(p, s, stage) {
  const f = await get(`/api/feature?${q(stage === undefined ? { p, s } : { p, s, stage })}`);
  // The rail, the waits box and the page say where it stands.
  let html = `<div class="meta quiet small">${esc(f.projectName)}</div>`;
  html += rail(f, p, s);
  html += waitsLink(f, p, s);
  html += stagePage(f, p, s);
  // Agents the stage page has no card for: the page's own show there.
  const onPage = new Set(f.page.sections.filter((x) => x.kind === "agent").map((x) => x.pane));
  const others = f.agents.filter((a) => !onPage.has(a.pane));
  if (others.length) {
    html += `<h2>${onPage.size ? "Other agents" : "Agents"} <span class="count">${others.length}</span></h2><div class="group">` +
      others.map((a) => `<a class="row" href="${href("pane", p, s, a.pane)}">
        <span>${esc(a.label)}</span>${chip(a, a.status)}</a>`).join("") + `</div>`;
  } else if (!f.agents.length && (!onPage.size || f.agentsError)) {
    html += `<h2>Agents</h2><p class="quiet small">${esc(f.agentsError || "No agent running for it here.")}</p>`;
  }
  html += history(f.history, f.now);
  return { title: f.title, back: stage === undefined ? "#/" : href("f", p, s), html };
}

function actions(offer) {
  if (offer.error) return `<p class="quiet small">Actions need gerdr: ${esc(offer.error)}</p>`;
  if (offer.elsewhere) return `<p class="quiet small">${esc(offer.elsewhere)} writes it: act there.</p>`;
  let html = row(offer.buttons);
  const back = offer.sendBack;
  if (back) {
    // The line comments made on the desk go with what is typed here.
    const lines = back.comments || [];
    const said = lines.length ? `<div class="small quiet">${lines.length} line comment${lines.length > 1 ? "s" : ""} go${lines.length > 1 ? "" : "es"} with it:</div>
      <ul class="lines small">${lines.map((c) => `<li>${c.line ? `<b>line ${c.line}</b> <span class="quiet">${esc(c.quote)}</span><br>` : ""}${esc(c.text)}</li>`).join("")}</ul>` : "";
    html += `<details class="sendback"${lines.length ? " open" : ""}><summary>Send ${esc(back.name || "it")} back with a comment</summary>
      ${back.error ? `<p class="error small">${esc(back.error)}</p>` : said}
      <textarea id="backText" rows="1" aria-label="What should change" placeholder="What should change"></textarea>
      <button data-sendback data-lines="${lines.length}">Send back</button></details>`;
  }
  return html;
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
      <textarea name="idea" rows="1" placeholder="Discover's agent starts from this"></textarea></label>
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

// Enter in a one-line field of the new feature form goes to the next
// field, as its key says: only Start feature starts it.
document.addEventListener("keydown", (e) => {
  if (e.key !== "Enter" || !e.target.matches?.("#newf input:not([type=checkbox])")) return;
  e.preventDefault();
  const all = [...e.target.form.elements].filter((x) => !x.disabled && x.offsetParent !== null && x.type !== "checkbox");
  all[all.indexOf(e.target) + 1]?.focus();
});

document.addEventListener("change", (e) => {
  if (e.target.name !== "project" || e.target.form?.id !== "newf") return;
  e.target.form.querySelectorAll(".for").forEach((x) => { x.hidden = x.dataset.for !== e.target.value; });
});

async function doc(p, s, name) {
  const d = await get(`/api/doc?${q({ p, s, f: name })}`);
  return { title: name, back: href("f", p, s), html: `<article class="doc">${d.html}</article>`, keep: true };
}

// How much of a pane is read: its last lines, more of them, then all its
// scrollback. Each scroll to the top reads the next; a new pane starts over.
const DEPTHS = [400, 1000, "all"];
let depth = { pane: "", at: 0, more: false, loading: false };

async function pane(p, s, id) {
  if (depth.pane !== id) depth = { pane: id, at: 0, more: false, loading: false };
  const d = await get(`/api/pane?${q({ p, s, pane: id, lines: DEPTHS[depth.at] })}`);
  depth.more = d.more && depth.at < DEPTHS.length - 1;
  // TUIs pad lines to their width: on a phone those runs wrap into blank rows.
  const raw = d.text.replace(/\r\n/g, "\n").replace(/ {6,}/g, "   ")
    .replace(/([─━═▀▄])\1{29,}/g, (_, c) => c.repeat(30));
  const text = readable(new AnsiUp().ansi_to_html(raw));
  const above = depth.more ? `<div class="above quiet small">Scroll up for earlier output</div>` : "";
  // A permission its agent asks for on gerdr's card: answered here too.
  const a = d.ask;
  const ask = a ? `<div class="ask"><div class="what"><b>${esc(a.tool)}</b> <code>${esc(a.gist)}</code></div>
      <details><summary class="quiet small">What it would run</summary><pre class="input">${esc(a.input)}</pre></details>
      <div class="buttons"><button class="primary" data-answer="allow" data-ask="${esc(a.ask)}">Allow</button><button data-answer="deny" data-ask="${esc(a.ask)}">Deny</button></div></div>` : "";
  // While it works, as on gerdr's chat: a row under its output, with its turn's clock.
  const since = d.working == null ? null : turnAt(id, d.working);
  const working = since === null ? "" : `<div class="work-row small"><span class="sdot blink"></span>working
    <span class="clock" data-since="${since}"></span></div>`;
  return { title: d.label, back: s ? href("f", p, s) : "#/", html: `${above}<div class="term-box"><pre class="term">${text}</pre>${working}</div>`, pane: true, since,
    bar: `${ask}<ul id="slash" hidden></ul><form id="say"><textarea id="line" rows="1" enterkeyhint="enter" autocomplete="off" aria-label="Reply to the agent" placeholder="Reply to the agent"></textarea>
      <button>Send</button></form>
      <div class="keys">${KEYS.map(([k, l, name]) => `<button data-key="${k}" aria-label="${name}">${l}</button>`).join("")}
        ${s ? `<button data-key="close" class="close" title="Close its tab, as on the desktop">Close</button>` : ""}</div>` };
}

// When the working agent's turn began, in this phone's time: kept across
// reads, so the page stays the same while its clock ticks.
let turn = { pane: "", at: 0 };
function turnAt(id, secs) {
  const at = Date.now() - secs * 1000;
  // Each answer comes a moment late: a start within 3s is the same turn.
  if (turn.pane !== id || Math.abs(turn.at - at) > 3000) turn = { pane: id, at };
  return turn.at;
}
// How long, as gerdr says it: `12s`, `2m 5s`, `1h 5m` (`clock::took`).
const took = (s) => s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m ${s % 60}s`
  : `${Math.floor(s / 3600)}h ${Math.floor(s / 60) % 60}m`;
const tickClocks = () => document.querySelectorAll(".clock[data-since]").forEach((e) => {
  e.textContent = took(Math.max(0, Math.floor((Date.now() - Number(e.dataset.since)) / 1000)));
});
setInterval(tickClocks, 1000);

// A program with a light theme paints light rows: text on them in the
// terminal's own colour gets one that reads there, and so on dark rows.
const readable = (html) => html.replace(/<span style="([^"]*)">/g, (m, st) => {
  const bg = /background-color:rgb\((\d+),(\d+),(\d+)\)/.exec(st);
  if (!bg || /(^|;)color:/.test(st)) return m;
  const light = 0.299 * bg[1] + 0.587 * bg[2] + 0.114 * bg[3] > 140;
  return `<span style="${st};color:${light ? "#2a2c33" : "#dce0e5"}">`;
});

// Keys for an agent's menus and prompts: herdr's name, the label, and
// the name a screen reader says.
const KEYS = [["esc", "Esc", "Escape"], ["1", "1", "1"], ["2", "2", "2"], ["3", "3", "3"],
  ["Up", "↑", "Up"], ["Down", "↓", "Down"], ["Enter", "⏎", "Enter"]];

// The view the address names, and how often it is read again.
function route() {
  const parts = location.hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
  switch (parts[0]) {
    case "f": return { load: () => feature(parts[1], parts[2], parts[3]), every: 5000 };
    case "doc": return { load: () => doc(parts[1], parts[2], parts[3]), every: 10000 };
    case "pane": return { load: () => pane(parts[1], parts[2], parts[3]), every: 2000 };
    case "agent": return { load: () => pane(parts[1], "", parts[2]), every: 2000 };
    case "go": return { load: () => go(parts[1]), every: 0 };
    case "new": return { load: newForm, every: 10000 };
    default: return { load: list, every: 5000 };
  }
}

// A fold is known by its title, so one added above it opens no other.
function foldKey(d, seen) {
  const s = d.querySelector(":scope > summary");
  const k = (s?.querySelector(".name, h2, h3")?.firstChild?.textContent || s?.textContent || "").trim();
  const n = seen.get(k) || 0;
  seen.set(k, n + 1);
  return `${k}#${n}`;
}
const foldsOpen = () => {
  const seen = new Map();
  return new Map([...main.querySelectorAll("details")].map((d) => [foldKey(d, seen), d.open]));
};

let timer = null;
let shown = "";
let barFor = null;
// Something typed on the page: a refresh would lose it. A checkbox (a
// doc's task list) is no typing.
const fields = () => [...main.querySelectorAll("textarea, input:not([type=checkbox]):not([type=radio])")];
const typing = () => fields().some((e) => e.value || e === document.activeElement);
// Presses on their way: their buttons stay off across a refresh.
const pressing = new Set();
// Only the latest read is shown: an earlier one may come back after it.
let reads = 0;
async function render(fresh, force, keepBottom) {
  clearTimeout(timer);
  const read = ++reads;
  const r = route();
  const hash = location.hash;
  // A pane read follows the end unless the user scrolled up from it.
  const atEnd = window.innerHeight + window.scrollY >= document.body.scrollHeight - 40;
  const fromBottom = document.body.scrollHeight - window.scrollY;
  try {
    const v = await r.load();
    if (hash !== location.hash || read !== reads) return;
    titleEl.textContent = v.title;
    // A working agent's page: its clock in the head, in place of when it
    // was read, and the sweep along the head's edge, as on gerdr's chat.
    const working = v.since != null;
    work.hidden = !working;
    seen.hidden = working;
    sweep.hidden = !working;
    if (working) work.querySelector(".clock").dataset.since = v.since;
    else delete work.querySelector(".clock").dataset.since;
    tickClocks();
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
      const open = foldsOpen();
      const railX = main.querySelector(".rail-wrap")?.scrollLeft;
      // What is written in a box goes on in it, a forced refresh too.
      const written = fields().filter((e) => e.id && e.value).map((e) => [e.id, e.value]);
      main.innerHTML = v.html;
      for (const [id, value] of written) {
        const e = document.getElementById(id);
        if (e && !e.value) { e.value = value; if (e.matches("textarea")) fit(e); }
      }
      main.querySelectorAll("button[data-press]").forEach((x) => { if (pressing.has(x.dataset.press)) x.disabled = true; });
      if (!fresh) {
        const seen = new Map();
        main.querySelectorAll("details").forEach((d) => {
          const k = foldKey(d, seen);
          if (open.has(k)) d.open = open.get(k);
        });
      }
      shown = v.html;
      tickClocks();
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
      // Earlier output came in above: what was read stays in view.
      else if (keepBottom) window.scrollTo(0, document.body.scrollHeight - fromBottom);
      else if (!fresh) window.scrollTo(0, y);
    }
    if (fresh && !v.pane) window.scrollTo(0, 0);
  } catch (e) {
    if (hash !== location.hash || read !== reads) return;
    if (!fresh && typing()) {
      // What is being written stays: the failure is said instead.
      if (e.message !== "offline") say([{ text: e.message }], true);
    } else if (fresh || e.message !== "offline") {
      // Home-screen apps have no browser Back: always a way out.
      back.hidden = route().load === list;
      back.href = "#/";
      main.innerHTML = `<p class="error">${esc(e.message === "offline" ? "Couldn't load." : e.message)}</p>`;
      shown = "";
    }
  }
  if (r.every && !document.hidden) timer = setTimeout(() => render(false), r.every);
}

// The route's feature and pane: `#/f/<p>/<s>` or `#/pane/<p>/<s>/<pane>`;
// a workspace's agent, `#/agent/<p>/<pane>`, as a pane of no feature.
const here = () => {
  const parts = location.hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
  return parts[0] === "agent" ? ["pane", parts[1], "", parts[2]] : parts;
};

// A question in a sheet over the page: a confirm with the button that
// does it (`go`), or, with `field`, the text a button asks for. Its box
// grows as every box does; Enter breaks the line, the button sends, and
// line breaks become spaces. Cancel (or Esc) keeps what was written as a
// draft for the next time the same button asks. True, the text, or null.
const sheet = document.getElementById("sheet");
const drafts = new Map();
function sheetAsk({ title, body, go, field, value = "", draft }) {
  const start = drafts.get(draft) ?? value;
  sheet.innerHTML = `<form method="dialog">${field ? `<label for="sheetText">${esc(title)}</label>
      <textarea id="sheetText" rows="1">${esc(start)}</textarea>`
    : `<h3 class="ask">${esc(title)}</h3>${body ? `<p>${esc(body)}</p>` : ""}`}
    <div class="buttons"><button value="">Cancel</button><button class="primary" value="ok">${esc(go)}</button></div></form>`;
  const box = sheet.querySelector("textarea");
  sheet.returnValue = "";
  sheet.showModal();
  if (box) {
    fit(box);
    box.focus();
    box.setSelectionRange(box.value.length, box.value.length);
  }
  return new Promise((done) => sheet.addEventListener("close", () => {
    const ok = sheet.returnValue === "ok";
    if (!box) return done(ok || null);
    if (draft !== undefined) {
      if (ok || box.value === value) drafts.delete(draft);
      else drafts.set(draft, box.value);
    }
    done(ok ? box.value.replace(/\s*\n\s*/g, " ").trim() : null);
  }, { once: true }));
}

// A tap outside an open ⋯ menu closes it. A pointer, not a click: iOS
// sends no click for a tap on the page's plain parts.
document.addEventListener("pointerdown", (e) => {
  document.querySelectorAll("details.menu[open]").forEach((m) => {
    if (!m.contains(e.target)) m.removeAttribute("open");
  });
});

document.addEventListener("click", async (e) => {
  const b = e.target.closest("button[data-press], button[data-sendback], button[data-key], button[data-push], button[data-answer]");
  if (!b || b.disabled) return;
  const [, p, s, pane] = here();
  // The feature's page names its stage where the address does.
  const stage = here()[0] === "f" && here()[3] !== undefined ? Number(here()[3]) : undefined;
  b.disabled = true;
  b.closest("details.menu")?.removeAttribute("open");
  try {
    if (b.dataset.push) {
      await push(b.dataset.push).catch((err) => say([{ text: err.message }], true));
    } else if (b.dataset.press) {
      const id = b.dataset.press;
      const label = b.textContent.trim().replace(/…$/, "");
      let text;
      if (b.dataset.asks !== undefined) {
        // "Model: default…" sets a value: its sheet saves.
        const go = label.includes(":") ? "Save" : label;
        text = await sheetAsk({ title: b.dataset.asks, field: true, value: b.dataset.value || "", go, draft: `${location.hash} ${id}` });
        if (text === null) return;
      } else if (!b.classList.contains("quiet-press") &&
          !(await sheetAsk({ title: `${label}?`, body: titleEl.textContent, go: label }))) {
        return;
      }
      if (pressing.has(id)) return;
      pressing.add(id);
      try { await ask("/api/press", { p, s, stage, id, text }); } finally { pressing.delete(id); }
    } else if (b.dataset.sendback !== undefined) {
      const box = document.getElementById("backText");
      if (!box.value.trim() && b.dataset.lines === "0") { say([{ text: "Say what should change" }], true); return; }
      if (await ask("/api/send-back", { p, s, text: box.value })) { box.value = ""; fit(box); }
    } else if (b.dataset.answer) {
      const a = await post("/api/answer", { p, s, pane, ask: b.dataset.ask, reply: b.dataset.answer });
      if (a.error) say([{ text: a.error }], true);
      setTimeout(() => render(false, true), 400);
    } else if (b.dataset.key) {
      if (b.dataset.key === "close" &&
          !(await sheetAsk({ title: `Close ${titleEl.textContent}?`, body: "Its tab closes, as on the desktop.", go: "Close" }))) return;
      const a = await post("/api/agent", { p, s, pane, key: b.dataset.key });
      if (a.error) say([{ text: a.error }], true);
      if (b.dataset.key === "close" && !a.error) { location.hash = href("f", p, s); return; }
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
  document.getElementById("slash").hidden = true;
  const a = await post("/api/agent", { p, s, pane, text: line.value });
  if (a.error) say([{ text: a.error }], true);
  else { line.value = ""; fit(line); }
  setTimeout(() => render(false, true), 600);
});

// A box grows with what is written in it, up to five rows, then scrolls:
// a shade along its top or bottom edge says there is more that way. Enter
// in it breaks the line; its button sends.
const ROWS = 5;
function fit(box) {
  const cs = getComputedStyle(box);
  const px = (k) => parseFloat(cs[k]) || 0;
  const edges = px("paddingTop") + px("paddingBottom") + px("borderTopWidth") + px("borderBottomWidth");
  box.style.height = "auto";
  const most = px("lineHeight") * ROWS + edges;
  box.style.height = `${Math.min(box.scrollHeight + px("borderTopWidth") + px("borderBottomWidth"), most)}px`;
  shade(box);
}
function shade(box) {
  // Typing scrolls the caret's line into view, not the padding past it: a
  // padding out of view hides no text.
  const cs = getComputedStyle(box);
  const [top, bottom] = [parseFloat(cs.paddingTop) || 0, parseFloat(cs.paddingBottom) || 0];
  box.classList.toggle("more-above", box.scrollTop > top + 1);
  box.classList.toggle("more-below", box.scrollTop + box.clientHeight < box.scrollHeight - bottom - 1);
}
document.addEventListener("input", (e) => { if (e.target.matches("textarea")) fit(e.target); });

// The `/` menu over the reply box, as on gerdr's chat: the agent's
// commands, read once per pane, those that start with what follows the `/`
// first, then those that hold it. A tap fills one in, to send or add to.
const commandsOf = new Map();
async function slashMenu(box) {
  const menu = document.getElementById("slash");
  const typed = /^\/(\S*)$/.exec(box.value);
  if (!menu) return;
  if (!typed) { menu.hidden = true; return; }
  const [, p, s, pane] = here();
  if (!commandsOf.has(pane)) {
    commandsOf.set(pane, get(`/api/commands?${q({ p, s, pane })}`)
      .catch(() => { commandsOf.delete(pane); return []; }));
  }
  const all = await commandsOf.get(pane);
  // Typed on meanwhile: that key's own read shows the menu.
  if (box.value !== typed[0]) return;
  const t = typed[1].toLowerCase();
  const starts = all.filter((c) => c.name.toLowerCase().startsWith(t));
  const holds = all.filter((c) => !starts.includes(c) && c.name.toLowerCase().includes(t));
  const shown = [...starts, ...holds].slice(0, 8);
  menu.innerHTML = shown.map((c) => `<li><button type="button" data-command="${esc(c.name)}">
    <b>/${esc(c.name)}</b>${c.hint ? ` <span class="quiet">${esc(c.hint)}</span>` : ""}
    ${c.about ? `<span class="about small quiet">${esc(c.about)}</span>` : ""}</button></li>`).join("");
  menu.hidden = !shown.length;
}
document.addEventListener("input", (e) => { if (e.target.id === "line") slashMenu(e.target); });
// A tap on a command keeps the keyboard up.
document.addEventListener("pointerdown", (e) => { if (e.target.closest?.("[data-command]")) e.preventDefault(); });
document.addEventListener("click", (e) => {
  const b = e.target.closest?.("[data-command]");
  if (!b) return;
  const line = document.getElementById("line");
  line.value = `/${b.dataset.command} `;
  fit(line);
  line.focus();
  document.getElementById("slash").hidden = true;
});
document.addEventListener("scroll", (e) => { if (e.target.matches?.("textarea")) shade(e.target); }, true);
// The page's end stays clear of the bar as its box grows.
new ResizeObserver(() => document.body.style.setProperty("--bar-h", `${bar.offsetHeight}px`)).observe(bar);

// At the top of a pane's output, read more of it; back at its end, only
// its last lines again.
window.addEventListener("scroll", async () => {
  if (depth.at && window.innerHeight + window.scrollY >= document.body.scrollHeight - 40) depth.at = 0;
  if (window.scrollY > 80 || !depth.more || depth.loading || here()[0] !== "pane") return;
  depth.loading = true;
  depth.at += 1;
  try { await render(false, true, true); } finally { depth.loading = false; }
}, { passive: true });

// Another page: the reply bar goes with the pane it was for, and a pane
// shown again starts from its last lines.
window.addEventListener("hashchange", () => {
  bar.innerHTML = "";
  bar.hidden = true;
  document.body.classList.remove("with-bar");
  barFor = null;
  depth.pane = "";
  render(true);
});
document.addEventListener("visibilitychange", () => { if (!document.hidden) render(false); });
render(true);
