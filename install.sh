#!/usr/bin/env bash
set -euo pipefail

KNOT_DATA_DIR="${KNOT_DATA_DIR:-$HOME/.knot}"
MCP_NAME="knot"

echo "Building Knot..."
if ! command -v cargo &>/dev/null; then
  echo "Error: cargo not found. Install Rust from https://rustup.rs" >&2
  exit 1
fi

cargo build --release

BIN="$(pwd)/target/release/knot"

if ! command -v claude &>/dev/null; then
  echo "Warning: 'claude' CLI not found — skipping MCP registration."
  echo "Add Knot manually to ~/.claude/settings.json:"
  echo ""
  echo '  "mcpServers": {'
  echo "    \"$MCP_NAME\": {"
  echo "      \"command\": \"$BIN\","
  echo "      \"env\": { \"KNOT_DATA_DIR\": \"$KNOT_DATA_DIR\" }"
  echo "    }"
  echo "  }"
  exit 0
fi

mkdir -p "$KNOT_DATA_DIR"

echo "Registering with Claude Code..."
claude mcp add "$MCP_NAME" "$BIN" \
  --scope user \
  -e "KNOT_DATA_DIR=$KNOT_DATA_DIR"

echo ""
echo "Done. Restart Claude Code, then verify with: claude mcp list"
