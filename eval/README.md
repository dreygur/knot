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
for c in knot-snapshot trail-code requests; do (cd reference/$c && trail build .); done
cargo build --release
```

## Run

```
cargo run --release --example eval                  # both engines, all corpora
cargo run --release --example eval -- --engine trail --corpus requests
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

Per corpus on the tuning set, knot leads on MRR everywhere but trails Trail on
hit@1 for Trail's own TypeScript code (39% vs 44%). Median latency, warm: knot
36 ms (Rust), 53 ms (Python), 190 ms (TS, 863 files); Trail 275 to 500 ms.

Tried and rejected on the tuning set (overall MRR, baseline 0.706): extra question
stop words with class bodies dropped (0.690), per field IDF (0.625), graph weight
0.15 / 0.40 / 0.55 (0.675 / 0.677 / 0.655), undirected graph walk (0.698).

With 7 held out questions per corpus, one question moves a corpus score by 14
points. Read per corpus held out rows as direction, not magnitude.
