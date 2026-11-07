# gerdr UX conventions

Rules every panel, popup and dialog follows. When a new surface needs a key
or a look, take it from here; when a rule has to change, change it here.
`docs/feature-checklist.md` lists where new features tend to break these
rules, for reviewing a branch before it merges.

## Keys inside panels and popups

| Key | Meaning everywhere |
| --- | --- |
| Esc | Step back one level: clear the query, then leave a sub-mode (search, comment, rename), then close. On the feature tab it folds the repository rows first. On a feature's Code review page, where its review is laid under the rail, it gives the rail the keys: Enter, Tab, Down or a press on the review gives them back. The footer hint reads "clear" while there is something to clear, "close" otherwise. A comment or commit draft is kept, never thrown away by Esc. |
| Enter | Do the selected row's main action. On the feature tab's page of a done stage it goes to the current stage: it never undoes a done stage. |
| Up / Down, PgUp / PgDn, Home / End | Move the selection. Ctrl+J / Ctrl+K also move it. Ctrl+N / Ctrl+P never move it. Where there is no list to page through, PgUp / PgDn scroll a page (the feature tab's stage page). |
| Tab / Shift+Tab | Next / previous focus area or form field. Shift+Tab always goes the other way. In a feature's code review: between the file list and the diff. |
| Left / Right | Move along a row of choices laid out across: the feature tab's stage rail (previous / next stage's page; Up / Down step it too), its repository strip, and the buttons of an unfolded repository row. |
| Ctrl+PgUp / Ctrl+PgDn | Previous / next item of a set shown one at a time: the feature tab's previous / next feature (its ‹ › buttons), the project commands dialog's next or previous tab. |
| Ctrl+N | New / create (project, branch, todo). |
| Ctrl+O | Open (in a tab, in the browser, the file; in a plan review's sheet, the suggestion's first citation in Zed). |
| Ctrl+Shift+O | Open the selected row's page where Ctrl+O already opens a tab (feature tab: a repository's pull request, Verify's evidence once a run of its checks wrote it, or a fix plan already sent). |
| Ctrl+E | Edit (settings, a todo, open the file in the editor, an AI review finding's fix; on the feature tab's Build and Verify, the project's commands: Setup and Checks). |
| Ctrl+R | Refresh / reload (feature tab: start the selected stage's agent afresh, Build's too). |
| c / Shift+E | On an AI review finding, in its diff: write or edit the user's comment on it, in the row editor under it / edit its fix. On AI review's page `c` does the same in a field on the focused finding's card, Shift+Del deletes the comment, Ctrl+E edits the fix; a dismissed finding's comment can still be written, changed and deleted, its fix not. On a card of a comment left in the diff, `c` and Ctrl+E edit it in a dialog. Off a reviewer's card `c` adds a comment of the user's own. |
| Shift+C / Shift+F | In a feature's diff: add a comment on no line or file, listed first as General comments / mark every undecided finding to fix (Shift+F also on AI review's page). On a file under review, Shift+C comments on the whole file. |
| Ctrl+Shift+R | On the feature tab: show the selected stage's agent, done stage or not, or go back to its chat in a new tab once its tab is closed (Claude Code and Cursor; Ctrl+R starts afresh). On a later stage's page, while work sent back to the Build agent is under way, it shows that agent. |
| Ctrl+Enter / Ctrl+Shift+Enter | The panel's finishing action: commit / commit and push (repos), create (branch menu), approve / send back (a file under review; Verify's evidence is only read, so there it says so and does nothing); approve, also from the feature tab's stage page while its brief or plan is ready for review (its "Approve" button on the file card; comments left on the file ask first); approve / plan fixes for the comments (a feature's diff at Code review); make the fixes with no plan / plan them first (a feature's diff and AI review page, when the user added no comment, edited no fix and added no item of their own: greyed in the diff until every finding is decided); plan fixes, on either chord (the same once the user wrote anything of their own: then a plan is required); mark passed / plan the fixes of the failures (a feature's Manual test page, with criteria); make the fixes / send back (a fix plan, in its review; Ctrl+Enter also on the page of the stage it came from, and again once its agent stopped with fixes left); write the final plan, or use the plan as it is / ask the reviewer (a plan review's sheet: while suggestions are still to decide, Ctrl+Enter stays and says how many). On the file of a done stage, which is only read: go to the current stage / reopen the stage, as on its page in the feature tab (Ctrl+Shift+Enter there), until the stage after it has made something. |
| Ctrl+Shift+V | In a feature's file's tab: go between Review and Split (its brief or plan, read on the stage's page), or to the next of Preview, Review and Split (Verify's evidence). Split, the two side by side, following its edits and caret and scrolling together, is there only on an output 1400 px wide or more. A new tab of a brief or plan opens on Split there, on Review on a narrower one; pressing its button again only switches to the open tab, keeping what it shows. Esc goes back to the feature tab. |
| F2 | Rename. |
| Ctrl+S | Switch to / check out a branch (repos, worktrees). |
| Ctrl+Shift+S | Squash a worktree into main. |
| Prefix, then Enter | Send the prompt of the stage agent waiting in the focused pane (also the arrow on its pane's bar, and Send prompt on the stage's page). A stage's agent starts on its prompt; one waits only when checks ran on the code as it opened, or its line couldn't be typed. |
| Ctrl+Shift+U | Set a feature's worktrees up (feature tab, Build and Verify): copy the ignored files and environments they lack from main, then run the project's setup commands. Nothing there is overwritten. |
| Ctrl+Shift+I | Show a feature's setup log (feature tab, Build and Verify, once a setup started; also the "Setup log" button, the setup's row in the tasks panel and a failure's toast): its last run in the select and copy viewer, followed live while it runs. Selecting holds the following until the selection goes. |
| Shift+Enter | In a field of several lines: a new line, Enter being the panel's action (a diff's review comment, the project commands dialog). |
| Space | In the project commands dialog, on a borrowed check of the "This feature" tab: skip it for this feature, or run it again (also its Skip / Run button). Nothing is saved until Save. On a check's card on Verify's page: skip the check it borrows, or stop skipping it (also its Skip / Don't skip button). It saves at once, with an Undo, and applies from the next run. |
| Alt+U | In the project commands dialog: fill the focused empty field with what is suggested for it (also its Use button). Nothing is saved until Save. |
| Alt+B | In the new feature dialog: choose the branch the selected repository's worktree starts from (the first ticked one from the title; also its "from <branch>" button). It lists the repository's branches, the default first; typing filters them, Enter uses the selected one, Esc clears the filter and then goes back. The default is the branch the project's settings give it (`workflow.base`), else what origin says now, else the local `origin/HEAD`, else the checked out branch. A repository added to a feature later starts from that default too. |
| Alt+X | In the dialog connecting a machine: press the button fixing what failed (Copy command, Open terminal). Only there. |
| Ctrl+Shift+T | Start a feature's sandbox on the feature tab's Manual test page, after a dialog asks which commands run in it (Space checks; the feature keeps the choice, and Ctrl+Shift+G and Ctrl+Shift+X go by it without asking); stop it, asking, once it runs (at once while it starts). With the sandbox on another machine it shows the sandbox's terminals instead, and Ctrl+Shift+Q stops it. |
| Ctrl+Shift+Y | Open the running sandbox's app, in a tab inside it (feature tab, Manual test). With the sandbox on another machine, in a tab here, through ports opened on this machine. |
| Ctrl+Shift+G | Start the sandbox again on the same data (feature tab, Manual test). |
| Ctrl+Shift+X | Reset the sandbox's data from main's, or remove a stopped sandbox with its data, asking first (feature tab, Manual test). |
| Ctrl+Shift+A | Pick the next agent on the feature tab's selected stage (the chips on its agent card). It applies from the agent's next start; one running goes on. |
| Ctrl+Shift+E | Take over a feature another machine writes, on the feature tab (also the "Take over" button): it is written here from then on. Any press that changes such a feature does this first. |
| Ctrl+Shift+Q | Stop the selected stage's agent on another machine, on the feature tab (also the "Stop agent" button on its agent card), asking while it works; on Manual test, the sandbox on another machine, asking. Closing its Show agent tab only ends the view. |
| Ctrl+Shift+W | Pick the machine the selected stage's next agent, Verify's next run or the next sandbox goes on, on the feature tab: this one, then each connected machine, then whichever is least busy (the "Runs here" or "Runs on" button on its agent card, or on Verify's and Manual test's page). It applies from the next start; what runs stays where it is. The project's file can give each stage a machine under `machines`, or `least busy`. Not offered with no machine saved, while Verify's checks run, for T3 Code, or while a sandbox has a record: its data is on its machine, so it is removed first. |
| Ctrl+Shift+I | Pick the model of the selected stage's agent on the feature tab (the "Model:" button on its agent card), for Claude Code and Cursor. It applies from the agent's next start; one already open keeps its own. Default leaves it to the CLI. Cursor also keeps a picked model as its default for all its chats, as its own `/model` does, and its picker says so. |
| Ctrl+Shift+B | On the feature tab's Build page: go between one agent through all milestones and a fresh agent per milestone (also the "Agents" choice on the page). It applies from the next milestone; an agent at work goes on. |
| Ctrl+Shift+K | Skip Plan review on the feature tab, asking only when something would be lost. |
| Ctrl+Shift+D | On the feature tab: from a final plan back to the decisions. |
| Ctrl+Shift+L | In the new feature dialog, the project picker (again: back to the repositories; also its "Link project…" button). On the feature tab, show or hide all repositories: the strip unfolds into rows of what the feature holds, its context, those removed and those not added, each with the buttons of what it can become (also the strip's end button). |
| Ctrl+Q | Stop a project's session (project menu). |
| Alt+E | On the feature tab's AI review page: show the findings of every earlier review, or fold them (also each review's Show and Hide buttons). Once the next review is read, the one before folds to a card with its name and how it went. Its findings are a record, as they stood then. Findings stay on the page once sent to Build, while the code changes and while the next review runs: until that one is read they can only be looked at, and `f`, Ctrl+E and Del say why. |
| Ctrl+H / Ctrl+Shift+H | Snooze / snooze until tomorrow; Alt+H shows the hidden items (inbox). |
| Ctrl+F or / | Search within what has focus. |
| Del | Delete the selected row: the first press arms it (row shows "Press Del again"), the second deletes; an Undo toast follows when it can be undone. On a repository of a feature with several, Del takes it out of the feature: one on the feature's branch asks at once (no arming, see "Destructive actions"), and is refused with commits; one kept only as context arms, and offers Undo. Elsewhere on the feature tab, Del abandons the feature. On an AI review finding, on its stage's page (its card) or in a feature's code review, Del dismisses it and asks why first (no arming), or reopens a dismissed one; `f` marks it to fix. On the page `f` on an undecided card and a confirmed dismissal move the keyboard to the next card left to decide; no card changes place, and dismissed or fixed ones close to a line until selected. An item the user added themselves, or a comment left in the diff shown as a card, is a row delete: the first Del arms it, the second deletes, and an Undo toast follows (not once it went to Build). Its Delete button goes at once, with the same Undo; in the diff an item of the user's own also goes on the first Del. On a criterion's card on a feature's Manual test page, Del clears its result at once (no arming: Space or `f` gives it again). In a plan review's sheet, Del clears a suggestion's decision at once (no arming: Space or `r` gives it again); one of your own it removes, so that arms first, and offers Undo. |
| Ctrl+Shift+Del | Abandon what is in progress, asking first: abort a merge or rebase (repos), start the feature tab's current stage over, drop the fix plan open on its page, before its fixes went or once its agent stopped with fixes left (what it was to fix comes back, and its agent isn't stopped). Starting over closes its agent, moves what it made to the feature's archive and frees the stage before it. |
| Ctrl+Del | Dismiss every listed row of the selected row's group (inbox Services; also its label's "Dismiss all" button). One Undo toast brings them all back. |
| Ctrl+I | Ignore the selected service error, or stop ignoring it (inbox). An ignored error goes under "Ignored errors", folded to its count; Alt+I (or the label's Show button, or a click on the label) lists them, dim. It sends no notifications or counts. Del there forgets it (armed first). Undo toast. |
| r / s, Shift+R / Shift+S | The feature tab's Verify page: on a check's card (Tab goes to the cards, Enter shows its terminal), run it again, or stop it; anywhere on the page, run the failed ones again, or stop them all. While the checks go, Enter only says they run: it never stops them. Ctrl+Shift+U and Ctrl+Enter refuse there, as a setup or fixes would change the code under them, and their buttons are hidden. |
| ] / [ | Any diff (repos panel, worktree, code review, a file under review): go to the next or previous AI review finding or comment, in the diff's order, past the last back to the first. A folded file unfolds, and a long one shows whole, to show it. The diff's title row has the same as buttons, and its scrollbar a tick per comment while no search is open. |
| Space | On the feature tab's Ship page, once Tab has gone to its rows: tick a step or what comes after it, or move a repository's choice to its next way (Pull request, Merge only, Nothing; also a click). It saves at once. Enter still presses Ship…, or Finish… with no step ticked, which asks first as nothing gets pushed or merged. |
| Space / Alt+Shift+Down | A feature's code review: mark the file viewed, or not (as Space checks a file in the repos panel), on the diff or on its line in the file list, the same tick; on a folder's or a repository's line, the files under it that can be, a toast saying how many were left out as not committed; go to the next file not viewed (Alt+Shift+Up: the one before). A criterion's card on a feature's Manual test page: mark it passed, or not; `f` marks it failed, with a note saying what went wrong, written on the card (Enter or Esc saves it; with nothing written, Esc leaves it as it was). A plan review's sheet: accept the suggestion, or clear that (a question not answered asks for its answer first, then is accepted); `r` rejects it, or clears that; `n` writes a note; `a` adds one of your own, Esc keeping what was written as a draft. |
| Alt+F / Alt+Left / Alt+Right | The repos panel's file tree and a feature's code review's file list: list the files flat or by folder (kept in `state.json`, one for both) / fold everything / unfold everything (also the buttons on the list's toolbar). |
| F1 | Every key of the focused panel, when its footer can't show them all. |
| Ctrl+C | Copy. Never closes anything. In any diff (repos panel, worktree, a commit, code review, a file under review, evidence) it copies what the cursor is on, as `y` yanks it: the selection, else the line (a removed one too), a file's path, a hunk's lines, a comment or finding's text. A read-only diff refuses edits and pastes, never yanks. The cursor's row has the same as a Copy button. |

