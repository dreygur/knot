---
title: "Unlocated qualified calls are never guessed to a same-named repo symbol, except value.method() to a repo method"
author: Rakibul
date: 2026-10-27
branch: audit-fixes
cost: { minutes: 31, tokens: 201981 }
id: l-00481982
scope: repo
status: active
src: agent
touches:
  - src/ask/cache.rs
  - src/ask/index.rs
  - src/ask/map.rs
  - src/ask/mod.rs
  - src/ast/extract.rs
  - src/ast/resolve.rs
  - src/cli/mod.rs
  - src/tools/mod.rs
  - src/ask/grep.rs
  - docs/COMPETITIVE_ANALYSIS.md
---

## Decided
`CodeGraph::resolve_call` in src/ast/resolve.rs links a call that no exact `Type::name`, same file, import or module path located only when it is unqualified, or a `value.method()` call to a candidate that is a Method. `::` paths, capitalised type qualifiers and names in `UBIQUITOUS_METHODS` (is_empty, join, write, ...) are left unresolved. Eval MRR stayed at 0.706 / 0.644 / 0.618 with the change.

## Tried and ruled out
The old fallback linked any qualified call to every repo symbol sharing its bare name when there were at most three. On Knot itself that made `TsAliases::is_empty` the most central symbol (67 callers from `Vec::is_empty`), `serde_json::from_str` callers of `EdgeType::from_str`, and 270 of 1126 edges bogus, which `knot map` hotspots and PageRank both surfaced.

## Watch out
Unqualified calls still use the at-most-three fallback; that is what keeps cross-file Rust and Python calls resolving. Do not tighten it without re-running all three eval sets.
