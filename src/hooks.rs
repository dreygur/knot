use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Markers that identify Knot's own entries in `~/.claude/settings.json`: the current
/// `knot hook` commands and the older shell script hooks they replace.
/// The pre-tool markers include the quoted binary's name so another tool's own
/// `<tool> hook pre-tool` is never mistaken for ours.
const KNOT_HOOK_MARKERS: &[&str] = &[
    " hook post-tool",
    " hook stop",
    "knot-pre-tool",
    "knot-stop",
    "knot\" hook pre-tool",
    "knot.exe\" hook pre-tool",
];

/// Register Claude Code hooks and, when none is set, the status line. Returns `Ok(true)`
/// when settings changed, `Ok(false)` when already in place (safe on every server start).
pub fn register(_data_dir: &str, bin_path: &Path) -> Result<bool> {
    let bin = format!("\"{}\"", bin_path.to_string_lossy());
    let settings = claude_settings_path()?;
    merge_settings(&settings, &bin)
}

fn home_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map_err(|_| anyhow::anyhow!("USERPROFILE not set"))?;
    #[cfg(not(windows))]
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home))
}

fn claude_settings_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".claude").join("settings.json"))
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

/// What `register_repo` left in a repo's `.claude/settings.local.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapInstall {
    /// Knot's hook rewrites grep and sed over code and stops python code edits.
    Installed,
    /// Another hook already rewrites Bash, so Knot's only stops python code edits.
    Deferred { to: String },
}

const LOCAL_SETTINGS: &str = ".claude/settings.local.json";

/// Install the `PreToolUse` Bash hook for this repo only, in `<repo>/.claude/settings.local.json`.
/// Another tool's Bash hook anywhere it could apply makes Knot's stop at the python block.
pub fn register_repo(repo_root: &Path, bin_path: &Path) -> Result<SwapInstall> {
    register_repo_in(repo_root, bin_path, &home_dir()?)
}

fn register_repo_in(repo_root: &Path, bin_path: &Path, home: &Path) -> Result<SwapInstall> {
    let local = repo_root.join(LOCAL_SETTINGS);
    let mut cfg = read_settings(&local);
    let before = cfg.clone();

    let others = [local.clone(), repo_root.join(".claude/settings.json"), home.join(".claude/settings.json")];
    let rival = others.iter().find_map(|p| other_bash_hook(&read_settings(p)));
    let bin = format!("\"{}\"", bin_path.to_string_lossy());
    let (command, result) = match rival {
        Some(to) => (format!("{bin} hook pre-tool --no-swap"), SwapInstall::Deferred { to }),
        None => (format!("{bin} hook pre-tool"), SwapInstall::Installed),
    };

    if !cfg.get("hooks").is_some_and(|h| h.is_object()) {
        cfg["hooks"] = json!({});
    }
    if !cfg["hooks"].get("PreToolUse").is_some_and(|v| v.is_array()) {
        cfg["hooks"]["PreToolUse"] = json!([]);
    }
    let list = cfg["hooks"]["PreToolUse"].as_array_mut().unwrap();
    list.retain(|e| !is_knot_entry(e));
    list.push(json!({"matcher": "Bash", "hooks": [{"type": "command", "command": command}]}));

    if cfg != before {
        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut content = serde_json::to_string_pretty(&cfg)?;
        content.push('\n');
        std::fs::write(&local, content)?;
    }
    ignore_in_git(repo_root)?;
    Ok(result)
}

fn read_settings(path: &Path) -> serde_json::Value {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}))
}

/// The command of the first `PreToolUse` hook in `cfg` that is not Knot's and runs for Bash.
fn other_bash_hook(cfg: &serde_json::Value) -> Option<String> {
    let entries = cfg.pointer("/hooks/PreToolUse")?.as_array()?;
    entries.iter().filter(|e| !is_knot_entry(e) && matches_bash(e)).find_map(|e| {
        e.get("hooks")?.as_array()?.iter().find_map(|h| h.get("command")?.as_str().map(String::from))
    })
}

/// A matcher is a `|` list of tool names; none, empty or `*` means every tool.
fn matches_bash(entry: &serde_json::Value) -> bool {
    match entry.get("matcher").and_then(|m| m.as_str()) {
        None | Some("") | Some("*") => true,
        Some(m) => m.split('|').any(|name| name.trim() == "Bash"),
    }
}

/// Keep the personal settings file out of commits: add it to `.git/info/exclude` unless
/// git already ignores it. A folder that is not a git repo has nothing to ignore.
fn ignore_in_git(repo_root: &Path) -> Result<()> {
    let Ok(repo) = git2::Repository::open(repo_root) else { return Ok(()) };
    if repo.status_should_ignore(Path::new(LOCAL_SETTINGS)).unwrap_or(false) {
        return Ok(());
    }
    let exclude = repo.path().join("info").join("exclude");
    let mut content = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(LOCAL_SETTINGS);
    content.push('\n');
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(exclude, content)?;
    Ok(())
}

