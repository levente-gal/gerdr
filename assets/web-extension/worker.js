// gerdr's end of the web pages it shows (see gerdr's `web_control`).
//
// gerdr opens each page at open.html?pane=<pane>&url=<page>: the tab is
// then known as that pane's, and the page goes on to its own (open.js), in
// place of itself: going back never lands on it. gerdr is told where
// each pane's tab is whenever it changes, whether it can go back or
// forward, and when it closes, over the native messaging host; it can send
// a pane's tab to another page, back, forward, or load it again, and keep
// T3 Code's app in it to the projects under some folders (see `scopeT3`).

const HOST = "dev.gerdr.web";
const BOOT = chrome.runtime.getURL("open.html");

// The pane of each tab gerdr opened, by the tab's id. Kept in the
// session's storage too: the worker comes and goes.
let panes = {};
// The folders whose T3 Code projects each tab's app shows, by tab id.
let scopes = {};
const loaded = chrome.storage.session.get(["panes", "scopes"]).then((stored) => {
  panes = stored.panes || {};
  scopes = stored.scopes || {};
});

function save() {
  return chrome.storage.session.set({ panes, scopes });
}

let port = null;

function send(message) {
  if (port) port.postMessage(message);
}

// Run `func` in `tab`'s page; what it gives, or `fallback` where the page
// can't be reached (the browser's own pages, error pages).
async function inPage(tab, func, fallback) {
  try {
    // Not waiting for the page to load: gerdr shows the pane once told.
    const [frame] = await chrome.scripting.executeScript({
      target: { tabId: tab },
      func,
      injectImmediately: true,
    });
    return frame.result;
  } catch {
    return fallback;
  }
}

// Whether the page can go back and forward. The Navigation API sees only
// the entries of the page's own origin: with others in the history too, a
// way it can't see is not known (null).
function pageHistory() {
  if (!window.navigation) return [null, null];
  const seen = navigation.entries().length === history.length;
  const or = (can) => can || (seen ? false : null);
  return [or(navigation.canGoBack), or(navigation.canGoForward)];
}

// Tell gerdr where `tab` is, if it is a pane's.
async function report(tab) {
  const pane = panes[tab.id];
  if (pane === undefined || !tab.url || tab.url.startsWith(BOOT)) return;
  const [back, forward] = await inPage(tab.id, pageHistory, [null, null]);
  send({ type: "page", pane, url: tab.url, title: tab.title || "", back, forward });
}

// Take `tab` (at `url`) as its pane's, if gerdr opened it; whether it is.
async function boot(tab, url = tab.url || tab.pendingUrl) {
  if (!url || !url.startsWith(BOOT)) return false;
  if (panes[tab.id] !== undefined) return true;
  const pane = new URL(url).searchParams.get("pane");
  if (!pane) return false;
  panes[tab.id] = pane;
  await save();
  return true;
}

// A browser started for a pane opens its page before its extensions are
// there at times, and refuses the boot page then: it is loaded again once
// the worker runs, once. A boot page that did load is one of the
// extension's pages in the tab. A refused one tells of no change when it
// is done loading: one still loading is looked at again a while later.
const retried = new Set();
async function retry(tab, looks = 5) {
  if (!tab.url || !tab.url.startsWith(BOOT) || retried.has(tab.id)) return;
  if (tab.status !== "complete") {
    if (looks > 0) {
      const again = (tab) => retry(tab, looks - 1);
      setTimeout(() => chrome.tabs.get(tab.id).then(again, () => {}), 1000);
    }
    return;
  }
  const pages = await chrome.runtime.getContexts({ contextTypes: ["TAB"], tabIds: [tab.id] });
  if (pages.length) return;
  retried.add(tab.id);
  await chrome.tabs.reload(tab.id);
}

// open.html asks to be taken before it goes on to its page.
chrome.runtime.onMessage.addListener((message, sender, reply) => {
  if (message.type !== "boot" || !sender.tab) return false;
  loaded.then(() => boot(sender.tab, sender.url)).then(reply, () => reply(false));
  return true;
});

// The id of `pane`'s tab, if the extension has it.
function tabOf(pane) {
  const tab = Object.keys(panes).find((id) => panes[id] === pane);
  return tab === undefined ? undefined : Number(tab);
}

