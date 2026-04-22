use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};

const PRE_TOOL_SH: &str = r#"#!/usr/bin/env bash
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

const STOP_SH: &str = r#"#!/usr/bin/env bash
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

const PRE_TOOL_PS1: &str = r#"$raw = [Console]::In.ReadToEnd()
try { $d = $raw | ConvertFrom-Json } catch { exit 0 }
if ($d.tool_name -notin @('Bash', 'Edit', 'Write')) { exit 0 }
$inp = $d.tool_input
$query = @($inp.command, $inp.file_path, $inp.description) | Where-Object { $_ } | Select-Object -First 1
if (-not $query) { exit 0 }
if ($query.Length -gt 150) { $query = $query.Substring(0, 150) }
$env:KNOT_DATA_DIR = if ($env:KNOT_DATA_DIR) { $env:KNOT_DATA_DIR } else { "$env:USERPROFILE\.knot" }
$env:KNOT_LOG = 'knot=error'
& '__KNOT_BIN__' recall $query --limit 3 2>$null
exit 0
"#;

const STOP_PS1: &str = r#"$dataDir = if ($env:KNOT_DATA_DIR) { $env:KNOT_DATA_DIR } else { "$env:USERPROFILE\.knot" }
$sessionFile = Join-Path $dataDir '.current_session'
if (-not (Test-Path $sessionFile)) { exit 0 }
$sessionId = (Get-Content $sessionFile -ErrorAction SilentlyContinue | Select-Object -First 1)
if (-not $sessionId) { exit 0 }
$project = $null
try {
    $origin = (git remote get-url origin 2>$null)
    if ($origin) { $project = [System.IO.Path]::GetFileNameWithoutExtension($origin.Split('/')[-1]) }
} catch {}
if (-not $project) { $project = Split-Path -Leaf (Get-Location).Path }
$env:KNOT_DATA_DIR = $dataDir
$env:KNOT_LOG = 'knot=error'
& '__KNOT_BIN__' commit $sessionId $project 2>$null
exit 0
"#;

/// Register Claude Code hooks. Returns `Ok(true)` when newly registered, `Ok(false)` when
/// already in place (idempotent - safe to call on every server start).
pub fn register(data_dir: &str, bin_path: &Path) -> Result<bool> {
    let hook_dir = PathBuf::from(data_dir).join("hooks");
    std::fs::create_dir_all(&hook_dir)?;
    let bin_str = bin_path.to_string_lossy().into_owned();
    let (pre_cmd, stop_cmd) = write_hooks(&hook_dir, &bin_str)?;
    let settings = claude_settings_path()?;
    merge_settings(&settings, &pre_cmd, &stop_cmd)
}

#[cfg(unix)]
fn write_hooks(hook_dir: &Path, bin_str: &str) -> Result<(String, String)> {
    let pre = hook_dir.join("knot-pre-tool.sh");
    let stp = hook_dir.join("knot-stop.sh");
    std::fs::write(&pre, PRE_TOOL_SH.replace("__KNOT_BIN__", bin_str))?;
    std::fs::write(&stp, STOP_SH.replace("__KNOT_BIN__", bin_str))?;
    set_executable(&pre)?;
    set_executable(&stp)?;
    Ok((
        pre.to_string_lossy().into_owned(),
        stp.to_string_lossy().into_owned(),
    ))
}

#[cfg(windows)]
fn write_hooks(hook_dir: &Path, bin_str: &str) -> Result<(String, String)> {
    let pre = hook_dir.join("knot-pre-tool.ps1");
    let stp = hook_dir.join("knot-stop.ps1");
    std::fs::write(&pre, PRE_TOOL_PS1.replace("__KNOT_BIN__", bin_str))?;
    std::fs::write(&stp, STOP_PS1.replace("__KNOT_BIN__", bin_str))?;
    let pre_cmd = format!(
        "powershell.exe -NoProfile -NonInteractive -File \"{}\"",
        pre.to_string_lossy()
    );
    let stp_cmd = format!(
        "powershell.exe -NoProfile -NonInteractive -File \"{}\"",
        stp.to_string_lossy()
    );
    Ok((pre_cmd, stp_cmd))
}

fn claude_settings_path() -> Result<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map_err(|_| anyhow::anyhow!("USERPROFILE not set"))?;
    #[cfg(not(windows))]
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home).join(".claude").join("settings.json"))
}

fn merge_settings(settings: &Path, pre_cmd: &str, stop_cmd: &str) -> Result<bool> {
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
                .map(|s| s.contains("knot-pre-tool"))
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
            "hooks": [{"type": "command", "command": pre_cmd}]
        }));
    cfg["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "hooks": [{"type": "command", "command": stop_cmd}]
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
