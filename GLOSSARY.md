# Glossary

**Retro**: an optional look back at a shipped feature. Its own agent reads what gerdr kept of the feature and writes proposals shaped as a brief (`retro.md`). Approved, the retro becomes a new feature that starts at Plan. See ADR 0001.
_Avoid_: retrospective stage, post-mortem.

**Context**: how many tokens an agent's last turn sent its model, against the model's window ("context 84k / 1M"). Kept on each stage run and fix round, so it shows how full the agent ended.
_Avoid_: context window (that is only the model's limit), context repo (a repository a feature reads but doesn't change).

**Milestone tokens**: how much a Build agent's context grew while it built one milestone, read when its tick was first seen. A fresh agent's first milestone counts its whole context.
_Avoid_: cost, usage (those are the billed totals in the stats panel).
