# Agent integrations

What gerdr knows of each agent CLI lives behind one trait, `Integration`
(`src/integrations/mod.rs`). Each agent implements it in a folder of its own.
Code that two agents share goes in `src/integrations/common/`. This is the
guide for adding an agent or changing one.

## The shape

- **`Integration`** has a method for each thing gerdr can do with an agent.
  Every method but `kind` and `label` has a default, and the default means
  "this agent doesn't have that".
- **`of(kind)`** gives the integration of an `AgentKind`. **`ALL`** lists
  every integration in `AgentKind`'s order. Code that has to ask every agent
  loops over `ALL`: the hook subcommands in `src/main.rs`, hook installs in
  `src/chat/live/serve.rs`, plan menus in `src/herdr.rs`.
- **`Caps`** is what a chat offers, all in one. `caps()` builds it from the
  other methods, so an agent never writes it by hand.

The chat's UI is `src/chat/view.rs`, `src/chrome/agent_chat.rs`,
`src/phone.rs`, `src/phone_control.rs` and `assets/phone/app.js`. It reads
only these:

- `Caps`
- `chat::Item`s, from the agent's parser
- `LiveEvent`s, from its hooks or plugin
- `Menu`s, read off its screen

So every agent's chat looks the same, and a feature shows for each agent that
has it. Of `Caps`, the UI checks `chat`, `modes`, `model`, `side`, `suggests`
and `context`. `earlier`, `asks`, `questions` and `plan_reply` only describe
the chat, for `docs/chat-agents.md`. The UI acts on what comes instead: the
reader says when there are earlier messages, and live `Ask` events bring
asks, questions and plans. The test `the_ui_names_no_agent` in `src/integrations/mod.rs` fails
when one of those files names an `AgentKind::` variant or an agent's name
outside its tests.

| Folder | Agent |
|---|---|
| `src/integrations/claude/` | Claude Code |
| `src/integrations/codex/` | Codex |
| `src/integrations/opencode/` | OpenCode |
| `src/integrations/cursor/` | Cursor CLI |
| `src/integrations/pi/` | pi |
| `src/integrations/antigravity/` | Antigravity CLI (agy) |
| `src/integrations/t3/` | T3 Code: finding only, no chat |
| `src/integrations/plain.rs` | an unknown agent: all defaults |

## The trait, method by method

### Identity

| Method | What gerdr does with it | Default |
|---|---|---|
| `kind` | Which `AgentKind` this is. | required |
| `label` | Its name in the stats panel and the model picker: "Claude Code". | required |
| `herdr_target` | herdr's name for its integration. When herdr names no chat for a pane, the problem gerdr shows names it: that integration isn't installed. | `kind().name()`; `None` means herdr doesn't know it |

### Finding a chat

| Method | What gerdr does with it | Default |
|---|---|---|
| `store` | `Lines` (a transcript that grows) or `Database` (rows read whole). Sets `Caps::chat` and `Caps::earlier`. | `None`: gerdr doesn't read its chats, and the pane says so |
| `guesses` | A pane's chat can be found by folder and start time when herdr names none. The chat head then says it may be another chat. | no guessing |
| `backfill_by_time` | When a feature's AI work from before it was kept is backfilled, a run's chat whose id wasn't kept is still found by time. | not found |
| `find` | A stage run's chat, by id, else by folder and time in a span, skipping claimed ones. | `Ok(None)` |
| `find_pane` | The same, for a chat in a pane. Cursor finds by time only here. | `locate::find_in`, which calls `find` |
| `transcript_at` | herdr's transcript path, made real, if the path is one of the agent's. | not its own |
| `transcripts_under` | Folders where its transcripts are written. A path named there that doesn't exist yet is a chat not written yet. | none |
| `id_in_path` | The chat id in a transcript's file name. | the file stem |
| `remote_script` | Shell lines that list the chat on another machine. | not kept from there |
| `remote_guesses` | That script lists chats to pick one by time. | it doesn't |
| `staged_store` | A chat pulled from another machine is a `store.db`, not a transcript. | a transcript |

### Reading it

