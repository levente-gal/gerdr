# gerdr UX conventions

Rules every panel, popup and dialog follows. When a new surface needs a key
or a look, take it from here; when a rule has to change, change it here.
`docs/feature-checklist.md` lists where new features tend to break these
rules, for reviewing a branch before it merges.

## Keys inside panels and popups

| Key | Meaning everywhere |
| --- | --- |
| Esc | Step back one level: clear the query, then leave a sub-mode (search, comment, rename), then close. On the feature tab it folds the repository rows first. On a feature's Code review page, where its review is laid under the rail, it gives the rail the keys: Enter, Tab, Down or a press on the review gives them back. The footer hint reads "clear" while there is something to clear, "back" where it goes back to a page, "close" otherwise. Esc never saves and never throws away: a note or comment being written (on a card, in a diff, on the criteria strip, in a dialog) is put away as a draft, unsaved, and the next one opened on the same card, lines or criterion goes on with it; the footer says "keep draft" then. Enter saves it; Ctrl+Shift+Del (or a Discard button) throws it away. A comment written in a dialog (one of the user's own, a general comment on Manual test) has the same Save and Discard buttons as one written inline; a dialog deciding an item keeps Cancel, which keeps the draft as Esc does. A commit draft is kept too. |
| Enter | Do the selected row's main action. On the feature tab's page of a done stage it goes to the current stage: it never undoes a done stage. On Manual test's page, with criteria left to try, no sandbox running and one to start, it starts the sandbox (Start sandbox…, then the page's primary); else it goes to the criteria. On a pull request's review, Enter on its current stage moves it to the new head GitHub has (Update to new head, back to Verify) while there is one, and finishes it, asking, once it was merged or closed. At Verify, while its worktree is at the head reviewed with nothing changed, GitHub's checks are its Verify: gerdr marks it done once they all passed ("by GitHub CI"), and Enter runs the failed GitHub Actions runs again on GitHub, opens its checks page while they go (or when they can't run again from here), or marks it done by hand when GitHub ran none. |
| Up / Down, PgUp / PgDn, Home / End | Move the selection. Ctrl+J / Ctrl+K also move it. Ctrl+N / Ctrl+P never move it. Where there is no list to page through, PgUp / PgDn scroll a page (the feature tab's stage page). |
| Tab / Shift+Tab | Next / previous focus area or form field. Shift+Tab always goes the other way. In a feature's code review: between the file list and the diff; a key the list doesn't take goes to the diff, and the keyboard with it. |
| Left / Right | Move along a row of choices laid out across: the feature tab's stage rail (previous / next stage's page; Up / Down step it too), its repository strip, and the buttons of an unfolded repository row. On a criterion of a feature's Manual test page, as on a tree's row: fold it to its line / open it to its steps (also its chevron). On a card of a review page (AI review's findings, Plan review's suggestions, comments) the same: fold it to its line / open it, kept while the panel lives, over what selection and its decision would show. |
| Ctrl+PgUp / Ctrl+PgDn | Previous / next item of a set shown one at a time: the feature tab's previous / next feature (its ‹ › buttons), the project commands dialog's next or previous tab, the AI work stats' next or previous tab (Features, Agents, Checks). |
| Ctrl+N | New / create (project, branch, todo). |
| Ctrl+O | Open (in a tab, in the browser, the file; on a Plan review suggestion's card, the first code it cites, in Zed; in the AI work stats, a run's kept transcript (a Cursor chat's messages) or a check's last failure log, as text in the select and copy viewer, whose Esc goes back to the stats as they were). |
| Ctrl+Shift+O | Open the selected row's page where Ctrl+O already opens a tab (feature tab: a repository's pull request, Verify's evidence once a run of its checks wrote it, or a fix plan already sent; else, on a pull request's review, that pull request on GitHub, its card's Open pull request). On the Ship page: the plan of a resolution of conflicts (in its review: Ctrl+Enter resolves as planned, Ctrl+Shift+Enter sends it back to its agent with the comments), or the selected repository's pull request. In the inbox, on a pull request: its review in gerdr, started if it has none (the GitHub card's chip menu says Start review / Open review). |
| Ctrl+E | Edit (settings, a todo, open the file in the editor, an AI review finding's fix, a Plan review suggestion's change in a dialog; on the feature tab's Build and Verify, the project's commands: Setup and Checks). On the Ship page: the summary, in Zed. |
| Ctrl+R | Refresh / reload (feature tab: start the selected stage's agent afresh, Build's too; the AI work stats: draw its stats again from the kept files, then read them). On the Ship page: write the summary again. |
| c / Shift+E | On an AI review finding, in its diff: write or edit the user's comment on it, in the row editor under it / edit its fix. On AI review's page `c` does the same in a field on the focused finding's card, Shift+Del deletes the comment, Ctrl+E edits the fix; a dismissed finding's comment can still be written, changed and deleted, its fix not. On a card of a comment left in the diff, `c` and Ctrl+E edit it in a dialog. Off a reviewer's card `c` adds a comment of the user's own. Plan review's suggestions take the same keys, on their cards on the page and as flags in the plan (its Review in plan tab): `c` writes the user's comment, a question's answer (Shift+Del on the page deletes it), Ctrl+E on the page and Shift+E in the plan edit the change in the user's words, which is accepted with it; off a reviewer's card, `c` adds a suggestion of the user's own in a dialog, its title on the first line. Once a card has the user's comment, its Comment button says Edit comment (Edit answer on a question) in the same place, so no button beside it moves; the comment's own card keeps Delete. In a diff or the plan the footer lists what the flag's buttons do, in their words (`f` accept, Del reject…, Shift+E edit change…). On a feature's Manual test page, and on the criteria strip over its sandbox, `c` adds a comment of the user's own on no criterion, in a dialog: a row after the criteria, open until marked done (Space; `f` makes it not done again), Ctrl+E edits it, Del deletes it (arming first, with an Undo). |
| Shift+C / Shift+F | In a feature's diff: add a comment on no line or file, listed first as General comments / mark every undecided finding to fix (Shift+F also on AI review's page; on Plan review's page it accepts every open suggestion, a question with no answer staying open). On a file under review, Shift+C comments on the whole file. |
| Ctrl+Shift+R | On the feature tab: show the selected stage's agent, done stage or not, or go back to its chat in a new tab once its tab is closed (Claude Code and Cursor; Ctrl+R starts afresh). On a later stage's page, while work sent back to the Build agent is under way, it shows that agent. |
| Ctrl+Enter / Ctrl+Shift+Enter | The panel's finishing action: commit / commit and push (repos), create (branch menu), approve / send back (a file under review; Verify's evidence is only read, so there it says so and does nothing); approve, also from the feature tab's stage page while its brief or plan is ready for review (its "Approve" button on the file card; comments left on the file ask first); approve / plan fixes for the comments (a feature's diff at Code review; on its page, also the buttons at the right of the line under the rail, which says what comes next. Approve is greyed, saying what holds it up, until nothing does, then it is the one to press); make the fixes with no plan / plan them first (a feature's diff and AI review page, when the user added no comment, edited no fix and added no item of their own: greyed in the diff until every finding is decided); once the user wrote anything of their own a plan is required: Ctrl+Shift+Enter plans the fixes and Ctrl+Enter says a plan comes first (Plan fixes is Ctrl+Shift+Enter on every page and in every diff, Make fixes Ctrl+Enter); make the fixes with no plan / plan them first (a feature's Verify page once its checks failed, also off its check cards); make the fixes with no plan / plan them first (a feature's Manual test page, while failures wait to be sent; with comments of the user's own among them a plan is required, so Ctrl+Shift+Enter plans them and Ctrl+Enter says a plan comes first; with none, Ctrl+Enter marks passed, which comments still open refuse; once fixes it asked for changed the code, mark passed takes the feature back to Verify, then AI review and Code review of the changes, then Manual test again with what it found kept, but for passes of criteria the fixes' agent said they changed, which are untested again and say so); make the fixes / send back (a fix plan, in its review; Ctrl+Enter also on the page of the stage it came from, and again once its agent stopped with fixes left, when Send back is gone. A fix plan already made is only read, and no key sends it again: its card's quiet "Send … again" button does, only while the stage has nothing new to send); write the final plan, or use the plan as it is / ask the reviewer about the user's comments (Plan review's page and its Review in plan tab: while suggestions are still to decide, Ctrl+Enter stays and says how many). On the file of a done stage, which is only read: go to the current stage / reopen the stage, as on its page in the feature tab (Ctrl+Shift+Enter there), until the stage after it has made something. On someone else's pull request's review nothing goes to an agent: findings marked to comment, Code review's comments and Manual test's failures wait for the review on GitHub, so Code review's Ctrl+Enter approves with comments left, and Manual test's marks it done once every criterion is tried, failures and all (Mark done); Ctrl+Shift+Enter says where they go instead. On a pull request's review's Ship page, Ctrl+Enter approves it and Ctrl+Shift+Enter requests changes, each asking first and posting one review to GitHub (someone else's only: GitHub takes neither of one's own); Enter pushes the user's own fixes, comments while something waits to be posted, finishes once a review went, else approves; Ctrl+E edits the review's own text (`review.md`). A submitted review leaves it open for the author's next commits. |
| Ctrl+Shift+V | In a feature's file's tab: go between Review and Split (its brief or plan, read on the stage's page), or to the next of Preview, Review and Split (Verify's evidence). Split, the two side by side, following its edits and caret and scrolling together, is there only on an output 1400 px wide or more. A new tab of a brief or plan opens on Split there, on Review on a narrower one; pressing its button again only switches to the open tab, keeping what it shows. Esc goes back to the feature tab. |
| F2 | Rename. |
| Shift+F2 | In the project menu: change the selected project's id in place (also its row's "#" button; not the shown project's). Its folders and the records naming it move with it, all or none, and an Undo toast changes it back. One still running asks to be stopped first. Refused while it has a sandbox, work running on another machine, a program on its folders, or a project file changed outside gerdr. Other machines show its features again once they use the same id. |
| Ctrl+S | Switch to / check out a branch (repos, worktrees). |
| Ctrl+Shift+S | Squash a worktree into main. |
| Ctrl+M / Ctrl+Shift+M / Alt+M | Take a branch's base in: merge it / rebase onto it (repos panel: the default branch into the one checked out; Alt+M a branch picked in a menu). On the feature tab's Ship page, the selected repository's (else the first card's that offers it; also its card's buttons): merge the base in or rebase onto it, asking, then back to Verify; with conflicts, Ctrl+M has an agent resolve them (as planned, once a plan is written) and Alt+M has it plan the resolution first. Rebase is offered only on a branch without merges. A squash held up as the branch took in commits of origin's base that the base here lacks gets the card's primary "Pull <base>" instead, which fast-forwards the base (Ship…, pressed, asks to); Ship… is no default while anything stops it. Conflicts with origin's base show in the Ship check too, not as "Ready to ship". The Ship check lists what fails first, each saying its way out, then on one line the named checks passed (checks passed, review done, base current, no conflicts); while an agent takes on conflicts, Ship waits for it and is no default. Shipped through a pull request, Ship says "pull request open, not merged yet" until GitHub merged it, and what comes next (Remove worktree and branch); undoing everything it shipped makes Ship to do again. |
| Alt+Z | Undo, asking first: the last batch (repos panel); on the feature tab's Ship page, the selected repository's last step that ran (else the latest of all; also the Undo button on its row): close its pull request, delete its pushed branch, take its squash off the base. What went to origin's base has no undo, nor does a push or pull request once GitHub merged it. Off the rows, Alt+Z also undoes what followed shipping when that is the latest, as the Afterwards rows' Undo button does, without asking: a closed workspace opens again with its Zed, app and file tabs (terminals don't come back), and removed worktrees come back on the commits they had, set up again. The toast saying either happened has the same Reopen / Restore button, and both launchers (Find and switch, Run) list "Restore the worktrees of …" while they can come back, and "Restore …" for an abandoned feature, the project's history (also the Features heading's right-click History…); with no feature open, Prefix then Shift+F offers those rows beside New feature. A workspace that shipping left open, its close ticked but something ran in it, keeps a "Close the workspace" row with a Close button (asking while something runs). The Ship confirm says what follows: the workspace closes, or the worktrees and branches go. |
| Prefix, then Enter | Send the prompt of the stage agent waiting in the focused pane (also the arrow on its pane's bar, and Send prompt on the stage's page). A stage's agent starts on its prompt; one waits only when checks ran on the code as it opened, or its line couldn't be typed. |
| Prefix, then U | Open the AI work stats, or close it again: what the features' AI work came to, by feature, by agent and model, and by check, for the open project or all of them. Over a feature's tab it opens on that feature in detail (also the tab's Stats button), and Esc goes to the Features list. Tab and Shift+Tab go between its tabs, its scope and its list, Left / Right pick the tab or scope that has focus, Enter on a feature shows its stages, runs, fix rounds and Verify runs (Esc goes back), its Kept column says what its AI work takes on disk, Shift+Del or its trash button deletes that after asking (never a bare Del: it is the feature's only record), and Ctrl+C copies the row. Nothing kept yet, stats still being built from what is kept, a read under way and a read that failed each say so in place of the rows; Enter during a read acts once the rows come. |
| Prefix, then O | Open the machines panel, or close it again (also the launcher's "Machines" row, and a machine's chip in the footer or its "Open" button among the background tasks, which open it on that machine). The machines gerdr reaches are listed on the left, with a last row adding one (it opens the connect dialog); the selected one on the right: its address, how its link stands, herdr's version, its load, and what of this gerdr's features runs there, each with Show, Stop and Open feature buttons. Up / Down move through the machines, Tab and Shift+Tab through the buttons of the one selected, Enter presses the focused button (on the list: the machine's main one, Connect or Disconnect), Del arms the machine and a second Del removes it (asking at once when sandboxes this gerdr answers for stay on it), Ctrl+E (or its Edit button) edits its name and address in a form in place of its detail (F2: on the name), Esc closes. In the form Tab goes between its fields and buttons, Enter or Ctrl+Enter saves, Esc leaves it (a second Esc closes the panel). Saving a new address tries it first, as the connect dialog does, and keeps it only once that machine answers, and only if it is the same machine (its gerdr machine id): Esc or Stop stops trying, and Save anyway keeps it untried (not once it reached another machine). A new name moves what gerdr keeps under the old one (the settings, the projects' `machines` and `machine_folders`) and keeps the link up; its toast's Undo gives the old name back. A machine can have several addresses (the office's and home's), tried in their order, the one that answered last first; a lost link tries them all again. In the form each address is a field, with a × taking it out and "+ Add address" under them (Ctrl+N adds one, Ctrl+Shift+Up / Ctrl+Shift+Down move the focused one up or down the list, Del in an empty one takes it out); an address that answers as another gerdr machine id than the machine's is passed over, and said so if none other answers. Two saved machines that are one (the same gerdr machine id, as any of them answered at some time) say so on each other's detail, with a "Merge" button: the selected one keeps its name and gets the other's addresses after its own, and the other goes, what was kept under its name (the projects' `machines` and `machine_folders`) moving to the one kept; it asks first, as it can't be undone. Ctrl+PgUp / Ctrl+PgDn (or the tabs in its title row) go between Machines and Placement: where each of the open project's stages runs when a feature doesn't choose its own (its agent, Verify's checks, Manual test's sandbox), kept in the project's file under `machines`. Up / Down go between the stages, Right, Enter or the stage's button give it the next machine (this one, each saved machine, least busy), Left the one before; each row says how many features choose their own. A press that changes a feature another machine writes takes it over first, as on its page. |
| Ctrl+Shift+U | Set a feature's worktrees up (feature tab, Build and Verify): copy the ignored files and environments they lack from main, then run the project's setup commands. Nothing there is overwritten. |
| Ctrl+Shift+I | Show a feature's setup log (feature tab, Build and Verify, once a setup started; also the "Setup log" button, the setup's row in the tasks panel and a failure's toast): its last run in the select and copy viewer, followed live while it runs. Selecting holds the following until the selection goes. |
| Shift+Enter | In a field of several lines: a new line, Enter being the panel's action (a diff's review comment, the project commands dialog, a Manual test criterion's note and the comment on an AI review finding, on their cards, and the note on the criteria strip). |
| Space | In the project commands dialog, on a borrowed check of the "This feature" tab: skip it for this feature, or run it again (also its Skip / Run button). Nothing is saved until Save. On a check's card on Verify's page: skip the check it borrows, or stop skipping it (also its Skip / Don't skip button). It saves at once, with an Undo, and applies from the next run. |
| Alt+U | In the project commands dialog: fill the focused empty field with what is suggested for it (also its Use button). Nothing is saved until Save. |
| Alt+B | In the new feature dialog: choose the branch the selected repository's worktree starts from (the first ticked one from the title; also its "from <branch>" button). It lists the repository's branches, the default first; typing filters them, Enter uses the selected one, Esc clears the filter and then goes back. The default is the branch the project's settings give it (`workflow.base`), else what origin says now, else the local `origin/HEAD`, else the checked out branch. A repository added to a feature later starts from that default too. In the review dialog ("Review pull requests…" in the sidebar's header and Features menus, or Review on a pull request of a project of several repositories), it lists the selected repository's open pull requests instead, yours first; Enter picks one, and each repository nobody chose for gets its one open pull request on the same branch. Space on a repository leaves it out, or lists its pull requests when none is picked. Repositories with none picked are left out of the review. |
| Alt+W | On the feature tab of someone else's pull request's review: fix it here (Fix here…, on its pull request's card), asking once gerdr read that the user can push to its branch (refused for a fork whose author doesn't let maintainers push): its findings, comments and failures then go to an agent as for one's own, it is set up, and Ship's Push… sends the fixes to its branch, a review optional. Again, comment on it instead, while nothing fixed here is unpushed. |
| Alt+N | On the feature tab of a pull request review: open the review dialog on the pull requests it holds (Pull requests…, on the first pull request's card). Picking one in another repository adds it, its worktree made next to the others (set up only when it is yours); leaving one out takes its repository out of the review, asking first when that loses work. A repository can't swap one pull request for another in one go: leave it out first. Repositories the review doesn't hold are never picked for their branch here. A title typed there renames the review, and its workspace when that still had the old name. A review still titled after its pull requests ("#12 Fix login +1") is retitled as they change. Save applies it. |
| Alt+X | In the dialog connecting a machine: press the button fixing what failed (Copy command, Open terminal). Only there. |
| Ctrl+Shift+T | Start a feature's sandbox on the feature tab's Manual test page, after a dialog asks which commands run in it (Space checks; the feature keeps the choice, and Ctrl+Shift+G and Ctrl+Shift+X go by it without asking); stop it, asking, once it runs (at once while it starts). With the sandbox on another machine it shows the sandbox's terminals instead, and Ctrl+Shift+Q stops it. |
| Ctrl+Shift+Y | Open the running sandbox's app, in a tab inside it (feature tab, Manual test). With the sandbox on another machine, in a tab here, through ports opened on this machine. The app's first page: the project's `workflow.sandbox_pages` list them, else its commands do, read through a shell script they run (a JavaScript dev server such as Vite or Nx first, its port guessed from what listens; a Django server: its site, then its admin; Expo with a web client last), else what listens. |
| Alt+Y | On Manual test, with the sandbox's app having more than one page: open the one after the last opened (also a button for each, after Open app). |
| Ctrl+Shift+N | Open the running sandbox to the network, for a phone, asking first; or close it at once (feature tab, Manual test; also its "Open to network…" / "Close to network" button). Only a sandbox on this machine. |
| Prefix, then Shift+C | Give the keys to the criteria strip over a feature's sandbox app (a web pane of its workspace at the sandbox's page, while it is at Manual test), unfolding it; again, or Esc, or a press on the app gives them back. With the keys: Space / Enter pass the criterion shown and go to the next not tried, F fails it and opens its note (Edit note… on a failed one), Del takes its result back, Ctrl+PgUp / Ctrl+PgDn (or Up / Down) go to the previous / next criterion, Ctrl+O shows it on Manual test's page, Alt+Down folds the strip to its chip (kept in `state.json`), Alt+C adds a screenshot of the page to its note. Writing the note, Enter saves it; Esc leaves the criterion as it was, what is written kept as a draft for its next note (Ctrl+Shift+Del discards it), Shift+Enter breaks the line and Alt+C adds a screenshot. The strip and the criterion's card on the page show one result and one note. |
| Ctrl+Shift+G | Start the sandbox again on the same data (feature tab, Manual test). |
| Ctrl+Shift+X | Reset the sandbox's data from main's, or remove a stopped sandbox with its data, asking first (feature tab, Manual test). |
| Ctrl+Shift+A | Pick the next agent on the feature tab's selected stage (the chips on its agent card), a toast naming it. It applies from the agent's next start; one running goes on. |
| Ctrl+Shift+E | Take over a feature another machine writes, on the feature tab (also the "Take over" button): it is written here from then on. Any press that changes such a feature does this first. On a feature taken over here, hand it back to the machine it came from (also the "Hand back" button): the gerdr open there takes it, its sandbox there included. |
| Ctrl+Shift+Q | Stop the selected stage's agent on another machine, on the feature tab (also the "Stop agent" button on its agent card), asking while it works; on Manual test, the sandbox on another machine, asking. Closing its Show agent tab only ends the view. |
| Ctrl+Shift+W | Pick the machine the selected stage's next agent, Verify's next run or the next sandbox goes on, on the feature tab: this one, then each connected machine, then whichever is least busy (the "Runs here" or "Runs on" button on its agent card, or on Verify's and Manual test's page). It applies from the next start; what runs stays where it is. The project's file can give each stage a machine under `machines`, or `least busy`. Not offered with no machine saved, while Verify's checks run, for T3 Code, or while a sandbox has a record: its data is on its machine, so it is removed first. |
| Alt+Shift+A | Pick the model of the selected stage's agent on the feature tab (the "Model:" button on its agent card), for Claude Code and Cursor. It applies from the agent's next start; one already open keeps its own. Default leaves it to the CLI. Cursor also keeps a picked model as its default for all its chats, as its own `/model` does, and its picker says so. |
| Ctrl+Shift+B | On the feature tab's Build page: go between one agent through all milestones and a fresh agent per milestone (also the "Agents" choice on the page). It applies from the next milestone; an agent at work goes on. |
| Ctrl+Shift+K | Skip Plan review on the feature tab, asking only when something would be lost. Its toast's Undo reopens Plan review, as Reopen does. |
| Ctrl+Shift+D | On the feature tab: from a final plan back to the decisions. |
| Ctrl+Shift+L | In the new feature dialog, the project picker (again: back to the repositories; also its "Link project…" button). On the feature tab, show or hide all repositories: the strip unfolds into rows of what the feature holds, its context, those removed and those not added, each with the buttons of what it can become (also the strip's end button). |
| Ctrl+Q | Stop a project's session (project menu). |
| Alt+1 / Alt+2 | The repos panel's Diff and Log tabs (Alt+3 Stashes). On the feature tab's Code review page: show the code, or the fixes its comments asked for (also the switch under the rail). Text being written in the diff takes them as any key. |
| Alt+E | On the feature tab's AI review page: show the findings of every earlier review, or fold them (also each review's Show and Hide buttons); on its Code review page, the comments of every earlier fix round the same way. Once the next review is read, the one before folds to a card with its name and how it went. Its findings are a record, as they stood then. Findings stay on the page once being fixed, while the code changes and while the next review runs: until that one is read they can only be looked at, and `f`, Ctrl+E and Del say why. |
| Ctrl+H / Ctrl+Shift+H | Snooze / snooze until tomorrow; Alt+H shows the hidden items (inbox). |
| Ctrl+F or / | Search within what has focus. On a feature's Manual test page, the search (also the magnifier at the right of the line under the rail) shows only the criteria saying what is typed, in their title, id, steps or note, open, the places tinted; Enter, Tab or Down give the keys to them, and Esc clears it, then closes it. With the keys on them, `n` / Shift+N go to the next place found or the one before, round the ends, the criterion holding it selected and the place tinted more (also the ↑ / ↓ buttons beside the field, whose count says "2 of 7"); after the selection moved, `n` goes on from the criterion selected, as vim's does. `*` / `#` search for the text selected with the pointer, else for the word under the pointer as a word of its own (the field says "Whole word"), and go to the place after it or before it; typing ends the whole word. A query with a capital letter finds only that case. |
| Del | Delete the selected row: the first press arms it (row shows "Press Del again"), the second deletes; an Undo toast follows when it can be undone. On a repository of a feature with several, Del takes it out of the feature: one on the feature's branch asks at once (no arming, see "Destructive actions"), and is refused with commits; one kept only as context arms, and offers Undo. Del never abandons a feature: elsewhere on the feature tab it does nothing, and Alt+Shift+Del abandons it (or removes a done one's worktrees), asking first (also the Abandon button). On an AI review finding, on its stage's page (its card) or in a feature's code review, Del dismisses it and asks why first (no arming), or reopens a dismissed one (Reopen); `f` marks it to fix, and on one to fix takes that back (Reopen). On the page `f` on an undecided card and a confirmed dismissal move the keyboard to the next card left to decide; no card changes place, and dismissed or fixed ones close to a line until selected. An item the user added themselves, or a comment left in the diff shown as a card, is a row delete: the first Del arms it, the second deletes, and an Undo toast follows (not once it went to Build). Its Delete button goes at once, with the same Undo; in the diff an item of the user's own also goes on the first Del. On a criterion's card on a feature's Manual test page, Del clears its result at once (no arming: Space or `f` gives it again). Plan review's suggestions go the same way, on their cards and as flags in the plan (its Review in plan tab): Del rejects one and asks why first (no arming), or reopens a rejected one (Reopen); `f` accepts it, or takes that back (Reopen; a question not answered opens its answer field on the page, and is accepted once answered; in the plan it says to answer first). A rejected one folds to its first line, on the page and in the plan, as a dismissed finding does. One of the user's own is a row delete, armed first on the page, with an Undo toast. On a feature of the AI work stats' Features tab, Shift+Del (or the row's trash button), not Del, deletes the feature's kept AI work on this machine; it can't be undone, so it asks at once (no arming). |
| Ctrl+Shift+Del | Abandon what is in progress, asking first: abort a merge or rebase (repos), start the feature tab's current stage over (not Discover: Ctrl+R starts its agent afresh), drop the fix plan open on its page, before its fixes went or once its agent stopped with fixes left (what it was to fix comes back, and its agent isn't stopped). Starting over closes its agent, moves what it made to the feature's archive and frees the stage before it. On the Ship page, while an agent merges a base in: stop it and abort the merge. While a note or comment is being written: discard it, and its draft. |
| Ctrl+Del | Dismiss every listed row of the selected row's group (inbox Services; also its label's "Dismiss all" button). One Undo toast brings them all back. In a diff (repos panel, a feature's, a file under review): delete the review's comments, asking on a sheet (Clear review). Not while a comment is being written: there it deletes a word. |
| Ctrl+I | Ignore the selected service error, or stop ignoring it (inbox). An ignored error goes under "Ignored errors", folded to its count; Alt+I (or the label's Show button, or a click on the label) lists them, dim. It sends no notifications or counts. Del there forgets it (armed first). Undo toast. |
| r / s, Shift+R / Shift+S | The feature tab's Verify page: on a check's card (Tab goes to the cards, Enter shows its terminal), run it again, or stop it; anywhere on the page, run the failed ones again, or stop them all. While the checks go, Enter only says they run: it never stops them. Ctrl+Shift+U and Ctrl+Enter refuse there, as a setup or fixes would change the code under them, and their buttons are hidden. Reviewing a pull request at its head, Shift+R runs the project's checks here (Run checks here), as only GitHub's are its Verify there. |
| ] / [ | Any diff (repos panel, worktree, code review, a file under review): go to the next or previous AI review finding or comment, in the diff's order (in the plan's Review in plan tab, the next or previous suggestion), past the last back to the first. A folded file unfolds, and a long one shows whole, to show it. The diff's title row has the same as buttons, and its scrollbar a tick per comment while no search is open. |
| Shift+A | A feature's code review at Code review, once fixes changed files viewed before: all its changes, or back to only what changed since the last review (also its All changes / Since last review button, beside Add comment…). It opens on the latter: a file viewed and committed again since shows only its diff from the blob viewed, saying so under it; one viewed and unchanged opens folded; one never viewed shows whole. A file with a comment of the user's on lines that diff hasn't shows whole, so the comment isn't taken for outdated. |
| Shift+O | In a feature's code review, on a comment whose fix is made ("fix made in 1a2b3c4"): show that commit's diff, read-only, over the review (also its Show fix button); Esc closes it. |
| Space | On the feature tab's Ship page, once Tab has gone to its rows: tick a step or what comes after it, or move a repository's choice to its next way (Pull request, Merge only, Nothing; also a click). It saves at once. Enter still presses Ship…, or Finish… with no step ticked, which asks first as nothing gets pushed or merged. |
| Space / Alt+Shift+Down | A feature's code review: mark the file viewed, or not (as Space checks a file in the repos panel), on the diff or on its line in the file list, the same tick; on a folder's or a repository's line, the files under it that can be, a toast saying how many were left out as not committed; go to the next file not viewed (Alt+Shift+Up: the one before), from the diff or the file list, as the ↑ and ↓ buttons over the file list do. A criterion's row on a feature's Manual test page: mark it passed, or not (passed, it folds to its line); `f` marks it failed, with a note saying what went wrong, written on the card (Enter saves it; Esc leaves the criterion as it was, what is written kept as a draft for its next note), or edits the note of a failed one. On the card, Pass and Fail stay in place: the one chosen is filled, and pressing it again takes it back. |
| Alt+F / Alt+Left / Alt+Right | The repos panel's file tree and a feature's code review's file list: list the files flat or by folder (kept in `state.json`, one for both) / fold everything / unfold everything (also the buttons on the list's toolbar). On a feature's Manual test page, Alt+Left / Alt+Right fold every criterion / open every one (the Fold row's All and Open all). |
| Alt+D / Alt+Shift+D, Alt+S / Alt+Shift+S | On a feature's Manual test page: fold / open the criteria passed, and those read: opened or marked. Also the Fold row over them (All, Passed, Read, Open all). A feature's criteria start folded; what is open and read is kept in `state.json`, per feature, and a criterion folds when it passes. |
| F1 | Every key of the focused panel, when its footer can't show them all. |
| Ctrl+C | Copy. Never closes anything. On the feature tab's page, text dragged over with the pointer (a double click takes a word, a triple a line) is selected, and Ctrl+C or `y` copies it, lines side by side or wrapped joined by a space; Esc lets it go. With nothing selected, Ctrl+C there copies the feature's path. In any diff (repos panel, worktree, a commit, code review, a file under review, evidence) it copies what the cursor is on, as `y` yanks it: the selection, else the line (a removed one too), a file's path, a hunk's lines, a comment or finding's text. A read-only diff refuses edits and pastes, never yanks. The cursor's row has the same as a Copy button. |

