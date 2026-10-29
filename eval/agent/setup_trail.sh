#!/bin/sh
# Builds setup C (Trail wired into each copied repo only) for the agent pilot.
# For every task: copy the plain A repo to C, put Trail's hooks (copied from your
# user settings) into C/.claude/settings.json, copy the repo .trail/hook.cjs and
# Trail's AGENTS.md block, write an MCP config for `graft mcp`, then `trail build`.
# It changes nothing outside /Volumes/TOTUL/Codes/arb/agent.
set -eu
A=/Volumes/TOTUL/Codes/arb/agent
W=$A/work
SRC=/Volumes/TOTUL/Codes/compare
PY=/Volumes/TOTUL/Codes/arb/venv/bin/python

"$PY" -I - <<'EOF'
import json, os
s = json.load(open(os.path.expanduser("~/.claude/settings.json")))
out = {}
for event, entries in s.get("hooks", {}).items():
  keep = [e for e in entries if any("graft" in h.get("command", "") or "trail" in h.get("command", "") for h in e.get("hooks", []))]
  if keep:
    out[event] = keep
out.setdefault("SessionStart", []).append({"hooks": [{"type": "command",
  "command": "node \"${CLAUDE_PROJECT_DIR:-.}/.trail/hook.cjs\" session-start || exit 0", "timeout": 10}]})
json.dump({"hooks": out}, open("/Volumes/TOTUL/Codes/arb/agent/trail-settings.json", "w"), indent=2)
print("[+] trail hooks by event:", {k: len(v) for k, v in out.items()})
EOF

echo '{"mcpServers":{"graft":{"command":"graft","args":["mcp"]}}}' > "$W/mcp-trail.json"

for plain in "$W"/*/A; do
  id=$(basename "$(dirname "$plain")")
  C="$W/$id/C"
  rm -rf "$C"
  cp -R "$plain" "$C"
  mkdir -p "$C/.claude" "$C/.trail"
  cp "$A/trail-settings.json" "$C/.claude/settings.json"
  cp "$SRC/.trail/hook.cjs" "$C/.trail/hook.cjs"
  if [ -f "$C/AGENTS.md" ]; then printf '\n' >> "$C/AGENTS.md"; fi
  cat "$SRC/AGENTS.md" >> "$C/AGENTS.md"
  (cd "$C" && trail build . 2>&1 | grep -E "wiring" | cut -c1-100)
  echo "[+] $id ready"
done

# Teach the session runner about setup C.
sed -i '' 's|mcp=$W/mcp-none.json; \[ "$2" = B \] \&\& mcp=$W/mcp-knot.json$|mcp=$W/mcp-none.json; [ "$2" = B ] \&\& mcp=$W/mcp-knot.json; [ "$2" = C ] \&\& mcp=$W/mcp-trail.json|' "$A/run_one.sh"
grep -n "mcp=" "$A/run_one.sh"