| Method | What gerdr does with it | Default |
|---|---|---|
| `parser` | A fresh `Box<dyn Parser>` for each chat read. `Reader` (`src/chat/parse/mod.rs`) feeds it lines or rows. | reads nothing |
| `db` | For a `Database` store: a `DbChat` that reads what changed since the last read, and hands its rows to `Parser::rows`. | `None`: not a database chat |
| `subagents` | Called on every read. Feeds the reader what subagents wrote in files of their own (Claude, agy), through `SubFiles`. | none |

`Parser` (in `src/integrations/mod.rs`) holds only what this agent keeps of
its own. What every agent keeps is in `Builder`, which each method gets.

| `Parser` method | Called for |
|---|---|
| `line` | each JSON line of a `Lines` transcript |
| `rows` | all rows of a `Database` chat, in order, each time they change |
| `sub_line`, `subagent` | a subagent's lines, read into its own reader |
| `model` | the model the chat's lines last named |
| `named_subagents`, `unfound_subagents`, `subagents`, `take_subagent` | subagents that are conversations of their own, matched to the call that started them (agy) |

`DbChat::read` reads what changed in the database. `DbChat::rows` gives the
rows as `Parser::rows` takes them. See `src/integrations/cursor/db.rs` and
`src/integrations/opencode/db.rs`.

### Model

| Method | What gerdr does with it | Default |
|---|---|---|
| `model_switch` | `Typed`: gerdr's picker types `/model <id>`. `OwnMenu`: `/model` opens the agent's menu, mirrored in the chat and on the phone. Sets `Caps::model`. | `None`: no picker |
| `offers` | The models a `Typed` picker lists. | none |
| `own_model` | The first row, back to the agent's own choice (Claude's Default). | none |
| `models_read` | The offers are read from the CLI. gerdr reads them once for the desktop and the phone. | a fixed list |
| `model_menu` | The agent's model menu, read off the bottom of its screen, as a `Menu`. The reader sets `Menu::search` and `Menu::sideways`. | no menu |
| `model_on_screen` | The model its screen says it is on. | not said |
| `model_in_chat` | The model the chat is on, read after each read of the chat. | not said |
| `model_now` | The same, without a reader, for the phone. | not said |
| `pane_transcript` | The transcript of a pane's chat, for its model, and for its words when herdr keeps no scrollback (alternate screen). | none |

### Modes

| Method | What gerdr does with it | Default |
|---|---|---|
| `modes` | The modes Shift+Tab steps through, in order. They show under the reply box, on desktop and phone. Sets `Caps::modes`. | none: no switcher |
| `mode_shown` | The mode its screen shows, from its last rows that aren't empty, last first. | not read |

### Plan menu and replies

| Method | What gerdr does with it | Default |
|---|---|---|
| `plan_menu` | The menu it waits on under a plan: its question, its "keep planning" choice, and that choice's key. A line sent there leaves the menu first (`src/herdr.rs`). | no such menu |
| `plan_reply` | How a plan review goes back: `DenyAsk` denies the plan ask with the comments, `Line` sends a line naming the review's file. Sets `Caps::plan_reply`. | `Line` |

### Suggestions

| Method | What gerdr does with it | Default |
|---|---|---|
| `suggests` | Its screen suggests what to say next. gerdr reads it while the agent is idle. Sets `Caps::suggests`. | no |
| `suggestion` | The suggestion read off its screen rows. Left fills the reply box with it, and the phone shows it as a chip. | none |

### Commands

| Method | What gerdr does with it | Default |
|---|---|---|
| `builtins` | Its built-in commands, for the `/` menu. A `btw` here sets `Caps::side`, which turns on the `/btw` side panel. | none |
| `disk_commands` | Its commands, prompts and skills on disk for a folder, added to the `/` menu. | none |

### Live (hooks or plugin, asks)

An agent tells gerdr what happens as it happens, as `LiveEvent`s on gerdr's
socket (`src/chat/live.rs`): the chat it holds, a tool starting and ending,
a turn's end, an ask. There are two ways in.