A chord means one thing across all panels. A verb with direction says it in
the label ("Merge main into branch", "Squash into main").

A key is written in one place, not on the button that does the same thing:

- A button shows its label only, with no key on it or in its text.
- The footer shows the panel's keys, as keycaps. A click on a hint presses its
  key, so a footer hint isn't a button in this sense.
- A tooltip shows a button's key on hover.
- F1 lists every key, including those the footer can't fit, in a column of
  keys beside what each does. Its footer hint reads "keys" on every panel.
- A menu row may show its key at its right end, since a menu has no footer.

The footer says what the buttons say:

- A key doing what a button on the page does is hinted with that button's
  label, first letter lowered ("Go to Plan" is "go to Plan", "Restart" is
  "restart"), taken from the button where the hints are made (the feature
  tab's `said`, the inbox's detail buttons, Ship's keys), so the two can't
  drift.
- One key per footer, one meaning per key on the feature tab: `Enter` presses
  the page's primary button, `Ctrl+Enter` passes the gate (approve, mark
  passed, make fixes), `Ctrl+Shift+Enter` sends back (plan fixes, reopen, ask
  the reviewer), `Esc` never saves, `Del` is only the focused card's or row's.
  A gate's key isn't hinted twice: where Enter does the same as `Ctrl+Enter`,
  the footer shows `Ctrl+Enter`.
