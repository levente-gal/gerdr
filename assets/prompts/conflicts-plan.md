Please plan how to resolve the conflicts that merging each repository's base into {{branch}} of the feature “{{feature.title}}” would bring. Each worktree, its base and the files that would conflict:
{{conflicts.repos}}

Read the brief at {{brief.path}} and the plan at {{plan.path}} for what the feature means to do, and in each worktree the base's new commits (`git log --oneline HEAD..<base>`) for what they mean to do. To see each conflict, `git merge-tree --write-tree HEAD <base>` in that worktree shows them without touching it. Do not change any file, do not merge and do not commit.

Write the plan to exactly {{conflicts.plan_path}}, for the user to review: under a `## Fixes` heading give each file a `### ` heading with its worktree's folder name and its path, and say what each side changed there and how you would bring both together, with a `## Questions` section for anything the user must settle. Then stop.
