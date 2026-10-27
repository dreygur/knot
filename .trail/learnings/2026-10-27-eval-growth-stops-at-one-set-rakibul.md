---
title: Eval growth stops at one set per uncovered code shape; graph weight is not worth retuning
author: Rakibul
date: 2026-10-27
branch: audit-fixes
cost: { minutes: 8, tokens: 49428 }
id: l-866c994d
scope: repo
status: active
src: agent
touches:
  - eval/README.md
  - eval/v2.json
  - src/ask/mod.rs
  - src/cli/mod.rs
  - src/tools/mod.rs
---

## Decided
New eval questions are added only for a kind of code no corpus covers yet (eval/v2.json added taxonomy for aliased TypeScript apps with export const components). Existing sets stay frozen so scores remain comparable across commits.

## Tried and ruled out
Growing every language to about 20 more questions on five new corpora (fd, click, chi, javapoet, taxonomy) to retune GRAPH_WEIGHT in src/ask/mod.rs. Dropped as over-engineering: the earlier sweep (0.15 / 0.40 / 0.55) all scored below 0.25, so the expected gain is a couple of points and does not justify about 80 hand-written questions.

## Watch out
Corpora live in the git-ignored reference/ folder and must each be their own git repo, or code commands resolve to the outer repo root.