- The feature tab's footer keeps to the page and what has the keys: the
  header's and the repository strip's buttons (Open tab, Copy path, New
  feature..., the other features, all repositories, Abandon) show their keys
  in their tooltips and F1.
- Every footer centres its rows, in the text colour.

## Text fields and the pointer

Every text field takes the pointer the same way, one line or many, in an
overlay, a dialog, a rename or a diff's comment:

- A click puts the caret by the nearest character, Shift+click reaches the
  selection there, a second click in a row selects the word, a third all of
  it.
- A press held and moved selects. It picks only text in view: moving past the
  field never jumps the selection on.
- A press held past the field's text scrolls it, moving or not, faster the
  further past (`scrollback::edge_speed`, `scrolling::edge_steps`). A diff's
  row range does the same past its rows.
- A field too small for its text scrolls only as far as its caret needs
  (`Line::scroll`): the text under the pointer stays put while the caret
  moves in view.

Text that is only read and can be selected (the feature tab's page, the
scrollback viewer) takes the pointer the same way: a press and drag select
by chars, a double click a word (or the gap between two) and a triple click
a line, and a drag on from either goes by whole words or lines, keeping the
first. Shift+click reaches the selection there by the same. Held past the
text's top or bottom it scrolls on, moving or not. Diffs differ: their
press and drag pick whole rows and a double click opens the line in Zed.

