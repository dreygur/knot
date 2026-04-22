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

echo "╔═══════════════════════════════════════════╗"
echo "║      Knot MCP Plugin Installer            ║"
echo "╚═══════════════════════════════════════════╝"
echo ""
echo "[KNOT] INFO:  Version  : $VERSION"
echo "[KNOT] INFO:  Data dir : $DATA_DIR"
echo "[KNOT] INFO:  Bin dir  : $INSTALL_DIR"
echo ""

ARTIFACT="$(detect_target)"
mkdir -p "$INSTALL_DIR"
BIN_PATH="$INSTALL_DIR/knot"

if [[ -n "$ARTIFACT" ]]; then
  URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARTIFACT}"
  echo "[KNOT] INFO:  Downloading $ARTIFACT..."
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
  (cd "$SCRIPT_DIR" && cargo build --release)
  cp "$SCRIPT_DIR/target/release/knot" "$BIN_PATH"
  chmod +x "$BIN_PATH"
  echo "[KNOT] INFO:  Built and installed to $BIN_PATH"
fi

BIN_PATH="$(resolve_abs "$BIN_PATH")"
mkdir -p "$DATA_DIR"

install_opencode() {
  echo "[KNOT] INFO:  Registering with OpenCode..."
  opencode mcp add --name "$MCP_NAME" --command "$BIN_PATH" \
    -e "KNOT_DATA_DIR=$DATA_DIR" \
    -e "KNOT_LOG=knot=info"
  echo "[KNOT] INFO:  Done. Restart OpenCode, then: opencode mcp list"
}

install_claude() {
  echo "[KNOT] INFO:  Registering with Claude Code..."
  claude mcp add --name "$MCP_NAME" --command "$BIN_PATH" \
    --scope user \
    -e "KNOT_DATA_DIR=$DATA_DIR" \
    -e "KNOT_LOG=knot=info"
  inject_claude_hooks
  echo "[KNOT] INFO:  Done. Restart Claude Code, then: claude mcp list"
}

inject_claude_hooks() {
  local hook_dir="$DATA_DIR/hooks"
  local pre_tool_script="$hook_dir/knot-pre-tool.sh"
  local stop_script="$hook_dir/knot-stop.sh"
  local claude_settings="$HOME/.claude/settings.json"
  local marker="knot-pre-tool"

  mkdir -p "$hook_dir"

  # PreToolUse hook: inject relevant memories before Bash/Edit/Write
  cat > "$pre_tool_script" <<HOOK
#!/usr/bin/env bash
TOOL_JSON=\$(cat)
TOOL_NAME=\$(printf '%s' "\$TOOL_JSON" | python3 -c "import sys,json; print(json.load(sys.stdin).get('tool_name',''))" 2>/dev/null)
case "\$TOOL_NAME" in
  Bash|Edit|Write)
    QUERY=\$(printf '%s' "\$TOOL_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
inp = d.get('tool_input', {})
print((inp.get('command') or inp.get('file_path') or inp.get('description') or '')[:150])
" 2>/dev/null)
    if [ -n "\$QUERY" ]; then
      KNOT_DATA_DIR="\${KNOT_DATA_DIR:-\$HOME/.knot}" KNOT_LOG=knot=error \\
        "$BIN_PATH" recall "\$QUERY" --limit 3 2>/dev/null || true
    fi
    ;;
esac
exit 0
HOOK
  chmod +x "$pre_tool_script"

  # Stop hook: commit session memories to project scope
  cat > "$stop_script" <<HOOK
#!/usr/bin/env bash
KNOT_DATA_DIR="\${KNOT_DATA_DIR:-\$HOME/.knot}"
SESSION_FILE="\$KNOT_DATA_DIR/.current_session"
if [ ! -f "\$SESSION_FILE" ]; then exit 0; fi
SESSION_ID=\$(cat "\$SESSION_FILE")
if [ -z "\$SESSION_ID" ]; then exit 0; fi
PROJECT=\$(git remote get-url origin 2>/dev/null | sed 's|.*/||; s|\\.git\$||' || basename "\$PWD")
KNOT_DATA_DIR="\$KNOT_DATA_DIR" KNOT_LOG=knot=error \\
  "$BIN_PATH" commit "\$SESSION_ID" "\$PROJECT" 2>/dev/null || true
exit 0
HOOK
  chmod +x "$stop_script"

  if ! command -v python3 &>/dev/null; then
    echo "[KNOT] WARN:  python3 not found — skipping hook registration in $claude_settings"
    echo "[KNOT] WARN:  Add hooks manually: $pre_tool_script, $stop_script"
    return
  fi

  if grep -qF "$marker" "$claude_settings" 2>/dev/null; then
    echo "[KNOT] INFO:  Knot hooks already in $claude_settings"
    return
  fi

  python3 - "$claude_settings" "$pre_tool_script" "$stop_script" <<'PY'
import sys, json, os

settings_path, pre_tool, stop_hook = sys.argv[1], sys.argv[2], sys.argv[3]

cfg = {}
if os.path.exists(settings_path):
    try:
        with open(settings_path) as f:
            cfg = json.load(f)
    except (json.JSONDecodeError, IOError):
        pass

cfg.setdefault("hooks", {})
cfg["hooks"].setdefault("PreToolUse", [])
cfg["hooks"].setdefault("Stop", [])

cfg["hooks"]["PreToolUse"].append({
    "matcher": "Bash|Edit|Write",
    "hooks": [{"type": "command", "command": pre_tool}]
})
cfg["hooks"]["Stop"].append({
    "hooks": [{"type": "command", "command": stop_hook}]
})

os.makedirs(os.path.dirname(os.path.abspath(settings_path)), exist_ok=True)
with open(settings_path, "w") as f:
    json.dump(cfg, f, indent=2)
    f.write("\n")
PY

  if [[ $? -eq 0 ]]; then
    echo "[KNOT] INFO:  Registered hooks in $claude_settings"
    echo "[KNOT] INFO:    PreToolUse : $pre_tool_script"
    echo "[KNOT] INFO:    Stop       : $stop_script"
  else
    echo "[KNOT] WARN:  Hook registration failed"
  fi
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
