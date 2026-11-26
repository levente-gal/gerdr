# Chat features by agent

What gerdr's chat view does for each agent CLI. This doc is partly
generated. The `Caps` table is rendered from each agent's
`integrations::of(kind).caps()`, and a test fails when it differs from the
code. The rest was read from the code on 2026-11-26, on branch
`agent-integrations`. Nothing here was tried in a live gerdr. How the agents
are built is in `docs/agent-integrations.md`.

Six agents have a chat: Claude Code, Codex, OpenCode, Cursor, pi and the
Antigravity CLI (agy). T3 has none. Its `store` is `None`, so its pane says
"Chats of … show in a later gerdr" (`src/chat/source.rs`, `find`).

✓ = works · — = missing

## What `Caps` says

Each row is a field of `Caps` (`src/integrations/mod.rs`). Don't edit this
table by hand: run `the_chat_doc_says_what_caps_say` in
`src/integrations/mod.rs` and paste the table it prints.

<!-- caps:begin -->
| `Caps` | Claude | Codex | OpenCode | Cursor | pi | agy |
|---|---|---|---|---|---|---|
| `chat`: gerdr reads its chats | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `earlier`: earlier messages on ask | ✓ | ✓ | — | — | ✓ | ✓ |
| `modes`: mode switcher | ✓ | ✓ | ✓ | ✓ | — | ✓ |
| `model`: model picker | typed | own menu | own menu | typed | own menu | own menu |
| `side`: `/btw` side panel | ✓ | — | ✓ | ✓ | — | ✓ |
| `suggests`: prompt suggestion | ✓ | — | — | — | — | — |
| `context`: context size | ✓ | ✓ | — | ✓ | ✓ | — |
<!-- caps:end -->

## Every feature

