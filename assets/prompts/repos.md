{{#if projects}}
This feature spans several projects. Each has a feature folder of its own;
you start in the home one, and the others are yours to read and change too
(you may need to give their full paths):

{{projects}}

Its repositories, each checked out in its own folder under its project's
feature folder, named as gerdr names them (another project's as
`project:repository`):

{{repos}}
{{else}}
{{#if multi_repo}}
This feature spans several repositories, each checked out in its own folder
under {{feature.dir}}:

{{repos}}
{{else}}
The code is the repository in {{feature.dir}}: {{repos}}
{{/if}}
{{/if}}
{{#if graphs}}

graphify keeps a graph of this code. For where something is or how parts
connect, ask it with `graphify query "<question>" --graph <file>`. When you
know the file or symbol, read it directly. The graphs:

{{graphs}}

A file that isn't there yet is still being built: search the code meanwhile.
{{#if changes_code}}
Before each commit, run `graphify update .` once in the folder that holds that
file's `graphify-out`, not in a repository under it.
{{/if}}
{{/if}}
{{#if context_repos}}

These repositories aren't changed by this feature. They are here for you to
read, as main checkouts that may not be on the base branch. Don't edit them:

{{context_repos}}

{{#if graphs}}
The graph doesn't hold them: search them directly.

{{/if}}
If the feature has to change one of them, say so: the user adds it to the
feature under Repositories on the feature tab (Ctrl+Shift+L).
{{/if}}
{{#if other_repos}}

The project has other repositories that aren't in this feature. Their main
checkouts are here to read only, and may not be on the base branch:

{{other_repos}}

{{#if graphs}}
The graph doesn't hold them: search them directly.

{{/if}}
If the feature has to change one of them, say so: the user adds it to the
feature under Repositories on the feature tab (Ctrl+Shift+L).
{{/if}}