// In T3 Code's app: show the projects under `dirs` only, by its sidebar's
// own project filter. That filter is kept in the page's localStorage by
// project key, which the app works out from each repository's remote: the
// keys are read from the app's own project groups, found in React's tree
// by their fields. The app reads the filter as it starts, so the page
// loads again once it changed, once per page session. Runs in the page.
//
// None of that is T3 Code's API: a version that changes it is told of,
// as null where the filter is set, else `{ reason, detail }` (see
// gerdr's `t3_unscoped`). The page loading again ends it unanswered.
function scopeT3(dirs) {
  if (window.__gerdrScope === JSON.stringify(dirs)) return null;
  window.__gerdrScope = JSON.stringify(dirs);
  const trim = (path) => path.replace(/\/+$/, "") || "/";
  const want = dirs.map(trim);
  const inside = (path, dir) => path === dir || path.startsWith(dir === "/" ? dir : dir + "/");
  const isGroup = (g) =>
    g && typeof g === "object" && typeof g.projectKey === "string" && Array.isArray(g.memberProjects);
  const isGroups = (v) => Array.isArray(v) && v.length > 0 && v.every(isGroup);
  // The React tree's root fiber, if the page has one.
  function root() {
    const host = document.getElementById("root") || document.body?.firstElementChild;
    const key = host && Object.keys(host).find((k) => k.startsWith("__reactContainer$"));
    return key ? host[key] : null;
  }
  // Every list of project groups in the tree: merged by key (one may be
  // the sidebar's, filtered already), and each list's keys.
  function groups() {
    const top = root();
    if (!top) return null;
    const found = new Map();
    const lists = [];
    const take = (v) => {
      if (!isGroups(v)) return;
      for (const g of v) found.set(g.projectKey, g);
      lists.push(v.filter((g) => !g.isScratch).map((g) => g.projectKey).sort());
    };
    const stack = [top];
    for (let seen = 0; stack.length && seen < 500000; seen++) {
      const fiber = stack.pop();
      const props = fiber.memoizedProps;
      if (props && typeof props === "object") for (const k in props) take(props[k]);
      for (let h = fiber.memoizedState, i = 0; h && typeof h === "object" && i < 200; h = h.next, i++) {
        take(h.memoizedState);
        if (Array.isArray(h.memoizedState)) take(h.memoizedState[0]);
      }
      for (const slot of fiber.updateQueue?.memoCache?.data || []) {
        if (Array.isArray(slot)) for (const v of slot) take(v);
      }
      if (fiber.sibling) stack.push(fiber.sibling);
      if (fiber.child) stack.push(fiber.child);
    }
    return found.size ? { all: [...found.values()], lists } : null;
  }
  const roots = (g) => g.memberProjects.map((p) => p.workspaceRoot).filter((r) => typeof r === "string").map(trim);
  // The groups with a project in one of the folders; else those holding
  // one of them, the nearest.
  function keys(all) {
    const real = all.filter((g) => !g.isScratch);
    const under = real.filter((g) => roots(g).some((r) => want.some((d) => inside(r, d))));
    if (under.length) return under.map((g) => g.projectKey).sort();
    let best = [], depth = -1;
    for (const g of real) {
      for (const r of roots(g)) {
        if (!want.some((d) => inside(d, r)) || r.length < depth) continue;
        if (r.length > depth) (best = []), (depth = r.length);
        if (!best.includes(g.projectKey)) best.push(g.projectKey);
      }
    }
    return best.sort();
  }
  const name =
    Object.keys(localStorage)
      .filter((k) => /^t3code:ui-state:v\d+$/.test(k))
      .sort((a, b) => Number(b.split(":v")[1]) - Number(a.split(":v")[1]))[0] || "t3code:ui-state:v1";
  const read = () => {
    try {
      return JSON.parse(localStorage.getItem(name) || "{}") || {};
    } catch {
      return {};
    }
  };
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  const mark = "gerdr-t3-scope";
  // How long the app may take to start, and to show the projects; how
  // long ago a filter set and loaded again counts as just set.
  const LOOKS = 120, EVERY = 500, FRESH = 15000;
  return new Promise((done) => {
    let looks = 0;
    // The page without its hash: a sign-in link's token is there.
    const fail = (reason, detail) =>
      done({ reason, detail: `${detail} (page ${location.origin}${location.pathname}, folders ${want.join(", ")}, storage ${name})` });
    // A change that breaks the reading itself is told too.
    function look() {
      try {
        step();
      } catch (err) {
        fail("error", String(err?.stack || err).slice(0, 800));
      }
    }
    function step() {
      // Signing in: the app and its projects come after, however long it
      // takes.
      if (location.pathname.startsWith("/pair")) return void setTimeout(look, EVERY);
      const seen = groups();
      const wanted = seen ? keys(seen.all) : [];
      if (!wanted.length) {
        // Not loaded yet, or the project not added yet.
        if (++looks < LOOKS) return void setTimeout(look, EVERY);
        if (!root()) return fail("no-react", "no React root (__reactContainer$) on #root");
        if (!seen) return fail("no-groups", "no arrays of {projectKey, memberProjects} in React's tree");
        const all = seen.all.filter((g) => !g.isScratch);
        if (all.length && all.every((g) => !roots(g).length)) {
          return fail("no-roots", `groups have no memberProjects[].workspaceRoot: ${JSON.stringify(all[0]).slice(0, 400)}`);
        }
        return fail("no-match", `no project at or around the folders; roots: ${all.flatMap(roots).join(", ") || "none"}`);
      }
      const state = read();
      const now = [...(state.sidebarProjectScopeKeys || [])].sort();
      if (same(now, wanted)) {
        // Set: the app shows those projects only, unless it no longer
        // reads the filter from there.
        if (seen.all.filter((g) => !g.isScratch).length === wanted.length) return done(null);
        if (seen.lists.some((l) => same(l, wanted))) return done(null);
        if (++looks < LOOKS) return void setTimeout(look, EVERY);
        return fail("ignored", `filter ${JSON.stringify(wanted)} stored, but no project list shows only those; lists: ${JSON.stringify(seen.lists).slice(0, 400)}`);
      }
      let set = {};
      try {
        set = JSON.parse(sessionStorage.getItem(mark) || "{}") || {};
      } catch {}
      if (same(set.keys, wanted)) {
        // Set before: changed since by the app as it started, or by hand.
        if (Date.now() - (set.at || 0) < FRESH) {
          return fail("overwritten", `filter ${JSON.stringify(wanted)} stored and the page loaded again, but it reads ${JSON.stringify(now)}`);
        }
        return done(null);
      }
      sessionStorage.setItem(mark, JSON.stringify({ keys: wanted, at: Date.now() }));
      state.sidebarProjectScopeKeys = wanted;
      state.sidebarProjectScopeKey = wanted.length === 1 ? wanted[0] : null;
      localStorage.setItem(name, JSON.stringify(state));
      location.reload();
    }
    look();
  });
}

