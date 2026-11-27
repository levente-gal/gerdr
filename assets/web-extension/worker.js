// gerdr's end of the web pages it shows (see gerdr's `web_control`).
//
// gerdr opens each page at open.html?pane=<code>&url=<page>: the tab is
// then known as that pane's, by the code until gerdr tells its pane's name
// for it, and the page goes on to its own (open.js), in
// place of itself: going back never lands on it. gerdr is told where
// each pane's tab is whenever it changes, whether it can go back or
// forward, and when it closes, over the native messaging host; it can send
// a pane's tab to another page (in a single-page app by the app's own
// router, see `goInApp`), back, forward, or load it again, send back a
// screenshot of it, or of an area of it the user selects there (see
// `pick`), keep T3 Code's app in it to the projects under some folders, and start a thread
// there in one of them (see `inT3`). A pane's tab can be the extension's
// preview page too, which shows a markdown file gerdr sends it.

const HOST = "dev.gerdr.web";
const BOOT = chrome.runtime.getURL("open.html");
const PREVIEW = chrome.runtime.getURL("preview.html");

// The pane of each tab gerdr opened, by the tab's id. Kept in the
// session's storage too: the worker comes and goes.
let panes = {};
// The folders whose T3 Code projects each tab's app shows, and the
// app's origin, by tab id: `{ origin, dirs }`.
let scopes = {};
// The new threads asked of each tab's T3 Code app, by tab id, until
// started: `{ origin, dirs, at }` (see `startThread`).
let threads = {};
const loaded = chrome.storage.session.get(["panes", "scopes", "threads"]).then((stored) => {
  panes = stored.panes || {};
  scopes = stored.scopes || {};
  threads = stored.threads || {};
});

