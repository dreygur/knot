#!/bin/sh
# code-graph-mcp 0.179.2 (npm @sdsrs/code-graph): the semantic_code_search MCP
# tool, hybrid vector plus FTS with RRF. `compact` only drops code bodies from
# the reply so every row keeps its lines; ranking is the same.
set -e
TOOL=/private/tmp/claude-501/tools/codegraphmcp
BINARY="$TOOL/node_modules/@sdsrs/code-graph-darwin-arm64/code-graph-mcp"
LIB="$(cd "$(dirname "$0")" && pwd)/lib"
# The harness may pass the corpus root relative to its own working directory.
ROOT=$(cd "$1" && pwd -P)
# HOME keeps the embedding model cache inside the install.
export HOME="$TOOL/home" CODE_GRAPH_NO_AUTO_UPDATE=1 CODE_GRAPH_NO_GITIGNORE=1 CODE_GRAPH_DISABLE_MODEL_DOWNLOAD=1
args=$(node -e 'console.log(JSON.stringify({query: process.argv[1], top_k: Number(process.argv[2]), compact: true}))' "$2" "$3")
raw=$(node "$LIB/mcp_client_a.mjs" "$ROOT" semantic_code_search "$args" -- "$BINARY" serve)
# AppleDouble files (._name) on this volume are never answers.
printf '%s\n' "$raw" | node "$LIB/codegraphmcp_pointers.mjs" "$3" | grep -Ev '(^|/)\._' | grep . || true
