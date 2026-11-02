## Feature review

`docs/feature-checklist.md` lists where new features break gerdr's UX rules (closing, errors, keys, overlays, look, words, settings, disk edits, state). The rules behind it are in `docs/ux-conventions.md`.

- Before you build a feature or change a surface, read the checklist sections it touches, and build to them.
- Before you merge a branch, and before you commit directly to main, review the diff against the checklist: run its quick greps, then go over it section by section. This includes small direct commits.
- Name findings by item id and place, like `E2 src/foo.rs:120`. Fix them, or list them in your report with why they stay.
- End the report with the keys that changed (K6) and what was not tried in a live gerdr (G3).
- A project hook (`.claude/hooks/checklist-gate.sh`) reminds you before `git merge` and before `git commit` on main. It doesn't block: the review is still yours to do.

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
