#!/bin/sh
# Usage: run_one.sh <task id> <A|B|C> [model]  One headless Claude Code session, isolated
# from user settings. A is plain, B has Knot, C has Trail. With a model, output files
# get the model as a suffix so runs on different models sit side by side.
# No permission bypass: the agent may only run the replay and read or search the repo,
# plus Knot's and Trail's own commands and MCP tools, the same list in every setup.
# find, sed and rg are left out on purpose: find -exec, sed's e command and rg --pre all
# run arbitrary programs; Read, Grep and Glob cover the same ground safely. The published
# results were produced before this list was narrowed, with those three allowed.
set -u
W=/Volumes/TOTUL/Codes/arb/agent/work
mcp=$W/mcp-none.json; [ "$2" = B ] && mcp=$W/mcp-knot.json; [ "$2" = C ] && mcp=$W/mcp-trail.json
model=${3:-}
out="$W/$1/$2${model:+-$model}"
cd "$W/$1/$2" || exit 1
claude -p "Run \`sh repro.sh\` in this repository. It replays a test failure from CI. Do not try to fix anything and do not install dependencies. Find the source file that holds the root cause of the failure. End your answer with one final line of the form ROOT CAUSE: <path relative to the repo root>" \
  ${model:+--model "$model"} \
  --allowedTools "Bash(sh repro.sh)" "Bash(cat:*)" "Bash(grep:*)" "Bash(head:*)" \
    "Bash(tail:*)" "Bash(ls:*)" "Bash(wc:*)" "Bash(knot:*)" "Bash(trail:*)" \
    "Read" "Grep" "Glob" "mcp__knot" "mcp__graft" \
  --output-format stream-json --verbose --setting-sources project --strict-mcp-config --mcp-config "$mcp" \
  --no-session-persistence > "$out.jsonl" 2> "$out.err"
echo "done $1 $2 ${model:-default}" >> "$W/progress.log"
