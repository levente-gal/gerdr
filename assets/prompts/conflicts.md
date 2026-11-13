Please resolve the merges in progress for the feature “{{feature.title}}”: gerdr merged each repository's base into {{branch}}, and they stopped on conflicts. Each worktree, what it took in and its conflicted files:
{{conflicts.repos}}

Read the brief at {{brief.path}} and the plan at {{plan.path}} for what the feature means to do, and in each worktree the base's commits it took in (`git log --oneline HEAD..MERGE_HEAD`) for what they mean to do.
{{conflicts.plan}}
Resolve every conflict so both keep working: the feature's change and what the base changed. Never drop one side to make the other fit, and never take a file wholesale from one side unless the other side's change there is gone on purpose. Where the repositories depend on each other, keep them working together. Run what quickly shows the result builds where you can.

Then in each worktree `git add` each file and conclude its merge with `git commit --no-edit`. Do not abort a merge, do not rebase and do not push. If something can't be settled without the user, write your questions to {{conflicts.questions}} and stop without committing that merge.
