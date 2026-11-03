// A markdown file's preview (see gerdr's `md_preview`). The page holds no
// file: it shows what gerdr sends through the worker, `{ type: "preview",
// title, html | error, dark, marks }`, the HTML rendered by gerdr
// with raw HTML left out. It goes into the article only; the extension's
// pages run no inline scripts anyway. Links open through gerdr, in a tab
// of their own: the page never goes elsewhere. `#` links scroll in it.
//
// In the file's tab (gerdr's `doc_tab`), the page marks the block
// the review's caret is in (`{ type: "caret", line }`) and the blocks
// with comments (`marks`, `[line, text]`); a click on a block puts the
// review's caret on its first line. The two scroll together: gerdr sends
// the line at the review's top (`{ type: "scroll", line }`, to the
// fraction), and the page tells it the line at its own.

const body = document.getElementById("body");
const error = document.getElementById("error");
const light = document.getElementById("light");
const dark = document.getElementById("dark");

// How long the page waits for its file before saying it hasn't come.
const WAIT = 4000;
let shown = false;
// The line the review's caret is on, and the block marked for it.
let caret = null;
let caretBlock = null;
// The lines with comments, and what each says.
let marks = [];
// The line at the top of the page and its review, once they scroll
// together; and where the page scrolled itself to for it, whose scroll
// isn't told back.
let topLine = null;
let ownY = null;

// Elements rendered with their lines that aren't blocks.
const INLINE = new Set(["A", "EM", "STRONG", "CODE", "DEL", "SPAN", "IMG", "SUP", "SUB", "INPUT"]);

// The article's blocks, `[first, last, block]` in document order; a table
// cell is its row.
function blocks() {
  const found = [];
  for (const element of body.querySelectorAll("[data-sourcepos]")) {
    if (INLINE.has(element.tagName) || element.tagName === "TD" || element.tagName === "TH") continue;
    const lines = sourceLines(element.getAttribute("data-sourcepos"));
    if (lines) found.push([lines[0], lines[1], element]);
  }
  return found;
}

// Each block's lines and place down the page, `[first, last, top, height]`.
function spans() {
  return blocks().map(([first, last, element]) => {
    const rect = element.getBoundingClientRect();
    return [first, last, rect.top + scrollY, rect.height];
  });
}

// Stand with `line` of the file at the top, as the review beside does.
function standAt(line) {
  topLine = line;
  scrollTo(scrollX, lineY(spans(), line));
  ownY = scrollY;
}

// The page scrolled, by the user's hand: its review follows, once a frame.
let told = false;
addEventListener("scroll", () => {
  if (topLine === null || told) return;
  told = true;
  requestAnimationFrame(() => {
    told = false;
    if (ownY !== null && Math.abs(scrollY - ownY) <= 1) return;
    ownY = null;
    topLine = yLine(spans(), scrollY);
    chrome.runtime.sendMessage({ type: "preview-scroll", line: topLine }).catch(() => {});
  });
});

// The review's caret on `line`, none for no line; the page scrolls to its
// block only when the block changes, and never while it scrolls with the
// review, which shows the caret itself.
function markCaret(line, scroll) {
  caret = Number.isInteger(line) ? line : null;
  const block = caret === null ? null : pickBlock(blocks(), caret);
  if (caretBlock && caretBlock !== block) caretBlock.classList.remove("gerdr-caret");
  if (!block) {
    caretBlock = null;
    return;
  }
  block.classList.add("gerdr-caret");
  if (scroll && topLine === null && block !== caretBlock) block.scrollIntoView({ block: "nearest" });
  caretBlock = block;
}

// A dot by each block with comments, which shows them when pointed at.
function markComments() {
  const all = blocks();
  const said = new Map();
  for (const [line, text] of marks) {
    const block = pickBlock(all, line);
    if (!block) continue;
    said.set(block, (said.has(block) ? said.get(block) + "\n\n" : "") + text);
  }
  for (const [block, text] of said) {
    block.classList.add("gerdr-said");
    block.title = text;
  }
}

function show(message) {
  shown = true;
  document.title = message.title || "Preview";
  document.documentElement.classList.toggle("dark", !!message.dark);
  light.disabled = !!message.dark;
  dark.disabled = !message.dark;
  if (typeof message.error === "string") {
    error.textContent = message.error;
    error.hidden = false;
    body.replaceChildren();
    return;
  }
  error.hidden = true;
  // Where it was, after an edit as before it: at its line beside the review.
  const [x, y] = [scrollX, scrollY];
  body.innerHTML = typeof message.html === "string" ? message.html : "";
  marks = Array.isArray(message.marks) ? message.marks : [];
  markComments();
  // The new blocks keep the caret, without the page moving for it.
  caretBlock = null;
  markCaret(caret, false);
  if (topLine === null) scrollTo(x, y);
  else standAt(topLine);
}

chrome.runtime.onMessage.addListener((message) => {
  if (message && message.type === "preview") show(message);
  if (message && message.type === "caret") markCaret(message.line, true);
  if (message && message.type === "scroll" && Number.isFinite(message.line)) standAt(message.line);
  return false;
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

// A click on a block, not on a link nor while choosing text, puts the
// review's caret on its first line.
body.addEventListener("click", (event) => {
  if (event.button !== 0 || (event.target.closest && event.target.closest("a[href]"))) return;
  if (String(getSelection()).length > 0) return;
  let element = event.target;
  while (element && element !== body) {
    if (!INLINE.has(element.tagName) && element.hasAttribute("data-sourcepos")) break;
    element = element.parentElement;
  }
  if (!element || element === body) return;
  const lines = sourceLines(element.getAttribute("data-sourcepos"));
  if (lines) chrome.runtime.sendMessage({ type: "preview-line", line: lines[0] }).catch(() => {});
});

chrome.runtime.sendMessage({ type: "preview-ready" }).catch(() => {});
setTimeout(() => {
  if (shown) return;
  error.textContent = "gerdr hasn't sent the file: close this tab and open the preview again";
  error.hidden = false;
}, WAIT);
