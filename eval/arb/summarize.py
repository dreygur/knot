#!/usr/bin/env python
"""Sample weighted totals over ARB's four positive subsets, one row per label.

Usage: summarize.py <results dir> <label>...
"""

import glob
import json
import sys
from pathlib import Path

METRICS = ("MRR", "Recall@20", "Recall@5")


def summarize(results: Path, label: str) -> str:
  """One row: weighted MRR, Recall@20 and Recall@5, then MRR/Recall@20 per subset.

  Args:
    results: folder holding `<label>-<subset>.json` summaries.
    label: the run's result prefix.

  Returns:
    The row as text.
  """
  totals, samples, parts = {}, 0, []
  for path in sorted(glob.glob(str(results / f"{label}-v2_*.json"))):
    overall = json.loads(Path(path).read_text())["metrics"]["overall"]
    count = overall["samples"]
    samples += count
    subset = Path(path).stem.split("-v2_")[1]
    parts.append(f"{subset} {overall['MRR']:.3f}/{overall['Recall@20']:.3f}")
    for key in METRICS:
      totals[key] = totals.get(key, 0.0) + overall[key] * count
  if not samples:
    return f"{label}: no results"
  weighted = " ".join(f"{key} {totals[key] / samples:.4f}" for key in METRICS)
  return f"{label:12} n={samples} {weighted} | " + " | ".join(parts)


def main() -> None:
  results = Path(sys.argv[1])
  for label in sys.argv[2:]:
    print(summarize(results, label))


if __name__ == "__main__":
  main()