// Apply `tab`'s folders to its T3 Code app; a version that doesn't take
// them is told to gerdr.
async function scope(tab) {
  const dirs = scopes[tab];
  if (!dirs) return;
  let failed;
  try {
    const [frame] = await chrome.scripting.executeScript({ target: { tabId: tab }, world: "MAIN", func: scopeT3, args: [dirs] });
    failed = frame?.result;
  } catch {
    // Not a page the extension reaches, or one that loaded again.
    return;
  }
  const pane = panes[tab];
  if (failed && pane !== undefined) send({ type: "unscoped", pane, reason: failed.reason, detail: failed.detail });
}

async function command(message) {
  await loaded;
  const tab = tabOf(message.pane);
  if (tab === undefined) return;
  try {
    switch (message.type) {
      case "navigate":
        await chrome.tabs.update(tab, { url: message.url });
        break;
      // From the page, as its own links would: the browser's goBack skips
      // the entries a page made without a click.
      case "back":
        if (!(await inPage(tab, () => (history.back(), true), false))) {
          await chrome.tabs.goBack(tab);
        }
        break;
      case "forward":
        if (!(await inPage(tab, () => (history.forward(), true), false))) {
          await chrome.tabs.goForward(tab);
        }
        break;
      case "reload":
        await chrome.tabs.reload(tab);
        break;
      case "scope":
        scopes[tab] = message.dirs;
        await save();
        await scope(tab);
        break;
    }
  } catch {
    // Nothing that way: gerdr is told where the tab is anyway.
  }
}

// Connect to gerdr, and tell it where the panes' tabs are. A lost
// connection is tried again a while later.
async function connect() {
  await loaded;
  port = chrome.runtime.connectNative(HOST);
  port.onMessage.addListener(command);
  port.onDisconnect.addListener(() => {
    port = null;
    setTimeout(connect, 5000);
  });
  const tabs = await chrome.tabs.query({});
  const open = new Set(tabs.map((tab) => String(tab.id)));
  for (const id of Object.keys(panes)) {
    if (!open.has(id)) delete panes[id];
  }
  for (const id of Object.keys(scopes)) {
    if (!open.has(id)) delete scopes[id];
  }
  await save();
  for (const tab of tabs) {
    await boot(tab);
    await retry(tab);
    await report(tab);
  }
}

chrome.tabs.onCreated.addListener(async (tab) => {
  await loaded;
  await boot(tab);
});

chrome.tabs.onUpdated.addListener(async (id, change, tab) => {
  await loaded;
  await boot(tab);
  if (change.status === "complete") await retry(tab);
  if (change.status === "complete") await scope(id);
  // A load done is when the page's history can be asked for sure.
  if (change.url || change.title || change.status === "complete") await report(tab);
});

chrome.tabs.onRemoved.addListener(async (id) => {
  await loaded;
  const pane = panes[id];
  delete scopes[id];
  if (pane === undefined) return;
  delete panes[id];
  await save();
  send({ type: "closed", pane });
});

// The worker starts with the browser, not only once a tab changes.
chrome.runtime.onStartup.addListener(() => {});

connect();
