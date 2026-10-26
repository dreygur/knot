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

## Baseline, 2026-10-27

Trail 0.26.0, before `knot ask` exists.

| engine | corpus | hit@1 | hit@5 | file@5 | MRR@10 | median ms |
|---|---|---|---|---|---|---|
| trail | knot (Rust) | 60% | 70% | 90% | 0.643 | 281 |
| trail | requests (Python) | 47% | 80% | 100% | 0.622 | 275 |
| trail | trail-code (TS) | 44% | 50% | 67% | 0.479 | 504 |
