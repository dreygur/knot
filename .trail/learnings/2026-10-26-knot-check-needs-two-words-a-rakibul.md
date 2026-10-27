---
title: "knot check needs two words, a backticked term, or a title word to flag a conflict"
author: Rakibul
date: 2026-10-26
branch: main
cost: { minutes: 43, tokens: 284865 }
id: l-4608edb7
scope: repo
status: active
src: agent
touches:
  - src/check/mod.rs
  - src/engine/memory_ops.rs
  - src/check/remote.rs
  - src/tools/mod.rs
  - src/cli/mod.rs
  - src/ast/extract.rs
  - src/ast/types.rs
  - src/ast/resolve.rs
  - docs/COMPETITIVE_ANALYSIS.md
  - src/ast/pagerank.rs
---

## Decided
`check_hunk_against_learning` in src/check/mod.rs flags a CONFLICT only when an added, non-comment line
contains a backticked term from the "Tried and ruled out" section, two distinct significant words from
one ruled-out line, or one word that also appears in the learning's title. Any other single-word match
is only a WARNING, which does not fail the exit code. Matching is whole-word (snake_case and CamelCase
parts are split), so "reload" does not match "load".

## Tried and ruled out
Substring matching on any single word of 4+ letters (the original implementation). A learning about
spinlocks failed `// reload config from disk` and `async fn read_code()` on the words "load" and "async",
so almost every diff became a CONFLICT and the commit gate was useless.

## Watch out
Programming-generic words (async, code, load, error, ...) are in STOP_WORDS on purpose. Removing them,
or treating a single shared word as a conflict, brings the false positives back. Authors who want an
exact match should put the ruled-out code in backticks.
