---
title: "Per-repo Knot state lives in <repo>/.knot/cache; ~/.knot keeps only the vault and log"
author: Rakibul
date: 2026-10-27
branch: audit-fixes
cost: { minutes: 17, tokens: 122911 }
id: l-22fa0e0e
scope: repo
status: active
replaces: l-df89a34b
src: agent
touches:
  - src/cli/mod.rs
  - src/cli/picker.rs
  - src/ask/cache.rs
  - src/ask/mod.rs
  - src/ask/session.rs
  - src/cli/hook.rs
  - src/cli/init.rs
  - install.sh
  - src/hosts/mod.rs
  - src/ast/resolve.rs
---

## Decided
Everything that belongs to one repository lives in its git-ignored `.knot/cache/`, because the user wants as little as possible in `~/.knot` and expects the index next to the code the way Trail keeps `.trail/cache`. That covers the index (`index.json`, `index.status.json`), `savings.jsonl`, `session.json`, `shown-<session>.txt` and the `last-fetch` marker. `crate::ask::state_dir(repo)` names the folder, and every writer calls `create_state_dir`, which also writes a `.gitignore` of `*`, so a repo without its own `.knot` rule never commits this state. `~/.knot` (or `$KNOT_DATA_DIR`) keeps only `knot.db`, `activity.log` and `.current_session`, because memories must carry across repos. install.sh no longer creates `~/.knot`; the vault and log create it on first use.

`knot init` writes instruction files only for the agents the person picks (picker on a terminal, `--agents`, or `--yes` for every detected one), because writing every agent's files at once filled repos with files for tools the person doesn't use (src/cli/init.rs, `configure_agents` in src/hosts/mod.rs).

## Tried and ruled out
Moving the vault into `<repo>/.knot/` as well: ruled out because memories would then stop carrying across repos.
Keeping the index, ledger and sessions under `~/.knot/` keyed by a hash of the repo path: ruled out because the user wants `~/.knot` nearly empty, and with no `.knot/` in the project an agent wrongly concluded Knot had nothing indexed.

## Watch out
`.knot` must stay in the skip list in src/ast/resolve.rs, otherwise the cache folder gets indexed. A pty with no size reports 0 columns; src/cli/picker.rs treats that as 80, because otherwise the redraw climbs hundreds of rows and erases the scrollback. Old caches under `~/.knot/cache/ask/` and `~/.knot/sessions/` are orphaned. Ask before deleting them, because they are the user's files.
