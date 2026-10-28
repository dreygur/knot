# Knot Share Card Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `knot card` writes a 1200×675 PNG of what Knot saved and copies it to the clipboard, for posting on X.

**Architecture:**
- **Events.** Learning reuse becomes an event in the per-repo event log, carrying the learning's id and its recorded cost.
- **Repo list.** A small list of repos with events is kept in `~/.knot`.
- **Pipeline.** A pure stats step totals every listed repo's log into `CardStats`. A render step fills an SVG template from `CardStats` and rasterises it with resvg, using the logo and IBM Plex fonts built into the binary. The CLI writes the file and hands it to the platform's clipboard.

**Tech Stack:** Rust; existing `serde_json`, `chrono`, `dirs`; new `resvg` (with its `usvg`/`tiny-skia`) for rendering.

**Spec:** docs/superpowers/specs/2026-10-28-knot-agent-efficiency-design.md, "Part 5 — Share card" (read it with "Part 0 — Event log", "Constraints" and "Anti-goals").

**Branch:** `share-card`, cut from `agent-efficiency` (PR #14), which holds Part 0.

## Global Constraints

- No LLM calls, no API key, no network; everything is deterministic.
- Every write into `<repo>/.knot/cache/` goes through `crate::ask::state_file(dir, name)`. `~/.knot` gains only one small file, the repo list.
- **Picture:** 1200×675, dark only. It uses the whole logo `assets/logo-light.png` in the top row, never the mark beside a text label.
- **Layout, top to bottom:**
  - Top row: logo left, time label right.
  - Main row: two big numbers, then the trend line or the per-session figure.
  - Three tiles, in the order graph answers, learnings reused, grep/sed swaps.
  - Bottom row: "runs in" chips, and `github.com/dreygur/knot`.
- **Python edits blocked never appear on the card.**
- **"Tokens saved"** = Σ `saved_tokens` of `answer` events + Σ `saved_tokens` of `swap` events + Σ `cost_tokens` of each distinct reused learning (once per learning).
- **"Less for the agent to read"** = 1 − Σ`bytes_after` / Σ`bytes_before` over `answer` and `swap` events only. Learnings are excluded.
- **Estimates:** every figure is labelled with one "≈ estimated" note on the card. A learning with no recorded cost adds nothing; Knot never invents a figure.
- **Time label:**
  - "last 30 days" when the first event is 30 or more days old;
  - otherwise "since Oct 21 · 7 days", from the first event;
  - "first day" with no events.
- **Trend line:** needs ≥ 3 distinct days with events. Otherwise its space shows "≈ N tokens saved per session".
- **No events:** the card is still made, with numbers at 0 and tiles at "0". Tiles never disappear.
- **Scope:** all listed repos by default, and no repo name appears. `--repo` uses only the current repo and shows its folder name in the top row.
- **Output:** `--out <path>` overrides the default path, which is `~/Downloads/knot-card-YYYY-MM-DD.png`, or the current directory when there is no Downloads folder. Knot prints the path, then `✓ copied to clipboard`, or a line saying copying failed. A clipboard failure never fails the command.
- Build and test with `CARGO_TARGET_DIR` off the TOTUL volume. Never point HOME at the real home in a test that writes. Never trigger GitHub CI; do not push.

## Review Focus

1. **A repo in the list was deleted or moved:** it is skipped silently. Test in Task 3 (`gone_repo_is_skipped`).
2. **A corrupt or half-written line in an event log:** it is skipped, and the rest still counts. Test in Task 3 (`bad_lines_are_skipped`).
3. **A large log covering months:** stats read the whole file line by line, not just the 2 MiB tail that `recent` reads. Test in Task 3 (`old_events_beyond_the_tail_still_count`).
4. **No clipboard tool available (headless Linux, SSH):** the file is still written and the command exits 0. Test in Task 5 (`clipboard_failure_still_writes_the_file`).
5. **Huge numbers or many agents** (billions of tokens, 6+ agents): the numbers are shortened (`4.1M`, `1.2B`), and chips after the third become `+N`, so nothing overflows the picture. Test in Task 4 (`big_numbers_and_many_agents_fit`).

---

### Task 1: Learning-reuse events

**Files:**
- Modify: `src/ask/events.rs` (new kind and fields, plus a whole-file reader)
- Modify: `src/cli/hook.rs` (`learnings_for_file`, log what was shown)
- Modify: `src/cli/mod.rs:~663` and `src/tools/mod.rs:~97` (log the learnings `knot ask` returned)

**Interfaces:**
- Produces:
  - `EventKind::Learning` (serialized `"learning"`).
  - `Event` gains `#[serde(default)] learning: Option<String>`, `#[serde(default)] cost_minutes: Option<u64>` and `#[serde(default)] cost_tokens: Option<u64>`.
  - `pub fn learning_shown(root: &Path, id: &str, cost_minutes: Option<u64>, cost_tokens: Option<u64>)` appends one `Learning` event with `tool = "learning"`, `saved_tokens = 0` and the current session.
  - `pub fn all(root: &Path) -> Vec<Event>` reads the whole log line by line, skipping bad lines.
- The id is the learning's file stem: from `RepoLearning.file_path` in the hook, and from the learning path carried by `LearningHit` in `knot ask`. If `LearningHit` has no learning-path field, add `pub id: String` filled where hits are built in `src/ask/learnings.rs`.

- [ ] **Step 1: Write failing tests** in `events.rs`:
  - `learning_event_round_trips`: append via `learning_shown(dir, "2026-10-27-x", Some(12), Some(27_000))`; `all(dir)` has one event with `kind == EventKind::Learning`, `learning == Some("2026-10-27-x")` and `cost_tokens == Some(27_000)`.
  - `old_lines_without_new_fields_still_parse`: a hand-written pre-change `answer` line parses with `learning == None`.
- [ ] **Step 2:** In `src/cli/hook.rs`, add a test `shown_learnings_are_logged`: a temp repo with one learning touching `src/a.rs`; after `learnings_for_file` runs for that file, `events::all` has one `Learning` event with its id.
- [ ] **Step 3:** Run `cargo test events:: hook::shown_learnings`. Expect FAIL.
- [ ] **Step 4:** Implement. Log only learnings actually shown: the `fresh` set in the hook, and the hits returned by `knot ask`.
- [ ] **Step 5:** Run the focused tests (PASS), then the full `cargo test`.
- [ ] **Step 6:** Commit `Log each learning Knot shows, with what it took to work out`.

### Task 2: Repo list in `~/.knot`

**Files:**
- Create: `src/card/mod.rs` (module root: `pub mod repos; pub mod stats; pub mod render; pub mod clipboard;`, with the modules added by their tasks) and `src/card/repos.rs`
- Modify: `src/lib.rs` or `src/main.rs` (declare `card`)
- Modify: `src/ask/events.rs` `append` (register on the first write) and `src/cli/init.rs` (register on init)

**Interfaces:**
- Produces:
  - `pub fn register(home: &Path, repo_root: &Path)` adds the canonical `<repo>/.knot/cache` path to `home/.knot/repos.txt` (one per line) if absent. It refuses a symlinked `repos.txt` or `.knot` and writes through temp+rename. Failures are ignored.
  - `pub fn list(home: &Path) -> Vec<PathBuf>` returns the listed repo roots (the parent of each `.knot/cache`), deduplicated and in insertion order.
- `append` calls `register` only when it creates `events.jsonl` (the file did not exist before the write). `knot init` calls it once.

- [ ] **Step 1: Write failing tests** in `repos.rs`, using temp home and temp repos only:
  - `register_twice_lists_once`.
  - `worktrees_sharing_a_cache_list_once`: two roots whose `.knot/cache` canonicalise to the same path give one entry.
  - `symlinked_list_is_refused`: `home/.knot/repos.txt` is a symlink to a file outside, and after `register` that file is unchanged.
- [ ] **Step 2:** Run `cargo test card::repos`. Expect FAIL.
- [ ] **Step 3:** Implement. Get the home path from `dirs::home_dir()` only at the call sites; the functions take `home` so tests inject it.
- [ ] **Step 4:** Run the tests (PASS), then the full suite.
- [ ] **Step 5:** Commit `Keep a list of repos Knot has worked in`.

### Task 3: Card statistics

**Files:**
- Create: `src/card/stats.rs`

**Interfaces:**
- Consumes: `events::all`, `EventKind::{Answer, Swap, Learning}`, `repos::list`.
- Produces:

```rust
pub struct CardStats {
    pub tokens_saved: u64,          // graph + swaps + learnings
    pub less_to_read: Option<f64>,  // 0.0..=1.0; None when Σbefore == 0
    pub graph: Tile,                // tokens from answer events, count = answers
    pub learnings: LearningTile,    // distinct learnings, tokens, minutes
    pub swaps: Tile,                // tokens from swap events, count = swaps
    pub label: String,              // "last 30 days" | "since Oct 21 · 7 days" | "first day"
    pub trend: Trend,
}
pub struct Tile { pub tokens: u64, pub count: u64 }
pub struct LearningTile { pub count: u64, pub tokens: u64, pub minutes: u64 }
pub enum Trend { Daily(Vec<u64>), PerSession(u64) }
pub fn collect(repos: &[PathBuf], now: chrono::DateTime<chrono::Local>) -> CardStats
```

- Window: events at or after `now − 30 days`. When the first event is older than that, the label is "last 30 days" and only in-window events count. Otherwise every event counts, and the label is "since {first:%b %-d} · {days} days" (or "· 1 day").
- `Trend::Daily` holds one total per calendar day from the first in-window day to today. It is used when ≥ 3 distinct days have events; otherwise `PerSession` = `tokens_saved` / the number of distinct sessions (0 when there are none).
- Learnings are deduplicated by `learning` id across all repos. `cost_tokens` and `cost_minutes` are taken from the latest event for that id.
- `EditBlocked` and `Override` events are ignored.

- [ ] **Step 1: Write failing tests** with events written into temp repos through `events::append`:
  - `headline_is_the_sum_of_the_three_tiles`: answers 2,000 and 600, swap 400, learning A shown twice with `cost_tokens` 27,000, learning B with none. Expect `tokens_saved == 30_000`, `learnings.count == 2`, `learnings.tokens == 27_000`.
  - `less_to_read_uses_answers_and_swaps_only`: before 1,000 / after 250 gives `Some(0.75)`; learning events change nothing.
  - `python_blocks_are_ignored`: an `EditBlocked` event changes no field.
  - `label_for_a_new_user`: the first event is 7 days before `now`, giving `"since Oct 21 · 7 days"` with `now` fixed at 2026-10-28.
  - `label_after_thirty_days`: gives `"last 30 days"`, and an event 40 days old does not count.
  - `no_events_is_first_day_and_zeros`.
  - `trend_needs_three_days`: 2 days of events gives `PerSession`; 3 days gives `Daily` with one entry per day.
  - `gone_repo_is_skipped`.
  - `bad_lines_are_skipped`.
  - `old_events_beyond_the_tail_still_count`: write more than 2 MiB of events, and the oldest still count.
- [ ] **Step 2:** Run `cargo test card::stats`. Expect FAIL.
- [ ] **Step 3:** Implement `collect`.
- [ ] **Step 4:** Run the tests (PASS), then the full suite.
- [ ] **Step 5:** Commit `Total what Knot saved across repos for the share card`.

### Task 4: Render the card

**Files:**
- Create: `src/card/render.rs`, `src/card/card.svg` (template, `include_str!`), and `assets/fonts/` holding IBM Plex Sans (Regular, SemiBold) and IBM Plex Mono (Regular) as Latin-subset TTF, plus `OFL.txt`
- Modify: `Cargo.toml` (add `resvg`)

**Interfaces:**
- Consumes: `CardStats`, `Tile`, `LearningTile`, `Trend`.
- Produces:
  - `pub struct CardView { pub stats: CardStats, pub repo_name: Option<String>, pub agents: Vec<String> }`
  - `pub fn render_png(view: &CardView) -> anyhow::Result<Vec<u8>>`, which returns a PNG 1200×675.
  - `pub fn short(n: u64) -> String`, giving `999`, `1.2k`, `4.1M`, `1.2B`. If `crate::ask::savings::short` already formats the same way, reuse it instead of adding this.
- The template follows the approved mockup `.superpowers/brainstorm/91185-1793180968/content/card-stacked-v3.html` at 2× (the mockup is 600×338). The fonts and `logo-light.png` are embedded with `include_bytes!` and loaded into a `usvg::fontdb::Database`; the logo goes in as a base64 data URI.
  - **Tiles:** the graph tile shows its tokens and "{count} answers". The learnings tile shows its tokens and "{count} learnings". When `minutes > 0` it adds "· ~{h} h" (or "· {m} min" below an hour). The swaps tile shows its tokens and "{count} runs".
  - **Trend:** `Daily` draws a smoothed line with an area fill. `PerSession` draws the text "≈ {short} tokens saved per session" in that space.
  - **Estimate note:** "≈ estimated" appears once, small, near the headline.
  - **Repo name:** when present, it sits in the top row beside the label.
  - **Chips:** the first three agents, then "+N". With no agents the chips row is hidden; the repo link stays.
  - **Colours:** RGB hex only.

- [ ] **Step 1: Write failing tests:**
  - `png_is_1200_by_675`: decode the header bytes (width/height in IHDR) of `render_png` on a sample view.
  - `zero_stats_render`: `CardStats` from `no_events` renders without error.
  - `short_formats`: 999 → "999", 1_234 → "1.2k", 4_100_000 → "4.1M", 1_200_000_000 → "1.2B".
  - `big_numbers_and_many_agents_fit`: a view with 9,999,999,999 tokens and 7 agents renders. The SVG text produced (expose `fn svg(view) -> String` as `pub(crate)`) contains "+4" and no agent beyond the third.
  - `python_edits_never_drawn`: the SVG text contains no "python".
- [ ] **Step 2:** Run `cargo test card::render`. Expect FAIL.
- [ ] **Step 3:** Subset the fonts (for example `pyftsubset` with `--unicodes=U+0020-007E,U+00B7,U+2248,U+2212,U+2713,U+2192`). Add `resvg` and implement.
- [ ] **Step 4:** Run the tests (PASS). Write one sample PNG into the scratchpad and look at it against the mockup. Then run the full suite.
- [ ] **Step 5:** Commit `Draw the share card as a PNG`.

### Task 5: `knot card` command and clipboard

**Files:**
- Create: `src/card/clipboard.rs`
- Modify: `src/card/mod.rs` (`pub async fn run(args) -> Result<()>` or sync, matching neighbouring commands), `src/hosts/agent.rs` (`knot_wired`), `src/cli/mod.rs` (dispatch `Some("card")`, add `"card"` to `COMMANDS`, one help line), `README.md` (a short "Share what Knot saved" section)

**Interfaces:**
- Consumes: `repos::list`, `stats::collect`, `render::{render_png, CardView}`.
- Produces:
  - `pub fn copy_png(path: &Path) -> Result<()>`:
    - macOS: `osascript -e 'set the clipboard to (read (POSIX file "<path>") as «class PNGf»)'`.
    - Linux: `wl-copy --type image/png < file` when `WAYLAND_DISPLAY` is set, else `xclip -selection clipboard -t image/png -i <file>`.
    - Windows: PowerShell `Add-Type -AssemblyName System.Windows.Forms; [Windows.Forms.Clipboard]::SetImage([Drawing.Image]::FromFile('<path>'))`.
    - The path is passed as an argument, never interpolated into a shell string. On macOS, escape it for the AppleScript string literal.
  - `Agent::knot_wired(self, home: &Path) -> bool` is true when that agent's user config holds a `knot` MCP entry:
    - Claude: `~/.claude.json` `mcpServers.knot`.
    - Codex: `~/.codex/config.toml` `[mcp_servers.knot]`.
    - Gemini and Antigravity: `~/.gemini/settings.json` `mcpServers.knot`.
    - Cursor: `~/.cursor/mcp.json` `mcpServers.knot`.
    - OpenCode: `~/.config/opencode/opencode.json` `mcp.knot`.
    - Check these against install.sh lines 120–132, and use the display name (`Agent::display`) for chips.
- **Flags:** `knot card [--repo] [--out <path>]`. Output lines are exactly the path, then `✓ copied to clipboard` or `· couldn't copy to the clipboard ({reason}); the file is saved`.

- [ ] **Step 1: Write failing tests:**
  - `knot_wired_reads_each_agents_config`: a temp home with only `~/.cursor/mcp.json` holding knot, so only Cursor is wired.
  - `default_path_is_downloads_then_cwd`.
  - `clipboard_failure_still_writes_the_file`: inject a copier that errors; the PNG exists and the result is `Ok`.
  - `repo_flag_limits_to_this_repo`: with two repos in the list and `--repo`, only the current one's events count and `repo_name` is its folder name.
- [ ] **Step 2:** Run `cargo test card::`. Expect FAIL.
- [ ] **Step 3:** Implement. Keep the copier injectable (a closure or trait) for the test.
- [ ] **Step 4:** Run the tests (PASS), then the full suite. Then by hand, with the built binary, run `knot card --out <scratchpad>/card.png`. Open the PNG, and on macOS paste into an app to confirm the clipboard holds the image.
- [ ] **Step 5:** Commit `Add knot card, which saves and copies a picture of what Knot saved`.
