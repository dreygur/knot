# knot viz Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace today's `knot viz` page (one vis-network hairball loaded from a CDN) with the approved "constellation" page, as one offline HTML file.

**Architecture:**
- **Data.** Rust builds the page's data from Knot's own graph: folder, kind, line, signature, PageRank from `src/ast/pagerank.rs`, and learnings from `active_learnings`.
- **Template.** The prototype's `page.html` becomes the template.
- **Inlined assets.** sigma.js, graphology, its ForceAtlas2 layout, the fonts and the logo are all inlined. The fonts are the renamed IBM Plex subsets already in `assets/fonts`.
- **Layout.** The folder-anchored ForceAtlas2 layout from `build.js` runs in the page, from a fixed seed, before the "settle in" motion.
- **Output.** The file is written through `state_file` into `.knot/cache/`.

**Tech Stack:** Rust (serde_json); vendored MIT JavaScript: graphology 0.25.4, sigma 3.0.2, and the graphology ForceAtlas2 layout from a released `graphology-library` UMD bundle (same graphology major).

**Spec:** docs/superpowers/specs/2026-10-28-knot-agent-efficiency-design.md, "Part 4 — `knot viz` redesign". The approved prototype is docs/superpowers/prototypes/knot-viz/ (`page.html`, `build.js`), and it is the visual reference.

**Branch:** `viz-redesign`, cut from local `main` (8ad7748).

## Global Constraints

- **One offline file.** It loads no `http(s)` scripts, stylesheets or fonts. sigma, graphology and the fonts are inlined, and their MIT and OFL licence texts ship in `assets/viz/` and `assets/fonts/`.
- **Fonts.** Use only the renamed subsets in `assets/fonts/`: `KnotSans-Regular.ttf`, `KnotSans-SemiBold.ttf` and `KnotMono-Regular.ttf`, with families "Knot Sans" and "Knot Mono". Never ship a file named or internally named "Plex", because the OFL reserves that name for modified fonts.
- **Colours.** sigma colours are RGB hex. Convert any `oklch()` value by painting one pixel and reading it back with getImageData. Folder hues step by 137.508°, starting at 200, with alternating lightness, as in the prototype.
- **Layout.** ForceAtlas2 runs with one hidden anchor per folder, tied to every symbol in that folder at weight 1.5. Settings: `linLogMode: true`, `outboundAttractionDistribution: true`, `gravity: 0.4`, `scalingRatio: 3`, `barnesHutOptimize: true`, `edgeWeightInfluence: 1`, 1200 iterations. The anchors are removed afterwards. Initial positions are deterministic (seeded), so the same graph always gives the same map.
- **Folder of a file.** The first two path segments when the path has more than 2 parts, the first segment when it has exactly 2, and "(root)" otherwise. This is `build.js` `folderOf`.
- **Brand.**
  - The header shows the whole logo: `assets/logo-light.png` on the dark theme and `assets/logo-dark.png` on the light theme.
  - The favicon is `mark-light-64.png` or `mark-dark-64.png`, chosen with `<link rel="icon" media="(prefers-color-scheme: …)">`.
  - Every image is inlined as a data URI.
- **Output.** The page is written to `<repo>/.knot/cache/graph.html` through `crate::ask::state_file`, never with a plain `fs::write`. When opening the browser fails, Knot prints the path and says it could not open it, never "Opened in default web browser".
- **Script safety.** Data JSON goes into the inline script with `<`, `>` and `&` escaped as `<`, `>` and `&` (as today). Text the page puts into HTML is escaped.
- Build and test with `CARGO_TARGET_DIR` off the TOTUL volume. Never trigger GitHub CI; do not push.

## Review Focus

