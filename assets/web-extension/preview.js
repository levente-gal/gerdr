// A markdown file's preview (see gerdr's `md_preview`). The page holds no
// file: it shows what gerdr sends through the worker, `{ type: "preview",
// title, html | error, dark, review }`, the HTML rendered by gerdr with raw
// HTML left out. It goes into the article only; the extension's pages run
// no inline scripts anyway. Links open through gerdr, in a tab of their
// own: the page never goes elsewhere. `#` links scroll in it.

const body = document.getElementById("body");
const error = document.getElementById("error");
const bar = document.getElementById("bar");
const light = document.getElementById("light");
const dark = document.getElementById("dark");

// How long the page waits for its file before saying it hasn't come.
const WAIT = 4000;
let shown = false;

function show(message) {
  shown = true;
  document.title = message.title || "Preview";
  document.documentElement.classList.toggle("dark", !!message.dark);
  light.disabled = !!message.dark;
  dark.disabled = !message.dark;
  bar.hidden = !message.review;
  if (typeof message.error === "string") {
    error.textContent = message.error;
    error.hidden = false;
    body.replaceChildren();
    return;
  }
  error.hidden = true;
  // Where it was, after an edit as before it.
  const [x, y] = [scrollX, scrollY];
  body.innerHTML = typeof message.html === "string" ? message.html : "";
  scrollTo(x, y);
}

chrome.runtime.onMessage.addListener((message) => {
  if (message && message.type === "preview") show(message);
  return false;
});

document.getElementById("review").addEventListener("click", () => {
  chrome.runtime.sendMessage({ type: "preview-review" }).catch(() => {});
});

// A link's page opens through gerdr; one in the page scrolls to its part.
function follow(event) {
  const link = event.target.closest && event.target.closest("a[href]");
  if (!link) return;
  const href = link.getAttribute("href");
  if (href.startsWith("#")) return;
  event.preventDefault();
  if (event.type === "click" || event.button === 1) {
    chrome.runtime.sendMessage({ type: "preview-open", url: link.href }).catch(() => {});
  }
}
document.addEventListener("click", follow);
document.addEventListener("auxclick", follow);

chrome.runtime.sendMessage({ type: "preview-ready" }).catch(() => {});
setTimeout(() => {
  if (shown) return;
  error.textContent = "gerdr hasn't sent the file: close this tab and open the preview again";
  error.hidden = false;
}, WAIT);
