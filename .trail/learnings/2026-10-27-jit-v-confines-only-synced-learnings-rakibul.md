---
title: "Jit-V confines only synced learnings to their repo, using the absolute Project scope id as root"
author: Rakibul
date: 2026-10-27
branch: audit-fixes
cost: { minutes: 15, tokens: 104967 }
id: l-31705cb6
scope: repo
status: active
src: agent
touches:
  - Cargo.lock
  - Cargo.toml
  - src/jitv/mod.rs
  - src/ask/learnings.rs
  - src/ask/mod.rs
  - src/cli/mod.rs
  - src/tools/mod.rs
  - src/engine/memory_ops.rs
  - src/engine/learnings.rs
  - src/utils.rs
---

## Decided
src/jitv/mod.rs `containment_root` treats a memory as a repo learning when its scope is `Project(id)` and `id` is an absolute path; `sync_repo_learnings` in src/engine/memory_ops.rs stores the repo's canonical path there. For those, `read_target_file` checks the opened descriptor's real path against that root and hashes the bytes from the same handle, because a check by name followed by a later read can be redirected by a symlink swapped in between.

## Tried and ruled out
Requiring every absolute verification path to equal its canonical form at read time. It would also stale memories users saved through symlinked paths (macOS /tmp is /private/tmp), so user-saved memories keep following links.

## Watch out
If anything starts storing a non-path project id for synced learnings, or promotes them to Global, they lose containment silently. Platforms other than macOS and Linux fall back to canonicalizing the name, which is not race-free.
