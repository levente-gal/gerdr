// The phone view: features, their stages and agents, read from gerdr's
// `--phone` server, which it polls while shown. Its presses go back there:
// a new feature, a stage page's buttons and send-backs to gerdr,
// lines and keys to an agent's pane.

const main = document.getElementById("main");
const titleEl = document.getElementById("title");
const back = document.getElementById("back");
const seen = document.getElementById("seen");
const work = document.getElementById("work");
const offline = document.getElementById("offline");
const note = document.getElementById("note");
const bar = document.getElementById("bar");
// On a pane's page scrolled up from its end, a button goes back to it.
const toEnd = document.getElementById("end");

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
      <span class="two"><span>${esc(a.label)}</span>${a.title ? `<span class="quiet small">${esc(a.title)}</span>` : ""}</span>${chip(a, a.status)}</a>`).join("") + `</div>`;
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
  // A permission its agent asks for on gerdr's card, or its questions: answered here too.
  const a = d.ask;
  const answers = !a ? "" : `<div class="buttons"><button class="primary" data-answer="allow" data-ask="${esc(a.ask)}">Allow</button><button data-answer="deny" data-ask="${esc(a.ask)}">Deny</button></div>`;
  // Plan mode's plan: rendered by gerdr (no raw HTML), shown whole, as it is what Allow approves.
  const ask = !a ? "" : a.questions?.length ? quizCard(a)
    : a.plan ? `<div class="ask"><div class="what"><b>Plan</b></div><div class="plan doc">${a.plan}</div>${answers}</div>`
    : `<div class="ask"><div class="what"><b>${esc(a.tool)}</b> <code>${esc(a.gist)}</code></div>
      <details><summary class="quiet small">What it would run</summary><pre class="input">${esc(a.input)}</pre></details>
      ${answers}</div>`;
  // While it works, as on gerdr's chat: a row under its output, with its turn's clock.
  const since = d.working == null ? null : turnAt(id, d.working);
  const working = since === null ? "" : `<div class="work-row small"><span class="sdot blink"></span>working
    <span class="clock" data-since="${since}"></span></div>`;
  // Its modes under the reply box, as on gerdr's chat: the one its screen shows lit;
  // its model at their right, the one it is on picked.
  const group = d.modes?.length ? `<div class="modes" role="group" aria-label="Agent mode">${d.modes.map((m) =>
    `<button type="button" data-mode="${esc(m)}" aria-pressed="${m === d.mode}">${esc(m)}</button>`).join("")}</div>` : "";
  const now = d.model_id ? "" : `<option value="" selected disabled>${esc(d.model || "Model")}</option>`;
  // An agent picking its model in a menu of its own: a button opening it, named for
  // the model its screen says it is on; the menu then shows over the reply box.
  const opener = d.menu ? `<button type="button" class="model" data-model-menu aria-label="Model">${esc(d.model || "Model")}</button>` : "";
  const model = d.models?.length ? `<select class="model" aria-label="Model">${now}${d.models.map((m) =>
    `<option value="${esc(m.id)}"${m.id === d.model_id ? " selected" : ""}>${esc(m.name)}</option>`).join("")}</select>` : opener;
  // How full its context is, as gerdr's chat head says it.
  const context = d.context ? `<span class="context">${esc(d.context)}</span>` : "";
  const modes = group || model || context ? `<div class="modes-row">${group}${context}${model}</div>` : "";
  return { title: d.title ? `${d.label} · ${d.title}` : d.label, back: s ? href("f", p, s) : "#/", html: `${above}<div class="term-box"><pre class="term">${text}</pre>${working}</div>`, pane: true, since,
    said: d.said || [], ask, askId: a?.ask || "", asking: !!a?.questions?.length, suggestion: d.suggestion || "",
    modes, modesKey: JSON.stringify([d.modes, d.mode, d.models, d.model, d.model_id, d.menu, d.context]),
    picker: d.picker || null, pickerKey: JSON.stringify(d.picker || null),
    sides: sideCard(id, d.sides),
    bar: `<div id="picker"></div><div id="asked"></div><div id="sides"></div><button type="button" id="suggest" hidden></button><div id="files" hidden></div><form id="say"><ul id="slash" hidden></ul><textarea id="line" rows="1" enterkeyhint="enter" autocomplete="off" aria-label="Reply to the agent" placeholder="Reply to the agent"></textarea>
      <button type="button" id="attach" aria-label="Attach files">Attach</button><input type="file" id="attachFile" multiple hidden>
      <button>Send</button></form>${modes}
      <div class="keys">${KEYS.map(([k, l, name]) => `<button data-key="${k}" aria-label="${name}">${l}</button>`).join("")}
        ${s ? `<button data-key="close" class="close" title="Close its tab, as on the desktop">Close</button>` : ""}</div>` };
}

// The agent's own model menu (Codex, OpenCode, pi, Antigravity), as gerdr reads it
// off its screen: its rows to tap, its search typed into it, its effort moved.
// The search box is drawn once per menu, so what is typed holds across reads.
let menuSent = "";
function pickerShow(el, m) {
  if (!m) { el.innerHTML = ""; menuSent = ""; return; }
  if (!el.firstChild) {
    menuSent = "";
    el.innerHTML = `<div class="picker"><div class="what"></div>
      ${m.search ? `<input type="search" class="menu-search" placeholder="Search" aria-label="Search the models" autocomplete="off">` : ""}
      <div class="opts" role="list"></div><div class="buttons"></div></div>`;
  }
  const [title, ...more] = m.ask;
  el.querySelector(".what").innerHTML = `<b>${esc(title || "Model")}</b>${more.map((t) => `<div class="quiet small">${esc(t)}</div>`).join("")}`;
  el.querySelector(".opts").innerHTML = m.rows.length ? m.rows.map((r) => `<button type="button" class="opt${r.at ? " at" : ""}" data-menu-pick="${esc(r.label)}"${r.at ? ` aria-current="true"` : ""}>
    <span><b>${esc(r.label)}</b>${r.detail ? `<small>${esc(r.detail)}</small>` : ""}</span></button>`).join("")
    : `<div class="quiet small">No models match</div>`;
  el.querySelector(".at")?.scrollIntoView({ block: "nearest" });
  el.querySelector(".buttons").innerHTML = (m.sideways ? `<button type="button" data-key="Left" aria-label="Less effort">←</button><button type="button" data-key="Right" aria-label="More effort">→</button>` : "")
    + `<button type="button" data-key="esc">Close</button>`;
}

// Letters typed in the menu's search go to the agent's own, a moment after the
// last: those taken back first, then those new.
let menuTimer;
document.addEventListener("input", (e) => {
  if (!e.target.matches?.(".menu-search")) return;
  clearTimeout(menuTimer);
  menuTimer = setTimeout(async () => {
    const want = e.target.value;
    let same = 0;
    while (same < want.length && same < menuSent.length && want[same] === menuSent[same]) same++;
    const erase = [...menuSent.slice(same)].length, type = want.slice(same);
    if (!erase && !type) return;
    menuSent = want;
    const [, p, s, pane] = here();
    const a = await post("/api/agent", { p, s, pane, type, erase });
    if (a.error) say([{ text: a.error }], true);
    render(false, true);
  }, 400);
});

// The side questions (`/btw …`) asked of the agent this run, as gerdr
// keeps them: its own chat doesn't. The last shows on a card until hidden,
// the earlier ones folded under it; a new one shows the card again.
let sidesHidden = { pane: "", n: 0 };
function sideCard(id, sides) {
  const hidden = sidesHidden.pane === id ? sidesHidden.n : 0;
  if (!sides?.length || sides.length <= hidden) return "";
  const one = (x) => {
    const going = x.state === "asked" || x.state === "answering";
    const answer = x.state === "failed" ? `<p class="quiet small">Side question: ${esc(x.why)}</p>`
      : x.html.trim() ? `<div class="answer">${x.html}</div>`
      : going ? `<p class="quiet small">answering on the side</p>` : "";
    return `<div class="question"><b>/btw</b> ${esc(x.question)}</div>${answer}`;
  };
  const earlier = sides.slice(0, -1).reverse();
  const n = earlier.length;
  const fold = n ? `<details><summary class="quiet small">${n} earlier side question${n === 1 ? "" : "s"}</summary>${earlier.map(one).join("")}</details>` : "";
  return `<div class="side">${one(sides[n])}${fold}
    <div class="buttons"><button data-hide-sides="${sides.length}">Hide</button></div></div>`;
}
// Claude's questions, one at a time: a tab for each, its options as rows,
// and an Other row to type in. The card is drawn once per ask (see
// render), so what is picked and typed on it holds across reads.
function quizCard(a) {
  const qs = a.questions;
  const last = qs.length - 1;
  const tabs = last ? `<div class="chips tabs">${qs.map((q, i) => `<button type="button" class="chip" data-qtab="${i}"${i ? "" : ` aria-current="true"`}>
    ${esc(q.header || `Question ${i + 1}`)}</button>`).join("")}</div>` : "";
  const each = qs.map((q, i) => {
    const t = q.multi ? "checkbox" : "radio";
    const row = (o) => `<label class="opt"><input type="${t}" name="q${i}" value="${esc(o.label)}">
      <span><b>${esc(o.label)}</b>${o.description ? `<small>${esc(o.description)}</small>` : ""}</span></label>`;
    return `<div class="q" data-question="${esc(q.question)}"${i ? " hidden" : ""}><p>${esc(q.question)}</p>
      <div class="opts" role="group" aria-label="${esc(q.question)}">${q.options.map(row).join("")}
        <label class="opt"><input type="${t}" name="q${i}" data-other aria-label="Other"><input type="text" class="other"
          placeholder="Other" aria-label="Other answer" autocomplete="off" enterkeyhint="${i === last ? "done" : "next"}"></label></div></div>`;
  }).join("");
  return `<div class="ask quiz" data-at="0">${tabs}${each}<div class="buttons">
    <button data-answer="pass" data-ask="${esc(a.ask)}">Answer in terminal</button>
    <button class="primary" data-qnext${last ? "" : " hidden"}>Next</button>
    <button class="primary" data-answer="allow" data-ask="${esc(a.ask)}"${last ? " hidden" : ""} disabled>Submit</button></div></div>`;
}
// A question's answer as gerdr's `Answers` take it (chat/live.rs): the
// picked labels joined by ", ", what was typed for Other in its place and
// never the word.
const quizAnswer = (q) => [...q.querySelectorAll("input:checked")]
  .map((x) => x.dataset.other === undefined ? x.value : x.nextElementSibling.value.trim()).filter(Boolean).join(", ");
const quizAnswers = (card) => Object.fromEntries([...card.querySelectorAll(".q")].map((q) => [q.dataset.question, quizAnswer(q)]));
// Question `at` shows; the tabs mark it and those answered, and Submit,
// on the last, waits for every answer.
function quizShow(card, at) {
  const qs = [...card.querySelectorAll(".q")];
  card.dataset.at = at;
  qs.forEach((q, i) => { q.hidden = i !== at; });
  card.querySelectorAll("[data-qtab]").forEach((t, i) => {
    t.classList.toggle("done", !!quizAnswer(qs[i]));
    t.toggleAttribute("aria-current", i === at);
  });
  card.querySelector("[data-qnext]").hidden = at === qs.length - 1;
  const submit = card.querySelector("[data-answer=allow]");
  submit.hidden = at !== qs.length - 1;
  submit.disabled = !qs.every(quizAnswer);
}
document.addEventListener("change", (e) => {
  const card = e.target.closest?.(".quiz");
  if (!card || e.target.type === "text") return;
  const at = Number(card.dataset.at);
  const other = e.target.dataset.other !== undefined;
  if (other && e.target.checked) e.target.nextElementSibling.focus();
  quizShow(card, at);
  // A single pick is the answer: the next question shows, once the pick has.
  if (e.target.type === "radio" && !other && at < card.querySelectorAll(".q").length - 1) {
    setTimeout(() => { if (Number(card.dataset.at) === at) quizShow(card, at + 1); }, 250);
  }
});
// Typing in Other picks it.
document.addEventListener("input", (e) => {
  if (!e.target.matches?.(".quiz input.other")) return;
  if (e.target.value.trim()) e.target.previousElementSibling.checked = true;
  const card = e.target.closest(".quiz");
  quizShow(card, Number(card.dataset.at));
});
document.addEventListener("keydown", (e) => {
  if (e.key !== "Enter" || !e.target.matches?.(".quiz input.other")) return;
  e.preventDefault();
  e.target.closest(".quiz").querySelector(".buttons .primary:not([hidden])").click();
});
document.addEventListener("click", (e) => {
  const b = e.target.closest?.("[data-qtab], [data-qnext]");
  if (!b) return;
  const card = b.closest(".quiz");
  quizShow(card, b.dataset.qtab !== undefined ? Number(b.dataset.qtab) : Number(card.dataset.at) + 1);
});

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
    // was read, as on gerdr's chat.
    const working = v.since != null;
    work.hidden = !working;
    seen.hidden = working;
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
      bar.dataset.modes = v.modesKey || "";
      filesShow();
    }
    // The rest of the bar keeps what is typed; the agent's modes follow its screen.
    if (v.modesKey !== undefined && bar.dataset.modes !== v.modesKey) {
      bar.querySelector(".modes-row")?.remove();
      document.getElementById("say")?.insertAdjacentHTML("afterend", v.modes);
      bar.dataset.modes = v.modesKey;
    }
    // The agent's model menu shows while its screen does; its keys and
    // replies would land in it.
    const picking = v.pane && document.getElementById("picker");
    if (picking && v.pickerKey !== undefined && picking.dataset.key !== v.pickerKey) {
      pickerShow(picking, v.picker);
      picking.dataset.key = v.pickerKey;
      bar.classList.toggle("picking", !!v.picker);
    }
    // An ask comes and goes between reads; its card stays while it waits,
    // with what was picked on it. Keys and replies would land in its menu.
    const asked = v.pane && document.getElementById("asked");
    if (asked && asked.dataset.ask !== v.askId) {
      asked.innerHTML = v.ask;
      asked.dataset.ask = v.askId;
      bar.classList.toggle("asking", v.asking);
    }
    const sug = v.pane && document.getElementById("suggest");
    if (sug && sug.dataset.text !== v.suggestion) {
      sug.dataset.text = v.suggestion;
      sug.textContent = v.suggestion;
      sug.setAttribute("aria-label", `Fill in: ${v.suggestion}`);
      suggestShow();
    }
    // The side card changes as its answer comes: its folds stay as they were.
    const sidesEl = document.getElementById("sides");
    if (sidesEl && v.sides !== undefined && sidesEl.dataset.html !== v.sides) {
      const open = sidesEl.querySelector("details")?.open;
      sidesEl.innerHTML = v.sides;
      sidesEl.dataset.html = v.sides;
      const fold = sidesEl.querySelector("details");
      if (fold && open) fold.open = true;
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
    // Where the user's words are in the output, found again as it changes.
    const saidNow = v.pane ? `${shown}\0${JSON.stringify(v.said)}` : "";
    if (saidNow !== saidKey) {
      saidKey = saidNow;
      findSaid(v.pane ? v.said : []);
    }
    endButton();
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

// A model picked: the agent is switched with its own /model, as on gerdr's chat.
document.addEventListener("change", async (e) => {
  const pick = e.target.closest?.("select.model");
  if (!pick?.value) return;
  const [, p, s, pane] = here();
  pick.disabled = true;
  try {
    const a = await post("/api/agent", { p, s, pane, model: pick.value });
    if (a.error) say([{ text: a.error }], true);
  } finally {
    pick.disabled = false;
  }
  setTimeout(() => render(false, true), 400);
});

document.addEventListener("click", async (e) => {
  const b = e.target.closest("button[data-press], button[data-sendback], button[data-key], button[data-push], button[data-answer], button[data-mode], button[data-hide-sides], button[data-menu-pick], button[data-model-menu]");
  if (!b || b.disabled) return;
  const [, p, s, pane] = here();
  if (b.dataset.hideSides) {
    sidesHidden = { pane, n: Number(b.dataset.hideSides) };
    document.getElementById("sides").innerHTML = "";
    return;
  }
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
    } else if (b.dataset.mode) {
      if (b.getAttribute("aria-pressed") === "true") return;
      const from = document.querySelector('.modes [aria-pressed="true"]')?.dataset.mode || "";
      const a = await post("/api/agent", { p, s, pane, mode: b.dataset.mode, from });
      if (a.error) say([{ text: a.error }], true);
      render(false, true);
    } else if (b.dataset.answer) {
      const quiz = b.dataset.answer === "allow" && b.closest(".quiz");
      const answers = quiz ? quizAnswers(quiz) : undefined;
      const a = await post("/api/answer", { p, s, pane, ask: b.dataset.ask, reply: b.dataset.answer, answers });
      if (a.error) say([{ text: a.error }], true);
      setTimeout(() => render(false, true), 400);
    } else if (b.dataset.menuPick !== undefined) {
      const a = await post("/api/agent", { p, s, pane, pick: b.dataset.menuPick });
      if (a.error) say([{ text: a.error }], true);
      setTimeout(() => render(false, true), 400);
    } else if (b.dataset.modelMenu !== undefined) {
      // Its own /model opens the menu, which then shows here as its screen does.
      const a = await post("/api/agent", { p, s, pane, text: "/model" });
      if (a.error) say([{ text: a.error }], true);
      setTimeout(() => render(false, true), 800);
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
    const quiz = b.closest(".quiz");
    if (quiz) quizShow(quiz, Number(quiz.dataset.at));
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
  const files = attachedHere();
  if (!line.value.trim() && !files.length) return;
  const [, p, s, pane] = here();
  document.getElementById("slash").hidden = true;
  // A bare /model opens the model list, as a tap on it does (Claude, Cursor).
  const models = document.querySelector("select.model");
  if (line.value.trim() === "/model" && !files.length && models) {
    line.value = "";
    fit(line);
    models.focus();
    try { models.showPicker(); } catch { /* old browser: focused only */ }
    return;
  }
  // The files' paths go after the words: the agent reads each from its path.
  const text = [line.value.trim(), ...files.map((f) => f.path)].filter(Boolean).join(" ");
  const a = await post("/api/agent", { p, s, pane, text });
  if (a.error) say([{ text: a.error }], true);
  else {
    line.value = "";
    files.length = 0;
    filesShow();
    fit(line);
    // Spent: the next read brings the agent's next one.
    const sug = document.getElementById("suggest");
    if (sug) sug.dataset.text = "";
    suggestShow();
  }
  setTimeout(() => render(false, true), 600);
});

// Files for the agent, attached to the reply: gerdr saves each, and its
// path goes after the reply's words, which the agent reads it from. A
// photo is scaled down to a JPEG here first. Each page's files stay till
// sent, its chips over the reply box; a press on one takes it off.
const PIC_SIDE = 2048;
const FILE_MOST = 20 * 1024 * 1024;
const attachedBy = {};
const attachedHere = () => (attachedBy[location.hash] ??= []);
function filesShow() {
  const box = document.getElementById("files");
  if (!box) return;
  const files = attachedHere();
  box.hidden = !files.length;
  box.innerHTML = files.map((f, i) =>
    `<button type="button" class="chip" data-unattach="${i}" aria-label="Take ${esc(f.name)} off the reply">${esc(f.name)} <span aria-hidden="true">×</span></button>`).join("");
}
async function scaled(file) {
  try {
    const bitmap = await createImageBitmap(file);
    const scale = Math.min(1, PIC_SIDE / Math.max(bitmap.width, bitmap.height));
    const canvas = document.createElement("canvas");
    [canvas.width, canvas.height] = [Math.round(bitmap.width * scale), Math.round(bitmap.height * scale)];
    canvas.getContext("2d").drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    return await new Promise((done) => canvas.toBlob(done, "image/jpeg", 0.85));
  } catch {
    return null;
  }
}
async function addFile(file) {
  if (!file) return;
  const files = attachedHere();
  let [blob, name] = [file, file.name || "file"];
  // A photo the browser can draw goes as a JPEG; one it can't, as it is.
  if (file.type.startsWith("image/") && file.type !== "image/gif" && file.type !== "image/svg+xml") {
    const jpeg = await scaled(file);
    if (jpeg) [blob, name] = [jpeg, `${name.replace(/\.[^.]*$/, "") || "image"}.jpg`];
  }
  if (blob.size > FILE_MOST) return say([{ text: `${name} is too big to send, over 20 MB` }], true);
  let a;
  try {
    const res = await fetch(`/api/file?name=${encodeURIComponent(name)}`, { method: "POST", headers: { "Content-Type": "application/octet-stream", "X-Gerdr": "phone" }, body: blob });
    a = await res.json().catch(() => ({ error: `Bad answer (${res.status})` }));
  } catch {
    a = { error: "Can't reach gerdr" };
  }
  if (a.error) return say([{ text: a.error }], true);
  files.push({ name, path: a.path });
  filesShow();
}
document.addEventListener("click", (e) => {
  if (e.target.id === "attach") document.getElementById("attachFile")?.click();
  const chip = e.target.closest?.("[data-unattach]");
  if (chip) {
    attachedHere().splice(Number(chip.dataset.unattach), 1);
    filesShow();
    document.getElementById("line")?.focus();
  }
});
document.addEventListener("change", async (e) => {
  if (e.target.id !== "attachFile") return;
  const picked = [...(e.target.files || [])];
  e.target.value = "";
  for (const file of picked) await addFile(file);
});
document.addEventListener("paste", async (e) => {
  if (e.target.id !== "line") return;
  const pasted = [...(e.clipboardData?.files || [])];
  if (!pasted.length) return;
  e.preventDefault();
  for (const file of pasted) await addFile(file);
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
document.addEventListener("input", (e) => { if (e.target.id === "line") { slashMenu(e.target); suggestShow(); } });

// What Claude Code suggests saying next, as on gerdr's chat: over the empty
// reply box; a tap fills it in, to send or change.
function suggestShow() {
  const b = document.getElementById("suggest");
  const line = document.getElementById("line");
  if (b && line) b.hidden = !b.dataset.text || !!line.value;
}
document.addEventListener("click", (e) => {
  const b = e.target.closest?.("#suggest");
  if (!b) return;
  const line = document.getElementById("line");
  line.value = b.dataset.text;
  fit(line);
  line.focus();
  suggestShow();
});
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
// The page's end stays clear of the bar as its box grows. A pane's page
// at its end stays there: the bar is measured only after it first shows.
new ResizeObserver(() => {
  const atEnd = window.innerHeight + window.scrollY >= document.body.scrollHeight - 40;
  document.body.style.setProperty("--bar-h", `${bar.offsetHeight}px`);
  if (atEnd && here()[0] === "pane") window.scrollTo(0, document.body.scrollHeight);
  endButton();
}).observe(bar);
// The bar sits on the keyboard, not behind it: iOS Safari and Android
// Chrome keep the page's height under the keyboard and only show less of it.
function onKeyboard() {
  const vv = window.visualViewport;
  const zoomed = vv.scale > 1.01;
  const under = zoomed ? 0 : Math.max(0, innerHeight - vv.height - vv.offsetTop);
  bar.style.bottom = `${under}px`;
  // The head stays in sight too: the page pans under the keyboard.
  document.querySelector("header").style.top = `${zoomed ? 0 : vv.offsetTop}px`;
  bar.classList.toggle("on-keyboard", under > 0);
  document.body.style.setProperty("--kb", `${under}px`);
  document.body.style.setProperty("--seen-h", `${zoomed ? innerHeight : vv.height}px`);
}
window.visualViewport?.addEventListener("resize", onKeyboard);
window.visualViewport?.addEventListener("scroll", onKeyboard);

function endButton() {
  const below = document.body.scrollHeight - window.innerHeight - window.scrollY;
  toEnd.hidden = here()[0] !== "pane" || below < 120;
  saidBar(below);
}

// The user's words in a pane's output, oldest first, each by a range on
// its first letter: the last one scrolled under the head shows on top
// (#said) while the page is scrolled up from its end, as Claude Code does.
let saids = [];
let saidKey = "";
const saidEl = document.getElementById("said");
const headEl = document.querySelector("header");

// Each of `said` (what the transcript says the user wrote) is found by its
// first words, newest first, each before the one found after it.
function findSaid(said) {
  saids = [];
  const term = main.querySelector(".term");
  if (!term || !said.length) return;
  const nodes = [];
  let all = "";
  const walk = document.createTreeWalker(term, NodeFilter.SHOW_TEXT);
  for (let n = walk.nextNode(); n; n = walk.nextNode()) { nodes.push([n, all.length]); all += n.data; }
  let bound = all.length;
  for (const text of [...said].reverse()) {
    if (bound < 0) break;
    // A TUI wraps and pads lines: only the first words of its first line.
    const first = text.split("\n").map((l) => l.trim()).find(Boolean) || "";
    const needle = first.split("  ")[0].slice(0, 24);
    if (needle.length < 3) continue;
    const at = all.lastIndexOf(needle, bound);
    if (at < 0) continue;
    bound = at - 1;
    const [node, from] = nodes.findLast(([, from]) => from <= at);
    const range = document.createRange();
    range.setStart(node, at - from);
    range.setEnd(node, Math.min(node.data.length, at - from + 1));
    saids.unshift({ range, text: text.trim() });
  }
}

function saidBar(below) {
  const head = headEl.getBoundingClientRect().bottom;
  let last = null;
  let next = null;
  if (here()[0] === "pane" && below >= 120) {
    for (const s of saids) {
      const top = s.range.getBoundingClientRect().top;
      if (top >= head) { next = top; break; }
      last = s;
    }
  }
  saidEl.said = last;
  if (last) {
    saidEl.style.top = `${head}px`;
    if (saidEl.textContent !== last.text) saidEl.textContent = last.text;
    saidEl.hidden = false;
  }
  // Not over the user's next words, starting under it.
  saidEl.hidden = !last || (next !== null && next < saidEl.getBoundingClientRect().bottom);
}
saidEl.addEventListener("click", () => {
  const s = saidEl.said;
  if (!s) return;
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const head = headEl.getBoundingClientRect().bottom;
  const top = s.range.getBoundingClientRect().top + window.scrollY - head - 8;
  window.scrollTo({ top, behavior: still ? "auto" : "smooth" });
});
toEnd.addEventListener("click", () => {
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  window.scrollTo({ top: document.body.scrollHeight, behavior: still ? "auto" : "smooth" });
});
window.addEventListener("scroll", endButton, { passive: true });

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
  saids = [];
  saidKey = "";
  render(true);
});
document.addEventListener("visibilitychange", () => { if (!document.hidden) render(false); });
render(true);
