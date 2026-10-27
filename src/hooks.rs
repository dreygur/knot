use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Markers that identify Knot's own entries in `~/.claude/settings.json`: the current
/// `knot hook` commands and the older shell script hooks they replace.
const KNOT_HOOK_MARKERS: &[&str] = &[" hook post-tool", " hook stop", "knot-pre-tool", "knot-stop"];

/// Register Claude Code hooks and, when none is set, the status line. Returns `Ok(true)`
/// when settings changed, `Ok(false)` when already in place (safe on every server start).
pub fn register(_data_dir: &str, bin_path: &Path) -> Result<bool> {
    let bin = format!("\"{}\"", bin_path.to_string_lossy());
    let settings = claude_settings_path()?;
    merge_settings(&settings, &bin)
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

fn is_knot_entry(entry: &serde_json::Value) -> bool {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .is_some_and(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| KNOT_HOOK_MARKERS.iter().any(|m| c.contains(m)))
            })
        })
}

/// The hook entries Knot wants, by event.
fn wanted_hooks(bin: &str) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            // Learnings on the file just read or edited, and teammates' clashing edits
            "PostToolUse",
            json!({
                "matcher": "Read|Edit|Write|MultiEdit",
                "hooks": [{"type": "command", "command": format!("{bin} hook post-tool")}]
            }),
        ),
        ("Stop", json!({"hooks": [{"type": "command", "command": format!("{bin} hook stop")}]})),
    ]
}

fn merge_settings(settings: &Path, bin: &str) -> Result<bool> {
    let mut cfg: serde_json::Value = if settings.exists() {
        let s = std::fs::read_to_string(settings)?;
        serde_json::from_str(&s).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    let before = cfg.clone();

    if !cfg.get("hooks").is_some_and(|h| h.is_object()) {
        cfg["hooks"] = json!({});
    }
    // Drop every Knot entry (older script hooks included) and add the current ones, so a
    // moved binary or an upgrade leaves exactly one set. Other tools' hooks are untouched.
    for event in ["PreToolUse", "PostToolUse", "Stop"] {
        if let Some(list) = cfg["hooks"].get_mut(event).and_then(|v| v.as_array_mut()) {
            list.retain(|e| !is_knot_entry(e));
        }
    }
    for (event, entry) in wanted_hooks(bin) {
        if !cfg["hooks"].get(event).is_some_and(|v| v.is_array()) {
            cfg["hooks"][event] = json!([]);
        }
        cfg["hooks"][event].as_array_mut().unwrap().push(entry);
    }
    for event in ["PreToolUse", "PostToolUse", "Stop"] {
        if cfg["hooks"].get(event).and_then(|v| v.as_array()).is_some_and(|a| a.is_empty()) {
            cfg["hooks"].as_object_mut().unwrap().remove(event);
        }
    }
    // A status line the user already set, ours or anyone's, is left alone
    if cfg.get("statusLine").is_none() {
        cfg["statusLine"] = json!({"type": "command", "command": format!("{bin} statusline")});
    }

    if cfg == before {
        return Ok(false);
    }
    if let Some(parent) = settings.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut content = serde_json::to_string_pretty(&cfg)?;
    content.push('\n');
    std::fs::write(settings, content)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn commands(cfg: &serde_json::Value, event: &str) -> Vec<String> {
        cfg.pointer(&format!("/hooks/{event}"))
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|e| e.pointer("/hooks/0/command")?.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }

    #[test]
    fn registers_hooks_and_statusline_once() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");

        assert!(merge_settings(&settings, "\"/bin/knot\"").unwrap());
        assert!(!merge_settings(&settings, "\"/bin/knot\"").unwrap(), "second run changes nothing");

        let cfg = read(&settings);
        assert_eq!(commands(&cfg, "PostToolUse"), vec!["\"/bin/knot\" hook post-tool"]);
        assert_eq!(commands(&cfg, "Stop"), vec!["\"/bin/knot\" hook stop"]);
        assert_eq!(cfg.pointer("/hooks/PostToolUse/0/matcher").unwrap(), "Read|Edit|Write|MultiEdit");
        assert_eq!(cfg["statusLine"]["command"], "\"/bin/knot\" statusline");
    }

    #[test]
    fn replaces_old_script_hooks_and_keeps_other_tools_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(
            &settings,
            json!({
                "statusLine": {"type": "command", "command": "mine"},
                "hooks": {
                    "PreToolUse": [
                        {"matcher": "Bash|Edit|Write", "hooks": [{"type": "command", "command": "/h/.knot/hooks/knot-pre-tool.sh"}]},
                        {"matcher": "Bash", "hooks": [{"type": "command", "command": "other-tool check"}]}
                    ],
                    "Stop": [{"hooks": [{"type": "command", "command": "/h/.knot/hooks/knot-stop.sh"}]}]
                }
            })
            .to_string(),
        )
        .unwrap();

        assert!(merge_settings(&settings, "\"/new/knot\"").unwrap());
        let cfg = read(&settings);
        assert_eq!(commands(&cfg, "PreToolUse"), vec!["other-tool check"]);
        assert_eq!(commands(&cfg, "Stop"), vec!["\"/new/knot\" hook stop"]);
        assert_eq!(cfg["statusLine"]["command"], "mine", "an existing status line is kept");

        // A moved binary replaces its own entries rather than adding a second set
        merge_settings(&settings, "\"/moved/knot\"").unwrap();
        assert_eq!(commands(&read(&settings), "PostToolUse"), vec!["\"/moved/knot\" hook post-tool"]);
    }
}
