---
title: "Knot's tokens-saved figures are an upper bound and not comparable with Trail's"
author: Rakibul
date: 2026-10-27
branch: audit-fixes
cost: { minutes: 6, tokens: 64096 }
id: l-a65e4059
scope: repo
status: active
src: agent
touches:
  - src/ask/map.rs
  - src/ask/mod.rs
  - src/ask/savings.rs
  - src/cli/mod.rs
  - src/tools/mod.rs
  - Cargo.lock
  - Cargo.toml
  - docs/COMPETITIVE_ANALYSIS.md
  - src/ask/cache.rs
  - src/ask/index.rs
---

## Decided
src/ask/savings.rs credits each answer with the bytes/4 of the files it points into, minus the answer itself; `map` counts only the files its hubs and hotspots name. Nothing checks that the agent would have read those files, so the status line totals are an upper bound, never measured savings.

## Tried and ruled out
Crediting `map` with every indexed file, as `trail map` does (it claimed 36 whole files on psf/requests for one call). Knot does not do this, so for the same work Trail's figure will look larger; do not quote the two side by side as a comparison.

## Watch out
The ledger is keyed by repo root, not by Claude Code session, so two sessions open in the same repo add into each other's totals.
