---
title: "Knot's code index lives in <repo>/.knot/cache; vault, logs and savings stay in ~/.knot"
author: Rakibul
date: 2026-10-27
branch: audit-fixes
cost: { minutes: 1, tokens: 1470 }
id: l-df89a34b
scope: repo
status: active
src: agent
touches:
  - src/ask/mod.rs
  - src/ask/cache.rs
  - src/hosts/mod.rs
  - src/ast/resolve.rs
---

## Decided
The code index lives in the repository, because the user expects it next to the code the way Trail keeps `.trail/cache`. `index_root(repo)` in src/ask/mod.rs returns `<repo>/.knot`, and src/ask/cache.rs writes `.knot/cache/index.json` plus a `.gitignore` (`*`) there, so a repo without a `.knot` rule never commits it. The vault `knot.db`, `activity.log`, `savings.jsonl` and `sessions/` stay under `default_cache_root()` (`$KNOT_DATA_DIR` or `~/.knot`), because they span repos.

`knot init` writes instruction files only for the agents the person picks (picker, `--agents`, or `--yes` for every detected one), because writing every agent's files at once filled repos with files for tools the person doesn't use. See `configure_agents` in src/hosts/mod.rs.

## Tried and ruled out
Moving the vault into `<repo>/.knot/` as well: ruled out because memories would then stop carrying across repos.
Keeping the index in `~/.knot/cache/ask/<hash>.json`: ruled out because, with no `.knot/` folder in the project, an agent wrongly concluded Knot had nothing indexed.

## Watch out
`.knot` must stay in the skip list in src/ast/resolve.rs, otherwise the cache folder gets indexed. Old caches under `~/.knot/cache/ask/` are orphaned. Ask before deleting them, because they are the user's files.
