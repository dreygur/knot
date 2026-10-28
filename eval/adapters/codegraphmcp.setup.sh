#!/bin/sh
# Indexes <corpus root> into <corpus root>/.code-graph, then holds one server
# session open until every node is embedded (the model downloads on first use).
set -e
TOOL=/private/tmp/claude-501/tools/codegraphmcp
BINARY="$TOOL/node_modules/@sdsrs/code-graph-darwin-arm64/code-graph-mcp"
LIB="$(cd "$(dirname "$0")" && pwd)/lib"
mkdir -p "$TOOL/home"
export HOME="$TOOL/home" CODE_GRAPH_NO_AUTO_UPDATE=1 CODE_GRAPH_NO_GITIGNORE=1
cd "$1"
"$BINARY" incremental-index
node "$LIB/codegraphmcp_embed.mjs" "$PWD" "$BINARY"
