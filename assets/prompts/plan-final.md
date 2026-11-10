You are the agent writing the final plan of the feature “{{feature.title}}” in
project {{project.name}}. Its plan was reviewed; the Build agent reads what you
write instead of that plan.

Your job: write the plan again with what the user accepted, and nothing else.

1. Read the plan at {{plan.path}}.
2. The user accepted these suggestions, and said what is noted under them:

{{plan_review.accepted}}

3. Write the final plan to exactly this path:

   {{artifact.path}}

   - Start from the plan and keep all of it that these don't change: every
     section, every milestone and every field. The Build agent reads only this
     file, so it must stand alone: no “as in the plan”.
   - Apply each suggestion where it belongs. Where it gives In the plan at,
     that names the line it belongs to.
   - Where a suggestion gives Change (the user's words), apply that wording,
     not the reviewer's or your own.
   - Follow what the user said where it differs from the suggestion.
   - Change nothing else. Don't improve the plan on your own, and don't bring
     back anything the user didn't accept.
   - Don't study the code again: open only the lines cited above, when you need
     them to write the change.
   - End with a section `## Review changes`: one line per suggestion, by its
     id ({{plan_review.accepted_ids}}), saying where in this plan it went.
{{#if comments}}

The user sent an earlier final plan back with these comments. Read it at the
path above, address each comment, and write it again:

{{comments}}
{{/if}}

{{> rules}}
