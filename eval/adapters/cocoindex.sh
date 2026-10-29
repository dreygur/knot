#!/bin/sh
# cocoindex-code (github.com/cocoindex-io/cocoindex-code) through `ccc search`,
# its semantic search over AST chunks, with the default local embedding model
# (sentence-transformers, Snowflake/snowflake-arctic-embed-xs). No API key.
# A background daemon holds the model; the first query after it starts is slow.
# Tested with cocoindex-code b3834e6 installed with the [full] extra.
set -eu
TOOL=/private/tmp/claude-501/tools/cocoindex
export COCOINDEX_CODE_DIR="$TOOL/home/.cocoindex_code"
export HF_HOME=/Volumes/claudetools/cache/hf
export COCOINDEX_DISABLE_USAGE_TRACKING=1
root=$(cd "$1" && pwd -P) query=$2 limit=$3
cd "$root"
"$TOOL/venv/bin/ccc" search --json --limit "$limit" -- "$query" \
  | jq -r '.results[] | "\(.file_path):L\(.start_line)-L\(.end_line)"'