1. **Large repos (10k+ symbols):** the page must not freeze the tab for long. Run the layout in chunks with `requestAnimationFrame`, or in a Web Worker built from an inline Blob, and show progress. Test in Task 3 (`layout_runs_off_the_main_frame`, a string check that the page uses a worker or a chunked loop, plus a manual run on a generated 10k-node graph).
2. **An empty or tiny repo** (0 symbols, 1 folder, no calls): the page renders an empty state, not a JavaScript error. Test in Task 2 (`empty_graph_data`) and Task 3 (`empty_page_renders`).
3. **Symbol text with HTML or `</script>`:** it cannot break out of the script or inject markup. Test in Task 3 (keep `test_symbol_text_cannot_close_the_script_tag`, and add `names_are_escaped_in_the_inspector` as a string check on the page's `esc()` use).
4. **A symlinked `.knot` or `.knot/cache`:** the write is refused, not redirected. Test in Task 4 (`symlinked_cache_is_refused`).
5. **No browser available:** this is SSH or headless. The path is printed with an honest message and the exit code is 0. Test in Task 4 (`open_failure_is_reported_honestly`).

---

### Task 1: Vendored assets

**Files:**
- Create: `assets/viz/graphology.umd.min.js`, `assets/viz/sigma.min.js` and `assets/viz/graphology-library.min.js` (exact released versions from npm, via cdn.jsdelivr.net), plus `assets/viz/LICENSES.txt` holding the MIT texts with versions and source URLs
- Create: `src/ui/viz_assets.rs`, which exposes the embedded strings and bytes
- Modify: `src/ui/mod.rs` (declare the module)

**Interfaces:**
- Produces:
  - `pub const GRAPHOLOGY_JS: &str`, `pub const SIGMA_JS: &str` and `pub const FA2_JS: &str`, all via `include_str!`.
  - `pub fn font_face_css() -> String`, which returns three `@font-face` rules with the TTFs as base64 data URIs, under the families "Knot Sans" (400 and 600) and "Knot Mono" (400).
  - `pub fn data_uri_png(bytes: &[u8]) -> String`.
  - `pub const LOGO_LIGHT: &[u8]`, `LOGO_DARK`, `MARK_LIGHT_64` and `MARK_DARK_64`, via `include_bytes!` from `assets/`.
- Use the same `include_bytes!` paths that `src/card/render.rs` uses for the fonts, or move those constants into one shared place if that is simpler. Do not duplicate the font files.

- [ ] **Step 1: Write the failing tests** in `viz_assets.rs`:
  - `vendored_scripts_are_present`: each JS constant is longer than 10 KB. `GRAPHOLOGY_JS` contains "graphology", `SIGMA_JS` contains "Sigma", and `FA2_JS` contains "forceAtlas2" or "forceatlas2".
  - `font_css_uses_knot_names_only`: the output contains `font-family: "Knot Sans"` and `"Knot Mono"`, and contains no "Plex".
  - `data_uri_is_png`: it starts with `data:image/png;base64,`.
- [ ] **Step 2:** Run `cargo test ui::viz_assets`. Expect FAIL.
- [ ] **Step 3:** Download the three bundles in their pinned versions, check that each is the minified UMD build exposing a global (`graphology`, `Sigma`, `graphologyLibrary`), and implement.
- [ ] **Step 4:** Run the tests (PASS), then the full suite.
- [ ] **Step 5:** Commit `Vendor sigma, graphology and the layout for an offline knot viz`.

### Task 2: Page data from Knot's graph

**Files:**
- Create: `src/ui/viz_data.rs`

**Interfaces:**
- Consumes: `CodeGraph` (`symbols`, `callees`), `crate::ast::pagerank::compute_pagerank`, `SymbolKind::as_str`, and `crate::engine::learnings::active_learnings(repo_root)` (`RepoLearning { frontmatter: { title, author, date, touches, .. }, body, file_path }`).
- Produces:

```rust
#[derive(Serialize)]
pub struct VizData { pub folders: Vec<String>, pub nodes: Vec<VizNode>, pub edges: Vec<(usize, usize)>, pub learnings: Vec<VizLearning> }
#[derive(Serialize)]
pub struct VizNode { pub id: String, pub l: String, pub f: String, pub ln: usize, pub s: String, pub fo: usize, pub k: String, pub pr: f64, pub le: Vec<usize> }
#[derive(Serialize)]
pub struct VizLearning { pub title: String, pub author: String, pub date: String }
pub fn folder_of(file: &str) -> String
pub fn viz_data(graph: &CodeGraph, learnings: &[RepoLearning]) -> VizData
```

- Edges are indices into `nodes` (smaller than the prototype's id pairs). Self-edges and edges to unknown ids are dropped, and duplicate edges are deduplicated.
- Folders are sorted. `fo` indexes them.
- `le` lists the indices into `learnings` of the active learnings whose `touches` include the node's file.
- Nodes are sorted by id, so the output is deterministic.
- `pr` is rounded to 6 decimals.

- [ ] **Step 1: Write failing tests:**
  - `folder_of_matches_the_prototype`: `"src/cli/mod.rs"` → `"src/cli"`, `"src/lib.rs"` → `"src"`, `"main.rs"` → `"(root)"`.
  - `edges_are_indices_without_self_or_duplicate_calls`.
  - `learnings_attach_by_touched_file`.
  - `kind_comes_from_symbol_kind`: a `Struct` gives `"struct"`.
  - `empty_graph_data`: no symbols gives empty vectors and no panic.
  - `output_is_deterministic`: two calls serialize identically.
- [ ] **Step 2:** Run `cargo test ui::viz_data`. Expect FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests (PASS), then the full suite.
- [ ] **Step 5:** Commit `Build knot viz data from the graph, PageRank and learnings`.

### Task 3: The page

**Files:**
- Create: `src/ui/viz.html`, the template, ported from `docs/superpowers/prototypes/knot-viz/page.html`
- Modify: `src/ui/viz.rs` (replace `generate_graph_html` and the vis-network export with `render_page`, and update its tests)

**Interfaces:**
- Consumes: Task 1 constants and `font_face_css()`, and Task 2 `VizData`.
- Produces: `pub fn render_page(data: &VizData) -> String`. It fills these template slots in one pass, so inserted text is never re-scanned: `{{FONTS_CSS}}`, `{{GRAPHOLOGY}}`, `{{SIGMA}}`, `{{FA2}}`, `{{DATA}}` (escaped JSON), `{{LOGO_LIGHT}}`, `{{LOGO_DARK}}`, `{{MARK_LIGHT}}` and `{{MARK_DARK}}`.
- **Port everything the spec's "What the prototype settled" lists:**
  - map regions with floating folder names;
  - size by rank, with labels only for the top symbols until you zoom in;
  - a switch to hide symbols with no calls;
  - selection that fades the rest, colours callers cyan and callees amber, and glides the camera to frame the symbol;
  - an inspector showing kind · folder, name, `file:line`, signature, rank, clickable callers and calls, and learnings. With no learnings it shows the command `knot note --touches <file>`.
  - search: `/` focuses it, results rank by match then rank, arrow keys and Enter work, and Esc clears.
  - the folder list: hover isolates a folder, click hides it;
  - dark and light themes following the system setting, with a toggle;
  - Knot Sans and Knot Mono, themed scrollbars, selection colour and focus rings;
  - the one settle-in motion.
- **Port these changes too:**
  - The layout now runs in the page. Port `build.js`'s anchor layout (Global Constraints) with a seeded random. It runs in a Web Worker created from an inline Blob that holds `FA2_JS`, or in chunked frames. A "Laying out N symbols…" state shows until it finishes.
  - Edges arrive as index pairs.
  - The header shows the logo (`<img>` with the light or dark data URI per theme), plus two favicon `<link>`s with `media` queries.

- [ ] **Step 1: Write failing tests** in `viz.rs`:
  - `page_is_offline`: there is no `src="http`, no `href="http` and no `@import url(http`.
  - `page_has_logo_and_both_favicons`: the page contains `rel="icon"`, `prefers-color-scheme: dark`, `prefers-color-scheme: light`, and the base64 prefix of `LOGO_LIGHT`.
  - `page_uses_knot_fonts_only`: the page contains "Knot Sans" and no "Plex".
  - `layout_runs_off_the_main_frame`: the page contains `new Worker(` or `requestAnimationFrame` alongside `forceAtlas2`.
  - `empty_page_renders`: `render_page` of empty data still contains the empty-state text "No symbols indexed yet".
  - Keep `test_symbol_text_cannot_close_the_script_tag`, adapted to `render_page`.
  - `names_are_escaped_in_the_inspector`: the template defines `esc(` and uses it for names, signatures and learning titles. This is a string check.
- [ ] **Step 2:** Run `cargo test ui::viz`. Expect FAIL.
- [ ] **Step 3:** Port the template and implement `render_page`.
- [ ] **Step 4:** Run the tests (PASS). Then render this repo's page into the scratchpad. Screenshot it with headless Chrome (`"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --use-angle=swiftshader --window-size=1440,900 --virtual-time-budget=15000 --screenshot=<png> file://<html>`), once with the system dark theme and once forcing light. Look at both screenshots against the prototype. Generate a 10k-node synthetic graph and confirm it lays out (time it). Run the full suite.
- [ ] **Step 5:** Commit `Redesign knot viz as an offline constellation map`.

### Task 4: Output and the command

**Files:**
- Modify: `src/ui/viz.rs` (`render_and_launch_viz`, `open_browser`) and `src/cli/mod.rs:~367` (`cli_viz` message)

**Interfaces:**
- Consumes: `render_page`, `viz_data`, `crate::ask::{load_graph, state_file, state_dir}`, `active_learnings`.
- Produces: `pub fn render_and_launch_viz(repo_root: &Path) -> Result<(PathBuf, bool)>`, where the bool says whether the browser opened. Its path is `state_file(&state_dir(repo_root), "graph.html")`.
  - `open_browser` returns `Err` when the spawn fails, and on Linux/macOS when the opener exits non-zero.
  - `cli_viz` prints `[KNOT] Code graph map: <path>`, then `[KNOT] Opened in your browser.` or `[KNOT] Couldn't open a browser; open the file above.`

- [ ] **Step 1: Write failing tests:**
  - `page_is_written_into_the_cache`: the path ends with `.knot/cache/graph.html`.
  - `symlinked_cache_is_refused`: `.knot/cache` is a symlink to an outside directory, `render_and_launch_viz` errors, and the outside directory stays empty. Factor out `write_page(repo_root) -> Result<PathBuf>` so the test does not open a browser.
  - `open_failure_is_reported_honestly`: make the opener injectable, so an opener that fails gives `false` and the message says "Couldn't open".
- [ ] **Step 2:** Run `cargo test ui::viz`. Expect FAIL.
- [ ] **Step 3:** Implement. Remove the old `.knot/graph.html` write path. Leave an old file in place if one exists.
- [ ] **Step 4:** Run the tests (PASS), then the full suite.
- [ ] **Step 5:** Commit `Write knot viz into .knot/cache and say when the browser did not open`.
