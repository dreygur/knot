use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Markers that identify Knot's own entries in `~/.claude/settings.json`: the current
/// `knot hook` commands and the older shell script hooks they replace.
/// The pre-tool and session-start markers include the quoted binary's name so another tool's own
/// `<tool> hook pre-tool` or `hook session-start` is never mistaken for ours.
const KNOT_HOOK_MARKERS: &[&str] = &[
    " hook post-tool",
    " hook stop",
    "knot-pre-tool",
    "knot-stop",
    "knot\" hook pre-tool",
    "knot.exe\" hook pre-tool",
    "knot\" hook session-start",
    "knot.exe\" hook session-start",
    "knot\" hook usage",
    "knot.exe\" hook usage",
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
        // The limit warning, after every tool, so a session that only runs Bash is warned too.
        // A separate entry keeps the learnings lookup off every Bash call.
        ("PostToolUse", json!({"hooks": [{"type": "command", "command": format!("{bin} hook usage")}]})),
        ("Stop", json!({"hooks": [{"type": "command", "command": format!("{bin} hook stop")}]})),
        // Index counts and where to start, so the agent asks Knot before grepping
        ("SessionStart", json!({"hooks": [{"type": "command", "command": format!("{bin} hook session-start")}]})),
    ]
}

fn merge_settings(settings: &Path, bin: &str) -> Result<bool> {
    // A file that does not parse is the user's to fix; rewriting it from `{}` would lose it
    let mut cfg = read_settings(settings)?;
    let before = cfg.clone();

    if !cfg.get("hooks").is_some_and(|h| h.is_object()) {
        cfg["hooks"] = json!({});
    }
    // Drop every Knot entry (older script hooks included) and add the current ones, so a
    // moved binary or an upgrade leaves exactly one set. Other tools' hooks are untouched.
    for event in ["PreToolUse", "PostToolUse", "Stop", "SessionStart"] {
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
    for event in ["PreToolUse", "PostToolUse", "Stop", "SessionStart"] {
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
    // A hostile clone can commit `.claude` or the settings file as a symlink
    refuse_symlink(&repo_root.join(".claude"))?;
    refuse_symlink(&local)?;
    let mut cfg = read_settings(&local)?;
    let before = cfg.clone();

    let mut rival = None;
    for path in [local.clone(), repo_root.join(".claude/settings.json"), home.join(".claude/settings.json")] {
        let found = if path == local { other_bash_hook(&cfg) } else { other_bash_hook(&read_settings(&path)?) };
        if found.is_some() {
            rival = found;
            break;
        }
    }
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
        let dir = repo_root.join(".claude");
        std::fs::create_dir_all(&dir)?;
        // Belt and braces: the created or existing folder must really sit inside the repo
        if !dir.canonicalize()?.starts_with(repo_root.canonicalize()?) {
            anyhow::bail!("{} resolves outside the repo; Knot does not write there", dir.display());
        }
        let mut content = serde_json::to_string_pretty(&cfg)?;
        content.push('\n');
        write_atomic(&local, &content)?;
    }
    ignore_in_git(repo_root)?;
    Ok(result)
}

pub(crate) fn refuse_symlink(path: &Path) -> Result<()> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        anyhow::bail!("{} is a symlink; Knot does not write through it", path.display());
    }
    Ok(())
}

/// Write to a sibling temp file and rename it into place, so a crash leaves the old file.
/// The temp file is created fresh, never opened through a planted link, and takes the
/// old file's mode, so a private config holding a token stays private.
pub(crate) fn write_atomic(path: &Path, content: &str) -> Result<()> {
    use std::io::Write;
    refuse_symlink(path)?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".knot-tmp");
    let tmp = path.with_file_name(name);
    // A crash can leave one behind; removing a link removes the link, not its target
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => anyhow::bail!("cannot replace {}: {e}", tmp.display()),
        _ => {}
    }

    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    if let Ok(old) = std::fs::metadata(path) {
        file.set_permissions(old.permissions())?;
    }
    file.write_all(content.as_bytes())?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// A missing file is `{}`; one that cannot be read or is not a JSON object is an error
/// naming it, so a hand-edited file is never overwritten from an empty object.
pub(crate) fn read_settings(path: &Path) -> Result<serde_json::Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => anyhow::bail!("cannot read {}: {e}", path.display()),
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => anyhow::bail!("{} is not a JSON object; Knot leaves it alone until it is fixed", path.display()),
        Err(e) => anyhow::bail!("{} is not valid JSON ({e}); Knot leaves it alone until it is fixed", path.display()),
    }
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
    // git reads info/exclude from the common dir, which a linked worktree's own gitdir is not
    let info = repo.commondir().join("info");
    let exclude = info.join("exclude");
    refuse_symlink(&info)?;
    refuse_symlink(&exclude)?;
    let mut content = match std::fs::read_to_string(&exclude) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => anyhow::bail!("cannot read {}: {e}", exclude.display()),
    };
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(LOCAL_SETTINGS);
    content.push('\n');
    std::fs::create_dir_all(&info)?;
    write_atomic(&exclude, &content)
}

