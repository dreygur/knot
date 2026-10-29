#!/bin/sh
# Registers and indexes <corpus root>. The index database lives under the install;
# trace-mcp only drops a small .trace-mcp/ marker directory in the corpus.
# `trace-mcp init` (agent configs, hooks, CLAUDE.md) is never run.
set -e
TOOL=/private/tmp/claude-501/tools/tracemcp
mkdir -p "$TOOL/home"
export HOME="$TOOL/home" TRACE_MCP_TELEMETRY=off
cd "$1"
"$TOOL/node_modules/.bin/trace-mcp" add --json --force . >/dev/null
"$TOOL/node_modules/.bin/trace-mcp" index . >/dev/null