A chord means one thing across all panels. A verb with direction says it in
the label ("Merge main into branch", "Squash into main").

A key is written in one place, not on the button that does the same thing:

- A button shows its label only, with no key on it or in its text.
- The footer shows the panel's keys, as keycaps. A click on a hint presses its
  key, so a footer hint isn't a button in this sense.
- A tooltip shows a button's key on hover.
- F1 lists every key, including those the footer can't fit.
- A menu row may show its key at its right end, since a menu has no footer.

## Destructive actions

- Ask before anything that stops a running process, deletes unmerged work or
  can't be undone. Don't ask when nothing is lost (idle chat, empty shell).
- Dialog buttons: Cancel on the left, the action on the right. Destructive
  actions are painted in the error colour and are never the default:
  Enter on a destructive dialog means Cancel until the user moves to it.
- A dialog offers "Don't ask again" only where the choice is remembered and
  can be found again in Settings.
- Cheap destructive actions don't ask; they show an Undo toast instead.
- One exception, asked for: a repository on a feature's branch never leaves the
  feature without asking, even with nothing to lose. It takes a worktree and a
  branch with it, and agents may be working there.

## Feedback

- Toast levels: Success / Info for what happened, Warning for what needs a
  look, Error for what failed. Never report a failure with Info.
- Errors and warnings stay longer than info and pause while hovered; every
  toast can be dismissed. Toasts are painted above every non-modal popup,
  and above the plan review sheet, which is worked in for a while.