/// What `register_repo` installed here, for `knot status`; `None` when nothing is.
pub fn repo_swaps(repo_root: &Path) -> Option<SwapInstall> {
    let cfg = read_settings(&repo_root.join(LOCAL_SETTINGS));
    let ours = cfg.pointer("/hooks/PreToolUse")?.as_array()?.iter().find(|e| is_knot_entry(e))?;
    let command = ours.pointer("/hooks/0/command")?.as_str()?;
    if !command.ends_with("--no-swap") {
        return Some(SwapInstall::Installed);
    }
    // The rival is whatever the settings files show now; it may have gone since
    let to = other_bash_hook(&cfg)
        .or_else(|| other_bash_hook(&read_settings(&repo_root.join(".claude/settings.json"))))
        .or_else(|| home_dir().ok().and_then(|h| other_bash_hook(&read_settings(&h.join(".claude/settings.json")))))
        .unwrap_or_default();
    Some(SwapInstall::Deferred { to })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIN: &str = "/bin/knot";

    fn project_with_bash_hook(command: &str) -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
        std::fs::write(
            repo.path().join(".claude/settings.json"),
            json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": command}]}]}})
                .to_string(),
        )
        .unwrap();
        repo
    }

    #[test]
    fn register_repo_writes_only_the_local_settings() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::write(home.path().join(".claude/settings.json"), "{\"theme\":\"dark\"}\n").unwrap();

        let got = register_repo_in(repo.path(), Path::new(BIN), home.path()).unwrap();

        assert_eq!(got, SwapInstall::Installed);
        let cfg = read(&repo.path().join(".claude/settings.local.json"));
        assert_eq!(commands(&cfg, "PreToolUse"), vec!["\"/bin/knot\" hook pre-tool"]);
        assert_eq!(cfg.pointer("/hooks/PreToolUse/0/matcher").unwrap(), "Bash");
        assert_eq!(std::fs::read_to_string(home.path().join(".claude/settings.json")).unwrap(), "{\"theme\":\"dark\"}\n");
        assert!(!repo.path().join(".claude/settings.json").exists());
        assert_eq!(repo_swaps(repo.path()), Some(SwapInstall::Installed));
    }

    #[test]
    fn rerun_keeps_one_knot_entry() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let local = repo.path().join(".claude/settings.local.json");
        std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
        std::fs::write(
            &local,
            json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "mine --check"}]}]}})
                .to_string(),
        )
        .unwrap();

        register_repo_in(repo.path(), Path::new("/old/knot"), home.path()).unwrap();
        register_repo_in(repo.path(), Path::new(BIN), home.path()).unwrap();

        // The user's own hook is theirs to decide about; ours is replaced, not doubled
        let cmds = commands(&read(&local), "PreToolUse");
        assert_eq!(cmds.iter().filter(|c| c.contains("hook pre-tool")).count(), 1, "{cmds:?}");
        assert!(cmds.contains(&"\"/bin/knot\" hook pre-tool --no-swap".to_string()), "{cmds:?}");
        assert!(cmds.contains(&"mine --check".to_string()));
    }

    #[test]
    fn swap_skipped_when_another_bash_rewriter_exists() {
        let repo = project_with_bash_hook("rtk hook pre-tool");
        let home = tempfile::tempdir().unwrap();

        let got = register_repo_in(repo.path(), Path::new(BIN), home.path()).unwrap();

        assert_eq!(got, SwapInstall::Deferred { to: "rtk hook pre-tool".to_string() });
        let cmds = commands(&read(&repo.path().join(".claude/settings.local.json")), "PreToolUse");
        assert_eq!(cmds, vec!["\"/bin/knot\" hook pre-tool --no-swap"]);
        assert_eq!(repo_swaps(repo.path()), Some(got));
    }

    #[test]
    fn local_settings_are_git_ignored() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let git = git2::Repository::init(repo.path()).unwrap();

        register_repo_in(repo.path(), Path::new(BIN), home.path()).unwrap();
        assert!(git.status_should_ignore(Path::new(".claude/settings.local.json")).unwrap());

        // A second run does not list it twice (none at all when a global ignore already covers it)
        register_repo_in(repo.path(), Path::new(BIN), home.path()).unwrap();
        let exclude = std::fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.matches(".claude/settings.local.json").count() <= 1, "{exclude}");
    }

    #[test]
    fn a_lookalike_hook_is_not_knots() {
        let entry = json!({"hooks": [{"type": "command", "command": "rtk hook pre-tool"}]});
        assert!(!is_knot_entry(&entry));
        assert!(is_knot_entry(&json!({"hooks": [{"command": "\"/x/knot\" hook pre-tool"}]})));
    }

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
