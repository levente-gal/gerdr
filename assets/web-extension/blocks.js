// Which block of a preview holds a line of its file. gerdr renders each
// block with the lines it comes from (`data-sourcepos="3:1-5:20"`); the
// page marks the block its review's caret is in, and those with comments.
// No DOM here, so it runs under node too (see gerdr's web_control tests).

// The lines `3:1-5:20` covers, `[3, 5]`; none if it isn't one.
function sourceLines(pos) {
  const found = /^(\d+):\d+-(\d+):\d+$/.exec(pos || "");
  return found ? [Number(found[1]), Number(found[2])] : null;
}

// Of `blocks`, `[first, last, block]` in document order, the one holding
// `line`: the innermost holding it, or else the last before it (a blank
// line between blocks); none before the first block.
function pickBlock(blocks, line) {
  let holding = null;
  let before = null;
  for (const [first, last, block] of blocks) {
    // A block inside another comes after it, so the last holding the line
    // is the innermost.
    if (first <= line && line <= last) holding = block;
    if (first <= line) before = block;
  }
  return holding !== null ? holding : before;
}

// Where the page and its review stand: the line of the file at the top of
// each, to the fraction. Of `spans`, `[first, last, top, height]` of each
// block in document order (in pixels down the page), the place down the
// page `line` is at: as far into its block as the line is into its lines,
// or between the blocks around a line between them.
function lineY(spans, line) {
  if (spans.length === 0) return 0;
  let holding = null;
  let before = null;
  let after = null;
  for (const span of spans) {
    const [first, last] = span;
    if (first <= line && line < last + 1) holding = span;
    if (last + 1 <= line && (!before || last >= before[1])) before = span;
    if (first > line && after === null) after = span;
  }
  if (holding) {
    const [first, last, top, height] = holding;
    return top + ((line - first) / (last + 1 - first)) * height;
  }
  // Ends of blocks and starts of the next, the lines and places between;
  // the top of the page is the start of the line before the first block.
  const [from, fromY] = before ? [before[1] + 1, before[2] + before[3]] : [after ? after[0] - 1 : 1, 0];
  if (!after) return fromY;
  const [to, toY] = [after[0], after[2]];
  if (to <= from) return toY;
  return Math.max(0, fromY + ((line - from) / (to - from)) * (toY - fromY));
}

// The line at place `y` down the page, to the fraction: `lineY` turned
// round. In the innermost block there, or between the blocks around it.
function yLine(spans, y) {
  if (spans.length === 0) return 1;
  let holding = null;
  let before = null;
  let after = null;
  for (const span of spans) {
    const [, , top, height] = span;
    if (top <= y && y < top + height) holding = span;
    if (top + height <= y && (!before || top + height >= before[2] + before[3])) before = span;
    if (top > y && (!after || top < after[2])) after = span;
  }
  if (holding) {
    const [first, last, top, height] = holding;
    return first + ((y - top) / Math.max(height, 1)) * (last + 1 - first);
  }
  const [from, fromY] = before ? [before[1] + 1, before[2] + before[3]] : [after ? after[0] - 1 : 1, 0];
  if (!after) return from;
  const [to, toY] = [after[0], after[2]];
  if (toY <= fromY) return to;
  return from + ((y - fromY) / (toY - fromY)) * (to - from);
}

// The line a click at place `y` down the page landed on, in a block of lines
// `first` to `last` at `top` and `height`.
function clickedLine(first, last, top, height, y) {
  const line = Math.floor(yLine([[first, last, top, height]], y));
  return Math.min(Math.max(line, first), last);
}

if (typeof module !== "undefined") module.exports = { sourceLines, pickBlock, lineY, yLine, clickedLine };
