#!/bin/sh
# Creates Serena's project config in <corpus>/.serena/ (language inferred) and
# fills its LSP symbol cache. The only config change is ignoring macOS "._*"
# AppleDouble files, which the corpus volume creates and which crash Serena's
# UTF-8 file reader; it does not change ranking.
set -eu
TOOL=/private/tmp/claude-501/tools/serena
export SERENA_HOME="$TOOL/home"
export PATH="$TOOL/bin:$PATH"  # gopls
root=$(cd "$1" && pwd -P)
if [ ! -f "$root/.serena/project.yml" ]; then
  # Answers no to every optional extra language server: main language only.
  yes n | "$TOOL/venv/bin/serena" project create "$root"
fi
sed -i '' 's|^ignored_paths: \[\]$|ignored_paths: ["._*"]|' "$root/.serena/project.yml"
"$TOOL/venv/bin/serena" project index "$root" --log-level WARNING
