#!/bin/sh
# Builds code-review-graph's graph for one corpus into <corpus>/.code-review-graph/.
set -eu
TOOL=/private/tmp/claude-501/tools/crgraph
export CRG_HOME="$TOOL/home"
root=$(cd "$1" && pwd -P)
"$TOOL/venv/bin/code-review-graph" build --repo "$root"
