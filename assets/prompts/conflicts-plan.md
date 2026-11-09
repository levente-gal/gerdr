Please plan how to resolve the conflicts that merging {{conflicts.onto}} into {{branch}} of the feature “{{feature.title}}” would bring, in {{conflicts.dir}}. They are in:
{{conflicts.files}}

Read the brief at {{brief.path}} and the plan at {{plan.path}} for what the feature means to do, and the base's new commits (`git log --oneline HEAD..{{conflicts.onto}}`) for what they mean to do. To see each conflict, `git merge-tree --write-tree HEAD {{conflicts.onto}}` shows them without touching the worktree. Do not change any file, do not merge and do not commit.

Write the plan to exactly {{conflicts.plan_path}}, for the user to review: under a `## Fixes` heading give each file a `### ` heading with its path, and say what each side changed there and how you would bring both together, with a `## Questions` section for anything the user must settle. Then stop.
