# Feature review checklist

What to check on every new feature branch before it merges. Each item is a
mistake the 2026-11 UX audit found more than once (~180 findings), or found
again in the features merged after it. The rules themselves are in
`docs/ux-conventions.md`; this list says where they tend to break and where
to look.

How to use it:

- Go over the branch diff (`git diff main...HEAD`) section by section.
- Name each finding by its item id and place, like `E2 src/foo.rs:120`. Then
  findings group by kind, and one fix can cover all places of the same kind.
- Run the quick greps at the end first. They find the usual suspects in a
  minute.
- End the report with what was not tried in a live gerdr.

## C. Closing, stopping, deleting

- **C1** Every new way to close, stop or delete goes through `confirm.rs`
  (`ask_close_pane`, `ask_close_tab`, `ask_close_workspace`, `stop_project`,
  `forget_project`, or a new `ConfirmDialog` there). No direct kill in a
  click or key handler.
- **C2** It asks only when something is lost: a running process, unmerged
  work, something that can't be undone. An idle chat or empty shell closes
  at once. One exception: a repository on a feature's branch never leaves
  the feature without asking, even with nothing to lose.
- **C3** Dialog buttons: Cancel left and default, the action right in
  `ButtonStyle::Destructive`. Enter means Cancel until the user moves.
- **C4** Cheap deletes don't ask. A row delete arms on the first Del
  ("Press Del again"); an Undo toast follows when it can be undone.
- **C5** All routes reach the same path: ×, middle-click, menu entry, key,
  prefix key. Check every route, not only the one the feature was built for.
- **C6** Nothing stays behind. Forgetting or removing a thing also stops what
  it started (herdr session, server, watcher, child process), or says that it
  keeps running.

## E. Errors and feedback

- **E1** No failure is reported with `show_notice` (Info). Failures use
  `show_error`; things that need a look use `show_warning`; done actions use
  `show_success`.
- **E2** Nothing fails silently. Check each `let _ =`, `.ok()`, `debug!` and
  ignored `Result` on outside work: spawning a tool, gh, curl, git, zenity,
  reading a file or database. A missing tool gets a toast.
- **E3** What the user must fix (settings, missing tool, unreachable source,
  fallback mode) also goes to `problems::report(source, text)`, and
  `problems::clear(source)` runs once it is fixed. Then the "!" badge shows it
  after the toast is gone.
- **E4** Errors reach the log (`warn!`/`error!`), not only stderr or debug.
- **E5** An empty list tells "nothing here" apart from "couldn't load". A
  failed load never shows the welcome or "no items" text.
- **E6** An error with a fix gets a toast button (Open settings, Open log,
  Retry, Update).
- **E7** Errors from background threads and other projects reach the UI. If
  one is dropped on purpose (too late, too old), the code says why.

## K. Keys

- **K1** New chords come from the table in `docs/ux-conventions.md`. A chord
  never gets a second meaning in another panel. Grep the chord before you add
  it, and check the prefix keys in `keys.rs` and the help panel.
- **K2** Esc steps back one level: query, then sub-mode, then close. A draft
  is never thrown away. The footer hint says "clear" or "close" to match.
- **K3** Footer hints, the F1 sheet and the handler agree. The hint shows the
  real chord ("Shift+R", not "R"), and clicking a hint does the same as the
  key (`chrome/hints.rs`).
- **K4** Every mouse action has a key, and every key action can be reached by
  mouse or menu: rename, context menus, row buttons.
- **K5** Shift+Tab goes back; Ctrl+J/K move; Ctrl+N/P never move.
- **K6** A key that changed meaning is listed in the merge report under
  "keys that changed".
- **K7** A button shows its label only. Its key goes in the footer, its tooltip
  and F1, never on the button or in its text. Footer hints and menu rows may
  show keys.

## O. Overlays, popups, panels

- **O1** A new popup is a new `Overlay` variant in `overlay.rs`, placed in
  `ORDER`. Set `is_dialog`/`is_panel`/`grabs`/`takes_keys`/`toggled_by`/`name`,
  and add `closing()` in `input.rs`. Don't write new `if self.x.is_some()`
  chains for pointer, key, scroll or paint order.
- **O2** Only the left button activates rows. Right-click and middle-click do
  nothing unless they are meant to.
- **O3** Opening it over another panel stacks right, or closes the other one.
  The prefix key and its own toggle key close it.
- **O4** It fits a small output (about 870 px wide or less). It clamps to the
  output, rows past the fold scroll, and Enter never acts on a row the user
  can't see.
- **O5** Hover and tooltip don't swallow keys or clicks. Pointer focus leaves
  the client under the popup.

## L. Look

- **L1** Sizes come from the tokens in `chrome/consts.rs` (`POPUP_RADIUS`,
  `ROW_HIGHLIGHT_RADIUS`, `DIALOG_WIDTH`). No new literal radius or dialog
  width.
- **L2** Use the shared painters: `Painter::button` with `ButtonStyle`,
  `row_highlight`, `query_field`, `segmented`, `chrome/thumb.rs` scrollbars,
  `layers::scrim` behind dialogs. One hover style: selected row in
  `selected`, hovered row in `element`.
