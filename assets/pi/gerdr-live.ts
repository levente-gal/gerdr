// installed by gerdr; gerdr writes this file again, so changes here go.
// @ts-nocheck
//
// Tells gerdr what this pi TUI does, as it does it: the session it holds,
// the tools that run, the end of its work. One JSON line per event on
// gerdr's live socket (see gerdr's src/chat/live.rs). pi asks no
// permissions, so there are no asks to answer.

import net from "node:net";
import { createHash } from "node:crypto";
import path from "node:path";

const AGENT = "pi";
// A tool input's keys that say its gist, first found first (as gerdr's chat
// says it).
const GIST_KEYS = ["command", "path", "pattern", "filePath", "url", "query", "description", "name"];

// gerdr's socket for the herdr session this pane is in, as gerdr names it:
// $XDG_RUNTIME_DIR/gerdr-live-<16 hex of sha256(HERDR_SOCKET_PATH)>.sock.
function socketPath() {
  const herdr = process.env.HERDR_SOCKET_PATH;
  const dir = process.env.XDG_RUNTIME_DIR;
  if (!herdr || !dir?.startsWith("/")) return;
  const hash = createHash("sha256").update(herdr).digest("hex").slice(0, 16);
  return `${dir}/gerdr-live-${hash}.sock`;
}

function gist(input) {
  if (!input || typeof input !== "object") return "";
  for (const key of GIST_KEYS) {
    if (typeof input[key] === "string") {
      const line = input[key].split("\n").map((l) => l.trim()).find((l) => l) ?? "";
      return line.length > 200 ? `${line.slice(0, 200)}…` : line;
    }
  }
  return "";
}

export default function (pi) {
  const pane = process.env.HERDR_PANE_ID;
  const where = socketPath();
  if (!pane || !where) return;

  // Only the TUI's own session: print, JSON and RPC runs have no pane to show.
  let tui = false;
  let socket;
  let session;

  // One connection, opened when there is something to say; a gone gerdr just
  // loses what is said till it listens again.
  function send(what) {
    if (!tui) return;
    try {
      if (!socket) {
        const conn = net.createConnection(where);
        socket = conn;
        conn.unref?.();
        const gone = () => {
          if (socket === conn) socket = undefined;
        };
        conn.on("error", gone);
        conn.on("close", gone);
      }
      socket.write(`${JSON.stringify({ pane, agent: AGENT, at_ms: Date.now(), what })}\n`);
    } catch {
      socket = undefined;
    }
  }

  function tellSession(ctx) {
    let file;
    try {
      file = ctx?.sessionManager?.getSessionFile?.();
    } catch {}
    if (typeof file !== "string" || !path.isAbsolute(file) || file === session) return;
    session = file;
    send({ type: "session", kind: "path", value: file });
  }

  pi.on("session_start", (_event, ctx) => {
    tui = ctx?.mode === "tui";
    session = undefined;
    tellSession(ctx);
  });
  pi.on("agent_start", (_event, ctx) => tellSession(ctx));
  pi.on("tool_execution_start", (event) => {
    if (typeof event?.toolCallId !== "string") return;
    const name = event.toolName ?? "tool";
    send({ type: "tool_start", call: event.toolCallId, name, gist: gist(event.args) });
  });
  pi.on("tool_execution_end", (event) => {
    if (typeof event?.toolCallId !== "string") return;
    send({ type: "tool_end", call: event.toolCallId, failed: event.isError === true });
  });
  // agent_end may be followed by a retry or queued work; settled is final.
  pi.on("agent_settled", () => send({ type: "turn_end" }));
  pi.on("session_shutdown", () => {
    socket?.destroy();
    socket = undefined;
  });
}
