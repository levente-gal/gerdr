Please resolve a merge in progress in {{conflicts.dir}}: gerdr merged {{conflicts.onto}} into {{branch}} of the feature “{{feature.title}}”, and it stopped on conflicts in:
{{conflicts.files}}

Read the brief at {{brief.path}} and the plan at {{plan.path}} for what the feature means to do, and the base's commits it took in (`git log --oneline HEAD..MERGE_HEAD`) for what they mean to do.
{{conflicts.plan}}
Resolve every conflict so both keep working: the feature's change and what the base changed. Never drop one side to make the other fit, and never take a file wholesale from one side unless the other side's change there is gone on purpose. Run what quickly shows the result builds where you can.

Then `git add` each file and conclude the merge with `git commit --no-edit`. Do not abort the merge, do not rebase and do not push. If something can't be settled without the user, write your questions to {{conflicts.questions}} and stop without committing.
