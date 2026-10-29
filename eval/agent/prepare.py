#!/usr/bin/env python
"""Builds the with and without Knot work folders for the agent pilot.

Usage: prepare.py <count> <seed>
Each task gets two copies of its ARB snapshot: A (plain) and B (Knot wired in the
repo only: hooks in .claude/settings.json, MCP through a config file, the CLAUDE.md
block, a warm index). Nothing is written to the user's own Claude Code config.
"""

import json
import random
import re
import shutil
import subprocess
import sys
from pathlib import Path

ARB = Path("/Volumes/TOTUL/Codes/arb")
SAMPLES = ARB / "src/data/benchmark/v2_trace2code/samples.jsonl"
SNAPSHOTS = ARB / "snap"
WORK = ARB / "agent/work"
KNOT = ARB / "bin/knot-agent"
SKILL = Path("/Volumes/TOTUL/Codes/compare/skills/knot/SKILL.md")
# .claude is skipped so an upstream repo's own settings and hooks never load in a session.
SKIP = {".knot", ".arb-complete", ".claude"}


def knot_block() -> str:
  """The CLAUDE.md block knot init writes: SKILL.md without frontmatter, one heading level down."""
  body = SKILL.read_text().split("\n---\n", 1)[1].strip()
  body = "\n".join("#" + l if l.startswith("#") else l for l in body.splitlines())
  return f"<!-- knot:start -->\n{body}\n<!-- knot:end -->\n"


def hooks() -> dict:
  bin_ = f'"{KNOT}"'
  return {"hooks": {
    "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": f"{bin_} hook pre-tool"}]}],
    "PostToolUse": [
      {"matcher": "Read|Edit|Write|MultiEdit", "hooks": [{"type": "command", "command": f"{bin_} hook post-tool"}]},
      {"hooks": [{"type": "command", "command": f"{bin_} hook usage"}]},
    ],
    "Stop": [{"hooks": [{"type": "command", "command": f"{bin_} hook stop"}]}],
    "SessionStart": [{"hooks": [{"type": "command", "command": f"{bin_} hook session-start"}]}],
  }}


def git(root: Path, *args: str) -> None:
  subprocess.run(["git", *args], cwd=root, check=True, capture_output=True)


def make_repo(snapshot: Path, dest: Path, excerpt: str) -> None:
  shutil.copytree(snapshot, dest, ignore=lambda d, names: [n for n in names if n in SKIP or n.startswith("._")])
  # The excerpt is data from the benchmark; it stays out of the shell script entirely.
  (dest / "repro.txt").write_text(excerpt + "\n")
  (dest / "repro.sh").write_text("#!/bin/sh\ncat repro.txt\nexit 1\n")
  git(dest, "add", "-A")
  git(dest, "-c", "user.name=pilot", "-c", "user.email=pilot@example.com", "commit", "-qm", "base")


def wire_knot(repo: Path) -> None:
  (repo / ".claude").mkdir()
  (repo / ".claude/settings.json").write_text(json.dumps(hooks(), indent=2))
  claude_md = repo / "CLAUDE.md"
  claude_md.write_text((claude_md.read_text() + "\n" if claude_md.exists() else "") + knot_block())
  subprocess.run([str(KNOT), "ask", "warm the index"], cwd=repo, capture_output=True, check=True)


def main() -> None:
  count, seed = int(sys.argv[1]), int(sys.argv[2])
  rows = [json.loads(l) for l in SAMPLES.read_text().splitlines()]
  usable = [r for r in rows if (SNAPSHOTS / r["repo"].replace("/", "__") / r["base_commit"] / ".arb-complete").exists()]
  random.Random(seed).shuffle(usable)
  picked, repos = [], set()
  for row in usable:
    if row["repo"] not in repos:
      picked.append(row)
      repos.add(row["repo"])
    if len(picked) == count:
      break

  WORK.mkdir(parents=True, exist_ok=True)
  (WORK / "mcp-knot.json").write_text(json.dumps({"mcpServers": {"knot": {"command": str(KNOT)}}}))
  (WORK / "mcp-none.json").write_text(json.dumps({"mcpServers": {}}))
  tasks = []
  for row in picked:
    snapshot = SNAPSHOTS / row["repo"].replace("/", "__") / row["base_commit"]
    excerpt = row["query"]["failure_excerpt"]
    for cond in ("A", "B"):
      dest = WORK / row["id"] / cond
      if dest.exists():
        shutil.rmtree(dest)
      make_repo(snapshot, dest, excerpt)
      if cond == "B":
        wire_knot(dest)
    tasks.append({"id": row["id"], "repo": row["repo"], "gold": row["gold"]["root_cause_files"]})
    print(f"[+] {row['id']} {row['repo']} gold={row['gold']['root_cause_files']}")
  (WORK / "tasks.json").write_text(json.dumps(tasks, indent=2))


if __name__ == "__main__":
  main()
