// installed by gerdr; gerdr writes this file again, so changes here go.
//
// Tells gerdr what this OpenCode TUI does, as it does it: the chat it shows,
// the tools that run, a turn's end, the permissions it asks for. gerdr's chat
// of this herdr pane can answer an ask; the TUI's own prompt still can too.
// One JSON line per event on gerdr's live socket (see gerdr's
// src/chat/live.rs); answers come back as lines {"ask", "reply"}.
//
// For OpenCode's TUI plugin API v2 (`setup(api)`, OpenCode 2.x).

import net from "node:net";
import { createHash } from "node:crypto";

const AGENT = "opencode";
// The most of a tool's input an ask shows, in chars.
const MAX_INPUT = 4000;
// How often the selected chat is looked at.
const ROUTE_POLL_MS = 500;
// A tool input's keys that say its gist, first found first (as gerdr's
// chat says it).
const GIST_KEYS = [
  "command", "filePath", "pattern", "glob_pattern", "path", "target_directory",
  "url", "query", "description", "name",
];

export default {
  id: "gerdr.live",
  setup,
};

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
  if (typeof input === "string") return firstLine(input);
  if (!input || typeof input !== "object") return "";
  for (const key of GIST_KEYS) {
    if (typeof input[key] === "string") return firstLine(input[key]);
  }
  return "";
}

function firstLine(text) {
  const line = text.split("\n").map((l) => l.trim()).find((l) => l) ?? "";
  return line.length > 200 ? `${line.slice(0, 200)}…` : line;
}

function shown(value) {
  let text;
  try {
    text = JSON.stringify(value ?? {}, null, 2) ?? "";
  } catch {
    text = "";
  }
  return text.length > MAX_INPUT ? `${text.slice(0, MAX_INPUT)}…` : text;
}

function setup(api) {
  const pane = process.env.HERDR_PANE_ID;
  const path = socketPath();
  if (!pane || !path) return;

  let disposed = false;
  let socket;
  let selected;
  const names = new Map();
  // The asks told to gerdr, by request id: their session.
  const asks = new Map();

  // One connection, opened when there is something to say; a gone gerdr
  // just loses what is said till it listens again.
  function send(what) {
    if (disposed) return;
    try {
      if (!socket) connect();
      socket.write(`${JSON.stringify({ pane, agent: AGENT, at_ms: Date.now(), what })}\n`);
    } catch {
      socket = undefined;
    }
  }

  function connect() {
    const conn = net.createConnection(path);
    socket = conn;
    conn.unref?.();
    conn.setEncoding("utf8");
    let buffer = "";
    conn.on("data", (chunk) => {
      buffer += chunk;
      let end;
      while ((end = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, end);
        buffer = buffer.slice(end + 1);
        try {
          answer(JSON.parse(line));
        } catch {}
      }
    });
    const gone = () => {
      if (socket === conn) socket = undefined;
    };
    conn.on("error", gone);
    conn.on("close", gone);
  }

  // gerdr's answer to an ask: once or reject; "pass" leaves it to the TUI.
  function answer({ ask, reply }) {
    const sessionID = asks.get(ask);
    const decision = { allow: "once", deny: "reject" }[reply];
    if (!sessionID || !decision) return;
    asks.delete(ask);
    const input = { sessionID, requestID: ask, decision };
    const permission = api.data?.session?.permission;
    const sent = permission?.reply ? permission.reply(input) : api.client?.permission?.reply(input);
    Promise.resolve(sent).catch(() => {});
  }

  function root(id) {
    if (typeof id !== "string") return;
    try {
      return api.data.session.root?.(id) ?? id;
    } catch {
      return id;
    }
  }

  // Whether session `id` is the selected chat or one of its subagents'.
  function mine(id) {
    return !!selected && root(id) === selected;
  }

  function look() {
    if (disposed) return;
    let route;
    try {
      route = api.ui.router.current();
    } catch {}
    const id = route?.type === "session" ? root(route.sessionID) : undefined;
    if (id === selected) return;
    selected = id;
    if (id) send({ type: "session", kind: "id", value: id });
  }

  const handlers = {
    "session.tool.input.started": (data) => names.set(data.id, data.name),
    "session.tool.called": (data) => {
      if (!mine(data.sessionID)) return;
      const name = names.get(data.id) ?? data.name ?? "tool";
      send({ type: "tool_start", call: data.id, name, gist: gist(data.input) });
    },
    "session.tool.success": (data) => end(data, false),
    "session.tool.failed": (data) => end(data, true),
    "session.execution.succeeded": turnEnd,
    "session.execution.failed": turnEnd,
    "session.execution.interrupted": turnEnd,
    "permission.asked": (data) => {
      if (!mine(data.sessionID) || typeof data.id !== "string") return;
      asks.set(data.id, data.sessionID);
      const tool = data.action ?? data.permission ?? "permission";
      const what = (data.resources ?? data.patterns ?? []).join(" ") || data.message || "";
      send({ type: "ask", ask: data.id, tool, gist: firstLine(what), input: shown(data.metadata ?? data) });
    },
    "permission.replied": (data) => {
      if (!asks.delete(data.requestID)) return;
      send({ type: "ask_gone", ask: data.requestID });
    },
  };

  function end(data, failed) {
    names.delete(data.id);
    if (mine(data.sessionID)) send({ type: "tool_end", call: data.id, failed });
  }

  function turnEnd(data) {
    if (selected && data.sessionID === selected) send({ type: "turn_end" });
  }

  const unsubscribe = Object.entries(handlers).map(([type, handle]) =>
    api.data.on(type, (event) => {
      try {
        look();
        if (event?.data) handle(event.data);
      } catch {}
    }),
  );
  look();
  const poll = setInterval(look, ROUTE_POLL_MS);
  poll.unref?.();
  return () => {
    disposed = true;
    clearInterval(poll);
    for (const off of unsubscribe) {
      try {
        off?.();
      } catch {}
    }
    socket?.destroy();
    socket = undefined;
  };
}