- **A hook subcommand** (Claude, Cursor, agy). The agent's hooks run
  `gerdr <agent>-hook`. `run_hook` reads the payload with `hook::pane_input`,
  turns it into a `What`, and sends it with `hook::tell`. For an ask, it
  prints the chat's answer the way the agent takes it.
- **A plugin of gerdr's** (OpenCode's TUI plugin in `assets/opencode/`, pi's
  extension in `assets/pi/`). It talks to the socket itself. `install_live`
  writes it.

| Method | What gerdr does with it | Default |
|---|---|---|
| `hook_command` | The subcommand its hooks run. `src/main.rs` runs `run_hook` for it and exits. | no hook |
| `run_hook` | Read the event, send it, and end. | nothing |
| `install_live` | Add gerdr's hooks or plugin under a home folder, if the agent is installed. Runs once a run when the setting allows. Each file changed comes back for the Undo toast. | nothing installed |
| `asks` | Its asks can be allowed or denied from the chat. Sets `Caps::asks`. | no |
| `questions` | Its questions show as cards of their own. Sets `Caps::questions`. | no |

The events do the work. An `Ask` shows Allow and Deny. An `Ask` with
`questions` shows a question card. An `Ask` with `plan: true` is a plan
waiting to go ahead: the plan card's review sends a deny. The agent decides
`plan`, so the UI never compares tool names.

### Stats (context, usage)

| Method | What gerdr does with it | Default |
|---|---|---|
| `has_context` | Its chats say how full the context is. Sets `Caps::context`. | no |
| `context_in` | The context from the end of a transcript: "context 84k / 1M" on the chat head, the phone and the agent card. | none |
| `context` | The same from the chat's path. Override it for a chat that isn't a transcript (Cursor's `store.db`). | `context_in` on the file's tail |
| `usage` | The messages of a transcript with their tokens, for the stats panel. | none |

## What a parser must produce

A parser turns what the agent wrote into `chat::Item`s (`src/chat/mod.rs`),
through `Builder` (`src/integrations/common/build.rs`).

| Item | Rule | Builder helper |
|---|---|---|
| any | Its id comes from the transcript: a message's id, a call's id, `<id>.<n>` for a message's n-th part. It must stay the same when the chat is read again, since folds go by it. `line<n>` (`b.lines`) only for lines with no id of their own. | |
| `User` | What the user typed, with its time. | `push` |
| `Assistant` | Markdown. Parts of one message join into one item. | `answer(message, id, at, text)` |
| `Thinking` | Trimmed. Nothing is pushed when it is empty. A bold first line (`**…**`) heads its row. | `thinking(id, text)` |
| `Tool` | Name, one-line gist, input, and its result once it comes. | `push_call(call, at, item)`, then `fill(call, ToolOut)`; `gist`, `arg_gist`, `input_text`, `cap` |
| `Edit` | A file's path and its change. When there isn't both, it is a `Tool`. | `edit_or_tool`, `change`, `flat_edits`, `capped` |
| `Plan` | A plan's markdown, and its file if it has one. A plan handed over again (plan mode's file, then its ask) shows once. | `plan(id, path, text)`, `plan_item`, `is_plan_file` |
| `Subagent` | A subagent's work, in the place of the call that started it, with id `agent:<call>`. Its items come from a reader of its own in `b.subs` (`Sub::new`), copied in by `sync_subs`, or are built whole from rows (OpenCode). | `Sub`, `sync_subs`, `knows_call` |
| `Note` | What gerdr says in the chat's place. | `INTERRUPTED`, `COMPACTED`, `CHAT_COMPACTED`, `cant_show(kind)` |

`Reader` adds a note itself for a line that isn't JSON. It skips a cut first
line, since reading can start mid-file. Text parts of message content come
from `texts`, and a tag's text from `tag`.

## The common helpers

