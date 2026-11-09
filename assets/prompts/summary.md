Please write a short summary of the feature “{{feature.title}}” for the people who will review and merge it. It becomes the body of its pull requests and the message of its squash commit.

Read the brief at {{brief.path}} and the plan at {{plan.path}}. Then read what the feature changed, in each repository it changes:
{{summary.diffs}}

Write the summary to exactly {{summary.path}}, in Markdown, and nothing else in that file:
- First a paragraph of two or three sentences: what the feature does for its users, in plain words.
- Then a paragraph on how it is built: the main changes and where they are, and what a reviewer should look at closely.
- No headings, no lists of files, and nothing the diff doesn't show.

Do not change any code, do not commit and do not push. Then stop.