- What the user must fix (config, missing tool, unreachable source) goes to
  `problems::report` as well, so it can be seen after the toast is gone. It
  also lands in the log file.
- Every edit to files on disk (diff view cuts, restores) says what it did.
- A stage's agent starts on its prompt in a tab behind the stage's page,
  clicked or not, and the page's Show agent goes to it. When Verify passes,
  the AI review page comes to the front of the feature's workspace; another
  workspace keeps the screen. Discover's agent is the exception: its first
  message is a question, so Start agent shows its tab, ready to answer in.
- An agent placed on another machine opens no tab here: nothing from another
  machine shows until asked. Show agent opens one tab attached over SSH, as
  does Discover's Start agent once its agent is up, if the user stayed on the
  feature's page. That tab takes the terminal from any other view, and closing
  it asks nothing, as it only ends the view. The agent's card says "on
  <machine>". A machine out of
  reach reads "<machine> can't be reached", and its runs wait: only a list of
  that machine's panes can say a pane is gone.
- Verify's checks and a setup placed on another machine follow the same rule:
  a check's Show opens its terminal in one tab, on request, and closing that
  tab ends only the view. A check told to stop while its machine is out of reach
  ends here after a moment and says it may still run there. A setup there sends
  small ignored files like `.env` to that account, privately, and its page
  says what it copied; environments stay here, and its commands build them.
