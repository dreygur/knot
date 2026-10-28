# Event Log and Shell Swaps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record every token-saving action per Claude Code session, and let Knot answer the agent's `grep`/`sed -n` calls over code itself (saying so), and block python scripts that write code files, in repos that opt in.

**Architecture:** A new append-only event log (`src/ask/events.rs`) replaces the savings ledger, keyed by session. A new `src/shell/` module classifies a Bash command and runs Knot's version of it. A `PreToolUse` hook (`knot hook pre-tool`) rewrites swappable commands to `knot shell -- '<original>'` and denies python code writes once. `knot init --swaps` installs that hook into the repo's `.claude/settings.local.json` only.

**Tech Stack:** Rust 2021, serde_json, git2, regex, the existing `crate::ask::grep` and `CodeGraph::file_skeleton`; new crate `shell-words = "1.1"` for parsing and quoting commands.

**Spec:** `docs/superpowers/specs/2026-10-28-knot-agent-efficiency-design.md` (Parts 0 and 1, and "Where it is on"). Parts 2, 3 and 4 get their own plans.

## Global Constraints

- No LLM calls, no API key; everything here is deterministic.
- Every write into `<repo>/.knot/cache/` goes through `crate::ask::state_file(dir, name)`; never `fs::write`/`OpenOptions` on a joined path.
- Hook output the model must see is `hookSpecificOutput` JSON, never plain text.
- Swaps and the python block are on only in repos where the user turned them on; their hook lives in `<repo>/.claude/settings.local.json`, never in `~/.claude/settings.json`.
- Every swapped result starts with one notice line naming what ran instead and how to get raw output: `KNOT_RAW=1 <original command>`.
- `KNOT_RAW=1` at the start of a command always bypasses Knot.
- Savings figures stay estimates (bytes / 4 via `savings::estimate_tokens`) and are never negative.
- Build and test with `CARGO_TARGET_DIR` set off the TOTUL volume, e.g. `export CARGO_TARGET_DIR=$TMPDIR/knot-target`. Never trigger GitHub CI; do not push.

## Review Focus

1. **A command Knot misreads runs as something else.** Any grep/sed/python form the classifier is unsure of (pipes, `;`, `&&`, `||`, redirects, `$(...)`, backticks, globs, `-l`, `-c`, `-v`, `-o`, `-A/-B/-C`, `sed -i`, `sed s///`) must pass through untouched. Owned by Task 3; test `pass_through_anything_not_understood`.
2. **Grep over non-code.** `grep -rn TODO notes/`, `grep foo app.log`, `grep x README.md` must pass through. Task 3; test `non_code_paths_pass_through`.
3. **Paths outside the repo.** `grep -rn x /etc` or `sed -n '1,5p' ../other/a.rs` must pass through, never be read by Knot. Task 3; test `paths_outside_the_repo_pass_through`.
4. **Another tool already rewrites Bash** (RTK). Knot must not install its swap; it installs the python block only and says which hook it deferred to. Task 6; test `swap_skipped_when_another_bash_rewriter_exists`.
5. **Two sessions in one repo.** Each session's status line shows only its own savings. Task 1; test `totals_are_per_session`.

---

### Task 1: Event log

