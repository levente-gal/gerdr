{{#if absent}}
gerdr can't find {{artifact.path}}. Please write it there now, with the sections your prompt asked for.
{{else}}
The file {{artifact.path}} isn't ready for review yet: {{missing}}. Please fix it in place.
{{/if}}
