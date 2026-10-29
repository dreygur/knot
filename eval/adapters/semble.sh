#!/bin/sh
# Semble (github.com/MinishLab/semble) through its CLI, default hybrid search:
# BM25 plus static embeddings (potion code model) with its own rerank.
# The index is cached per repo under SEMBLE_CACHE_LOCATION; semble.setup.sh warms it.
# Tested with semble 0.7.0 (44b7975), venv at /private/tmp/claude-501/tools/semble.
set -eu
TOOL=/private/tmp/claude-501/tools/semble
export HF_HOME=/Volumes/claudetools/cache/hf
export SEMBLE_CACHE_LOCATION="$TOOL/cache"
root=$(cd "$1" && pwd -P) query=$2 limit=$3
"$TOOL/venv/bin/semble" search "$query" "$root" --top-k "$limit" --max-snippet-lines 0 --format json \
  | jq -r '.results[] | "\(.file_path):L\(.start_line)-L\(.end_line)"'