- A connected machine's features of the open project show here too, with the
  machine's name first on their cards. The gerdr that made a feature writes it,
  and only one writes it at a time. Its page says where it is written, whether that
  machine can be reached, and whether its gerdr is open. It stays on the sidebar
  with the machine away, as it last was.
- A feature's page offers the same buttons on any machine. Pressing one that
  changes it moves the feature here first (Take over does only that), and then the
  press goes on, or says the feature changed on the way. Its diff and files take no
  comments or ticks until then. While it moves, the page and a tasks row with Stop
  say so. Files and commits come, and what isn't committed there keeps it there
  until it is. Work on a worker goes on and is followed from here. What runs in the
  writer's own tabs stays there, so while it works the press says to wait; at rest,
  its tab stays there, and its chat goes back on that machine. A sandbox running
  there keeps the feature there too; one on a worker goes on, and comes along. The old writer shows the feature from the new
  one if connected to it, and otherwise says so.
- A gerdr open on the writer has to agree. When it doesn't answer, or the machine
  can't be reached, a dialog asks, as what happens there from then on is kept aside
  (a toast, the "!" badge and the file named there), its commits staying on its
  branch. Once both can be seen, the record moved most often stands.
- A terminal of such a feature opens here on request as well. One on a worker
  takes the terminal, as any Show does. One in the writer's own session only
  looks: that machine's gerdr keeps it, and keys go nowhere. A feature's code comes
  here once it is opened, by commits: what isn't committed there doesn't show.
