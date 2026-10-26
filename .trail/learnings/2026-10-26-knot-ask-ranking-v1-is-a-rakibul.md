---
title: knot ask ranking v1 is a local optimum; bump CACHE_VERSION on any tokenizer change
author: Rakibul
date: 2026-10-26
branch: audit-fixes
cost: { minutes: 76, tokens: 306013 }
id: l-de405490
scope: repo
status: active
src: agent
touches:
  - eval/README.md
  - examples/eval.rs
  - src/ask/cache.rs
  - src/ask/index.rs
  - src/ask/mod.rs
  - src/ast/resolve.rs
  - src/cli/mod.rs
  - src/tools/mod.rs
  - eval/holdout.json
  - src/ask/tokenize.rs
---

## Decided
src/ask keeps BM25F (field weights 3/1.5/2/1/0.5 for name/signature/doc/path/body) plus a
Personalized PageRank rerank at GRAPH_WEIGHT 0.25 over directed call edges. Tuning set MRR 0.706,
held out 0.644, against Trail 0.581 / 0.405 (eval/README.md). Any ranking change must beat 0.706
overall by more than 0.02 without lowering hit@5, and be confirmed on eval/holdout.json.

## Tried and ruled out
Extra question stop words plus empty class bodies (0.690), per field IDF summed BM25 (0.625),
graph weight 0.15 / 0.40 / 0.55 (0.675 / 0.677 / 0.655), undirected graph walk (0.698).
Single question rank shifts on 15 to 20 questions are noise; do not chase them.

## Watch out
src/ask/cache.rs stores tokenized docs. Changing tokenize.rs, build_doc or field layout without
bumping CACHE_VERSION silently reuses old tokens; one experiment here was mis-scored that way.
Never tune on eval/holdout.json, or it stops measuring overfitting.
