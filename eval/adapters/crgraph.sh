#!/bin/sh
# code-review-graph (github.com/tirth8205/code-review-graph) through its
# `search` command, the CLI face of the semantic_search_nodes MCP tool. With the
# default install (no embeddings extra) that is SQLite FTS5 keyword search over
# graph nodes (functions, classes, files). Absolute paths are made relative.
# Tested with code-review-graph 2.3.9 (d72e8fb).
set -eu
TOOL=/private/tmp/claude-501/tools/crgraph
export CRG_HOME="$TOOL/home"
root=$(cd "$1" && pwd -P) query=$2 limit=$3
"$TOOL/venv/bin/code-review-graph" search "$query" --limit "$limit" --repo "$root" 2>/dev/null \
  | jq -r --arg root "$root/" '.results[]
      | (.file_path | ltrimstr($root)) as $f
      | if .line_start then "\($f):L\(.line_start)-L\(.line_end // .line_start)" else $f end'
