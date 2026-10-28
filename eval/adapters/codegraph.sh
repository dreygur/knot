#!/bin/sh
# CodeGraph 1.6.2 (npm @colbymchenry/codegraph): the retrieval behind its
# codegraph_explore tool, ranked hits first. See lib/codegraph_search.cjs.
set -e
BUNDLE=/private/tmp/claude-501/tools/codegraph/node_modules/@colbymchenry/codegraph-darwin-arm64
LIB="$(cd "$(dirname "$0")" && pwd)/lib"
# The harness may pass the corpus root relative to its own working directory.
ROOT=$(cd "$1" && pwd -P)
export CODEGRAPH_TELEMETRY=0 DO_NOT_TRACK=1
raw=$("$BUNDLE/node" --liftoff-only --disable-warning=ExperimentalWarning \
  "$LIB/codegraph_search.cjs" "$BUNDLE/lib/dist/index.js" "$ROOT" "$2" "$3")
# AppleDouble files (._name) on this volume are never answers.
printf '%s\n' "$raw" | grep -Ev '(^|/)\._' | grep . || true