| Feature | Claude | Codex | OpenCode | Cursor | pi | agy |
|---|---|---|---|---|---|---|
| Where the chat is read from | jsonl | rollout jsonl | `opencode.db` | `store.db` | session jsonl | brain transcript |
| Finds the chat by folder and start time when herdr names none | — (needs herdr's name) | ✓ | ✓ | ✓ | ✓ | ✓ |
| Earlier messages on ask | ✓ | ✓ | not needed (whole DB) | not needed (whole DB) | ✓ | ✓ |
| Thinking, tool and edit-diff rows | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Subagents folded into a card | ✓ (subagent files) | — | ✓ | — | — | ✓ |
| Plan cards | ✓ `ExitPlanMode` and plans file | ✓ `<proposed_plan>` | — | ✓ `CreatePlan` | — | — |
| Plan review tab with comments | ✓ | ✓ | — | ✓ | — | — |
| How a review is sent back | deny with the comments | line naming a file | — | line naming a file | — | — |
| A line typed on the plan menu leaves it first | not needed | ✓ Esc | — | ✓ `p` | — | — |
| Live events (tool running, turn end) | hooks | — | TUI plugin | hooks | extension | hooks |
| Allow/Deny from the chat | ✓ | — | ✓ | — | no asks (pi asks no permissions) | — (no permission hook) |
| Native question cards (AskUserQuestion) | ✓ | — | — | — | — | — |
| Waiting panel (menu read off the screen) | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `/` menu, built-in commands | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `/` menu, commands found on disk | commands, skills, plugins | prompts only | command dirs, `opencode.json` | `.cursor` and `.claude` commands, skills | prompts, skills (no extension commands) | skills, including plugins' |
| `/btw` side panel | ✓ | — (CLI has no `/btw`) | ✓ | ✓ | — (CLI has no `/btw`) | ✓ |
| Prompt suggestion (Left fills it) | ✓ | — | — | — | — | — |
| Model picker | gerdr's list, types `/model id` | agent's own menu, mirrored | agent's own menu, mirrored | gerdr's list, types `/model id` | agent's own menu, mirrored | agent's own menu, mirrored |
| Mode switcher (Shift+Tab) | Auto / Manual / Edits / Plan | Build / Plan | Build / Plan, plus the user's own agents | Agent / Plan / Debug / Ask | — | Auto / Plan / Default / Edits |
| Context "84k / 1M" | ✓ | ✓ | — | ✓ | ✓ | — |

These work the same for all six: image paste, file attach, pinned prompt, the
↓ button, the working row with its turn clock, and lines sent with Enter
apart. The phone shows the same rows through `src/phone.rs`, gated by the same
checks.

## Where each row is decided

Each row comes from `Integration` methods (`src/integrations/mod.rs`),
implemented in the agent's folder under `src/integrations/<agent>/`. The
shared engine reads what they give. `<agent>/` below is any agent's folder.

| Feature | Trait method | Agent files | Shared code |
|---|---|---|---|
| Where the chat is read from | `store`, `parser`, `db` | `<agent>/parse.rs`, `cursor/db.rs`, `opencode/db.rs` | `src/chat/tail.rs`, `src/chat/parse/mod.rs` |
| Finding by folder and start time | `guesses`, `find`, `find_pane` | `<agent>/find.rs` | `src/chat/source.rs`, `src/integrations/common/pick.rs` |
| Earlier messages on ask | `store` (`Lines`) | | `src/chat/tail.rs` |
| Thinking, tool and edit-diff rows | `Parser::line`, `Parser::rows` | `<agent>/parse.rs` | `src/integrations/common/build.rs` |
| Subagents | `subagents`, `Parser::subagent`, `Parser::take_subagent` | `claude/subagents.rs`, `antigravity/subagents.rs`, `opencode/db.rs` | `src/integrations/common/files.rs` (`SubFiles`) |
| Plan cards | `parser` (`Builder::plan`, `plan_item`) | `claude/parse.rs` (`ExitPlanMode`), `codex/parse.rs` (`proposed_plan`), `cursor/parse.rs` (`CreatePlan`) | `src/integrations/common/build.rs` |
| Plan review, and how it goes back | a live `Ask` with `plan` | `claude/live.rs` | `src/chat/plan_review.rs` (`send_plan_back`), `src/chat/view.rs` (`review_target`) |
| A line on the plan menu leaves it first | `plan_menu` | `codex/screen.rs`, `cursor/screen.rs` | `src/herdr.rs` (`plan_menu_key`) |
| Live events | `hook_command`, `run_hook`, `install_live` | `<agent>/live.rs`, `assets/opencode/gerdr-live/tui.js`, `assets/pi/gerdr-live.ts` | `src/chat/live.rs`, `src/integrations/common/hook.rs`, `src/integrations/common/install.rs` |
| Allow/Deny | a live `Ask` | `claude/live.rs`, `assets/opencode/gerdr-live/tui.js` | `src/chat/live.rs` |
| Question cards | a live `Ask` with `questions` | `claude/live.rs` | `src/chat/questions.rs` |
| Waiting panel | none, the same for all | | `src/chat/menu.rs` (`menu`) |
| `/` menu | `builtins`, `disk_commands` | `<agent>/commands.rs` | `src/chat/commands.rs` (`list`), `src/integrations/common/commands.rs` |
| `/btw` | `side_panel` (sets `Caps::side`) | `<agent>/screen.rs` | `src/chat/btw.rs` |
| Prompt suggestion | `suggests`, `suggestion` | `claude/screen.rs` | `src/chat/view.rs` |
| Model picker | `model_switch`, `offers`, `own_model`, `models_read`, `model_menu`, `model_on_screen` | `<agent>/screen.rs` | `src/chat/model.rs`, `src/chat/model_menu.rs`, `src/integrations/common/screen.rs` |
| Modes | `modes`, `mode_shown` | `<agent>/screen.rs` | `src/chat/mode.rs` |
| Context | `has_context`, `context_in`, `context` | `<agent>/stats.rs` | `src/ai_work/context.rs` |

## Gaps

- **Codex is the thinnest:**
  - It has no live events, so no tool-running row from hooks and no
    Allow/Deny. The chat has only its transcript and herdr's status.
  - It has no subagents, and its only commands from disk are `prompts/`.
- **Context:** OpenCode and agy show none. OpenCode's database probably
  stores token counts on its messages; not checked.
- **Plan cards:** OpenCode, pi and agy have none. agy's `PLANNER_RESPONSE`
  steps become plain rows, not plans.
- **Cursor subagents:** each one is its own chat, and the hook's `agentId`
  names it. It could be folded the way Claude's subagents are, but isn't yet.
- **Allow/Deny:** Cursor, agy and Codex fall back to the screen-read waiting
  panel.
- **Claude-only:** question cards and prompt suggestions.
- **Waiting panel:** it reads numbered menus marked `❯`, `›` or `>`. Its tests
  only use Claude menus, so other agents' approval menus may not read cleanly.
