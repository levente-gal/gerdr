// gerdr's end of the web pages it shows (see gerdr's `web_control`).
//
// gerdr opens each page at open.html?pane=<pane>&url=<page>: the tab is
// then known as that pane's, and the page goes on to its own (open.js), in
// place of itself: going back never lands on it. gerdr is told where
// each pane's tab is whenever it changes, whether it can go back or
// forward, and when it closes, over the native messaging host; it can send
// a pane's tab to another page, back, forward, or load it again.

const HOST = "dev.gerdr.web";
const BOOT = chrome.runtime.getURL("open.html");

// The pane of each tab gerdr opened, by the tab's id. Kept in the
// session's storage too: the worker comes and goes.
let panes = {};
const loaded = chrome.storage.session.get("panes").then((stored) => {
  panes = stored.panes || {};
});

function save() {
  return chrome.storage.session.set({ panes });
}

let port = null;

function send(message) {
  if (port) port.postMessage(message);
}

// Run `func` in `tab`'s page; what it gives, or `fallback` where the page
// can't be reached (the browser's own pages, error pages).
async function inPage(tab, func, fallback) {
  try {
    const [frame] = await chrome.scripting.executeScript({ target: { tabId: tab }, func });
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
  await save();
  for (const tab of tabs) {
    await boot(tab);
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
  // A load done is when the page's history can be asked for sure.
  if (change.url || change.title || change.status === "complete") await report(tab);
});

chrome.tabs.onRemoved.addListener(async (id) => {
  await loaded;
  const pane = panes[id];
  if (pane === undefined) return;
  delete panes[id];
  await save();
  send({ type: "closed", pane });
});

// The worker starts with the browser, not only once a tab changes.
chrome.runtime.onStartup.addListener(() => {});

connect();
