#!/bin/sh
# Serena (github.com/oraios/serena) driven over MCP stdio. Serena is LSP based
# and has no natural language search, so lib/serena_mcp.py calls find_symbol
# (substring match) once per content word of the question and ranks symbols by
# how many words their name contains. One server, and one language server
# start, per question; that start dominates the latency.
# Tested with serena-agent 2.1.1.dev0 (ede329a), gopls 0.24.0 under $TOOL/bin.
set -eu
TOOL=/private/tmp/claude-501/tools/serena
export SERENA_HOME="$TOOL/home"
export PATH="$TOOL/bin:$PATH"  # gopls
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$1" && pwd -P)
exec "$TOOL/venv/bin/python" -I "$here/lib/serena_mcp.py" "$TOOL/venv/bin/serena" "$root" "$2" "$3"
