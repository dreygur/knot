#!/bin/sh
# GitNexus 1.6.12: `gitnexus query`, its process grouped hybrid search (BM25 plus
# local embeddings, fused with RRF), with default limits.
set -e
TOOL=/private/tmp/claude-501/tools/gitnexus
LIB="$(cd "$(dirname "$0")" && pwd)/lib"
# The harness may pass the corpus root relative to its own working directory.
ROOT=$(cd "$1" && pwd -P)
# HOME keeps its registry, LadybugDB extensions and model cache inside the install.
export HOME="$TOOL/home" GITNEXUS_STORAGE_ROOT="$TOOL/indexes"
cd "$ROOT"
raw=$("$TOOL/node_modules/.bin/gitnexus" query "$2" 2>/dev/null)
# AppleDouble files (._name) on this volume are never answers.
printf '%s\n' "$raw" | node "$LIB/gitnexus_pointers.mjs" "$3" | grep -Ev '(^|/)\._' | grep . || true