/// What `register_repo` installed here, for `knot status`; `None` when nothing is.
pub fn repo_swaps(repo_root: &Path) -> Option<SwapInstall> {
    let cfg = read_settings(&repo_root.join(LOCAL_SETTINGS)).ok()?;
    let ours = cfg.pointer("/hooks/PreToolUse")?.as_array()?.iter().find(|e| is_knot_entry(e))?;
    let command = ours.pointer("/hooks/0/command")?.as_str()?;
    if !command.ends_with("--no-swap") {
        return Some(SwapInstall::Installed);
    }
    // The rival is whatever the settings files show now; it may have gone since
    let to = other_bash_hook(&cfg)
        .or_else(|| other_bash_hook(&read_settings(&repo_root.join(".claude/settings.json")).ok()?))
        .or_else(|| home_dir().ok().and_then(|h| other_bash_hook(&read_settings(&h.join(".claude/settings.json")).ok()?)))
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

    /// Point the repo's excludes file at an empty one, so a global ignore that already
    /// covers the settings file cannot make a test pass without Knot's own exclude.
    fn without_global_ignore(git: &git2::Repository, dir: &Path) {
        let empty = dir.join(".git/empty-excludes");
        std::fs::write(&empty, "").unwrap();
        git.config().unwrap().set_str("core.excludesFile", &empty.to_string_lossy()).unwrap();
    }

    #[test]
    fn local_settings_are_git_ignored_in_a_linked_worktree() {
        let main = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let git = git2::Repository::init(main.path()).unwrap();
        let sig = git2::Signature::now("Maya", "m@x").unwrap();
        let tree = git.find_tree(git.index().unwrap().write_tree().unwrap()).unwrap();
        git.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
        let place = tempfile::tempdir().unwrap();
        let wt_path = place.path().join("wt");
        git.worktree("wt", &wt_path, None).unwrap();
        without_global_ignore(&git, main.path());
        assert!(!git2::Repository::open(&wt_path).unwrap().status_should_ignore(Path::new(LOCAL_SETTINGS)).unwrap());

        register_repo_in(&wt_path, Path::new(BIN), home.path()).unwrap();

        let wt = git2::Repository::open(&wt_path).unwrap();
        assert!(wt.status_should_ignore(Path::new(LOCAL_SETTINGS)).unwrap());
        let exclude = std::fs::read_to_string(main.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.contains(LOCAL_SETTINGS), "{exclude}");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_git_exclude_is_an_error_not_wiped() {
        use std::os::unix::fs::PermissionsExt;
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let git = git2::Repository::init(repo.path()).unwrap();
        without_global_ignore(&git, repo.path());
        let exclude = repo.path().join(".git/info/exclude");
        std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        std::fs::write(&exclude, "keep-me/\n").unwrap();
        std::fs::set_permissions(&exclude, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = register_repo_in(repo.path(), Path::new(BIN), home.path());

        std::fs::set_permissions(&exclude, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = result.unwrap_err().to_string();
        assert!(err.contains("exclude"), "{err}");
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), "keep-me/\n");
    }

    #[test]
    fn malformed_user_settings_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        let broken = "{\"permissions\": {\"allow\": [\"x\"],},}\n";
        std::fs::write(&settings, broken).unwrap();

        let err = merge_settings(&settings, "\"/bin/knot\"").unwrap_err().to_string();

        assert!(err.contains("settings.json"), "{err}");
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), broken);
    }

    #[test]
    fn malformed_local_settings_are_an_error_and_left_alone() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
        let local = repo.path().join(".claude/settings.local.json");
        let broken = "{\"permissions\": {\"allow\": [\"x\"],},}\n";
        std::fs::write(&local, broken).unwrap();

        let err = register_repo_in(repo.path(), Path::new(BIN), home.path()).unwrap_err().to_string();

        assert!(err.contains("settings.local.json"), "{err}");
        assert_eq!(std::fs::read_to_string(&local).unwrap(), broken);
    }

    #[test]
    fn a_broken_rival_file_is_an_error_not_no_rival() {
        let repo = project_with_bash_hook("rtk hook pre-tool");
        std::fs::write(repo.path().join(".claude/settings.json"), "{ nope").unwrap();
        let home = tempfile::tempdir().unwrap();
        let err = register_repo_in(repo.path(), Path::new(BIN), home.path()).unwrap_err().to_string();
        assert!(err.contains("settings.json"), "{err}");
        assert!(!repo.path().join(".claude/settings.local.json").exists());
    }

    #[test]
    fn a_non_object_top_level_is_an_error_not_a_panic() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
        let local = repo.path().join(".claude/settings.local.json");
        std::fs::write(&local, "[]").unwrap();

        let err = register_repo_in(repo.path(), Path::new(BIN), home.path()).unwrap_err().to_string();

        assert!(err.contains("not a JSON object"), "{err}");
        assert_eq!(std::fs::read_to_string(&local).unwrap(), "[]");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_claude_folder_or_settings_file_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();

        // `.claude` itself a link to a folder elsewhere
        let repo = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join(".claude")).unwrap();
        let err = register_repo_in(repo.path(), Path::new(BIN), home.path()).unwrap_err().to_string();
        assert!(err.contains("symlink"), "{err}");
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0, "nothing written through the link");

        // the settings file a link to a file elsewhere
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
        let target = outside.path().join("victim");
        std::fs::write(&target, "keep me").unwrap();
        std::os::unix::fs::symlink(&target, repo.path().join(".claude/settings.local.json")).unwrap();
        let err = register_repo_in(repo.path(), Path::new(BIN), home.path()).unwrap_err().to_string();
        assert!(err.contains("symlink"), "{err}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep me");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_git_exclude_is_not_written_through() {
        let outside = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        git2::Repository::init(repo.path()).unwrap();
        let target = outside.path().join("victim");
        std::fs::write(&target, "keep me").unwrap();
        let exclude = repo.path().join(".git/info/exclude");
        std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&exclude);
        std::os::unix::fs::symlink(&target, &exclude).unwrap();

        let git = git2::Repository::open(repo.path()).unwrap();
        // Only meaningful where the global ignore does not already cover the file
        if !git.status_should_ignore(Path::new(".claude/settings.local.json")).unwrap() {
            assert!(register_repo_in(repo.path(), Path::new(BIN), home.path()).is_err());
        }
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep me");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_the_mode_and_clears_a_stale_temp_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("exclude");
        std::fs::write(&file, "old").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::fs::write(dir.path().join("exclude.knot-tmp"), "left by a crash").unwrap();

        write_atomic(&file, "new").unwrap();

        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o640);
        assert!(!dir.path().join("exclude.knot-tmp").exists());
    }

    #[test]
    fn a_lookalike_hook_is_not_knots() {
        let entry = json!({"hooks": [{"type": "command", "command": "rtk hook pre-tool"}]});
        assert!(!is_knot_entry(&entry));
        assert!(is_knot_entry(&json!({"hooks": [{"command": "\"/x/knot\" hook pre-tool"}]})));
        assert!(is_knot_entry(&json!({"hooks": [{"command": "\"/x/knot\" hook usage"}]})));
        assert!(is_knot_entry(&json!({"hooks": [{"command": "\"C:\\knot.exe\" hook usage"}]})));
        assert!(!is_knot_entry(&json!({"hooks": [{"command": "ccusage hook usage"}]})));
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
        assert_eq!(commands(&cfg, "PostToolUse"), vec!["\"/bin/knot\" hook post-tool", "\"/bin/knot\" hook usage"]);
        assert!(cfg.pointer("/hooks/PostToolUse/1/matcher").is_none(), "the usage check runs after every tool");
        assert_eq!(commands(&cfg, "Stop"), vec!["\"/bin/knot\" hook stop"]);
        assert_eq!(commands(&cfg, "SessionStart"), vec!["\"/bin/knot\" hook session-start"]);
        assert!(cfg.pointer("/hooks/SessionStart/0/matcher").is_none(), "every session start, no matcher");
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
        assert_eq!(
            commands(&read(&settings), "PostToolUse"),
            vec!["\"/moved/knot\" hook post-tool", "\"/moved/knot\" hook usage"]
        );
        assert_eq!(commands(&read(&settings), "SessionStart"), vec!["\"/moved/knot\" hook session-start"]);
    }

    #[test]
    fn another_tools_session_start_hook_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(
            &settings,
            json!({ "hooks": { "SessionStart": [{"hooks": [{"type": "command", "command": "rtk hook session-start"}]}] } }).to_string(),
        )
        .unwrap();

        assert!(merge_settings(&settings, "\"/bin/knot\"").unwrap());
        merge_settings(&settings, "\"/bin/knot\"").unwrap();
        assert_eq!(
            commands(&read(&settings), "SessionStart"),
            vec!["rtk hook session-start", "\"/bin/knot\" hook session-start"]
        );
        assert!(is_knot_entry(&json!({"hooks": [{"command": "\"C:\\knot.exe\" hook session-start"}]})));
    }
}
