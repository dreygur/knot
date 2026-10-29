#!/bin/sh
# Indexes one corpus with codebase-memory-mcp; the graph lives under CBM_CACHE_DIR.
set -eu
TOOL=/private/tmp/claude-501/tools/cbmemory
export CBM_CACHE_DIR="$TOOL/cache"
root=$(cd "$1" && pwd -P)
"$TOOL/bin/codebase-memory-mcp" cli --quiet index_repository --repo-path "$root" | jq -c '{project, status, nodes, edges}'
