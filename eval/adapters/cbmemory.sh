#!/bin/sh
# codebase-memory-mcp (github.com/DeusData/codebase-memory-mcp) through its CLI
# mode, `cli search_graph` with the free text `query` argument: BM25 (SQLite
# FTS5, camelCase and snake_case aware) over graph symbols. Its other search,
# semantic_query, takes a keyword array rather than a question, so it is not used.
# Tested with codebase-memory-mcp 0.12.1 (dca2a44), built from source with scripts/build.sh.
set -eu
TOOL=/private/tmp/claude-501/tools/cbmemory
export CBM_CACHE_DIR="$TOOL/cache"
cbm="$TOOL/bin/codebase-memory-mcp"
root=$(cd "$1" && pwd -P) query=$2 limit=$3
project=$("$cbm" cli --quiet list_projects --format json --limit 500 \
  | jq -r --arg root "$root" '.projects[] | select(.root_path == $root) | .name' | head -n 1)
if [ -z "$project" ]; then
  echo "cbmemory: $root is not indexed; run cbmemory.setup.sh" >&2
  exit 1
fi
"$cbm" cli --quiet search_graph --project "$project" --query "$query" --limit "$limit" --format json \
  | jq -r '.cols as $c | .rows[] | [$c, .] | transpose | map({(.[0]): .[1]}) | add
      | if .lines then "\(.file):L\(.lines | sub("-"; "-L"))" else .file end'
