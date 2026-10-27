---
title: "Claude Code transcripts store prompts with images or pastes as content arrays, and the last prompt can be megabytes from the end"
author: Rakibul
date: 2026-10-27
branch: audit-fixes
cost: { minutes: 4, tokens: 29313 }
id: l-c4d40096
scope: repo
status: active
src: agent
touches:
  - src/ask/savings.rs
  - src/check/mod.rs
  - src/cli/hook.rs
  - src/cli/mod.rs
  - src/engine/learnings.rs
---

## Decided
`is_human_prompt` in src/ask/savings.rs counts a `"type":"user"` line as a prompt when `message.content` is a string or an array with no `tool_result` part, and isMeta is not true. `last_prompt_ms` searches tails of 512 KB, then 4 MB, then 32 MB (capped at 64 MB) until it finds one.

## Tried and ruled out
Matching only string content in the last 512 KB: prompts with a screenshot or pasted text were missed, and one long turn of tool output pushed the prompt out of the window, so "this reply" fell back to the session start and equalled the session total.

## Watch out
Tool results are also `"type":"user"` lines (with a `tool_result` array); counting them as prompts would reset "this reply" after every tool call.
