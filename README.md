# gerdr

gerdr is a nested Wayland compositor for working on code. It shows your
editor, terminals, coding agents and other apps together in one window.

A sidebar lists **workspaces**: project folders, grouped into **projects**.
Each workspace has tabs. A workspace's first tab tiles Zed (top 70%) over a
terminal (bottom 30%). Each tab after that starts with one terminal. Split any
pane right or down. The new half gets a terminal in the workspace folder, or
any other Wayland app you launch into it.

Terminals live in a [herdr](https://github.com/herdrdev/herdr) session of
gerdr's own. Shells, servers and agents keep running when gerdr quits, and
they come back when it starts again.

## What's in it

- **Agents.** Claude Code, Codex, Gemini, cursor-agent, opencode and other
  agent CLIs on your `PATH` start from a workspace's AI button. Each runs in
  its own tab. Running agents, Cursor chats and T3 Code threads show as
  sidebar cards, with a dot that says when one needs you.
- **Repositories panel** (prefix `G`). Status of every repository in the
  workspace, a diff of all of them in one view, staging, commits, stashes, a
  commit log and a branch picker. The diff has vim keys (search, yank,
  delete, restore) and review comments you can copy for an agent.
- **Worktrees** (prefix `W`). Create, open, check out and delete git
  worktrees.
- **Inbox** (prefix `I`). One place for agents that wait on you, repositories
  to push or pull, pull requests, CI runs, tickets (GitHub, Linear) and
  errors from your services.
- **Features** (prefix `Shift+F`). Take a feature from discovery to a merged
  branch in nine stages, with agents doing the work. A stage's agent is Claude
  Code, Cursor or T3 Code, picked on its page. T3 Code opens its prompt as a
  draft in its app, where you pick the model and send it.
- **Web panes.** A Chromium extension lets gerdr steer browser panes: back,
  forward, reload and copy the URL.
- **Kept apps.** Apps such as Cursor can keep running across gerdr restarts
  through [wprs](https://github.com/wayland-transpositor/wprs).

## Requirements

- Linux with a Wayland session. gerdr opens as a window in it.
- A Rust toolchain with edition 2024 (Rust 1.85 or newer).
- `herdr` and `alacritty` on your `PATH`, for terminals.
- `zed` on your `PATH`, for project workspaces.
- Optional: `git` (repositories panel), `gh` (pull requests, CI and tickets),
  `wprsd` and `wprsc` (kept apps), and the agent CLIs you use.

## Install

```sh
./install.sh
```

The script builds a release binary and installs it to `~/.local/bin/gerdr`. It
also adds a desktop entry and an icon under `~/.local/share`, so app launchers
list gerdr. Run it again to update.

To build only:

```sh
cargo build --release
```

## Run

```text
usage: gerdr [--project <name>] [--attach] [--] [folder…]
  --project <name>  show this project (created if new)
  --attach          join a project even if another gerdr has it open
  -h, --help        show this help
  -V, --version     show the version
```

Folders given on the command line open as workspaces. Each project can be
open in one gerdr at a time. Use `--attach` to join one that is already open.

## Keys

Most actions are a prefix key, then one key. The prefix is `Ctrl+B` by
default. Press the prefix, then `?` to keep the full list on screen. A few to
start with:

| After the prefix | Action |
| --- | --- |
| `C` | Open a new tab |
| `V` / `-` | Split the pane right / down |
| `H` `J` `K` `L` | Focus the pane left / down / up / right |
| `X` | Close the pane |
| `Z` | Zoom the pane, or unzoom |
| `Shift+A` | Start an agent |
| `F` | Find and switch |
| `R` | Run an app in a split |
| `Shift+N` | Open a folder as a workspace |
| `Shift+P` | Choose a project |
| `G` / `W` / `I` | Open repositories / worktrees / inbox |
| `Shift+Q` | Quit |

## Configuration

Settings are in `$XDG_CONFIG_HOME/gerdr/config.toml`
(`~/.config/gerdr/config.toml` by default). gerdr runs without the file. An
example:

```toml
[keys]
prefix = "Ctrl+A"
new-tab = "t"

[apps]
# Run by `sh` in the workspace folder.
browser = "firefox http://localhost:3000"

[ui]
theme = "system"      # or "light" or "dark"
density = "compact"   # or "comfortable", the default

[agents]
claude = "claude --dangerously-skip-permissions"

[keep]
apps = ["cursor"]
```

The other sections are `[git]`, `[inbox]`, `[services]`, `[t3]`, `[terminal]`
and `[notifications]`. The comments at the top of `src/config.rs` and
`src/keys.rs` describe every key.

Other files:

- Projects: `~/.local/share/gerdr/projects/<id>.json` and `state.json`
- Log: `$XDG_STATE_HOME/gerdr/gerdr.log` (`~/.local/state/gerdr/gerdr.log`)

### Running out of inotify instances

Zed saying "Could not start inotify", or a terminal showing "Unable to watch
config file: Too many open files", means your user has no inotify instances
left. The limit is 128 by default, and each terminal pane takes one to follow
its Alacritty config. gerdr warns at 85% of the limit. To raise it, press "How
to fix" on the warning, or run:

```sh
echo fs.inotify.max_user_instances=1024 | sudo tee /etc/sysctl.d/90-gerdr-inotify.conf
sudo sysctl -p /etc/sysctl.d/90-gerdr-inotify.conf
```

To undo it, delete that file and reboot.

To use fewer instead, set `live_config_reload = false` under `[terminal]`, or
use the launcher's "Turn off terminal config reload" row. New terminals then
read their config once. Open ones let go of theirs when gerdr restarts.

## Development

```sh
cargo test
```

To try a change at near-release speed, `cargo build --profile fast` builds
`target/fast/gerdr` in about a quarter of the time. Its painting is about
twice as slow as release, so install with `install.sh`.

`docs/ux-conventions.md` gives the UX rules gerdr follows.
`docs/feature-checklist.md` turns them into a checklist for each new feature.

## License

MIT. See [LICENSE](LICENSE). The agent logos in `assets/agents/` keep their
own license (see `assets/agents/LICENSE`).
