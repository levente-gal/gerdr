# 0001: A retro is not a stage

## Context

A shipped feature's retro proposes checks and guidance that stop its problems from happening to the next feature. It could have been a tenth stage after Ship, or something a done feature offers.

## Decision

A retro is not a `Stage`. A done feature's Ship page offers it, by the project's `workflow.retro` (`off`, `offer` by default, `auto`). Its agent is one of the feature's own agents. Its file, `retro.md`, gets the same review as a stage's file. Approved, it becomes a new feature: the retro is the new feature's brief, and Discover is approved.

## Why

- A stage after Ship would reopen every shipped feature and keep it from being done.
- The proposals change the project, not the shipped feature. As a new feature they go through Plan, Build and the reviews like any other change.

## Consequences

- A retro needs the feature's page. The page is its workspace's tab, so there is no retro once the Ship plan removes the worktrees. Where retros are offered, the Ship page's Remove row and the Ship confirm say so.
- The new feature takes the shipped feature's touched repositories, those of linked projects too.