| File | What | Use it when |
|---|---|---|
| `common/build.rs` | `Builder`, `Sub`, the item helpers above, the note texts, the output caps (`MAX_LINES`, `MAX_BYTES`) | writing a parser |
| `common/files.rs` | `stamp`, `with_wal` (a database and its WAL as they are now), `first_line`, `json_lines`, `lines_back` (a tail's JSON lines, last first), `read_at`, `whole_end`, `SubFiles` (subagents' transcripts as they grow) | finding chats, reading context and usage, reading subagent files |
| `common/hook.rs` | `input`, `pane_input`, `tell`, `MAX_INPUT` | writing `run_hook` |
| `common/pick.rs` | `Begun`, `nearest`: of chats with a start, the one in a run's folder and span, nearest in time | finding a chat by folder and time |

Some shared helpers still live next to the code that uses them:

- `src/chat/live.rs`: `install`, `with_key`, `hook_command` (writing hooks).
- `src/chat/commands.rs`: `md_files`, `named_skills`, `read_md`, `one_line` (commands on disk).
- `src/chat/model_menu.rs`: `asked`, `choice`, `col_of`, `cut`, `style_at`, `wide_split` (reading menus off the screen).
- `src/ai_work/locate.rs`: `found`, `plain_id`, `subdirs`, `FIRST_LINE` (finding).

## Adding an agent

`src/integrations/pi/` is the smallest full one to copy. It has a chat of
lines, its own model menu, commands, a plugin and stats. Copy it and cut
what the new agent lacks.

1. **The kind.** Add a variant to `AgentKind` in `src/ai_work/mod.rs`, with
   its arms in `of` (the names it is called by) and `name`. If it keeps its
   chats in a home of its own, add that to `Homes` in
   `src/ai_work/locate.rs`.
2. **The folder.** Make `src/integrations/<agent>/mod.rs` with
   `pub struct <Agent>;` and `impl Integration` with `kind` and `label`. Its
   module doc says where its chats are and where its live word comes from.
3. **The registry.** In `src/integrations/mod.rs`, add `mod <agent>;`, its
   place in `ALL` (in `AgentKind`'s order), and its arm in `of`.
4. **The files, in order.** Each one adds methods to the impl in `mod.rs`.
   - `find.rs`: `store`, `find`, `guesses`, `transcript_at`,
     `transcripts_under`, `id_in_path`, and `remote_script` if its chats are
     kept from other machines.
   - `parse.rs`: the `Parser`, given by `parser`. Add `db.rs` for a database
     chat, and `subagents.rs` for subagents in files of their own.
   - `screen.rs`: `modes`, `mode_shown`, `model_switch`, `model_menu`,
     `model_on_screen`, `plan_menu`, `suggestion`. Note the CLI version its
     words were read from, in a `shortcut:` comment.
   - `commands.rs`: `BUILTINS` for `builtins`, and `disk_commands`.
   - `live.rs`: `install_live`, and `hook_command` with `run_hook` for a
     hook. Set `asks` and `questions` if its asks come through.
   - `stats.rs`: `has_context`, `context_in`, `usage`.
5. **Tests and fixtures.** Test each file in its own `mod tests`. Real hook
   payloads and screen captures go in `src/integrations/<agent>/fixtures/`
   (`.json`, `.ansi`). Run tests with `TMPDIR` outside the repo.
6. **The guard test.** `the_ui_names_no_agent` picks up the new variant from
   `ALL`. If the agent goes by a short name too (like `agy`), add it to the
   test's names. If the UI needs something new from the agent, add a trait
   method and a `Caps` field. Don't match on the kind in the UI.
7. **The chat doc.** Add a column for the agent in `docs/chat-agents.md`, and
   to `CHATS` in `src/integrations/mod.rs`'s tests. Run
   `the_chat_doc_says_what_caps_say`, and paste the table it prints between
   `<!-- caps:begin -->` and `<!-- caps:end -->`.

Launching isn't behind the trait yet. A new agent may also need entries in
`src/feature/agents.rs` (`Flavor`), `src/cli_agents.rs` (`KNOWN`),
`src/stage_models.rs` and `src/agent_icons.rs`.

## Rules

- What an agent does lives in its folder.
- When two agents need the same code, it goes in `src/integrations/common/`,
  once.
- The UI never matches on an agent. It reads `Caps`, items, live events and
  menus.
