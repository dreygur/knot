#!/bin/sh
# Builds and caches Semble's index for one corpus; first run downloads the embedding model.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
"$here/semble.sh" "$1" "warm up" 1 >/dev/null
