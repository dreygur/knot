#!/usr/bin/env python
"""Score Knot on Agent Retrieval Bench with ARB's own loop and metrics.

Only the ranking function is swapped. Everything else (sample loading, the JSON
query text, the leakage filter, every metric) is ARB's code, so Knot is scored
exactly like the published BM25, lexical and RepoMap rows.

Usage: knot_rank.py <ranker: knot|bm25|lexical> <subset> <out dir>
Environment: ARB_HOME (required), KNOT_BIN (default: knot on PATH),
KNOT_LABEL (result file prefix, default: the ranker).
"""

import json
import os
import subprocess
import sys
from pathlib import Path

from agent_retrieval_bench import baseline as arb

ARB_HOME = Path(os.environ["ARB_HOME"])
DATA = ARB_HOME / "src" / "data"
SNAPSHOTS = ARB_HOME / "snap"
KNOT = os.environ.get("KNOT_BIN", "knot")
KNOT_HITS = "200"
KNOT_TIMEOUT_SECONDS = 600

original_ranker = arb.rank_chunks_for_ranker
original_validate = arb.validate_ranker


def snapshot_dir(chunks: list) -> Path:
  """Rebuilds the repo at its base commit from ARB's whole file chunks, once.

  Args:
    chunks: every chunk of one (repo, base commit) corpus.

  Returns:
    The folder holding the rebuilt files.
  """
  repo = chunks[0]["repo"].replace("/", "__")
  root = SNAPSHOTS / repo / chunks[0]["base_commit"]
  done = root / ".arb-complete"
  if done.exists():
    return root
  for chunk in chunks:
    if chunk.get("kind") != "file":
      continue
    path = root / chunk["path"]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(chunk["text"] + "\n", encoding="utf-8")
  subprocess.run(["git", "init", "-q"], cwd=root, check=True)
  done.write_text("")
  return root


def knot_hits(root: Path, query: str) -> list:
  """Runs `knot ask` in the rebuilt repo.

  Args:
    root: the rebuilt repo.
    query: ARB's query text.

  Returns:
    Knot's hits, best first; empty when Knot fails.
  """
  result = subprocess.run(
    [KNOT, "ask", query, "--json", "-n", KNOT_HITS],
    cwd=root, capture_output=True, text=True, timeout=KNOT_TIMEOUT_SECONDS,
  )
  if result.returncode != 0:
    print(f"[-] knot failed in {root}: {result.stderr.strip()[:300]}", file=sys.stderr)
    return []
  start = result.stdout.find("{")
  if start < 0:
    return []
  return json.loads(result.stdout[start:]).get("hits", [])


def overlap(a_start: int, a_end: int, b_start: int, b_end: int) -> int:
  return max(0, min(a_end, b_end) - max(a_start, b_start) + 1)


def rank_with_knot(query: str, chunks: list) -> list:
  """ARB's chunks in Knot's order.

  Each hit maps to the symbol chunk it overlaps most, else to its file's whole
  file chunk. Files Knot never names are left out, so they score as unranked.

  Args:
    query: ARB's query text.
    chunks: the corpus chunks for the sample's repo and commit.

  Returns:
    Ranked chunks, best first.
  """
  root = snapshot_dir(chunks)
  by_path = {}
  for chunk in chunks:
    by_path.setdefault(chunk["path"], []).append(chunk)

  ranked, seen = [], set()
  for hit in knot_hits(root, query):
    path, _, span = hit.get("pointer", "").rpartition(":L")
    candidates = by_path.get(path)
    if not candidates:
      continue
    start, _, end = span.partition("-L")
    start, end = int(start or 0), int(end or start or 0)
    symbols = [c for c in candidates if c.get("kind") != "file"]
    best = max(symbols, key=lambda c: overlap(start, end, c["start_line"], c["end_line"]), default=None)
    if best is None or overlap(start, end, best["start_line"], best["end_line"]) == 0:
      best = next((c for c in candidates if c.get("kind") == "file"), candidates[0])
    if best["chunk_id"] not in seen:
      seen.add(best["chunk_id"])
      ranked.append(best)
  return ranked


def rank_chunks_for_ranker(query: str, chunks: list, ranker: str) -> list:
  if ranker == "knot":
    return rank_with_knot(query, chunks)
  return original_ranker(query, chunks, ranker)


def validate_ranker(ranker: str) -> None:
  if ranker != "knot":
    original_validate(ranker)


def main() -> None:
  ranker, subset, out = sys.argv[1], sys.argv[2], Path(sys.argv[3])
  label = os.environ.get("KNOT_LABEL", ranker)
  arb.rank_chunks_for_ranker = rank_chunks_for_ranker
  arb.validate_ranker = validate_ranker
  out.mkdir(parents=True, exist_ok=True)
  result = arb.evaluate_corpus_baseline(
    [DATA / "benchmark" / subset / "samples.jsonl"],
    DATA / "corpus" / subset,
    out_path=out / f"{label}-{subset}.json",
    details_path=out / f"{label}-{subset}.jsonl",
    ranker=ranker,
  )
  print(json.dumps({"label": label, "subset": subset, "evaluated": result["evaluated"],
                    "skipped": result["skipped"]}, sort_keys=True))


if __name__ == "__main__":
  main()
