---
title: "Every write into .knot/cache goes through state_file, because a cloned repo can commit it as a symlink"
author: Rakibul
date: 2026-10-27
branch: audit-fixes
cost: { minutes: 15, tokens: 80323 }
id: l-69577d8d
scope: repo
status: active
src: agent
touches:
  - src/ask/cache.rs
  - src/ask/mod.rs
  - src/ask/savings.rs
  - src/ask/session.rs
  - src/cli/hook.rs
  - README.md
  - install.ps1
  - install.sh
  - src/cli/init.rs
  - src/cli/mod.rs
---

## Decided
`state_file` in src/ask/cache.rs is the only way to get a writable path in a repo's `.knot/cache/`. It refuses when `.knot`, `.knot/cache` or the file itself is a symlink, because a cloned repo can commit any of them as a link (to `~/.ssh`, say), and Knot would otherwise write session.json, the ledger or the index wherever the link points. `session::trusted` uses a committed `session.json` only when `seen_ms` is not in the future and the transcript resolves to a file under `$CLAUDE_CONFIG_DIR/projects` (or `~/.claude/projects`), because otherwise a planted session points the status line at any file.

## Tried and ruled out
Moving the state back to `~/.knot` keyed by a repo hash: ruled out because the user wants `~/.knot` nearly empty, and refusing symlinks closes the hole.

## Watch out
A new writer that uses `dir.join(name)` with `fs::write` or `OpenOptions` directly bypasses the check and reopens the hole. The check is not race free against a local attacker swapping files mid write; it covers what a static cloned repo can do.
