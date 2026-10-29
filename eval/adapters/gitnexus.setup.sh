#!/bin/sh
# Indexes <corpus root> with embeddings. The index lives under the install, not
# the corpus; --index-only stops analyze from writing AGENTS.md, CLAUDE.md or skills.
set -e
TOOL=/private/tmp/claude-501/tools/gitnexus
mkdir -p "$TOOL/home" "$TOOL/indexes"
export HOME="$TOOL/home" GITNEXUS_STORAGE_ROOT="$TOOL/indexes" GITNEXUS_LBUG_EXTENSION_INSTALL=auto
cd "$1"
"$TOOL/node_modules/.bin/gitnexus" analyze --index-only --embeddings -- .
