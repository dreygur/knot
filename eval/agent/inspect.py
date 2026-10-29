#!/usr/bin/env python
"""Reads one pilot session's stream-json: startup, tool calls, Knot's hook output, answer, cost.

Usage: inspect.py <session .jsonl>
"""

import json
import sys

events = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
init = next((e for e in events if e.get("type") == "system" and e.get("subtype") == "init"), {})
print("model:", init.get("model"))
print("tools:", init.get("tools"))
print("mcp:", init.get("mcp_servers"))
print("plugins:", init.get("plugins"), "skills:", len(init.get("skills") or []), "agents:", init.get("agents"))

for e in events:
  if e.get("type") == "system" and e.get("subtype", "").startswith("hook"):
    out = json.dumps(e)
    if "knot" in out.lower() and ("likeliest" in out or "additionalContext" in out):
      print("HOOK:", out[:400])
  if e.get("type") == "assistant":
    for block in e["message"].get("content", []):
      if block.get("type") == "tool_use":
        print("TOOL:", block["name"], json.dumps(block["input"])[:160])

result = next((e for e in events if e.get("type") == "result"), {})
print("turns:", result.get("num_turns"), "cost:", result.get("total_cost_usd"), "ms:", result.get("duration_ms"))
print("ANSWER TAIL:", (result.get("result") or "")[-300:])
