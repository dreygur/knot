#!/usr/bin/env bash
set -euo pipefail

REPO="dreygur/knot"
INSTALL_DIR="${KNOT_INSTALL_DIR:-$HOME/.local/bin}"
MCP_NAME="knot"

# One aligned line per step, the same shape `knot init` prints
done_line() { printf '✓ %-12s  %s\n' "$1" "$2"; }
note_line() { printf '· %-12s  %s\n' "$1" "$2"; }
fail_line() { printf '✗ %-12s  %s\n' "$1" "$2" >&2; }
tilde() { echo "${1/#$HOME/~}"; }

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
    fail_line "download" "curl or wget is required"
    exit 1
  fi
}

VERSION="${KNOT_VERSION:-$(latest_version)}"
if [[ -z "$VERSION" ]]; then
  fail_line "version" "could not read the latest release from github.com/$REPO"
  exit 1
fi

echo "knot $VERSION · installing to $(tilde "$INSTALL_DIR")"
echo ""

ARTIFACT="$(detect_target)"
mkdir -p "$INSTALL_DIR"
BIN_PATH="$INSTALL_DIR/knot"

if [[ -n "$ARTIFACT" ]]; then
  download "https://github.com/${REPO}/releases/download/${VERSION}/${ARTIFACT}" "$BIN_PATH"
  chmod +x "$BIN_PATH"
  done_line "binary" "$(tilde "$BIN_PATH")"
else
  if ! command -v cargo &>/dev/null; then
    fail_line "binary" "no release build for $(uname -s) $(uname -m), and cargo is missing: https://rustup.rs"
    exit 1
  fi
  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  note_line "binary" "no release build for $(uname -s) $(uname -m); building from source"
  (cd "$SCRIPT_DIR" && cargo build --release --quiet)
  cp "$SCRIPT_DIR/target/release/knot" "$BIN_PATH"
  chmod +x "$BIN_PATH"
  done_line "binary" "$(tilde "$BIN_PATH") (built from source)"
fi

BIN_PATH="$(resolve_abs "$BIN_PATH")"

# Claude Code. Knot adds its own hooks and status line to ~/.claude/settings.json the
# first time Claude Code starts it, so nothing else is needed here.
if command -v claude &>/dev/null; then
  if claude mcp get "$MCP_NAME" &>/dev/null; then
    note_line "claude" "MCP server '$MCP_NAME' already registered"
  elif claude mcp add -s user "$MCP_NAME" -- "$BIN_PATH" &>/dev/null; then
    done_line "claude" "MCP server added (user scope)"
  else
    fail_line "claude" "registration failed; run: claude mcp add -s user $MCP_NAME -- '$BIN_PATH'"
  fi
fi

# OpenAI Codex CLI
if command -v codex &>/dev/null; then
  if codex mcp add "$MCP_NAME" -- "$BIN_PATH" &>/dev/null; then
    done_line "codex" "MCP server added"
  else
    fail_line "codex" "registration failed; add [mcp_servers.$MCP_NAME] command = \"$BIN_PATH\" to ~/.codex/config.toml"
  fi
fi

cat <<EOF
· other agents  any stdio MCP client works; point it at the binary:
                  Gemini CLI  ~/.gemini/settings.json              "mcpServers": { "knot": { "command": "$BIN_PATH" } }
                  OpenCode    opencode.json                        "mcp": { "knot": { "type": "local", "command": ["$BIN_PATH"], "enabled": true } }
                  Cursor      ~/.cursor/mcp.json                   "mcpServers": { "knot": { "command": "$BIN_PATH" } }
                  Windsurf    ~/.codeium/windsurf/mcp_config.json  "mcpServers": { "knot": { "command": "$BIN_PATH" } }
                  VS Code     .vscode/mcp.json                     "servers": { "knot": { "type": "stdio", "command": "$BIN_PATH" } }
· next          run knot init in each repo: pick your agents, then it indexes the code into .knot/cache/
EOF