## Destructive actions

- Ask before anything that stops a running process, deletes unmerged work or
  can't be undone. Don't ask when nothing is lost (idle chat, empty shell).
- Dialog buttons: Cancel on the left, the action on the right. Destructive
  actions (`confirm::Then::loses`: something is lost for good) are painted
  in the error colour; an action that goes on in the flow (Ship, Approve,
  Finish, Change decisions) is the primary button, so red keeps meaning
  "lost". Neither is the default: Enter on a dialog means Cancel until the
  user moves to its action.
- A dialog offers "Don't ask again" only where the choice is remembered and
  can be found again in Settings.
- Cheap destructive actions don't ask; they show an Undo toast instead.
  Reopening a done stage asks only while the next stage's agent runs; its
  toast's Undo marks the stage done again. Closing a feature's idle workspace
  doesn't ask; its toast's Open brings it back. Abandoning a feature asks:
  its worktrees and branches go, but their commits and uncommitted changes
  are kept, and its toast's Restore, or its row in Find and switch, brings it
  back open. Abandon is an outlined danger button, never the page's loudest.
- One exception, asked for: a repository on a feature's branch never leaves the
  feature without asking, even with nothing to lose. It takes a worktree and a
  branch with it, and agents may be working there.

## Feedback

- Toast levels: Success / Info for what happened, Warning for what needs a
  look, Error for what failed. Never report a failure with Info.
