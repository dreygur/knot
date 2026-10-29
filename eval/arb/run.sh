#!/bin/sh
# Runs ARB's four positive subsets through knot_rank.py for one ranker.
# Usage: ARB_HOME=<arb folder> [KNOT_BIN=...] [KNOT_LABEL=...] run.sh <knot|bm25|lexical>
set -eu
here=$(cd "$(dirname "$0")" && pwd)
results="$ARB_HOME/results"
label=${KNOT_LABEL:-$1}
mkdir -p "$results"
cd "$ARB_HOME/src"
for subset in v2_edit2ripple v2_comment2context v2_trace2code v2_code2test; do
  "$ARB_HOME/venv/bin/python" -I "$here/knot_rank.py" "$1" "$subset" "$results" \
    > "$results/$label-$subset.log" 2> "$results/$label-$subset.err"
  echo "done $label $subset" >> "$results/progress.log"
done
"$ARB_HOME/venv/bin/python" -I "$here/summarize.py" "$results" "$label"
