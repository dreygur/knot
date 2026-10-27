---
title: install.sh only runs agent registration commands whose syntax was verified; it prints config for the rest
author: Rakibul
date: 2026-10-27
branch: audit-fixes
cost: { minutes: 7, tokens: 823511 }
id: l-f2c8b6f8
scope: repo
status: active
src: agent
touches:
  - README.md
  - ".claude-plugin/marketplace.json"
  - ".claude-plugin/plugin.json"
  - install.ps1
  - install.sh
  - src/cli/hook.rs
  - src/cli/mod.rs
  - src/ask/savings.rs
  - Cargo.toml
---

## Decided
install.sh runs `claude mcp add -s user knot -- <bin>` (syntax checked against `claude mcp add --help`) and `codex mcp add knot -- <bin>`, and prints JSON/TOML snippets for Gemini CLI, OpenCode, Cursor, Windsurf and VS Code. Knot registers its own Claude Code hooks and status line on first MCP start, so the installer writes no hooks.

## Tried and ruled out
Running `opencode mcp add` (it is interactive, so it hangs a piped install) and `gemini mcp add` (a machine can have `gemini` as a shim for another CLI, which then treats the arguments as a model prompt). The old `claude mcp add --name --command` form is not accepted by the Claude CLI.

## Watch out
.claude-plugin/plugin.json must declare `mcpServers`, or `/plugin install` adds nothing; it starts `knot` from PATH, so the binary has to be installed first. Keep its version in step with Cargo.toml.
