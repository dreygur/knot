---
title: Tool hook output must be hookSpecificOutput.additionalContext JSON or the model never sees it
author: Rakibul
date: 2026-10-27
branch: audit-fixes
cost: { minutes: 32, tokens: 202850 }
id: l-29ceecd0
scope: repo
status: active
src: agent
touches:
  - src/ask/learnings.rs
  - src/ask/mod.rs
  - src/ask/session.rs
  - src/check/mod.rs
  - src/check/remote.rs
  - src/cli/hook.rs
  - src/cli/mod.rs
  - src/engine/learnings.rs
  - src/hooks.rs
  - src/hosts/mod.rs
---

## Decided
`knot hook` in src/cli/hook.rs answers PreToolUse/PostToolUse events with `{"hookSpecificOutput":{"hookEventName":..., "additionalContext":...}}`, because Claude Code sends a tool hook's plain stdout only to its debug log. Only UserPromptSubmit and SessionStart add plain stdout to the model's context.

## Tried and ruled out
The old knot-pre-tool.sh hook printed `knot recall` results as plain text before Bash/Edit/Write. It never reached the model, so its recall never had any effect; it was removed in favour of file-scoped learnings on PostToolUse.

## Watch out
In src/check/remote.rs `symbol_clashes`, "what you changed" is the working tree against HEAD, not against the merge base with the teammate's branch: from an old merge base, everything main gained since then counts as yours and old branches show false clashes.
