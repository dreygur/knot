#!/bin/sh
# Builds CodeGraph's index in <corpus root>/.codegraph. `init` only creates that
# directory; agent wiring is the separate `install` command, never run here.
set -e
CODEGRAPH=/private/tmp/claude-501/tools/codegraph/node_modules/.bin/codegraph
export CODEGRAPH_TELEMETRY=0 DO_NOT_TRACK=1
if [ -d "$1/.codegraph" ]; then
  "$CODEGRAPH" index --quiet "$1"
else
  "$CODEGRAPH" init "$1" </dev/null
fi