**Files:**
- Create: `src/ask/events.rs`
- Modify: `src/ask/mod.rs` (add `pub mod events;`)
- Test: in-file `#[cfg(test)] mod tests` (the repo's convention)

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")] pub enum EventKind { Answer, Swap, EditBlocked, Override }` (Part 2's plan adds its own kinds)
  - `#[derive(Debug, Clone, Serialize, Deserialize)] pub struct Event { pub ts_ms: i64, pub session: String, pub kind: EventKind, pub tool: String, #[serde(default)] pub command: Option<String>, #[serde(default)] pub bytes_before: u64, #[serde(default)] pub bytes_after: u64, #[serde(default)] pub saved_tokens: usize }`
  - `pub fn append(root: &Path, event: &Event)` — appends one JSON line to `state_dir(root)/events.jsonl` via `state_file`; ignores errors.
  - `pub fn recent(root: &Path) -> Vec<Event>` — parses the last 2 MiB (reuse the tail-read logic now in `savings.rs`; move `read_tail` here as `pub(crate)`).
  - `pub fn saved_since(root: &Path, session: &str, since: &[i64]) -> Vec<usize>` — per `since` entry, sum of `saved_tokens` of events with that `session` and `ts_ms >= since`.
  - `pub fn current_session(root: &Path) -> String` — `session::current(root)` id, or `""`.

- [ ] **Step 1: Write failing tests** `appends_and_reads_back_events`, `totals_are_per_session` (two sessions "a" and "b", 100 and 40 tokens; `saved_since(root, "a", &[0]) == vec![100]`), `symlinked_cache_is_refused` (unix: `.knot/cache` symlinked to a temp dir; after `append` the target dir is empty).
- [ ] **Step 2:** `cargo test events::` — expect compile failure (module missing).
- [ ] **Step 3:** Implement `src/ask/events.rs` with the interfaces above.
- [ ] **Step 4:** `cargo test events::` — expect 3 passed.
- [ ] **Step 5:** Commit `Add a per-session event log in .knot/cache`.

### Task 2: Savings and status line read the event log

**Files:**
- Modify: `src/ask/savings.rs` (drop `LEDGER_FILE`, `Entry`, `record_at`, `saved_since`, `saved_since_in`; its transcript helpers now call `events::read_tail`; keep `estimate_tokens`, `saved_tokens`, transcript helpers, `short`)
- Modify: `src/cli/mod.rs` `statusline_savings` (around line 816)
- Modify: `README.md` file table row for `savings.jsonl`

**Interfaces:**
- Consumes: Task 1 `events::{append, saved_since, current_session, Event, EventKind}`.
- Produces: `pub fn record(root: &Path, tool: &str, files: &[String], output: &str)` keeps its signature; it now appends `EventKind::Answer` with `session = events::current_session(root)`, `bytes_after = output.len()`, `saved_tokens = saved_tokens(..)`, skipping zero savings as today.

- [ ] **Step 1: Write failing test** `record_writes_an_answer_event` in `savings.rs`: after `record`, `events::recent(root)` has one `Answer` with the expected `saved_tokens`.
- [ ] **Step 2:** run it; expect FAIL.
- [ ] **Step 3:** Change `record` as above; delete the ledger code. In `statusline_savings`, read `session_id` from the status line JSON and call `events::saved_since(root, &session_id, &[last_prompt, session_start])`. Update the README row to `<repo>/.knot/cache/events.jsonl | What Knot did and saved, per agent session`.
- [ ] **Step 4:** `cargo test` — all pass; existing statusline tests unchanged.
- [ ] **Step 5:** Commit `Read savings from the event log, per session`.

### Task 3: Classify a Bash command

**Files:**
- Create: `src/shell/mod.rs`, `src/shell/classify.rs`
- Modify: `src/lib.rs` (`pub mod shell;`), `Cargo.toml` (`shell-words = "1.1"`)

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, PartialEq)] pub enum Plan { Grep { opts: crate::ask::grep::GrepOptions, word: bool }, SedRange { file: String, start: usize, end: usize }, PythonWrite { file: String }, PassThrough }`
  - `pub fn classify(command: &str, root: &Path, cwd: &Path) -> Plan` — `file`/`within` are repo-relative.

Rules (decided here; the spec leaves detection to the builder):
- `PassThrough` when the command starts with `KNOT_RAW=1`, fails `shell_words::split`, or contains any of `|`, `;`, `&&`, `||`, `>`, `<` (except a python heredoc, below), `$(`, `` ` ``, `*`, `?` outside quotes.
- **grep / rg / egrep / fgrep:** allowed flags `-r -R -n -H -i -F -E -w` (combined short flags like `-rn` too) and `--` ; rg: `-i -F -n -w`. Any other flag → `PassThrough`. Exactly one pattern; zero or one path. No path: grep needs `-r`/`-R` (else it reads stdin → `PassThrough`), rg searches cwd. The path, resolved from `cwd`, must be inside `root` and be a directory or a file whose extension `SupportedLang::from_path` accepts; otherwise `PassThrough`. `-F`/`fgrep` → `fixed`; `-w` → `word` (wrap in `\b`); `-i` → `ignore_case`.
- **sed:** exactly `sed -n '<A>,<B>p' <file>` with `1 <= A <= B`, file inside `root` and a supported source file. Anything else → `PassThrough`.
- **python / python3:** `-c '<script>'`, or `python3 - <<'X' ... X` / `python3 <<X`, where the script contains `open(<path literal>, '<mode>')` with mode containing `w`, `a` or `+`, or `<path>.write_text(`/`.write_bytes(`, and the path literal is a supported source file inside `root` → `PythonWrite`. Python that only reads, or runs a file (`python3 tests.py`), → `PassThrough`.

- [ ] **Step 1: Write failing tests** in `classify.rs`, each a table of `(command, expected Plan)` against a temp repo with `src/a.rs`, `notes/todo.txt`, `app.log`:
  - `swaps_grep_over_code`: `grep -rn foo src` → `Grep { opts: GrepOptions { pattern: "foo", within: Some("src"), ..Default::default() }, word: false }`; `rg -i Foo` → `opts.ignore_case == true`, `opts.within == None`; `grep -rnw foo .` → `word: true`, `within: None` (`.` is the repo root).
  - `swaps_sed_ranges_on_source_files`: `sed -n '10,40p' src/a.rs` → `SedRange{file:"src/a.rs", start:10, end:40}`.
  - `blocks_python_writes_to_code`: `python3 -c "open('src/a.rs','w').write('x')"` → `PythonWrite{file:"src/a.rs"}`; heredoc form with `Path('src/a.rs').write_text(...)` → same.
  - `pass_through_anything_not_understood`: `grep -rn foo src | head`, `grep -c foo src/a.rs`, `grep -rl foo src`, `grep -A3 foo src/a.rs`, `sed -i 's/a/b/' src/a.rs`, `sed -n '1,5p;9p' src/a.rs`, `grep foo src/*.rs`, `KNOT_RAW=1 grep -rn foo src`, `python3 -c "print(open('src/a.rs').read())"`, `python3 tests.py` → all `PassThrough`.
  - `non_code_paths_pass_through`: `grep -rn TODO notes`, `grep foo app.log`, `sed -n '1,5p' notes/todo.txt` → `PassThrough`.
  - `paths_outside_the_repo_pass_through`: `grep -rn x /etc`, `sed -n '1,5p' ../a.rs` → `PassThrough`.
- [ ] **Step 2:** `cargo test shell::classify` — expect compile failure.
- [ ] **Step 3:** Implement `classify` per the rules above.
- [ ] **Step 4:** `cargo test shell::classify` — 6 passed.
- [ ] **Step 5:** Commit `Classify Bash commands Knot can answer better`.

### Task 4: Run a swapped command (`knot shell`)

**Files:**
- Create: `src/shell/run.rs`
- Modify: `src/cli/mod.rs` (dispatch `Some("shell")`, add `"shell"` to the command list at line 98, one help line)

**Interfaces:**
- Consumes: Task 3 `classify`, `Plan`; Task 1 `events::append`; `crate::ask::grep::{grep, format_report}`; `CodeGraph::file_skeleton`.
- Produces: `pub fn run(command: &str, root: &Path, cwd: &Path) -> Result<String>` — returns the text to print. `knot shell -- <command>` prints it; if `classify` returns `PassThrough` it runs the command with `sh -c` and passes its stdout/stderr/exit code through unchanged (the hook never sends such a command, but a stale rewrite must still work).

Output (exact notice copy):
- Grep: `[knot] ran \`knot grep\` in place of grep: {shown} of {hits} matches, grouped by function. Raw output: KNOT_RAW=1 {original}` then `format_report`. For `rg` say "in place of rg". `bytes_before` = Σ over every matched line of `file.len() + digits(line) + text.len() + 3`; `bytes_after` = output length.
- Sed range: snap `start` back and `end` forward to the edges of any definition the range cuts, each by at most 60 lines. Notice: `[knot] showed lines {s}-{e} of {file}, widened to whole definitions. Raw output: KNOT_RAW=1 {original}`. Saved tokens = 0 when snapping grew the output (record it anyway). If `end - start + 1 > 400`: print the file's skeleton for definitions overlapping the range instead, with notice `[knot] {n} lines is a lot to read; here are the definitions in that range. Ask for a narrower range, or raw output: KNOT_RAW=1 {original}`.
- Each run appends `EventKind::Swap` with `tool` = `grep`/`rg`/`sed`, `command` = original.

- [ ] **Step 1: Write failing tests** in `run.rs` on a temp repo: `grep_swap_says_what_ran_and_how_to_get_raw` (first line starts `[knot] ran \`knot grep\` in place of grep:` and contains `KNOT_RAW=1 grep -rn alpha src`); `sed_range_widens_to_whole_definitions` (`sed -n '2,2p'` inside a 5-line fn returns lines 1-5); `huge_sed_range_returns_the_skeleton`; `each_swap_is_logged` (one `Swap` event with `bytes_before > bytes_after` for a grep with many hits).
- [ ] **Step 2:** run; expect FAIL.
- [ ] **Step 3:** Implement `run` and the CLI dispatch.
- [ ] **Step 4:** `cargo test shell::run` — 4 passed.
- [ ] **Step 5:** Commit `Add knot shell, which answers grep and sed ranges over code`.

### Task 5: The pre-tool hook

**Files:**
- Modify: `src/cli/hook.rs` (`run`: new `"pre-tool"` arm; accept a `--no-swap` argument)
- Modify: `src/cli/mod.rs` hook dispatch (line ~1195) to pass remaining args
- Modify: `src/ask/session.rs` (python-deny memory)

**Interfaces:**
- Consumes: Task 3 `classify`; Task 1 `events::append`.
- Produces:
  - `pub(super) fn pre_tool_output(plan: &Plan, command: &str, bin: &Path, swap: bool, already_denied: bool) -> Option<serde_json::Value>`
  - In `session.rs`: `pub fn deny_once(root: &Path, session_id: &str, command: &str) -> bool` — true the first time this exact command is seen in this session (stores blake3 hashes in `state_dir/denied-<safe_id>.txt` via `state_file`), false after.

Behaviour, for `tool_name == "Bash"` only:
- `Grep`/`SedRange` and swap on → `{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"command":"<bin> shell -- <shell_words::quote(original)>"}}}`. "allow" is safe only because Task 3 admits nothing but read-only searches of source files inside the repo.
- `PythonWrite` and `deny_once` true → `permissionDecision: "deny"`, reason: `Knot: this edits {file} with a python script, which rewrites it without you seeing the result. Use your Edit tool instead. If you really need the script, run the same command again.` Log `EditBlocked`.
- `PythonWrite` and seen before → no output (the command runs); log `Override`.
- Everything else → no output; `PassThrough` is not logged (it would flood the log).

- [ ] **Step 1: Write failing tests**: `grep_is_rewritten_to_knot_shell` (updatedInput command is `"/bin/knot" shell -- 'grep -rn foo src'`), `swap_off_leaves_grep_alone` (`swap: false` → `None`), `python_write_is_denied_once_then_allowed`, `deny_once_is_per_session`.
- [ ] **Step 2:** run; expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test hook::` — all pass.
- [ ] **Step 5:** Commit `Rewrite grep and sed over code to knot shell, and stop python code edits once`.

### Task 6: Opt in per repo with `knot init --swaps`

**Files:**
- Modify: `src/hooks.rs` (new `register_repo`; add `" hook pre-tool"` to `KNOT_HOOK_MARKERS`)
- Modify: `src/cli/init.rs` (`--swaps` flag; a second picker row when Claude Code is picked on a terminal, unchecked by default; result line)
- Modify: `src/ask/mod.rs` `repo_summary` (one more line)
- Modify: `README.md` (one short section), `src/hosts/mod.rs` instructions (one line telling agents what a `[knot] ran ...` notice means)

**Interfaces:**
- Produces:
  - `pub enum SwapInstall { Installed, Deferred { to: String } }`
  - `pub fn register_repo(repo_root: &Path, bin_path: &Path) -> Result<SwapInstall>` — writes a `PreToolUse` entry with matcher `Bash` and command `"<bin>" hook pre-tool` (or `... hook pre-tool --no-swap` when deferring) into `<repo>/.claude/settings.local.json`, replacing any earlier Knot entry. Deferred when any other `PreToolUse` hook matching `Bash` exists in that file, `<repo>/.claude/settings.json` or `~/.claude/settings.json`; `to` is that hook's command. Then makes sure git ignores the file: if `git2` says it is not ignored, append `.claude/settings.local.json` to `.git/info/exclude`.
  - `pub fn repo_swaps(repo_root: &Path) -> Option<SwapInstall>` — what is installed, for `knot status`.
- `knot init` result line: `✓ swaps  grep and sed over code answered by knot, python code edits stopped · this repo only` or `· swaps  another hook already rewrites Bash ({to}); python code edits stopped only`.
- `repo_summary` adds `        swaps=on|off|python-only`.

- [ ] **Step 1: Write failing tests** in `hooks.rs`: `register_repo_writes_only_the_local_settings` (home settings untouched), `rerun_keeps_one_knot_entry`, `swap_skipped_when_another_bash_rewriter_exists` (an `rtk` PreToolUse Bash hook in the project settings → `Deferred{to}` and the written command ends with `--no-swap`), `local_settings_are_git_ignored`.
- [ ] **Step 2:** run; expect FAIL.
- [ ] **Step 3:** Implement `register_repo`, `repo_swaps`, the init flag/row, the status line, README and agent instruction line.
- [ ] **Step 4:** `cargo test` — all pass. Then by hand in a scratch git repo: `knot init --agents claude --swaps --yes`, run `echo '{"tool_name":"Bash","tool_input":{"command":"grep -rn fn src"},"cwd":"'$PWD'","session_id":"s"}' | knot hook pre-tool` and see the rewrite JSON; run the rewritten command and see the notice line.
- [ ] **Step 5:** Commit `Turn on shell swaps per repo with knot init --swaps`.

### Task 7: Measure it

**Files:** none in `src/`; results go in the PR description.

- [ ] **Step 1:** In a scratch clone of this repo with swaps on, run a short real Claude Code session that explores code (ask it to explain how `knot check` finds conflicts).
- [ ] **Step 2:** From `.knot/cache/events.jsonl`, report: number of swaps by tool, total `bytes_before` vs `bytes_after`, estimated tokens saved, and any case where the agent re-ran a swapped command with `KNOT_RAW=1` or different flags (the spec's success test: there should be none caused by confusion).
- [ ] **Step 3:** Run `trail check` and `knot check --base main`; fix or explain conflicts in the commit message.
