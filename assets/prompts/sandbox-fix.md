The user started you for the feature “{{feature.title}}” in project
{{project.name}}, to fix why its sandbox didn't start. gerdr runs the
feature's code in a sandbox for its Manual test: a private copy of the
project's containers and data, with its own network. Before the sandbox's
services start, gerdr runs the project's Prepare steps in it. This one
failed:

- Step: `{{sandbox.step}}`
- Run in: {{sandbox.dir}}
- Shell line: `{{sandbox.line}}`

What it printed, in short:

```
{{sandbox.excerpt}}
```

All it printed is at the end of {{sandbox.log}}, after the line starting
with `== ` and the shell line above.

{{> repos}}
{{#if branch}}
The feature's branch is `{{branch}}`.
{{/if}}

Find the cause before you change anything. Read the log, then what it
points at.

- If the cause is in the feature's repositories, fix it there on the
  feature's branch, and commit. Conflicting database migrations after the
  base was merged in are an example: a merge migration, or the feature's own
  migration put after the base's, fixes them. Keep the feature's change and
  the base's both working. Don't rebase and don't push.
- If it isn't the code's (a missing tool, a setting, data, a service that
  didn't come up), change nothing, and say what the user should do instead.

Don't run the step itself, and don't start the project's services or touch
any database: the sandbox's database is only reachable from inside the
sandbox, and the others are the user's own. Commands that need no database
are fine, such as `python manage.py makemigrations --merge` or a build.

When you're done, say in a few lines what was wrong and what you changed, and
tell the user to press Start sandbox again on the feature's Manual test page.