- Errors and warnings stay longer than info and pause while hovered; every
  toast can be dismissed. Toasts are painted above every non-modal popup.
- What the user must fix (config, missing tool, unreachable source) goes to
  `problems::report` as well, so it can be seen after the toast is gone. It
  also lands in the log file.
- Every edit to files on disk (diff view cuts, restores) says what it did.
- A stage's agent starts on its prompt in a tab behind the stage's page,
  clicked or not, and the page's Show agent goes to it. When Verify passes,
  the AI review page comes to the front of the feature's workspace; another
  workspace keeps the screen. Discover's agent is the exception: its first
  message is a question, so Start agent shows its tab, ready to answer in.
- A stage agent's tab is named after its agent and stage ("claude · Plan"),
  and its sidebar row goes by its tab's name. Approving a stage closes the
  tabs of its files (its drafts' changes, Plan review's suggestions), and a
  fix plan's tab closes once its fixes go or its round ends: from then on
  they are only read, and the stage's page opens them again. One with a
  comment being written stays.
- Under a dialog, the feature tab's footer offers no keys: the dialog has its
  own.
- An agent placed on another machine opens no tab here: nothing from another
  machine shows until asked. Show agent opens one tab attached over SSH, as
  does Discover's Start agent once its agent is up, if the user stayed on the
  feature's page. That tab takes the terminal from any other view, and closing
  it asks nothing, as it only ends the view. A view taken by another machine
  ends saying which ("Opened on desk"). The agent's card says "on
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
  press goes on, or says the feature changed on the way. Ship pressed before the
  feature's code was read here, as just after a move, says so and asks once it
  is. Its diff and files take no
  comments or ticks until then. While it moves, the page and a tasks row with Stop
  say so. Files and commits come, and what isn't committed there keeps it there
  until it is. Work on a worker goes on and is followed from here. What runs in the
  writer's own tabs stays there, so while it works the press says to wait; at rest,
  its tab stays there, and its chat goes back on that machine. A sandbox running
  there keeps the feature there too; one on a worker goes on, and comes along. The old writer shows the feature from the new
  one if connected to it. With the new one saved but out of reach, it shows the
  feature as it left, written there, until that machine is back and tells of it;
  with that one not saved, it says so. A move made while gerdr was closed is told
  once, at the next start, the same way.
