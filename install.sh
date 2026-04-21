#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA_DIR="${KNOT_DATA_DIR:-$HOME/.knot}"
MCP_NAME="knot"

# Portable absolute path resolution (realpath not available on older macOS)
resolve_abs() {
  local path="$1"
  if command -v realpath &>/dev/null; then
    realpath "$path"
  else
    cd "$(dirname "$path")" && echo "$PWD/$(basename "$path")"
  fi
}

VERSION_FILE="${SCRIPT_DIR}/plugin.json"
if [[ -f "$VERSION_FILE" ]]; then
  VER=$(grep -o '"version": *"[^"]*"' "$VERSION_FILE" | cut -d'"' -f4)
else
  VER="unknown"
fi

echo "╔═══════════════════════════════════════════╗"
echo "║      Knot MCP Plugin Installer  v${VER}   ║"
echo "╚═══════════════════════════════════════════╝"
echo ""

BIN_PATH="${SCRIPT_DIR}/target/release/knot"

if [[ ! -f "$BIN_PATH" ]]; then
  echo "[KNOT] INFO:  Building Knot..."
  if ! command -v cargo &>/dev/null; then
    echo "[KNOT] ERROR: cargo not found. Install Rust from https://rustup.rs" >&2
    exit 1
  fi
  (cd "$SCRIPT_DIR" && cargo build --release)
fi

BIN_PATH="$(resolve_abs "$BIN_PATH")"
mkdir -p "$DATA_DIR"

install_opencode() {
  echo "[KNOT] INFO:  Registering with OpenCode..."
  local bin_abs
  bin_abs="$(resolve_abs "$(command -v opencode)")"
  echo "[KNOT] INFO:  opencode binary: $bin_abs"
  opencode mcp add --name "$MCP_NAME" --command "$BIN_PATH" \
    -e "KNOT_DATA_DIR=$DATA_DIR" \
    -e "KNOT_LOG=knot=info"
  echo "[KNOT] INFO:  Done. Restart OpenCode, then: opencode mcp list"
}

install_claude() {
  echo "[KNOT] INFO:  Registering with Claude Code..."
  local bin_abs
  bin_abs="$(resolve_abs "$(command -v claude)")"
  echo "[KNOT] INFO:  claude binary: $bin_abs"
  claude mcp add --name "$MCP_NAME" --command "$BIN_PATH" \
    --scope user \
    -e "KNOT_DATA_DIR=$DATA_DIR" \
    -e "KNOT_LOG=knot=info"
  echo "[KNOT] INFO:  Done. Restart Claude Code, then: claude mcp list"
}

inject_rules() {
  local agent="$1"
  local rules_file=""

  case "$agent" in
    opencode) rules_file="$HOME/AGENTS.md" ;;
    claude)   rules_file="$HOME/.clauderules" ;;
    *)        return ;;
  esac

  local marker="# Knot Protocol"

  # Create the file if it doesn't exist yet.
  if [[ ! -f "$rules_file" ]]; then
    touch "$rules_file"
    echo "[KNOT] INFO:  Created $rules_file"
  fi

  if grep -qF "$marker" "$rules_file" 2>/dev/null; then
    echo "[KNOT] INFO:  Knot Protocol already present in $rules_file"
    return
  fi

  cat >> "$rules_file" <<'EOF'

# Knot Protocol
- Before starting any task, call recall_memory to check for prior context.
- After completing multi-step tasks (exit 0), call save_skill to formalize the procedure.
- Use recall_skills to find reusable skills before manual execution.
- Use commit_session to promote session learnings to project scope.
EOF
  echo "[KNOT] INFO:  Injected Knot Protocol into $rules_file"
}

REGISTERED=0

if command -v opencode &>/dev/null; then
  install_opencode
  inject_rules "opencode"
  REGISTERED=1
fi

if command -v claude &>/dev/null; then
  install_claude
  inject_rules "claude"
  REGISTERED=1
fi

if [[ $REGISTERED -eq 0 ]]; then
  echo "[KNOT] WARN:  No MCP client detected (opencode / claude)."
  echo ""
  echo "Manual installation:"
  echo ""
  echo "  OpenCode:"
  echo "    opencode mcp add --name $MCP_NAME --command '$BIN_PATH' \\"
  echo "      -e 'KNOT_DATA_DIR=$DATA_DIR'"
  echo ""
  echo "  Claude Code:"
  echo "    claude mcp add --name $MCP_NAME --command '$BIN_PATH' \\"
  echo "      --scope user -e 'KNOT_DATA_DIR=$DATA_DIR'"
fi

echo ""
echo "[KNOT] INFO:  Binary    : $BIN_PATH"
echo "[KNOT] INFO:  Data dir  : $DATA_DIR"
echo "[KNOT] INFO:  Configure : export KNOT_DATA_DIR='<path>'"
echo "[KNOT] INFO:  Read-only : export KNOT_READ_ONLY=1"
