#!/bin/sh
# trace-mcp 3.42.0: the get_feature_context MCP tool, its natural language
# feature lookup (FTS over symbols, PageRank and graph neighbours), default
# config, which leaves AI and embeddings off.
set -e
TOOL=/private/tmp/claude-501/tools/tracemcp
LIB="$(cd "$(dirname "$0")" && pwd)/lib"
# The harness may pass the corpus root relative to its own working directory.
ROOT=$(cd "$1" && pwd -P)
# HOME keeps its registry and index databases inside the install.
export HOME="$TOOL/home" TRACE_MCP_TELEMETRY=off
db=$(node -e 'const r=require(process.argv[1]).projects[process.argv[2]];if(!r){console.error("not registered: "+process.argv[2]);process.exit(1)}console.log(r.dbPath)' \
  "$HOME/.trace/registry.json" "$ROOT")
args=$(node -e 'console.log(JSON.stringify({description: process.argv[1]}))' "$2")
raw=$(node "$LIB/mcp_client_a.mjs" "$ROOT" get_feature_context "$args" -- "$TOOL/node_modules/.bin/trace-mcp" serve)
# AppleDouble files (._name) on this volume are never answers.
printf '%s\n' "$raw" | node --disable-warning=ExperimentalWarning "$LIB/tracemcp_pointers.mjs" "$db" "$3" \
  | grep -Ev '(^|/)\._' | grep . || true