- A gerdr open on the writer has to agree. When it doesn't answer, or the machine
  can't be reached, a dialog asks, as what happens there from then on is kept aside
  (a toast, the "!" badge and the file named there), its commits staying on its
  branch. Once both can be seen, the record moved most often stands.
- A terminal of such a feature opens here on request as well. One on a worker
  takes the terminal, as any Show does. One in the writer's own session only
  looks: that machine's gerdr keeps it, and keys go nowhere. A feature's code comes
  here once it is opened, by commits: what isn't committed there doesn't show.
- A sandbox runs on the machine writing its feature, or on the one Manual test's
  page picked: that one needs Docker or Podman, pasta and the project's containers
  running, as those are what's copied, and its page says what it found none of.
  Podman's compose can be Docker's or podman-compose; a sandbox's copy runs under
  the same one as main's. Its setup runs there first. It stays there until it is
  removed: its data is there.
- A sandbox says which gerdr started it. Each time a machine connects, gerdr looks
  there for sandboxes whose feature is gone. Each one stays as a row in the tasks
  panel, with Remove, which asks; a toast offers this gerdr's own once as well.
  Another gerdr's shows only when that gerdr isn't connected here and hasn't been
  on that machine for two days, and the row and the question name it: it may still
  want it. Removing a machine that holds this gerdr's sandboxes asks first and names
  them; they stay there, and adding the machine again reaches them, or removes those
  of features already removed.