function save() {
  return chrome.storage.session.set({ panes, scopes, threads });
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

// What a preview page asks of gerdr, as told to it: that it loaded,
// to open a link, to review its file, to put its review's caret on a line,
// that it scrolled to a line.
const FROM_PREVIEW = {
  "preview-ready": "ready",
  "preview-open": "open",
  "preview-line": "line",
  "preview-scroll": "scroll",
};

// open.html asks to be taken before it goes on to its page. A pane's
// preview page (see preview.js) tells gerdr what it asks; only its own
// tab's.
chrome.runtime.onMessage.addListener((message, sender, reply) => {
  if (!sender.tab || !message) return false;
  const asked = FROM_PREVIEW[message.type];
  if (asked) {
    if (!sender.url || !sender.url.startsWith(PREVIEW)) return false;
    loaded.then(() => {
      const pane = panes[sender.tab.id];
      if (pane === undefined) return;
      if (asked === "open") {
        if (typeof message.url === "string") send({ type: "open", pane, url: message.url });
      } else if (asked === "line") {
        if (Number.isInteger(message.line) && message.line > 0) send({ type: "line", pane, line: message.line });
      } else if (asked === "scroll") {
        if (Number.isFinite(message.line)) send({ type: "scroll", pane, line: message.line });
      } else {
        send({ type: asked, pane });
      }
    });
    return false;
  }
  if (message.type !== "boot") return false;
  loaded.then(() => boot(sender.tab, sender.url)).then(reply, () => reply(false));
  return true;
});

// The id of `pane`'s tab, if the extension has it.
function tabOf(pane) {
  const tab = Object.keys(panes).find((id) => panes[id] === pane);
  return tab === undefined ? undefined : Number(tab);
}

// In a single-page app's page, go to `href` (a path of the page's own
// origin) by the app's router, found in React's tree as the `router` of
// its provider (TanStack Router's `navigate`): the page isn't loaded again.
// Whether it went: not at a sign-in page, which a load gets past, nor
// where no router shows within a few seconds. Runs in the page, in its
// own world.
function routeTo(href) {
  const EVERY = 250;
  const LOOKS = 20;
  function router() {
    const root = document.getElementById("root");
    const key = root && Object.keys(root).find((k) => k.startsWith("__reactContainer$"));
    const stack = key && root[key] ? [root[key]] : [];
    let seen = 0;
    while (stack.length && seen++ < 50000) {
      const fiber = stack.pop();
      const found = fiber.memoizedProps?.router;
      if (found && typeof found.navigate === "function" && found.history) return found;
      if (fiber.sibling) stack.push(fiber.sibling);
      if (fiber.child) stack.push(fiber.child);
    }
    return null;
  }
  return new Promise((done) => {
    let looks = 0;
    function look() {
      if (location.pathname.startsWith("/pair")) return done(false);
      let found = null;
      try {
        found = router();
      } catch {}
      if (!found) return ++looks < LOOKS ? void setTimeout(look, EVERY) : done(false);
      try {
        found.navigate({ href });
        done(true);
      } catch {
        done(false);
      }
    }
    look();
  });
}

// Send `tab` to `url` by its app's router when it shows that site already;
// whether it went.
async function goInApp(tab, url) {
  try {
    const target = new URL(url);
    const now = (await chrome.tabs.get(tab)).url;
    if (!now || new URL(now).origin !== target.origin) return false;
    const href = target.pathname + target.search + target.hash;
    const [frame] = await chrome.scripting.executeScript({ target: { tabId: tab }, world: "MAIN", func: routeTo, args: [href] });
    return frame?.result === true;
  } catch {
    // Not a page the extension reaches, or one that loaded meanwhile.
    return false;
  }
}

// In T3 Code's app, for the folders `dirs`: `job` "scope" shows their
// projects only, "thread" starts a new thread in one of them. Runs in the
// page, in its own world.
//
// Scope: by the app's sidebar's own project filter. That filter is kept
// in the page's localStorage by project key, which the app works out from
// each repository's remote: the keys are read from the app's own project
// groups, found in React's tree by their fields. The filter is set in the
// app's own ui-state store (a zustand store in its `uiStateStore` chunk,
// imported again from the page's module preloads: the same module), which
// shows it at once and keeps it. Where that store isn't found, it is set
// in localStorage, which the app reads as it starts: the page loads again
// once then, once per page session.
//
// Thread: by the app's own `handleNewThread`, which its new-thread
// buttons call, found in React's tree next to the fields it comes with,
// given the project: the one at a folder (the workspace's own), else the
// one the shown thread or draft is of when it is in the folders, else the
// app's default when it is in them, else the first. It opens a draft of that project. With a
// `prompt` (a feature stage's) the project is the one at a folder and no
// other, or the agent would run in another checkout; the draft says the
// prompt, on the project's own folder, and the user sends it. The draft
// shown already saying it is left as it is. With a `model`
// ({instanceId, model}) the draft is set to it by the app's own composer
// draft store, as its model picker does, found among the modules the page
// loaded; one not set is told as `{ ok: true, model: { reason, detail } }`.
//
// None of that is T3 Code's API: a version that changes it is told of,
// as `{ ok: true }` where it worked, else `{ reason, detail }` (see
// gerdr's `t3_unscoped` and `t3_no_thread`). The page loading again ends
// it unanswered, as does a page asked to scope to the same folders
// already (undefined).
function inT3(job, dirs, prompt, model) {
  if (job === "scope") {
    if (window.__gerdrScope === JSON.stringify(dirs)) return undefined;
    window.__gerdrScope = JSON.stringify(dirs);
  }
  const trim = (path) => path.replace(/\/+$/, "") || "/";
  const want = dirs.map(trim);
  // A feature's folder (`<root>/.features/<slug>`, gerdr's FEATURES_DIR)
  // holds its worktrees: `git init` there would make it a repository over
  // theirs, so the app's "Initialize Git" button is hidden while it shows,
  // with its panel section when it is that section's only button.
  if (job === "scope" && want.length && want.every((d) => /\/\.features\/[^/]+$/.test(d)) && !window.__gerdrNoGitInit) {
    window.__gerdrNoGitInit = true;
    let queued = false;
    const hide = () => {
      queued = false;
      for (const button of document.querySelectorAll("button")) {
        if (!/^(Initialize Git|Initializing\.\.\.)$/.test(button.textContent.trim())) continue;
        const section = button.closest("section");
        (section?.querySelectorAll("button").length === 1 ? section : button).style.display = "none";
      }
    };
    new MutationObserver(() => {
      if (!queued) (queued = true), requestAnimationFrame(hide);
    }).observe(document.documentElement, { childList: true, subtree: true });
    hide();
  }
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
  // Give `take` every value React's tree holds in its props, hooks and
  // memo caches; whether the page has a tree.
  function walk(take) {
    const top = root();
    if (!top) return false;
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
    return true;
  }
  // Every list of project groups in the tree: merged by key (one may be
  // the sidebar's, filtered already), and each list's keys.
  function groups() {
    const found = new Map();
    const lists = [];
    walk((v) => {
      if (!isGroups(v)) return;
      for (const g of v) found.set(g.projectKey, g);
      lists.push(v.filter((g) => !g.isScratch).map((g) => g.projectKey).sort());
    });
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
  // How long the app may take to start, and to show the projects.
  const LOOKS = 120, EVERY = 500;
  // The first store of the app's that `isStore` takes, from the modules
  // the page loaded already (those at `named` first): importing one again
  // gives the same module, not a new one.
  async function moduleStore(isStore, named) {
    const tags = document.querySelectorAll('script[type="module"][src], link[rel="modulepreload"][href]');
    const loaded = performance.getEntriesByType("resource").map((e) => e.name);
    const urls = [...new Set([...[...tags].map((t) => t.src || t.href), ...loaded])]
      .filter((url) => /\.js$/.test(url.split(/[?#]/)[0]) && new URL(url).origin === location.origin)
      .sort((a, b) => named.test(b) - named.test(a));
    for (const url of urls) {
      try {
        const found = Object.values(await import(url)).find(isStore);
        if (found) return found;
      } catch {}
    }
    return null;
  }
  // The page without its hash: a sign-in link's token is there.
  const failing = (done) => (reason, detail) =>
    done({ reason, detail: `${detail} (page ${location.origin}${location.pathname}, folders ${want.join(", ")})` });
  if (job === "thread") {
    const isProject = (p) =>
      p && typeof p.workspaceRoot === "string" && p.id != null && p.environmentId != null;
    const ref = (p) => ({ environmentId: p.environmentId, projectId: p.id });
    const same = (a, p) => a && a.environmentId === p.environmentId && a.projectId === p.id;
    const isDrafts = (v) => {
      const state = v && typeof v.getState === "function" && v.getState();
      return state && typeof state.setModelSelection === "function" && typeof state.getComposerDraft === "function";
    };
    // Set draft `draftId` to `model`; why it isn't, if so.
    async function setModel(draftId) {
      const drafts = await moduleStore(isDrafts, /\/(utils|main)-[^/]*$/);
      if (!drafts) return { reason: "no-model", detail: "no composer draft store with setModelSelection among the page's modules" };
      drafts.getState().setModelSelection(draftId, model, { explicit: true, replaceOptions: true });
      const draft = drafts.getState().getComposerDraft(draftId);
      const set = draft?.modelSelectionByProvider?.[model.instanceId];
      if (draft?.activeProvider === model.instanceId && set?.model === model.model) return null;
      return { reason: "no-model", detail: `draft ${draftId} reads ${JSON.stringify(set || null)} on ${draft?.activeProvider}, not ${JSON.stringify(model)}` };
    }
    return new Promise((resolve) => {
      // The draft made or shown is set to the model, if one.
      const done = (result, draftId) => {
        if (!result.ok || !model) return resolve(result);
        if (!draftId) return resolve({ ok: true, model: { reason: "no-model", detail: "no draft id from the app" } });
        setModel(draftId).then(
          (unset) => resolve(unset ? { ok: true, model: unset } : result),
          (err) => resolve({ ok: true, model: { reason: "no-model", detail: String(err?.stack || err).slice(0, 800) } }),
        );
      };
      const fail = failing(done);
      let looks = 0, refused = 0;
      function look() {
        try {
          step();
        } catch (err) {
          fail("error", String(err?.stack || err).slice(0, 800));
        }
      }
      function step() {
        if (location.pathname.startsWith("/pair")) return void setTimeout(look, EVERY);
        let app = null;
        const projects = new Map();
        const tree = walk((v) => {
          if (!v || typeof v !== "object") return;
          if (!app && typeof v.handleNewThread === "function" && "defaultProjectRef" in v) app = v;
          if (!isGroups(v)) return;
          for (const g of v) {
            if (g.isScratch) continue;
            for (const p of g.memberProjects) if (isProject(p)) projects.set(`${p.environmentId}\n${p.id}`, p);
          }
        });
        const all = [...projects.values()];
        let picks = prompt
          ? all.filter((p) => want.includes(trim(p.workspaceRoot)))
          : all.filter((p) => want.some((d) => inside(trim(p.workspaceRoot), d)));
        if (!picks.length && !prompt) {
          // A folder inside a project: the nearest project holding it.
          const holding = all.filter((p) => want.some((d) => inside(d, trim(p.workspaceRoot))));
          const depth = Math.max(-1, ...holding.map((p) => trim(p.workspaceRoot).length));
          picks = holding.filter((p) => trim(p.workspaceRoot).length === depth);
        }
        if (!app || !picks.length) {
          // Not loaded yet, or the project not added yet.
          if (++looks < LOOKS) return void setTimeout(look, EVERY);
          if (!tree) return fail("no-react", "no React root (__reactContainer$) on #root");
          if (!app) return fail("no-handler", "no object with handleNewThread and defaultProjectRef in React's tree");
          return fail("no-match", `no project at or around the folders; roots: ${all.map((p) => p.workspaceRoot).join(", ") || "none"}`);
        }
        picks.sort((a, b) => a.workspaceRoot.localeCompare(b.workspaceRoot));
        const shown = [app.activeThread, app.activeDraftThread];
        const project =
          picks.find((p) => want.includes(trim(p.workspaceRoot))) ||
          picks.find((p) => shown.some((t) => same(t, p))) ||
          picks.find((p) => same(app.defaultProjectRef, p)) ||
          picks[0];
        if (prompt) {
          const first = prompt.trim().split("\n")[0].trim();
          const shows = !app.activeThread && same(app.activeDraftThread, project);
          const text = document.querySelector('[contenteditable="true"]')?.textContent || "";
          if (shows && first && text.trim().startsWith(first)) return done({ ok: true }, app.routeDraftId);
        }
        const options = prompt ? { prompt, envMode: "local", branch: null, worktreePath: null } : undefined;
        Promise.resolve(app.handleNewThread(ref(project), options)).then(
          (made) => {
            if (made) return done({ ok: true }, made.draftId);
            // The app gives none when the page went elsewhere meanwhile,
            // as its home page does as it opens its own draft.
            if (++refused < 5) return void setTimeout(look, EVERY);
            fail("refused", `handleNewThread(${JSON.stringify(ref(project))}) gave ${made}`);
          },
          (err) => fail("error", String(err?.stack || err).slice(0, 800)),
        );
      }
      look();
    });
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
  const isStore = (v) =>
    v && typeof v.getState === "function" && typeof v.getState()?.setSidebarProjectScopeKeys === "function";
  const mark = "gerdr-t3-scope";
  // How long ago a filter set and loaded again counts as just set.
  const FRESH = 15000;
  return new Promise((done) => {
    let looks = 0;
    // The app's store, once looked for; whether the filter was set in it.
    let store = null, setLive = false;
    const fail = (reason, detail) =>
      failing(done)(reason, `${detail}, storage ${store ? "ui-state store" : name}`);
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
        // An app without projects shows no groups either: gerdr tells that
        // apart by T3 Code's database.
        if (!seen) return fail("no-groups", "no arrays of {projectKey, memberProjects} in React's tree");
        const all = seen.all.filter((g) => !g.isScratch);
        if (all.length && all.every((g) => !roots(g).length)) {
          return fail("no-roots", `groups have no memberProjects[].workspaceRoot: ${JSON.stringify(all[0]).slice(0, 400)}`);
        }
        return fail("no-match", `no project at or around the folders; roots: ${all.flatMap(roots).join(", ") || "none"}`);
      }
      const state = read();
      const held = store ? store.getState().sidebarProjectScopeKeys : state.sidebarProjectScopeKeys;
      const now = [...(held || [])].sort();
      if (same(now, wanted)) {
        // Set: the app shows those projects only, unless it no longer
        // reads the filter from there.
        if (seen.all.filter((g) => !g.isScratch).length === wanted.length) return done({ ok: true });
        if (seen.lists.some((l) => same(l, wanted))) return done({ ok: true });
        if (++looks < LOOKS) return void setTimeout(look, EVERY);
        return fail("ignored", `filter ${JSON.stringify(wanted)} stored, but no project list shows only those; lists: ${JSON.stringify(seen.lists).slice(0, 400)}`);
      }
      if (store) {
        // Set once: the next look sees whether the app shows it.
        if (setLive) {
          return fail("overwritten", `filter ${JSON.stringify(wanted)} set in the app's store, but it reads ${JSON.stringify(now)}`);
        }
        setLive = true;
        store.getState().setSidebarProjectScopeKeys(wanted);
        return void setTimeout(look, EVERY);
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
        return done({ ok: true });
      }
      sessionStorage.setItem(mark, JSON.stringify({ keys: wanted, at: Date.now() }));
      state.sidebarProjectScopeKeys = wanted;
      state.sidebarProjectScopeKey = wanted.length === 1 ? wanted[0] : null;
      localStorage.setItem(name, JSON.stringify(state));
      location.reload();
    }
    // The app's modules are there once it started: looked for then.
    function start() {
      // Signing in first, however long it takes.
      if (location.pathname.startsWith("/pair")) return void setTimeout(start, EVERY);
      if (!groups()) {
        if (++looks < LOOKS) return void setTimeout(start, EVERY);
        return look();
      }
      looks = 0;
      moduleStore(isStore, /\/uiStateStore-[^/]*$/).then(
        (found) => ((store = found), look()),
        () => look(),
      );
    }
    start();
  });
}

// The tabs whose T3 Code app is being scoped, or starting a thread, now.
const scoping = new Set();
const starting = new Set();

// Apply `tab`'s folders to its T3 Code app, while the tab is at that
// app's origin; whether it took them is told to gerdr.
async function scope(tab) {
  const scoped = scopes[tab];
  if (!scoped?.origin) return;
  let result;
  scoping.add(tab);
  try {
    const { url } = await chrome.tabs.get(tab);
    // At another site since: not T3 Code's app.
    if (!url || new URL(url).origin !== scoped.origin) return;
    const [frame] = await chrome.scripting.executeScript({ target: { tabId: tab }, world: "MAIN", func: inT3, args: ["scope", scoped.dirs] });
    result = frame?.result;
  } catch {
    // Not a page the extension reaches, or one that loaded again.
    return;
  } finally {
    scoping.delete(tab);
  }
  const pane = panes[tab];
  if (pane === undefined) return;
  if (result?.ok) send({ type: "scoped", pane });
  else if (result?.reason) send({ type: "unscoped", pane, reason: result.reason, detail: result.detail });
}

// How long a new thread asked of a tab waits for its app.
const THREAD_WAIT = 120000;

// Start the new thread asked of `tab`'s T3 Code app, once the tab is done
// loading at the app and its folders are applied (a page loaded again for
// them would end it); one that failed is told to gerdr.
async function startThread(tab) {
  const asked = threads[tab];
  if (!asked || scoping.has(tab) || starting.has(tab)) return;
  if (Date.now() - asked.at > THREAD_WAIT) {
    delete threads[tab];
    await save();
    return;
  }
  let result;
  starting.add(tab);
  try {
    const { url, status } = await chrome.tabs.get(tab);
    // Still loading, or at another page: tried again at its next load.
    if (status !== "complete" || !url || new URL(url).origin !== asked.origin) return;
    // Absent is null: an undefined argument fails the injection.
    const args = ["thread", asked.dirs, asked.prompt ?? null, asked.model ?? null];
    const [frame] = await chrome.scripting.executeScript({ target: { tabId: tab }, world: "MAIN", func: inT3, args });
    result = frame?.result;
  } catch {
    return;
  } finally {
    starting.delete(tab);
  }
  // The page loaded again meanwhile: tried again at its next load.
  if (!result) return;
  delete threads[tab];
  await save();
  const pane = panes[tab];
  const failed = result.reason ? result : result.model;
  if (pane !== undefined && failed) {
    send({ type: "nothread", pane, reason: failed.reason, detail: failed.detail });
  }
}

// Let the user select an area of the page: a box dragged over it, or one
// element clicked. What it gives: `{ url, rect, elements, view }`, the
// elements being the page's main ones in the area (see gerdr's
// `manual::Picked`), every number whole CSS pixels of the viewport; or
// `{ cancelled: true }` for Esc, or another select or unselect (they call
// `window.__gerdrPick`). Runs in the page, in its own world: React's
// fibers are on the page's own DOM nodes.
// shortcut: viewport only, no scrolling while selecting; upgrade if long pages need it
function pick() {
  window.__gerdrPick?.();
  const MOST = 12;
  const area = (r) => ({ x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) });
  const shown = (el, r = el.getBoundingClientRect()) => {
    if (!r.width || !r.height) return false;
    const s = getComputedStyle(el);
    return s.display !== "none" && s.visibility !== "hidden";
  };
  // `#id` or `[data-testid]` ends the path; else `tag.class:nth-of-type(n)`
  // steps. Classes a build made up (`a__b`, long hashes) aren't taken.
  function selector(el) {
    const steps = [];
    for (let e = el; e && steps.length < 5; e = e.parentElement) {
      if (e.id) {
        steps.unshift(`#${CSS.escape(e.id)}`);
        break;
      }
      const test = e.getAttribute("data-testid");
      if (test) {
        steps.unshift(`[data-testid="${CSS.escape(test)}"]`);
        break;
      }
      let step = CSS.escape(e.localName);
      const classes = [...e.classList].filter((c) => !c.includes("__") && c.length <= 40).slice(0, 3);
      for (const c of classes) step += `.${CSS.escape(c)}`;
      const same = e.parentElement ? [...e.parentElement.children].filter((s) => s.localName === e.localName) : [];
      if (same.length > 1) step += `:nth-of-type(${same.indexOf(e) + 1})`;
      steps.unshift(step);
    }
    return steps.join(" > ");
  }
  // A component's name: memo and forwardRef wrap theirs.
  function named(type) {
    if (typeof type === "function") return type.displayName || type.name || null;
    if (type && typeof type === "object") return type.displayName || named(type.type || type.render);
    return null;
  }
  // The React components `el` is in, innermost first, and where its JSX
  // (or the nearest one above with a place) is written: dev builds only.
  function react(el) {
    const key = Object.keys(el).find((k) => k.startsWith("__reactFiber$"));
    const component = [];
    let source = null;
    for (let f = key && el[key], i = 0; f && (component.length < 2 || !source) && i < 2000; f = f.return, i++) {
      const at = f._debugSource;
      if (!source && at?.fileName) source = `${at.fileName}:${at.lineNumber}`;
      const name = component.length < 2 && named(f.type);
      // A memo's fiber and its function's both name it.
      if (name && component[component.length - 1] !== name) component.push(name);
    }
    return { component, source };
  }
  function styles(el) {
    const s = getComputedStyle(el);
    const family = s.fontFamily.split(",")[0].trim().replace(/^["']|["']$/g, "");
    const all = {
      margin: s.margin,
      padding: s.padding,
      font: `${s.fontWeight} ${s.fontSize} ${family}`,
      color: s.color,
      background: s.backgroundColor === "rgba(0, 0, 0, 0)" ? "" : s.backgroundColor,
      gap: s.gap === "0px" ? "" : s.gap,
      radius: s.borderRadius === "0px" ? "" : s.borderRadius,
    };
    return Object.fromEntries(Object.entries(all).filter(([, v]) => v && v !== "normal"));
  }
  function describe(el) {
    const out = {
      tag: el.localName,
      selector: selector(el),
      text: (el.innerText ?? el.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 200),
      rect: area(el.getBoundingClientRect()),
    };
    const { component, source } = react(el);
    if (component.length) out.component = component;
    if (source) out.source = source;
    out.styles = styles(el);
    return out;
  }
  // The elements wholly in `box`, the outermost of them; else the smallest
  // one holding it.
  function inBox(box) {
    const inside = new Set();
    for (const el of document.body?.querySelectorAll("*") || []) {
      const r = el.getBoundingClientRect();
      const fits = r.left >= box.x && r.top >= box.y && r.right <= box.x + box.w && r.bottom <= box.y + box.h;
      if (fits && shown(el, r)) inside.add(el);
    }
    const outer = [...inside].filter((el) => {
      for (let p = el.parentElement; p; p = p.parentElement) if (inside.has(p)) return false;
      return true;
    });
    if (outer.length) return outer;
    let el = document.elementFromPoint(box.x + box.w / 2, box.y + box.h / 2);
    for (; el?.parentElement; el = el.parentElement) {
      const r = el.getBoundingClientRect();
      if (r.left <= box.x && r.top <= box.y && r.right >= box.x + box.w && r.bottom >= box.y + box.h) break;
    }
    return el ? [el] : [];
  }

  // Its own styles, each kept over the page's: the page's CSS reaches it
  // only through the host's, and the shadow root keeps out the rest.
  const host = document.createElement("div");
  host.style.cssText =
    "all:initial!important;position:fixed!important;inset:0!important;z-index:2147483647!important;cursor:crosshair!important";
  const shadow = host.attachShadow({ mode: "closed" });
  const style = document.createElement("style");
  style.textContent = `
    .bar { position: fixed; top: 8px; left: 50%; transform: translateX(-50%); padding: 6px 12px;
      border-radius: 6px; background: #1c1c1e; color: #fff; font: 13px/1.4 system-ui, sans-serif;
      white-space: nowrap; pointer-events: none; box-shadow: 0 2px 8px rgba(0, 0, 0, .3); }
    .box { position: fixed; display: none; outline: 2px solid #ff3b30; background: rgba(255, 59, 48, .08);
      pointer-events: none; }`;
  const bar = document.createElement("div");
  bar.className = "bar";
  bar.textContent = "Drag over an area or click an element · Esc cancels";
  const drawn = document.createElement("div");
  drawn.className = "box";
  shadow.append(style, bar, drawn);

  return new Promise((resolve) => {
    let start = null;
    const boxTo = (e) => {
      const [x, y] = start;
      return { x: Math.min(x, e.clientX), y: Math.min(y, e.clientY), w: Math.abs(e.clientX - x), h: Math.abs(e.clientY - y) };
    };
    function end() {
      host.remove();
      window.removeEventListener("keydown", key, true);
      if (window.__gerdrPick === cancel) delete window.__gerdrPick;
    }
    function cancel() {
      end();
      resolve({ cancelled: true });
    }
    function key(e) {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopImmediatePropagation();
      cancel();
    }
    // With the overlay gone, so the page shows as it is and is found under
    // it; two frames on, so it is gone from the screen too.
    function done(e) {
      const moved = Math.hypot(e.clientX - start[0], e.clientY - start[1]) >= 4;
      const box = boxTo(e);
      end();
      let rect, found;
      if (moved) {
        rect = area({ x: box.x, y: box.y, width: box.w, height: box.h });
        found = inBox(box);
      } else {
        const el = document.elementsFromPoint(e.clientX, e.clientY).find((el) => el !== host && shown(el));
        found = el ? [el] : [];
        rect = el ? area(el.getBoundingClientRect()) : { x: Math.round(e.clientX), y: Math.round(e.clientY), w: 0, h: 0 };
      }
      const size = (el) => {
        const r = el.getBoundingClientRect();
        return r.width * r.height;
      };
      const elements = found.sort((a, b) => size(b) - size(a)).slice(0, MOST).map(describe);
      const view = window.innerWidth;
      requestAnimationFrame(() => requestAnimationFrame(() => resolve({ url: location.href, rect, elements, view })));
    }
    // The page gets none of the pointer: no clicks, no hover, no scrolling.
    for (const type of ["mousedown", "mousemove", "mouseup", "click", "dblclick", "contextmenu", "wheel", "touchstart", "touchmove"]) {
      host.addEventListener(type, (e) => (e.preventDefault(), e.stopPropagation()), { passive: false });
    }
    host.addEventListener("pointerdown", (e) => {
      e.stopPropagation();
      if (e.button !== 0) return;
      start = [e.clientX, e.clientY];
      host.setPointerCapture(e.pointerId);
    });
    host.addEventListener("pointermove", (e) => {
      e.stopPropagation();
      if (!start) return;
      const b = boxTo(e);
      Object.assign(drawn.style, { display: "block", left: `${b.x}px`, top: `${b.y}px`, width: `${b.w}px`, height: `${b.h}px` });
    });
    host.addEventListener("pointerup", (e) => {
      e.stopPropagation();
      if (start) done(e);
    });
    window.addEventListener("keydown", key, true);
    window.__gerdrPick = cancel;
    document.documentElement.append(host);
  });
}

// The selection asked of each tab, by tab id, until answered: `{ pane }`.
// Each is answered once, as selected or unselected.
const selecting = new Map();

const why = (err) => String((err && err.message) || err);

// End the selection asked of `tab` with `unselected` for `reason`, if one is
// asked; whether one was.
function unselect(tab, reason) {
  const asked = selecting.get(tab);
  if (!asked) return false;
  selecting.delete(tab);
  send({ type: "unselected", pane: asked.pane, reason });
  return true;
}

// Take the page's selection away, if it is still there.
function unpick(tab) {
  const func = () => window.__gerdrPick?.();
  return chrome.scripting.executeScript({ target: { tabId: tab }, world: "MAIN", func }).catch(() => {});
}

// What the tab shows of `rect` (CSS px of a viewport `view` wide), with a
// margin round it and the area outlined: a PNG data URL.
async function crop(tab, rect, view) {
  const MARGIN = 16;
  const { windowId } = await chrome.tabs.get(tab);
  const shot = await chrome.tabs.captureVisibleTab(windowId, { format: "png" });
  const image = await createImageBitmap(await (await fetch(shot)).blob());
  // The shot's pixels per CSS pixel: the page's device pixel ratio, as the
  // browser took it.
  const scale = view > 0 ? image.width / view : 1;
  const left = Math.max(0, Math.floor((rect.x - MARGIN) * scale));
  const top = Math.max(0, Math.floor((rect.y - MARGIN) * scale));
  const right = Math.min(image.width, Math.ceil((rect.x + rect.w + MARGIN) * scale));
  const bottom = Math.min(image.height, Math.ceil((rect.y + rect.h + MARGIN) * scale));
  if (right <= left || bottom <= top) throw new Error("the area is outside the page's view");
  const canvas = new OffscreenCanvas(right - left, bottom - top);
  const draw = canvas.getContext("2d");
  draw.drawImage(image, left, top, right - left, bottom - top, 0, 0, right - left, bottom - top);
  // Just outside the area, so none of it is hidden.
  const line = 2 * scale;
  draw.strokeStyle = "#ff3b30";
  draw.lineWidth = line;
  draw.strokeRect(rect.x * scale - left - line / 2, rect.y * scale - top - line / 2, rect.w * scale + line, rect.h * scale + line);
  const bytes = new Uint8Array(await (await canvas.convertToBlob({ type: "image/png" })).arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return `data:image/png;base64,${btoa(binary)}`;
}

// Let the user select an area of `tab`'s page for `pane` (see `pick`), and
// tell gerdr what was selected, with a shot of it. One asked already is
// cancelled: the new pick takes its overlay away.
async function select(tab, pane) {
  unselect(tab, "cancelled");
  const asked = { pane };
  selecting.set(tab, asked);
  // Answered already otherwise: cancelled, or the page went elsewhere.
  const current = () => selecting.get(tab) === asked;
  let picked;
  try {
    const [frame] = await chrome.scripting.executeScript({ target: { tabId: tab }, world: "MAIN", func: pick });
    picked = frame?.result;
  } catch (err) {
    if (current()) unselect(tab, `error: ${why(err)}`);
    return;
  }
  if (!current()) return;
  if (!picked) return void unselect(tab, "error: no answer from the page");
  if (picked.cancelled) return void unselect(tab, "cancelled");
  selecting.delete(tab);
  const { url, rect, elements, view } = picked;
  const message = { type: "selected", pane, url, rect, elements };
  try {
    message.data = await crop(tab, rect, view);
  } catch (err) {
    message.error = why(err);
  }
  send(message);
}

async function command(message) {
  await loaded;
  // A tab is known by its boot page's code until gerdr tells it the
  // pane's own name (see `boot`): from then on, by that.
  if (message.type === "pane") {
    for (const id of Object.keys(panes)) {
      if (panes[id] === message.code) panes[id] = message.pane;
    }
    return save();
  }
  const tab = tabOf(message.pane);
  if (tab === undefined) return;
  try {
    switch (message.type) {
      case "navigate":
        if (!(message.soft && (await goInApp(tab, message.url)))) {
          await chrome.tabs.update(tab, { url: message.url });
        }
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
        await chrome.tabs.reload(tab, { bypassCache: !!message.bypassCache });
        break;
      case "scope":
        scopes[tab] = { origin: message.origin, dirs: message.dirs };
        await save();
        await scope(tab);
        await startThread(tab);
        break;
      // To the pane's preview page, which shows it (see preview.js).
      case "preview":
      case "caret":
      case "scroll":
        await chrome.tabs.sendMessage(tab, message);
        break;
      // What the pane's tab shows, for a manual test's failure: a PNG,
      // or why there is none, goes back either way.
      case "capture": {
        const pane = message.pane;
        try {
          const { windowId } = await chrome.tabs.get(tab);
          const data = await chrome.tabs.captureVisibleTab(windowId, { format: "png" });
          send({ type: "shot", pane, data });
        } catch (err) {
          send({ type: "shot", pane, error: String((err && err.message) || err) });
        }
        break;
      }
      // An area of the pane's page, for a manual test's comment: answered
      // once the user selected it, or gave up.
      case "select":
        await select(tab, message.pane);
        break;
      case "unselect":
        if (unselect(tab, "cancelled")) await unpick(tab);
        break;
      case "newthread":
        threads[tab] = { origin: message.origin, dirs: message.dirs, prompt: message.prompt, model: message.model, at: Date.now() };
        await save();
        await startThread(tab);
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
  for (const id of Object.keys(threads)) {
    if (!open.has(id)) delete threads[id];
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
  // A page that went elsewhere while selecting (in a single-page app, on
  // its own) has no area to give.
  if (change.url && unselect(id, "navigated")) unpick(id);
  await loaded;
  await boot(tab);
  if (change.status === "complete") await retry(tab);
  // A load done is when the page's history can be asked for sure.
  if (change.url || change.title || change.status === "complete") await report(tab);
  // Not waited for: setting the filter can take a minute.
  if (change.status === "complete") {
    scope(id)
      .then(() => startThread(id))
      .catch(() => {});
  }
});

chrome.tabs.onRemoved.addListener(async (id) => {
  unselect(id, "navigated");
  await loaded;
  const pane = panes[id];
  delete scopes[id];
  delete threads[id];
  if (pane === undefined) return;
  delete panes[id];
  await save();
  send({ type: "closed", pane });
});

// The worker starts with the browser, not only once a tab changes.
chrome.runtime.onStartup.addListener(() => {});

connect();
