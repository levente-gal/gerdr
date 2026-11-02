# gerdr UX conventions

Rules every panel, popup and dialog follows. When a new surface needs a key
or a look, take it from here; when a rule has to change, change it here.
`docs/feature-checklist.md` lists where new features tend to break these
rules, for reviewing a branch before it merges.

## Keys inside panels and popups

| Key | Meaning everywhere |
| --- | --- |
| Esc | Step back one level: clear the query, then leave a sub-mode (search, comment, rename), then close. The footer hint reads "clear" while there is something to clear, "close" otherwise. A comment or commit draft is kept, never thrown away by Esc. |
| Enter | Do the selected row's main action. |
| Up / Down, PgUp / PgDn, Home / End | Move the selection. Ctrl+J / Ctrl+K also move it. Ctrl+N / Ctrl+P never move it. |
| Tab / Shift+Tab | Next / previous focus area or form field. Shift+Tab always goes the other way. |
| Ctrl+N | New / create (project, branch, todo). |
| Ctrl+O | Open (in a tab, in the browser, the file). |
| Ctrl+E | Edit (settings, a todo, open the file in the editor). |
| Ctrl+R | Refresh / reload. |
| F2 | Rename. |
| Ctrl+S | Switch to / check out a branch (repos, worktrees). |
| Ctrl+Shift+S | Squash a worktree into main. |
| Ctrl+Q | Stop a project's session (project menu). |
| Ctrl+H / Ctrl+Shift+H | Snooze / snooze until tomorrow; Alt+H shows the hidden items (inbox). |
| Ctrl+F or / | Search within what has focus. |
| Del | Delete the selected row: the first press arms it (row shows "Press Del again"), the second deletes; an Undo toast follows when it can be undone. |
| Ctrl+Del | Dismiss every listed row of the selected row's group (inbox Services; also its label's "Dismiss all" button). One Undo toast brings them all back. |
| Ctrl+I | Ignore the selected service error, or stop ignoring it (inbox). An ignored error stays listed, dim, under "Ignored errors", and sends no notifications or counts. Del there forgets it (armed first). Undo toast. |
| F1 | Every key of the focused panel, when its footer can't show them all. |
| Ctrl+C | Copy. Never closes anything. |

A chord means one thing across all panels. A verb with direction says it in
the label ("Merge main into branch", "Squash into main").

## Destructive actions

- Ask before anything that stops a running process, deletes unmerged work or
  can't be undone. Don't ask when nothing is lost (idle chat, empty shell).
- Dialog buttons: Cancel on the left, the action on the right. Destructive
  actions are painted in the error colour and are never the default:
  Enter on a destructive dialog means Cancel until the user moves to it.
- A dialog offers "Don't ask again" only where the choice is remembered and
  can be found again in Settings.
- Cheap destructive actions don't ask; they show an Undo toast instead.

## Feedback

- Toast levels: Success / Info for what happened, Warning for what needs a
  look, Error for what failed. Never report a failure with Info.
- Errors and warnings stay longer than info and pause while hovered; every
  toast can be dismissed. Toasts are painted above every non-modal popup.
- What the user must fix (config, missing tool, unreachable source) goes to
  `problems::report` as well, so it can be seen after the toast is gone. It
  also lands in the log file.
- Every edit to files on disk (diff view cuts, restores) says what it did.

## Words

- Project: the top level (one store file, one herdr session). Workspace: a
  folder or app inside a project. Never use one for the other.
- "Settings" for the config file, everywhere.
- An ellipsis ends a label only when more input follows (a dialog or a
  picker), not when a panel opens.
- Help labels start with a verb ("Open inbox", "Close tab").
- Toast and status text has no full stop; form errors are sentences.
- Status words: lowercase where drawn inline (bars, pills), sentence case in
  tooltips.

## Look

- Tokens in `chrome/consts.rs`: `POPUP_RADIUS` for popup corners,
  `ROW_HIGHLIGHT_RADIUS` for list row highlights, `DIALOG_WIDTH` for modal
  dialogs. One hover style: the selected row in `selected`, a hovered row in
  `element`.
- One scrollbar (`chrome/thumb.rs`), one button painter
  (`Painter::button`), one query field with the magnifier.
- `[ui] motion = "full" | "reduced" | "off"`: blinking dots and pulses only
  with "full"; fades only with "full" or "reduced" (see `anim::motion`).
- Text meets 4.5:1 contrast, marks and dots 3:1, in both themes.
