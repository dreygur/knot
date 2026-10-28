#!/bin/sh
# Initializes and indexes one corpus for cocoindex-code. `ccc init` appends
# /.cocoindex_code/ to the corpus .gitignore; when that file was clean the edit
# is undone and the entry goes to .git/info/exclude instead.
set -eu
TOOL=/private/tmp/claude-501/tools/cocoindex
export COCOINDEX_CODE_DIR="$TOOL/home/.cocoindex_code"
export HF_HOME=/Volumes/claudetools/cache/hf
export COCOINDEX_DISABLE_USAGE_TRACKING=1
root=$(cd "$1" && pwd -P)
cd "$root"
clean_ignore=no
if [ -f .gitignore ] && git diff --quiet -- .gitignore 2>/dev/null; then clean_ignore=yes; fi
"$TOOL/venv/bin/ccc" init -f </dev/null
if [ -d .git ]; then
  if [ "$clean_ignore" = yes ]; then git checkout -q -- .gitignore; fi
  grep -qx '/.cocoindex_code/' .git/info/exclude 2>/dev/null || echo '/.cocoindex_code/' >> .git/info/exclude
fi
"$TOOL/venv/bin/ccc" index
