{{#if multi_repo}}
This feature spans several repositories, each checked out in its own folder
under {{feature.dir}}:

{{repos}}
{{else}}
The code is the repository in {{feature.dir}}: {{repos}}
{{/if}}