- A sandbox runs on the machine writing its feature, or on the one Manual test's
  page picked: that one needs Docker, pasta and the project's containers running,
  as those are what's copied, and its page says what it found none of. Its setup
  runs there first. It stays there until it is removed: its data is there.
- Any gerdr showing its feature says how it stands, and can open its app, show its
  terminals, and stop, restart, reset or remove it, with no need to take the feature
  over. A gerdr open on the writer does that, so its record says so. With none open,
  Stop and Remove go to the sandbox's machine directly, and Restart and Reset say to
  open gerdr there. Starting one is a change to the feature, so it moves the feature
  here first, as any press does.
- Open app opens the app's ports here under
  the same numbers, for as long as a page on `localhost` shows in the feature's
  workspace and the link is up. A port taken here is said by name, on the "!" badge
  too, and a page never opens on it. Show terminals only looks, as above. Anything
  on this machine can reach those ports while they are open, as with `ssh -L`.
- A line gerdr types to an agent and finds still in its input box gets Enter
  again, up to six times over about four minutes. Only then is the user told.
- Work the user sends to an agent (findings, review comments, failed tests)
  shows that agent's tab, where it runs or waits for its prompt. Work gerdr
  sends by itself doesn't take the screen. Either way the stage's node on the
  rail and the page of the stage the feature is at say the agent runs again or
  waits, with a way to its tab. While that work is under way the rail reads as
  turned back: done stages from Build on go grey, as they ran once, and a line
  runs from the stage that asked back to Build. The stages before Build stay
  as they are, as fixes don't touch what they made. A stage whose mark went
  stays grey until it is marked done again, so it doesn't read as one that
  never ran.
- Build's page keeps a "Verify failures" section for the fix rounds Verify's
  failures started: what the newest one was sent, then a line for each older
  one (its run, its agent, how it stands, how the next run went). It only
  reads: the full output is on Verify's page, the ticks in the list below it
  under the same "Verify N", where other rounds read "Review N". Setup goes
  last on that page; a failed setup is still said by the status at its top.

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