- A gerdr connecting to a machine leaves word there of who it is and where it came
  from, and refreshes it hourly while connected. A gerdr open on that machine that
  doesn't know it offers once, by a toast, to add it back: each shows only the
  features of machines it added. Add opens the connect dialog filled in.
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
- A sandbox on this machine opens to the network only when asked, and asks
  first: anything on the network then reaches its app and its copy of the
  data. Its app's ports open on this machine's network address under the same
  numbers, so the addresses its commands print (Expo's QR code) work on a
  phone. `{lan_ip}` in `workflow.sandbox_env` is that address, for an app's
  API URL. A port something here holds is said by name, on the "!" badge too.
  Its page says it is open in the warning colour. It closes when the sandbox
  stops or starts again, and with gerdr.
- A line gerdr types to an agent and finds still in its input box gets Enter
  again, up to six times over about four minutes. Only then is the user told.
- A stage agent that waits on the user has its card on the stage's page
  framed in the warning colour on a tint of it, its status bold in that
  colour: it asked something, waits for its prompt, is a T3 Code draft, or
  sits idle with its stage's work not done. Its pane's last lines on the card
  are in the text colour then, as they hold what it asks.
- Work sent to an agent (findings, review comments, failed tests, Make
  fixes), by the user or by gerdr, doesn't take the screen: the agent is told
  it in its tab behind the page. Only an agent that waits for the user to
  send its prompt has its tab shown. Either way the stage's node on the
  rail and the page of the stage the feature is at say the agent runs again or
  waits, with a way to its tab. While that work is under way the rail reads as
  turned back: done stages from Build on go grey, as they ran once, and a line
  runs from the stage that asked back to Build. The stages before Build stay
  as they are, as fixes don't touch what they made. A stage whose mark went
  stays grey until it is marked done again, so it doesn't read as one that
  never ran.
- A loop back from Verify (Manual test's fixes, a base update) shows as a
  round, not a toast: every stage it passes is titled "<Stage> · after fix
  round N" with one line saying why and where it ends, the rail's line runs
  from the stage that turned it to the stage the flow is at, and back there
  the page says its marks are kept and it is to be marked again.
- Fixes are made at once (Make fixes, the primary) or planned first (Plan
  fixes) on every page that sends them, by one rule: what the user wrote as
  an instruction (an edited fix, an item or a comment of their own) goes
  through a plan, and the page says so in plain words ("Your own words are in
  these fixes …"). A failure's note only says what went wrong and needs none.
- Build's page keeps a "Verify failures" section for the fix rounds Verify's
  failures started: what the newest one was sent, then a line for each older
  one (its run, its agent, how it stands, how the next run went). It only
  reads: the full output is on Verify's page, the ticks in the list below it.
  A fix round has one name everywhere, "Fix round N", N counting the feature's
  rounds: its card, its plan's tab, its buttons ("Send fix round N again"),
  the activity and the rows it changed. Its card's meta line says which stage
  asked it and that the Build agent makes it. Its plan opens by "Open plan",
  shown only when an agent wrote one. Round cards are always on top of the page
  that shows them. That list has the newest round on top
  and Build's own milestones under the rounds; until the build log lists any,
  the plan's milestones show there unticked, saying what happens to them. A round shows in it from its
  start: how it stands at first, the plan's fixes unticked once the plan waits.
  Build doesn't wait for the user between milestones: they tick off as their
  commits land, and what the agent is at work on blinks while it works (several
  at once when subagents build them). A milestone gate only opens for an agent
  that stops with some left and gets no further when told to go on.
  Setup goes last on that page; a failed setup is still said by the status at
  its top.
- Work running on a feature off the thread (setup, checks, code graph,
  sandbox, ship…) shows on every page of its feature tab, in a card under the
  header: a row each, with a spinner, a lowercase word, what it does now, a
  bar when it counts steps, how long it has run, and the buttons its stage's
  page has for it (Log, Stop). A row goes to that stage's page. Work shows
  there only once it ran 300 ms; past three rows the card counts the rest.

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
- One word and one colour per state, the same on the rail, the sidebar's
  cards, a stage page's lines and cards, agent cards, toasts and the activity
  (`chrome::Standing` in `chrome/theme.rs`; `NodeMark`, `CardTone`,
  `CheckTone`, `Dot` and `Palette::status` take their colours from it):
  "your turn" yellow (the warning colour), "working" blue (the accent),
  "failed" or "blocked" red (the error colour), "done" green (the success
  colour), idle grey. A stage that failed or is blocked (`stage_failed` in
  `features.rs`) is red on all of them. A stage agent says whose turn it is
  and, when it's the user's, why ("your turn · it stopped before brief.md was
  ready"), from `feature_agents::agent_standing`, which the rail line, the
  sidebar card and the agent card all read. A decided card (accepted, to fix)
  is marked in the done colour, never in the selection's accent.
  Toasts keep to it: a file or plan ready for review is the user's turn, a
  warning (yellow); a stage done a success (green); a failure an error (red).
- Severity is a pill before the item's title (`pill_inks` / `pill_mark` in
  `chrome/features.rs`): high filled with the error colour, medium with its
  own orange (`Palette::medium`, apart from the yellow of "your turn"), low on
  the element colour, the user's own tinted with the accent. Each severity has
  its own mark (triangle, diamond, ring), so colour is never the only sign.
- Approving a stage logs one line, "Plan approved" ("Build done: …" when gerdr
  marks it), and its toast says that line and what comes next ("Plan approved
  · Plan review is next", `features::done_toast`).
- The "Needs you" list and the inbox's count hold only what is the user's
  turn: gates, agents asking something, failed test runs, errors not looked
  at, reviews asked for. A finished agent is listed in the inbox, not counted.
- AI review's and Plan review's pages share one layout
  (`features/review_page.rs`): the agent's card, what the stage made (Plan
  review's final plan), a summary (its run past the first, a pill per
  severity saying how many are still to decide, faded once none is, that
  goes to its first card; a bar of how many are decided, wanted in the done
  colour and refused muted after it; the "all open" button), then the cards,
  the user's own first, then earlier reviews. No heading per severity: the
  pills and each card's pill say it.
- A review card says where it stands in a word on every card ("to decide"
  too), and folds to its line once decided, whatever the decision, so what
  is left to decide stands out; selected, it opens. Its first button is the
  decision (Fix, Accept; Reopen once decided, the one word for taking any
  decision back), filled on the selected card, the page's one primary while
  the keyboard is on the cards. The same order holds on a flag in the plan
  or the diff: decision, comment, edit, refuse. Pressing a card's button
  selects the card. A card the latest run raised, past the first, says so:
  "new in run 2", "changed in run 2", "raised again in run 2". The user's
  wording of a change shows over the reviewer's, which stays to read.
- A page's buttons for its items stay while they can be decided, greyed
  with why once there is nothing for them to do, so none shifts: Fix all
  open / Accept all open, Review in diff, Ask reviewer. A card's Comment
  becomes Edit comment in its place once written. Manual test's Mark passed
  sits last before the primary, greyed with why until every criterion
  passed; the primary is what to press next (Make fixes, Plan fixes, Start
  sandbox…). Ask reviewer waits,
  greyed, for the reviewer's answer once asked, and the page says so. Skip
  review is offered only while suggestions are still to decide. The plan's
  "Review in plan" tab (one name for its button, tab and heading) greys its
  final plan button by the same rule as the page.
- The words for where a finding (or a comment sent to Build) stands, on
  every surface: "to decide", "to fix", "in fix plan" (its plan is written
  or waits), "being fixed" (the fixes went), "fix made", "fixed" (the next
  review didn't raise it), "dismissed". A question answered says
  "answered", and answering it accepts it.
- A file's toolbar under review has its one filled button rightmost: Send
  back once a comment is written, else Approve (on a fix plan: Make fixes).
  The diff's title buttons carry their `ButtonStyle`, set where the view is
  made (`chrome::mark_primary`), not guessed by the painter. Code review's line over the
  page does the same: Plan fixes past a greyed Approve while comments wait.
- A diff's line actions sit right after the line's text, and over several
  lines say how many ("Copy 17 lines"). A decided finding or suggestion in a
  diff shows its first line. Back on Code review after fixes changed files
  already viewed, it opens on what changed since the last review (Shift+A
  or All changes for the rest): a file viewed and committed again shows only
  its diff since the blob viewed, the files viewed and unchanged open
  folded, and a file's line in the list says "changed" when another blob
  was committed since viewed. Edits not committed don't make it changed: it
  says "not committed".
