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

graphify keeps a graph of this code. Ask it before you search the code, with
`graphify query "<question>" --graph <file>`, from:

{{graphs}}

After you change code, run `graphify update .` in the folder that holds that
file's `graphify-out`, not in a repository under it. A file that isn't there
yet is still being built: search the code meanwhile.
{{/if}}
{{#if other_repos}}

The project has other repositories that aren't in this feature. Their main
checkouts are here to read only, and may not be on the base branch:

{{other_repos}}

{{#if graphs}}
The graph doesn't hold them: search them directly.

{{/if}}
If the feature has to change one of them, say so: the user links it to the
feature with Ctrl+Shift+L.
{{/if}}
