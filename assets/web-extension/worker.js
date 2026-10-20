// gerdr's end of the web pages it shows (see gerdr's `web_control`).
//
// gerdr opens each page at open.html?pane=<pane>&url=<page>: the tab is
// then known as that pane's, and sent on to the page. gerdr is told where
// each pane's tab is whenever it changes, and when it closes, over the
// native messaging host; it can send a pane's tab to another page.

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

// Tell gerdr where `tab` is, if it is a pane's.
function report(tab) {
  const pane = panes[tab.id];
  if (pane === undefined || !tab.url || tab.url.startsWith(BOOT)) return;
  send({ type: "page", pane, url: tab.url, title: tab.title || "" });
}

// Take `tab` as its pane's and send it to its page, if gerdr opened it.
async function boot(tab) {
  const url = tab.url || tab.pendingUrl;
  if (!url || !url.startsWith(BOOT) || panes[tab.id] !== undefined) return;
  const query = new URL(url).searchParams;
  const pane = query.get("pane");
  const page = query.get("url");
  if (!pane || !page) return;
  panes[tab.id] = pane;
  await save();
  await chrome.tabs.update(tab.id, { url: page });
}

async function command(message) {
  await loaded;
  if (message.type === "navigate") {
    const tab = Object.keys(panes).find((id) => panes[id] === message.pane);
    if (tab !== undefined) await chrome.tabs.update(Number(tab), { url: message.url });
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
    report(tab);
  }
}

chrome.tabs.onCreated.addListener(async (tab) => {
  await loaded;
  await boot(tab);
});

chrome.tabs.onUpdated.addListener(async (id, change, tab) => {
  await loaded;
  await boot(tab);
  if (change.url || change.title) report(tab);
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
