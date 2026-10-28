# Knot as an all-in-one efficiency layer for coding agents

Date: 2026-10-28. Status: design, approved in conversation; not yet planned.

## Why

The owner wants Knot to be the one tool an AI coding agent needs to spend fewer tokens
and credits, and a place to see that it is working. Two kinds of waste dominate in
practice:

1. **Building on a guess.** Agents almost never ask clarifying questions unless the user
   tells them to. They guess what is wanted, build it, and every token spent on the wrong
   guess is lost. Research agrees: agents default to not asking, and asking recovers much
   of the lost quality, but only when the questions are specific
   ([Ambig-SWE](https://arxiv.org/html/2502.13069v2)).
2. **Shell noise.** Agents explore code with many rounds of `grep` and `sed -n`, and edit
   code with throwaway `python3` scripts that rewrite files blind and then need re-reading.

Knot already saves tokens on finding code (`knot ask`, `grep`, `skeleton`, `map`) but
does nothing about either kind of waste, and has no single view of what it saves.

## Goals

- Cut tokens spent on shell commands that search or read code, without the agent losing
  information it needs.
- Stop code edits done through python scripts when the agent has a real edit tool.
- Make agents ask a few targeted questions before building on an unclear request, and
  never on a clear one.
- Show, in one terminal screen, what Knot saved, what it changed and where waste remains.

## Non-goals

- Compressing output of non-code commands (builds, tests, git, package managers). RTK
  already does this; Knot stays code-aware.
- A web dashboard or a cross-machine/team view.
- Dollar figures. Token savings are upper-bound estimates; converting them to money makes
  them look precise. Dollars may come later, once billed input is read from transcripts.
- Saving the user's clarification answers across sessions. Agreed as a later step.

## Anti-goals (failure even if it technically works)

- **Silent rewrites.** If the agent does not know its command was swapped, it misreads
  the output, retries with other flags and wastes more than was saved. Every swap must
  say so in the result.
- **Questions on clear tasks.** Asking about "fix the plural in `statusline_text`" is
  waste in the other direction.
- **A gate that fights the agent.** Each block happens at most once per case and always
  names the way through.
- **Claimed savings.** Every figure shown is measured from recorded events and labelled
  as an estimate, as the status line does today.

## Constraints

- No LLM calls and no API key inside Knot; everything it decides is deterministic and $0.
  Judgement that needs a model is delegated to the agent through instructions.
- Claude Code first, through its hooks. Other agents get the same rules as written
  instructions from `knot init`, without enforcement.
- All per-repo state lives in `<repo>/.knot/cache/` and every write goes through
  `state_file` (src/ask/cache.rs), which refuses symlinks. `~/.knot` stays nearly empty.
- Hook output the model must see is `hookSpecificOutput` JSON, never plain text.
- Build with `CARGO_TARGET_DIR` off the TOTUL volume; never trigger GitHub CI.

## Where it is on

Decided (owner): the shell swaps, the python-edit block and the question gate are on
only in repos where the user turns them on, never everywhere Knot is installed.
*Designer's call on the mechanism:* `knot init` offers them as a choice, and their hooks
go into that repo's `.claude/settings.local.json`, which Claude Code does not commit. So
the choice is per repo and per person, and teammates are not opted in by a commit. The
existing global hooks (learnings after a read/edit, the stop nudge) and the status line
are unchanged. `knot status` lists which of these are on in the current repo.

## Order

Agreed: 0 → 1 → 2 → 3. Each part ships and is usable on its own.

---

## Part 0 — Event log (foundation)

Every Knot action that saves or costs tokens is appended to
`<repo>/.knot/cache/events.jsonl` as one JSON object per line:

- `ts` (ms), `session` (Claude Code session id, or empty), `kind`, and kind-specific
  fields: `tool`/`command`, `bytes_before`, `bytes_after`, `saved_tokens`, `detail`.
- Kinds: `answer` (today's `knot ask`/`grep`/`map`/`skeleton` savings), `swap`,
  `edit_blocked`, `prompt_flagged`, `prompt_cleared`, `gate_block`, `override`.

This replaces `savings.jsonl`. It fixes the known flaw that the ledger is keyed by repo,
so two sessions in one repo add into each other's totals: totals are now per session id.
The status line reads its "this reply / this session" figures from it.

Decided:
- Append-only, read from the tail (as `savings.rs` does today) so it stays cheap at any size.
- Session id comes from hook input and from the session record `knot hook` already keeps.
  Commands run outside a session record an empty session and count only in repo totals.
- *Designer's call:* events written by hooks carry the exact session id. Events from `knot`
  run directly or through MCP are credited to the most recently seen session
  (`session.json`, src/ask/session.rs). With two sessions open in one repo these can be
  misattributed; that is accepted for the first cut. "Dashboard matches the log" means it
  matches what the log recorded, not ground truth.
- *Designer's call:* extra kinds `agent_asked` and `user_skipped` (see part 2).

Left to the builder: rotation/size cap, migrating old `savings.jsonl` (dropping it is fine).

## Part 1 — Shell swaps

A `PreToolUse` hook on `Bash` (`knot hook pre-tool`) inspects each command.

**grep.** When the command is a `grep`/`rg` over code in the repo (not a pipe into
something else, not on logs or non-code files), the hook rewrites it, through
`updatedInput`, to `knot shell -- <original command>`. That command runs the search with
Knot's grep and prints:

```
[knot] ran `knot grep` in place of grep: 14 of 230 matches, grouped by function. Raw output: KNOT_RAW=1 grep ...
<grouped, capped matches>
```

**sed -n ranges.** `sed -n 'A,Bp' file` on a code file is rewritten the same way. Knot
returns the range snapped to whole enclosing definitions, or the file's skeleton plus the
requested lines when the range is very large, with the same one-line notice.

**Pass-through.** Anything Knot cannot do better runs untouched: other sed uses
(substitution, `-i`), greps over non-code, commands in pipelines Knot doesn't understand.
`KNOT_RAW=1` before a command always bypasses Knot.

**python edits.** A command that runs python to write into a code file in the repo
(heredoc or `-c` script that opens a file for writing) is denied once, with a reason
naming the agent's own edit tool. A second identical attempt in the same session is
allowed and logged as `override`. Python that only reads, or runs tests or tools, is not
touched.

**Agents without edit tools.** The owner wants Knot to offer reading and writing code
by symbol to agents that lack native read/edit tools, or where Knot's tool does better.
*Designer's call:* not in the first cut. Every host Knot supports today (Claude Code,
Codex, Cursor, Gemini, opencode) has its own read and edit tools, so there is no agent to
offer it to yet. Revisit when one appears, or when the event log shows exact-text edits
failing often enough that a by-symbol replace would pay.

**Built-in Grep/Read.** Claude Code's own Grep and Read tools do not pass through the Bash
hook. Covering them is out of scope for the first cut; measure first (part 3 will show
how often they are used).

Every swap, pass-through and block is logged in the event log with before/after sizes.

Decided:
- Tell the agent on every swap (owner's call).
- The original command is preserved inside `knot shell -- ...`, so the transcript shows
  what the agent asked for and what ran.

Left to the builder: exact detection rules for "grep over code" and "python writing a
file", caps and grouping limits, how "before" size is measured (running the raw command
is not acceptable; estimate from matched files/lines).

Watch out: other tools may also register `PreToolUse` hooks that rewrite Bash (RTK does);
two rewriters on one command conflict. *Designer's call:* when another `PreToolUse` Bash
hook is present, Knot does not install its swap hook, and `knot init`/`knot status` say
so, naming the other hook. The python-edit block is a deny, not a rewrite, so it is still
installed.

## Part 2 — Question gate

**Free filter, on `UserPromptSubmit`.** Knot checks the prompt against the code index,
spending no model tokens:

- *Clear:* names something the index knows (file, symbol, pinned value, a pasted error
  string found in the code) and states the change. Nothing happens.
- *Flagged:* asks to build, add or change something and names nothing Knot can find, or
  never says what done looks like.
- "just do it" in the prompt clears it; "ask first" forces a flag.

**Readings, not open questions.** On a flagged prompt Knot injects context telling the
agent, before planning or writing, to:

1. write the 2–3 readings of the request that would lead to different code,
2. ask only where they differ, at most 3–5 questions, each answerable in a line,
3. say which reading it defaults to if the user just says "go".

This adapts ClarifyGPT's idea (ambiguity is where candidate solutions diverge) without
generating solutions ([ClarifyGPT](https://arxiv.org/pdf/2310.10996)); the 3–5 cap
follows spec-kit's clarify step.

**Soft gate.** If the agent calls Edit/Write (or a swapped shell write) on a flagged
prompt before the user has replied, the first such call is denied once with a reminder.
The next is allowed and logged.

The flag lasts until the next user prompt. *Designer's call:* a prompt that directly
follows a flagged one is taken as the user's answer and is never judged itself, so short
replies like "the second one" are not flagged again. Other prompts are judged on their own.

*Designer's call, recording asked and skipped (deterministic, no model):*
- `agent_asked`: at Stop after a flagged prompt, the agent's last message contains a
  question and no Edit/Write ran in that turn (both read from the transcript, which Knot
  already parses in src/ask/savings.rs).
- `user_skipped`: the prompt after a flagged one says "just do it", "go" or "skip the
  questions", or the agent edited before any reply (`gate_block` then `override`).

Decided (owner): question quality in-session matters more than remembering answers.

Left to the builder: the heuristics and word lists, and how "states the change" is
detected. These must be tuned against real prompts.

Decided (owner accepted the designer's proposal) on tuning and release:
- Take a set of real prompts from the owner's own past Claude Code transcripts
  (`~/.claude/projects`), at least 100, covering both clear and unclear requests.
- The builder labels each one clear or unclear; the owner checks a sample of the labels
  before tuning starts, so the bar for "clear" is the owner's.
- The bar: at most 1 in 20 clear prompts may be flagged. Report that rate and the share
  of unclear prompts caught.
- Until the bar is met, the gate is not offered by `knot init`; it can only be switched on
  by hand for testing. The labelled set is kept out of the repo, since it is the owner's
  prompts.

## Part 3 — Dashboard

`knot dash` opens a full-screen terminal view (ratatui, as `knot explore` uses) for the
current repo:

- **Saved:** this session, today, and a bar per recent session; all labelled estimates.
- **Swaps:** per kind (grep, sed range, python edit blocked): count and tokens saved.
- **Questions:** prompts flagged, how many the agent asked about, how many the user
  skipped.
- **Learnings:** active and replaced, newest first.
- **Index:** symbols and freshness, as `knot status` shows.
- **Detail:** enter on a row lists its events (each swapped command with before/after).

It reads only the event log, learnings and index status. It never writes.

Decided (owner): terminal screen, not web page; this repo only for now.

Left to the builder: layout, keys, refresh while open, behaviour in a narrow terminal.

## Part 4 — `knot viz` redesign

Separate from the token work above, and independent of it: it can be built in any order.

The owner found the current `knot viz` (one vis-network hairball, two colours, no search,
loaded from a CDN) below their standard. They chose the "constellation" direction and
approved a prototype built on this repo's real graph as meeting it. The prototype sources
are in `docs/superpowers/prototypes/knot-viz/`: `build.js` lays out an exported graph,
and `page.html` is the page, with `__DATA__` replaced by the layout JSON.

What the prototype settled, to keep in the real build:
- **Map.** Each folder settles into its own region with its name floating over the region's
  centre, like a map. Size shows centrality (PageRank). Only the most central symbols are
  labelled until you zoom in. Symbols with no calls are hidden behind a switch.
- **Selection.** Clicking a symbol fades everything else, colours caller edges cyan and
  callee edges amber, and glides the camera to frame the symbol with all its neighbours.
  The inspector shows kind and folder, name, `file:line`, signature, rank, callers and
  calls (each clickable), and learnings. With no learnings, it shows the `knot note
  --touches` command for that symbol.
- **Search.** `/` focuses it. Results rank by match quality, then centrality. Arrow keys
  and Enter work. Esc clears the search or the selection.
- **Folder list.** Hover a folder to isolate it, click to hide it.
- **Look.** Dark and light themes, following the system setting, with a toggle. IBM Plex
  Sans with IBM Plex Mono for code and numbers. Hues step by the golden angle with
  alternating lightness. Themed scrollbars, selection colour and focus rings. One authored
  motion: the map settles in from a wide view.

What changes from prototype to product:
- **One offline file.** sigma.js, graphology and the IBM Plex fonts (subset woff2) are
  inlined, not loaded from a CDN. sigma and graphology are MIT and Plex is OFL, so all
  three can be bundled. Today's page loads vis-network from unpkg.
- **Layout.** The folder-anchored ForceAtlas2 setup in `build.js` (see the learning on
  src/ui/viz.rs) runs either in the page, in a web worker, or in Rust before writing.
  Builder's choice; the result must match the prototype's clustering.
- **Brand.** The header shows the Knot logo from `assets/`: `logo-light.png` on the dark
  theme, `logo-dark.png` on the light one. The tab icon is the mark: `mark-light-64.png`
  and `mark-dark-64.png`, picked with `media="(prefers-color-scheme: …)"` on the icon
  links. Both are inlined as data URIs, so the file stays offline.
- **Real data.** Folder, kind, line and PageRank come from Knot's graph (src/ast/pagerank.rs
  already computes centrality), and learnings come from `active_learnings`.
- **Fixes.** The output is written through `state_file`, not a plain `fs::write`, so a
  symlinked `.knot` can't redirect it. Knot no longer prints "Opened in default web
  browser" when opening failed.
- **Not yet verified:** phone-width layout. The prototype has a mobile rule (the inspector
  becomes a bottom sheet), but it was never seen at that size.

## Success

- On a recorded session of ordinary work, shell swaps cut tokens from grep/sed calls by a
  measured amount, with no case where the agent re-ran a swapped command because the
  output confused it.
- On the labelled prompt set, the gate flags most unclear prompts and almost no clear
  ones; the builder reports both rates.
- `knot dash` shows numbers that match the event log exactly.
