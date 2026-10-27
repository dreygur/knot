#!/usr/bin/env bash
set -euo pipefail

REPO="dreygur/knot"
DATA_DIR="${KNOT_DATA_DIR:-$HOME/.knot}"
INSTALL_DIR="${KNOT_INSTALL_DIR:-$HOME/.local/bin}"
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

# Detect OS and arch, map to release artifact name
detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux)
      case "$arch" in
        x86_64) echo "knot-x86_64-unknown-linux-gnu" ;;
        *) echo "" ;;
      esac
      ;;
    Darwin)
      case "$arch" in
        x86_64)  echo "knot-x86_64-apple-darwin" ;;
        arm64)   echo "knot-aarch64-apple-darwin" ;;
        *) echo "" ;;
      esac
      ;;
    *) echo "" ;;
  esac
}

# Fetch latest release version from GitHub API
latest_version() {
  if command -v curl &>/dev/null; then
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
      | grep '"tag_name"' | cut -d'"' -f4
  elif command -v wget &>/dev/null; then
    wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" \
      | grep '"tag_name"' | cut -d'"' -f4
  else
    echo ""
  fi
}

# Download file using curl or wget
download() {
  local url="$1" dest="$2"
  if command -v curl &>/dev/null; then
    curl -fsSL "$url" -o "$dest"
  elif command -v wget &>/dev/null; then
    wget -qO "$dest" "$url"
  else
    echo "[KNOT] ERROR: curl or wget required" >&2
    exit 1
  fi
}

VERSION="${KNOT_VERSION:-$(latest_version)}"
if [[ -z "$VERSION" ]]; then
  echo "[KNOT] ERROR: Could not determine latest release version." >&2
  exit 1
fi

echo "Knot installer"
echo ""
echo "[KNOT] INFO:  Version  : $VERSION"
echo "[KNOT] INFO:  Data dir : $DATA_DIR"
echo "[KNOT] INFO:  Bin dir  : $INSTALL_DIR"
echo ""
echo "[KNOT] INFO:  This script will:"
echo "[KNOT] INFO:    1. Download the knot binary to $INSTALL_DIR"
echo "[KNOT] INFO:    2. Create a data directory at $DATA_DIR"
echo "[KNOT] INFO:    3. Register knot as an MCP server with the agent CLIs it finds"
echo "[KNOT] INFO:       (Claude Code, Codex) and print the config for every other client"
echo ""

ARTIFACT="$(detect_target)"
mkdir -p "$INSTALL_DIR"
BIN_PATH="$INSTALL_DIR/knot"

if [[ -n "$ARTIFACT" ]]; then
  URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARTIFACT}"
  echo "[KNOT] PERM:  Download binary from GitHub releases -> $BIN_PATH"
  download "$URL" "$BIN_PATH"
  chmod +x "$BIN_PATH"
  echo "[KNOT] INFO:  Installed to $BIN_PATH"
else
  echo "[KNOT] WARN:  No pre-built binary for this platform. Building from source..."
  if ! command -v cargo &>/dev/null; then
    echo "[KNOT] ERROR: cargo not found. Install Rust from https://rustup.rs" >&2
    exit 1
  fi
  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  echo "[KNOT] PERM:  Run cargo build --release in $SCRIPT_DIR"
  (cd "$SCRIPT_DIR" && cargo build --release)
  cp "$SCRIPT_DIR/target/release/knot" "$BIN_PATH"
  chmod +x "$BIN_PATH"
  echo "[KNOT] INFO:  Built and installed to $BIN_PATH"
fi

BIN_PATH="$(resolve_abs "$BIN_PATH")"
echo "[KNOT] PERM:  Create data directory $DATA_DIR"
mkdir -p "$DATA_DIR"
echo ""

REGISTERED=()

# Claude Code. Knot adds its own hooks and status line to ~/.claude/settings.json the
# first time Claude Code starts it, so nothing else is needed here.
if command -v claude &>/dev/null; then
  if claude mcp get "$MCP_NAME" &>/dev/null; then
    echo "[KNOT] INFO:  Claude Code already has an MCP server named '$MCP_NAME'"
  elif claude mcp add -s user "$MCP_NAME" -- "$BIN_PATH"; then
    REGISTERED+=("Claude Code")
  else
    echo "[KNOT] WARN:  Claude Code registration failed; run: claude mcp add -s user $MCP_NAME -- '$BIN_PATH'"
  fi
fi

# OpenAI Codex CLI
if command -v codex &>/dev/null; then
  if codex mcp add "$MCP_NAME" -- "$BIN_PATH"; then
    REGISTERED+=("Codex")
  else
    echo "[KNOT] WARN:  Codex registration failed; add to ~/.codex/config.toml:"
    echo "               [mcp_servers.$MCP_NAME]"
    echo "               command = \"$BIN_PATH\""
  fi
fi

echo ""
if [[ ${#REGISTERED[@]} -gt 0 ]]; then
  echo "[KNOT] INFO:  Registered with: ${REGISTERED[*]}"
fi
cat <<EOF

Other agents: Knot is a stdio MCP server; point any client at the binary.

  Gemini CLI    ~/.gemini/settings.json   "mcpServers": { "knot": { "command": "$BIN_PATH" } }
  OpenCode      opencode.json             "mcp": { "knot": { "type": "local", "command": ["$BIN_PATH"], "enabled": true } }
  Cursor        ~/.cursor/mcp.json        "mcpServers": { "knot": { "command": "$BIN_PATH" } }
  Windsurf      ~/.codeium/windsurf/mcp_config.json   "mcpServers": { "knot": { "command": "$BIN_PATH" } }
  VS Code       .vscode/mcp.json          "servers": { "knot": { "type": "stdio", "command": "$BIN_PATH" } }

Next, in each repository:
  knot init        add knot to PATH, write agent instructions (AGENTS.md, CLAUDE.md,
                   GEMINI.md, Cursor rule), and index the code

[KNOT] INFO:  Binary    : $BIN_PATH
[KNOT] INFO:  Data dir  : $DATA_DIR   (override with KNOT_DATA_DIR)
[KNOT] INFO:  Read-only : export KNOT_READ_ONLY=1
EOF
