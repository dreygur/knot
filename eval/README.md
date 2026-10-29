# Retrieval eval

Scores `trail ask` and `knot ask` on the same 53 questions over three corpora:
Knot itself (Rust), Trail-Code (TypeScript) and psf/requests v2.32.3 (Python).

Each question names the symbol that answers it. A hit is a returned
`file:Lstart-Lend` whose span overlaps that symbol's definition; spans come from
parsing the corpus, not from either tool's index. `file@5` gives partial credit
when a correct file appears in the top 5 without a usable span.

## Setup

Corpora live in `reference/`, which is git ignored. Each must be its own git repo,
or Trail skips it because the parent repo ignores `reference/`.

```
git clone --depth 1 --branch v2.32.3 https://github.com/psf/requests.git reference/requests
mkdir -p reference/knot-snapshot && git archive HEAD | tar -x -C reference/knot-snapshot
(cd reference/knot-snapshot && rm -rf reference && git init -q && git add -A && git commit -qm snapshot)
git clone --depth 1 --branch v1.8.1 https://github.com/spf13/cobra.git reference/cobra
git clone --depth 1 --branch gson-parent-2.11.0 https://github.com/google/gson.git reference/gson
for c in knot-snapshot trail-code requests cobra gson; do (cd reference/$c && trail build .); done
cargo build --release
```

## Run

```
cargo run --release --example eval                  # both engines, all corpora
cargo run --release --example eval -- --engine trail --corpus requests
cargo run --release --example eval -- --questions eval/langs.json   # Go and Java
```

Misses print to stderr with the engine's top result, which is how ground truth
gets reviewed: an equally valid answer is added to `expect`, a wrong one stays a miss.

## Results, 2026-10-27

Trail 0.26.0 against `knot ask` (BM25F plus graph rerank, warm cache).
`questions.json` is the tuning set; `holdout.json` was written before any tuning
and never used to choose settings, so it is the number to trust.

| set | engine | asked | hit@1 | hit@5 | file@5 | MRR@10 |
|---|---|---|---|---|---|---|
| tuning | knot | 53 | 60% | 83% | 92% | 0.706 |
| tuning | trail | 53 | 51% | 66% | 85% | 0.581 |
| held out | knot | 21 | 52% | 86% | 95% | 0.644 |
| held out | trail | 21 | 38% | 43% | 76% | 0.405 |
| Go and Java | knot | 32 | 50% | 78% | 88% | 0.618 |
| Go and Java | trail | 32 | 31% | 44% | 78% | 0.370 |
| TS app (taxonomy) | knot | 20 | 70% | 75% | 75% | 0.717 |
| TS app (taxonomy) | trail | 20 | 45% | 70% | 80% | 0.581 |

`v2.json` is 20 untuned questions on shadcn-ui/taxonomy at 298a885
(`git clone https://github.com/shadcn-ui/taxonomy.git reference/taxonomy`), a Next.js
app built on `@/` path aliases and `export const` components, which no other corpus has.
Knot's misses there are real (e.g. the open graph question finds the page that builds
the image URL, not the route that renders it).

`langs.json` covers spf13/cobra v1.8.1 (Go) and google/gson 2.11.0 (Java), 16
questions each, written from the sources' doc comments after Go and Java support
landed and never used to tune anything; it is a second held out set. Per corpus
MRR: cobra knot 0.591 vs Trail 0.263, gson knot 0.646 vs Trail 0.477. Knot's
misses there are real ranking misses (e.g. "top level command" finds `tmpl`,
which builds a template named "top"), so the ground truth stays as written.

Per corpus on the tuning set, knot leads on MRR everywhere but trails Trail on
hit@1 for Trail's own TypeScript code (39% vs 44%). Median latency, warm: knot
36 ms (Rust), 53 ms (Python), 190 ms (TS, 863 files); Trail 275 to 500 ms.

Tried and rejected on the tuning set (overall MRR, baseline 0.706): extra question
stop words with class bodies dropped (0.690), per field IDF (0.625), graph weight
0.15 / 0.40 / 0.55 (0.675 / 0.677 / 0.655), undirected graph walk (0.698).

With 7 held out questions per corpus, one question moves a corpus score by 14
points. Read per corpus held out rows as direction, not magnitude.

## Agent Retrieval Bench, 2026-10-29

