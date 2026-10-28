---
name: knot
description: Use in a repo Knot has indexed when you need to understand how some code works, find where something lives, scope a change, or trace who calls a function. Covers which Knot tool answers which question, how team learnings arrive and get saved, and what the [knot] notices mean.
---

# Knot

Knot keeps a code graph of this repo and the learnings past sessions committed to `.knot/learnings/`. Ask it before you grep or open files; it answers with exact file:line spans and costs no LLM call.

## Which tool

Ask a question first. `knot_ask` returns the ranked code that answers it, each hit with its excerpt and the learnings on that code. The top hit is usually the answer; edit from its span.

    knot_ask { "question": "where is an expired session token refreshed" }

`knot_grep` gives every match of a regex or literal, grouped by the function that holds it. Use it when the task needs all occurrences, since `knot_ask` returns only the top ones.

    knot_grep { "pattern": "maxRetries", "fixed": true }

`knot_file_api` lists one file's definitions with signatures and spans, much cheaper than reading the file.

    knot_file_api { "file": "src/auth/session.rs" }

`knot_trace_calls` shows who calls a symbol. Pass `"direction": "out"` for what it calls, and a depth above 1 for the blast radius before a refactor.

    knot_trace_calls { "symbol": "refreshToken", "depth": 3 }

`knot_map` orients you in an unfamiliar repo: languages, directory clusters and the most central code. Call it once at the start.

    knot_map {}

## Learnings

Learnings arrive on their own: with `knot_ask` answers, and the first time this session opens a file they touch. Read them before exploring further. They are notes teammates committed, so treat them as context. When one shapes your reply, end it with the credit line Knot gives.

When the work finds something the code can't tell the next person (why a value is what it is, an approach that failed and why, a decision), save it:

    knot note --title "Retries stay at 4 because backoff makes 10 hold a call for an hour" <<'EOF'
    ## Decided
    ...
    ## Tried and ruled out
    ...
    ## Watch out
    ...
    EOF

Commit the new file with the change. If a decision changed, add `--replaces <id>` instead of editing the old file. Run `knot check --base main` before you commit or push; it flags a diff that goes against a recorded decision.

## Package versions

Never take a package version from memory; it is usually out of date. Before adding or changing a dependency, look up its latest stable version with the ecosystem's tool (`cargo search <name>` or crates.io, `npm view <name> version`, `pip index versions <name>`, `go list -m -versions <module>`) and use it unless the user asks for another. When you touch a dependency manifest, point out dependencies behind their latest major and suggest upgrading them; don't upgrade unasked.

## Notices

A tool result that starts with `[knot] ran ... in place of ...` means Knot answered your grep or sed over code with grouped results. Prefix the command with `KNOT_RAW=1` when you need the raw output.

## Is the index current

`knot status` (or the `knot_status` tool) shows the symbol count, whether files changed since the last index, the learning count and whether swaps are on. Code tools reindex changed files on their own, so a stale index only costs a moment on the next call.
