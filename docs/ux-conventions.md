# gerdr UX conventions

Rules every panel, popup and dialog follows. When a new surface needs a key
or a look, take it from here; when a rule has to change, change it here.
`docs/feature-checklist.md` lists where new features tend to break these
rules, for reviewing a branch before it merges.

## Keys inside panels and popups

| Key | Meaning everywhere |
| --- | --- |
| Esc | Step back one level: clear the query, then leave a sub-mode (search, comment, rename), then close. The footer hint reads "clear" while there is something to clear, "close" otherwise. A comment or commit draft is kept, never thrown away by Esc. |
| Enter | Do the selected row's main action. On the feature tab's page of a done stage it goes to the current stage: it never undoes a done stage. |
| Up / Down, PgUp / PgDn, Home / End | Move the selection. Ctrl+J / Ctrl+K also move it. Ctrl+N / Ctrl+P never move it. Where there is no list to page through, PgUp / PgDn scroll a page (the feature tab's stage page). |
| Tab / Shift+Tab | Next / previous focus area or form field. Shift+Tab always goes the other way. |
| Left / Right | Move along a row of choices laid out across: the feature tab's stage rail (previous / next stage's page; Up / Down step it too). |
| Ctrl+PgUp / Ctrl+PgDn | Previous / next item of a set shown one at a time: the feature tab's previous / next feature (its ‹ › buttons). |
| Ctrl+N | New / create (project, branch, todo). |
| Ctrl+O | Open (in a tab, in the browser, the file). |
| Ctrl+Shift+O | Open the selected row's page where Ctrl+O already opens a tab (feature tab: a repository's pull request, or Verify's evidence once a run of its checks wrote it). |
| Ctrl+E | Edit (settings, a todo, open the file in the editor). |
| Ctrl+R | Refresh / reload (feature tab: start the selected stage's agent afresh, Build's too). |
| Ctrl+Shift+R | On the feature tab: show the selected stage's agent, done stage or not, or go back to its chat in a new tab once its tab is closed (Claude Code and Cursor; Ctrl+R starts afresh). |
| Ctrl+Enter / Ctrl+Shift+Enter | The panel's finishing action: commit / commit and push (repos), create (branch menu), approve / send back (a file under review, a feature's code review; Verify's evidence is only read, so there it says so and does nothing); mark passed / send the failures to Build (a feature's manual test sheet). On the file of a done stage, which is only read: go to the current stage / reopen the stage, as on its page in the feature tab (Ctrl+Shift+Enter there), until the stage after it has made something. |
| Ctrl+Shift+V | In a feature's file's tab: go between Review and Split (its brief or plan, read on the stage's page), or to the next of Preview, Review and Split (Verify's evidence). Split, the two side by side, following its edits and caret and scrolling together, is there only on an output 1400 px wide or more. Esc goes back to the feature tab. |
| F2 | Rename. |
| Ctrl+S | Switch to / check out a branch (repos, worktrees). |
| Ctrl+Shift+S | Squash a worktree into main. |
| Ctrl+Shift+A | Pick the next agent on the feature tab's selected stage (the chips on its agent card). It applies from the agent's next start; one running goes on. |
| Ctrl+Shift+L | In the new feature dialog, the project picker (again: back to the repositories; also its "Link project…" button). On the feature tab before Ship, the dialog that links more repositories and projects, and removes those unticked (also its "Link repositories…" button; the same picker inside). |
| Ctrl+Q | Stop a project's session (project menu). |
| Ctrl+H / Ctrl+Shift+H | Snooze / snooze until tomorrow; Alt+H shows the hidden items (inbox). |
| Ctrl+F or / | Search within what has focus. |
| Del | Delete the selected row: the first press arms it (row shows "Press Del again"), the second deletes; an Undo toast follows when it can be undone. On a repository of a feature with several, Del takes it out of the feature the same way; it asks first if that loses uncommitted work, and refuses one with commits. Elsewhere on the feature tab, Del abandons the feature. On an AI review finding in a feature's code review, Del dismisses it and asks why first (no arming), or reopens a dismissed one; `f` marks it to fix. In a feature's manual test sheet, Del clears a criterion's result at once (no arming: Space or `f` gives it again). |
| Ctrl+Shift+Del | Abandon what is in progress, asking first: abort a merge or rebase (repos), start the feature tab's current stage over. Starting over closes its agent, moves what it made to the feature's archive and frees the stage before it. |
| Ctrl+Del | Dismiss every listed row of the selected row's group (inbox Services; also its label's "Dismiss all" button). One Undo toast brings them all back. |
| Ctrl+I | Ignore the selected service error, or stop ignoring it (inbox). An ignored error goes under "Ignored errors", folded to its count; Alt+I (or the label's Show button, or a click on the label) lists them, dim. It sends no notifications or counts. Del there forgets it (armed first). Undo toast. |
| Space / Alt+Shift+Down | A feature's code review: mark the file viewed, or not (as Space checks a file in the repos panel); go to the next file not viewed (Alt+Shift+Up: the one before). A feature's manual test sheet: mark the criterion passed, or not; `f` marks it failed, with a note saying what went wrong. |
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
  toast can be dismissed. Toasts are painted above every non-modal popup,
  and above the manual test sheet, which is worked in for a while.
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