[Agent Retrieval Bench](https://github.com/eyuansu62/agent-retrieval-bench) (ARB,
commit 9e4d689) asks for the files a coding agent needs next, from real workflow
signals: a PR's intent (find its tests), a review comment (find more context), a
failure trace (find the root cause) and an edited file (find what else changes).
Its questions and answers come from real PRs, not from us, so it is the number to
quote. Knot sees the same JSON query text as ARB's own baselines.

`eval/arb/knot_rank.py` runs ARB's own loop and metrics and swaps in only the
ranking: Knot ranks symbols in the repo rebuilt at the base commit, and each hit
maps to ARB's chunk it overlaps. Files Knot never names count as unranked. Running
ARB's BM25 through the same script reproduces its published row exactly (MRR
0.1520, Recall@20 0.4452), which checks the setup.

```
git clone https://github.com/eyuansu62/agent-retrieval-bench.git $ARB_HOME/src
uv venv $ARB_HOME/venv -p 3.12 && VIRTUAL_ENV=$ARB_HOME/venv uv pip install -e $ARB_HOME/src huggingface_hub
(cd $ARB_HOME/src && PATH=$ARB_HOME/venv/bin:$PATH arb download-benchmark --all --local-dir data)
ARB_HOME=... KNOT_BIN=target/release/knot KNOT_LABEL=knot eval/arb/run.sh knot
```

All 345 answerable samples, ARB's sample weighted overall:

| ranker | MRR | Recall@20 | Recall@5 |
|---|---|---|---|
| BM25 (ARB, reproduced) | 0.152 | 0.445 | 0.190 |
| lexical (ARB, published) | 0.157 | 0.494 | |
| RepoMap (ARB, published) | 0.216 | 0.633 | |
| Qwen3-Embedding-4B (ARB, published) | 0.238 | 0.631 | |
| Qwen3-Embedding-8B (ARB, published) | 0.234 | 0.703 | |
| knot 0a46963 (before any ARB work) | 0.215 | 0.418 | 0.251 |
| knot 9ee4063, every match past the reranked head | 0.219 | 0.428 | 0.251 |
| knot 0da7884, test and source counterparts | **0.307** | 0.493 | **0.370** |

Per subset at 0da7884 (MRR / Recall@20): trace2code 0.552 / 0.835, edit2ripple
0.254 / 0.504, code2test 0.183 / 0.330, comment2context 0.202 / 0.267.

Rejected after one ARB run each: direct callers and callees of strong matches
entering the results (MRR 0.216, Recall@20 0.389; they crowd out real matches)
and a boost for matches in the folder of a named file (MRR 0.304, Recall@20 0.494;
noise). The counterpart rule was written after reading four ARB samples, so its
gain is informed by the benchmark; it rests on naming conventions, not on tuned
constants. Recall@20 is where Knot still trails RepoMap and the embedding models,
most of all on comment2context, whose answers share neither words nor paths with
the comment.

## Other tools on these question sets, 2026-10-29

`examples/eval.rs --engine ext:<name>` runs `eval/adapters/<name>.sh <corpus> <question>
<limit>`, which prints `file:Lstart-Lend` lines. Each tool was installed in its own
folder with default settings and scored on the four sets above. Summary tables are in
`eval/tools/results/` (one file per tool and set); the headline numbers are in the top
level README. trace-mcp's Java index came out empty, so its Go and Java row is not valid.
Serena has no natural language search; its adapter is a word by word symbol lookup.

## Agents with and without Knot or Trail, 2026-10-29

`eval/agent/` holds the pilot that ended the project. `prepare.py` picks 5 ARB failure
traces (seed 7), rebuilds each repo at its base commit with a `repro.sh` that replays the
failure, and makes three copies: plain, Knot wired into the repo only, Trail wired into
the repo only (`setup_trail.sh`). `run_one.sh` runs one headless Claude Code session per
copy with user settings and plugins switched off and only read and search commands
allowed. `score.py` scores success (the final `ROOT CAUSE:` line names a gold file), tool
calls, tokens, cost and time. Results for Opus 5.5 and Sonnet 5.5 are in
`eval/agent/results/`. The scripts keep the absolute paths of the machine they ran on.
The Opus sessions ran with `--permission-mode bypassPermissions`; the Sonnet sessions ran
with an allow list that still included find, sed and rg. The committed `run_one.sh` and
`prepare.py` are tightened after a security review (no bypass, no commands that can run
other programs, the failure text kept out of the shell script, upstream `.claude` folders
skipped), so a rerun will not match those conditions exactly.

Across 20 sessions with a tool installed, agents called Knot 0 times and Trail twice,
and solve rates and costs did not improve.
