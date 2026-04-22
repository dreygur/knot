use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};

const PRE_TOOL_SCRIPT: &str = r#"#!/usr/bin/env bash
TOOL_JSON=$(cat)
TOOL_NAME=$(printf '%s' "$TOOL_JSON" | python3 -c "import sys,json; print(json.load(sys.stdin).get('tool_name',''))" 2>/dev/null)
case "$TOOL_NAME" in
  Bash|Edit|Write)
    QUERY=$(printf '%s' "$TOOL_JSON" | python3 -c "
import sys, json
d = json.load(sys.stdin)
inp = d.get('tool_input', {})
print((inp.get('command') or inp.get('file_path') or inp.get('description') or '')[:150])
" 2>/dev/null)
    if [ -n "$QUERY" ]; then
      KNOT_DATA_DIR="${KNOT_DATA_DIR:-$HOME/.knot}" KNOT_LOG=knot=error \
        __KNOT_BIN__ recall "$QUERY" --limit 3 2>/dev/null || true
    fi
    ;;
esac
exit 0
"#;

const STOP_SCRIPT: &str = r#"#!/usr/bin/env bash
KNOT_DATA_DIR="${KNOT_DATA_DIR:-$HOME/.knot}"
SESSION_FILE="$KNOT_DATA_DIR/.current_session"
if [ ! -f "$SESSION_FILE" ]; then exit 0; fi
SESSION_ID=$(cat "$SESSION_FILE")
if [ -z "$SESSION_ID" ]; then exit 0; fi
PROJECT=$(git remote get-url origin 2>/dev/null | sed 's|.*/||; s|\.git$||' || basename "$PWD")
KNOT_DATA_DIR="$KNOT_DATA_DIR" KNOT_LOG=knot=error \
  __KNOT_BIN__ commit "$SESSION_ID" "$PROJECT" 2>/dev/null || true
exit 0
"#;

/// Register Claude Code hooks. Returns `Ok(true)` when newly registered, `Ok(false)` when
/// already in place (idempotent — safe to call on every server start).
pub fn register(data_dir: &str, bin_path: &Path) -> Result<bool> {
    let hook_dir = PathBuf::from(data_dir).join("hooks");
    std::fs::create_dir_all(&hook_dir)?;

    let pre_tool = hook_dir.join("knot-pre-tool.sh");
    let stop = hook_dir.join("knot-stop.sh");
    let bin_str = bin_path.to_string_lossy();

    std::fs::write(&pre_tool, PRE_TOOL_SCRIPT.replace("__KNOT_BIN__", &bin_str))?;
    std::fs::write(&stop, STOP_SCRIPT.replace("__KNOT_BIN__", &bin_str))?;
    set_executable(&pre_tool)?;
    set_executable(&stop)?;

    let settings = claude_settings_path()?;
    merge_settings(&settings, &pre_tool, &stop)
}

fn claude_settings_path() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home).join(".claude").join("settings.json"))
}

/// Merge hook entries into settings.json. Returns true when entries were added.
fn merge_settings(settings: &Path, pre_tool: &Path, stop: &Path) -> Result<bool> {
    let pre_tool_str = pre_tool.to_string_lossy().into_owned();
    let stop_str = stop.to_string_lossy().into_owned();

    let mut cfg: serde_json::Value = if settings.exists() {
        let s = std::fs::read_to_string(settings)?;
        serde_json::from_str(&s).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    // Idempotency: skip if our pre-tool hook is already registered
    if let Some(arr) = cfg.pointer("/hooks/PreToolUse").and_then(|v| v.as_array()) {
        let already = arr.iter().any(|e| {
            e.pointer("/hooks/0/command")
                .and_then(|v| v.as_str())
                .map(|s| s.ends_with("knot-pre-tool.sh"))
                .unwrap_or(false)
        });
        if already {
            return Ok(false);
        }
    }

    if cfg.get("hooks").is_none() {
        cfg["hooks"] = json!({});
    }
    if cfg["hooks"].get("PreToolUse").is_none() {
        cfg["hooks"]["PreToolUse"] = json!([]);
    }
    if cfg["hooks"].get("Stop").is_none() {
        cfg["hooks"]["Stop"] = json!([]);
    }

    cfg["hooks"]["PreToolUse"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "matcher": "Bash|Edit|Write",
            "hooks": [{"type": "command", "command": pre_tool_str}]
        }));
    cfg["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "hooks": [{"type": "command", "command": stop_str}]
        }));

    if let Some(parent) = settings.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut content = serde_json::to_string_pretty(&cfg)?;
    content.push('\n');
    std::fs::write(settings, content)?;
    Ok(true)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}
