#!/usr/bin/env python
"""Scores the agent pilot: one row per session, then A against B.

Success means the answer's final `ROOT CAUSE:` line names a gold file. Tool calls,
turns, cost and time come from the session's stream. For B it also records the
files Knot's failure hook offered and whether the agent opened one of them.

Usage: score.py [model suffix]
"""

import json
import subprocess
import sys
from pathlib import Path

WORK = Path("/Volumes/TOTUL/Codes/arb/agent/work")
KNOT = "/Volumes/TOTUL/Codes/arb/bin/knot-agent"


def hint(repo: Path) -> list:
  output = subprocess.run(["sh", "repro.sh"], cwd=repo, capture_output=True, text=True).stdout
  payload = json.dumps({"session_id": "score", "cwd": str(repo), "tool_name": "Bash",
                        "tool_response": {"stdout": output, "stderr": ""}})
  said = subprocess.run([KNOT, "hook", "usage"], input=payload, capture_output=True, text=True).stdout
  if "best first:" not in said:
    return []
  files = json.loads(said)["hookSpecificOutput"]["additionalContext"].split("best first: ")[1].split(". Leads")[0]
  return [f.strip() for f in files.split(",")]


def session(path: Path, gold: list) -> dict:
  events = [json.loads(l) for l in path.read_text().splitlines() if l.strip()]
  tools, touched = [], []
  for e in events:
    if e.get("type") != "assistant":
      continue
    for block in e["message"].get("content", []):
      if block.get("type") == "tool_use":
        tools.append(block["name"])
        touched.append(json.dumps(block["input"]))
  result = next((e for e in events if e.get("type") == "result"), {})
  answer = (result.get("result") or "").strip().splitlines()
  final = next((l for l in reversed(answer) if "ROOT CAUSE:" in l), "")
  named = final.split("ROOT CAUSE:", 1)[1].strip().strip("`* ") if final else ""
  usage = result.get("usage") or {}
  return {
    "ok": any(named == g or named.endswith("/" + g) or g.endswith("/" + named) for g in gold) if named else False,
    "named": named, "tools": len(tools), "turns": result.get("num_turns"),
    "cost": result.get("total_cost_usd") or 0.0, "seconds": (result.get("duration_ms") or 0) / 1000,
    "tokens": sum(usage.get(k, 0) for k in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens")),
    "touched": " ".join(touched),
    "names": tools,
    "denied": len(result.get("permission_denials") or []),
  }


def main() -> None:
  tasks = json.loads((WORK / "tasks.json").read_text())
  totals = {"A": [], "B": [], "C": []}
  print("task | cond | ok | named | tools | turns | tokens | cost | s | hint top1 ok | opened a hint file")
  for task in tasks:
    gold = task["gold"]
    for cond in ("A", "B", "C"):
      suffix = f"-{sys.argv[1]}" if len(sys.argv) > 1 else ""
      path = WORK / task["id"] / f"{cond}{suffix}.jsonl"
      if not path.exists():
        continue
      row = session(path, gold)
      extra = ""
      if cond == "B":
        offered = hint(WORK / task["id"] / "B")
        top_ok = bool(offered) and offered[0] in gold
        opened = any(f in row["touched"] for f in offered)
        extra = f"| {top_ok if offered else 'none'} | {opened if offered else '-'}"
      if cond == "C":
        uses = row["touched"].count("trail ") + sum(n.startswith(("mcp__graft", "mcp__trail")) for n in row["names"])
        extra = f"| trail commands or tool calls: {uses}"
      totals[cond].append(row)
      print(f"{task['repo']} | {cond} | {row['ok']} | {row['named']} | {row['tools']} | {row['turns']} | "
            f"{row['tokens']} | {row['cost']:.3f} | {row['seconds']:.0f} | denied {row['denied']} {extra}")
  for cond, rows in totals.items():
    if rows:
      n = len(rows)
      print(f"{cond}: solved {sum(r['ok'] for r in rows)}/{n}, mean tools {sum(r['tools'] for r in rows)/n:.1f}, "
            f"mean tokens {sum(r['tokens'] for r in rows)/n:.0f}, total cost ${sum(r['cost'] for r in rows):.2f}, "
            f"mean seconds {sum(r['seconds'] for r in rows)/n:.0f}")


if __name__ == "__main__":
  main()
