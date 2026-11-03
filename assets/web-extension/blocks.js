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

if (typeof module !== "undefined") module.exports = { sourceLines, pickBlock };
