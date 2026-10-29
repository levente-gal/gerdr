// The worker takes this tab as its pane's (see worker.js), then the page
// goes on to its own, in place of this one; should the worker not answer,
// the page goes there anyway.
const url = new URLSearchParams(location.search).get("url");
let gone = false;
function go() {
  if (gone || !url) return;
  gone = true;
  location.replace(url);
}
chrome.runtime.sendMessage({ type: "boot" }).then(go, go);
setTimeout(go, 2000);