- **L3** Contrast in both themes: text 4.5:1, marks and dots 3:1. A new pill,
  badge or status colour gets a contrast test like the pill tests.
- **L4** Blinking, pulsing and fades go through `anim::motion()` and respect
  `[ui] motion` (`full`, `reduced`, `off`).
- **L5** Long text ellipsizes and doesn't run into other parts. Rows of
  chips, tabs or pills overflow on purpose (a "› N" chip, a tooltip list).
  Two counts never hide each other.
- **L6** A drag handle shows a grab or resize cursor. A button that matters
  isn't shown only on hover or only on the active row.

## W. Words

- **W1** Project = top level (one store, one herdr session). Workspace = a
  folder or app inside it. "Settings" for the config file.
- **W2** Ellipsis only when more input follows. Help labels start with a
  verb. Toast and status text has no full stop; form errors are sentences.
  Inline status words are lowercase.
- **W3** A count counts what it says ("3 repos" counted workspaces).
- **W4** Lists that name all kinds are updated: the "+" menu and tooltip,
  launcher rows, help panel, inbox groups, docs.
- **W5** No internal jargon in user text. Say what happened and what the
  user can do ("A newer T3 Code is out, but gerdr can't fully read its
  threads yet", not "Newer T3 Code isn't readable yet").
- **W6** Dialog questions fit on one line at `DIALOG_WIDTH`. Check the
  dumped image, not the string.

## S. Settings

- **S1** A new table goes into `TABLES` in `config.rs` (fix the array length)
  and into `Config::read`. A typo in it shows on the "!" badge and doesn't
  drop the rest of the file.
- **S2** Saving the settings file applies it: `apply_settings` handles the
  new keys, and watchers or workers restart with the new values.
- **S3** A switch lives in the settings file, not in a side JSON file. A UI
  toggle writes it with `config::write_bool` (keeps comments). Old storage
  migrates once, with a toast.
- **S4** One switch per thing. Check that no other table already covers it
  (`[ui] notify` vs `[notifications] enabled`).
- **S5** Defaults are in the settings template and the docs, and match the
  code.

## D. Disk edits and long work

- **D1** Every edit to files on disk says what it did, and can be undone per
  file.
- **D2** Long work shows that it runs ("fetching…", the tasks panel), can be
  stopped, has a timeout, and doesn't block unrelated actions.
- **D3** It doesn't run git or another tool on the UI thread.

  Feature setup and Verify's checks are such work: each is a tasks panel
  row (chip "setup" or "checks", the feature named), says what runs on its
  stage's page on the feature tab ("setting up: api: npm ci (2 of 3)"),
  has the project's timeout, and Enter on a stage whose work runs says so
  rather than starting it again. Verify's checks each run in a terminal of
  the feature's workspace, all at once, and stop from their cards on its page.

## X. State across places

- **X1** The same count or "seen" state is shared everywhere it shows:
  sidebar, tab-strip inbox button, inbox panel, notifications.
- **X2** It works for every project, not only the active one, and survives a
  gerdr restart when it should (`state.json`, the store).
- **X3** Undo brings back everything the action changed, counts and seen
  marks included.
- **X4** No notification about what the user is looking at now. Notifications
  obey `[notifications] enabled`.
- **X5** A new kind (agent, workspace, card) keeps every capability the
  others have: close, rename, restart, move, inbox, help. Fix the general
  path; don't add a special case.

## G. Gate before merge

- **G1** `cargo build`, `cargo clippy` and `cargo test --bin gerdr` with 0
  warnings.
- **G2** A new or changed surface has a golden entry (`GOLDEN`,
  `GOLDEN_OVERLAYS`, `GOLDEN_STATUS` in `chrome/golden.rs`). Change a hash
  only after you look at the dump: `GERDR_DUMP_DIR=<dir> cargo test --bin
  gerdr dump_`.
- **G3** What tests can't see (pointer focus, popup stacking, notifications,
  real settings files) is tried in a nested niri on `wayland-1`, or listed as
  not tried live.
- **G4** When a rule changes, `docs/ux-conventions.md` changes with it.
  `graphify update .` runs after the code changes.

## Quick greps

Run these on the branch. They only find candidates; check each hit by hand.

```sh
added() { git diff main...HEAD -U0 -- src | grep '^+[^+]'; }

added | grep 'show_notice'                              # E1 failure as Info?
added | grep -E 'let _ = |\.ok\(\);|debug!\('           # E2 dropped errors
added | grep -E 'Command::new|spawn\('                  # E2 new tool calls
added | grep -E 'kill|stop_|close_|remove_dir|delete'   # C1 direct destructive calls
added | grep -E 'Keysym::|KEY_[A-Z]'                    # K1 new chords
added | grep -E 'keycaps\(|KEY_FONT_SIZE|hint_button\('  # K7 key on a button
added | grep -E 'radius|[0-9]+\.0 *[,)]|width: [0-9]+'  # L1 literal sizes
added | grep -E 'show_[a-z]+\(|toast' | grep '\."'      # W2 full stop in toasts
added | grep -E 'project|workspace' | grep '"'          # W1 term in user text
added | grep -E '\.json"|serde_json::to_'               # S3 side storage
git diff main...HEAD --stat -- src/config.rs src/overlay.rs src/input.rs
```
