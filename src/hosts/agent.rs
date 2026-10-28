use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use super::jsonc;

/// A coding agent `knot init` can wire, named by the id `--agents` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
    Codex,
    OpenCode,
    Gemini,
    Antigravity,
    Cursor,
}

/// Where an agent's instructions go: a file in the repo, or one on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Repo(&'static str),
    Home(&'static str),
}

impl Target {
    /// The path as the person reads it: repo relative, or under `~/`.
    pub fn display(self) -> String {
        match self {
            Target::Repo(path) => path.to_string(),
            Target::Home(path) => format!("~/{path}"),
        }
    }
}

pub const CLAUDE_MD: Target = Target::Repo("CLAUDE.md");
pub const AGENTS_MD: Target = Target::Repo("AGENTS.md");
pub const GEMINI_MD: Target = Target::Repo("GEMINI.md");
pub const CURSOR_RULE: Target = Target::Repo(".cursor/rules/knot.mdc");
pub const ANTIGRAVITY_SKILL: Target = Target::Home(".gemini/skills/knot/SKILL.md");

fn has_command(dir: &Path, name: &str) -> bool {
    if dir.join(name).is_file() {
        return true;
    }
    cfg!(windows) && ["exe", "cmd", "bat"].iter().any(|ext| dir.join(format!("{name}.{ext}")).is_file())
}

/// Whether a Codex `config.toml` has a `[mcp_servers.knot]` table (or one nested under it).
fn toml_has_knot(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    text.lines().any(|line| {
        let Some(rest) = line.trim().strip_prefix("[mcp_servers.knot") else { return false };
        rest.starts_with(']') || rest.starts_with('.')
    })
}

/// A TOML basic or literal string, as `command = "..."` gives it.
fn toml_string(value: &str) -> Option<String> {
    if let Some(literal) = value.strip_prefix('\'') {
        return literal.split_once('\'').map(|(s, _)| s.to_string());
    }
    let mut chars = value.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            _ => out.push(c),
        }
    }
    None
}

/// The `command` of the `[mcp_servers.knot]` table in a Codex `config.toml`.
fn toml_knot_command(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_knot = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_knot = line == "[mcp_servers.knot]";
            continue;
        }
        let Some((name, value)) = line.split_once('=') else { continue };
        if in_knot && name.trim() == "command" {
            return toml_string(value.trim());
        }
    }
    None
}

impl Agent {
    /// Picker and output order.
    pub const ALL: [Agent; 6] = [
        Agent::Claude,
        Agent::Codex,
        Agent::OpenCode,
        Agent::Gemini,
        Agent::Antigravity,
        Agent::Cursor,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
            Agent::Gemini => "gemini",
            Agent::Antigravity => "antigravity",
            Agent::Cursor => "cursor",
        }
    }

    /// The name the person knows it by, as the share card shows it.
    pub fn display(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::OpenCode => "OpenCode",
            Agent::Gemini => "Gemini CLI",
            Agent::Antigravity => "Antigravity",
            Agent::Cursor => "Cursor",
        }
    }

    pub fn from_id(id: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|a| a.id() == id)
    }

    /// The files this agent reads Knot's instructions from.
    pub fn targets(self) -> &'static [Target] {
        match self {
            Agent::Claude => &[CLAUDE_MD],
            Agent::Codex | Agent::OpenCode => &[AGENTS_MD],
            Agent::Gemini => &[GEMINI_MD],
            Agent::Antigravity => &[GEMINI_MD, ANTIGRAVITY_SKILL],
            Agent::Cursor => &[CURSOR_RULE],
        }
    }

    /// Folders under `~` each agent creates on first run.
    fn home_markers(self) -> &'static [&'static str] {
        match self {
            Agent::Claude => &[".claude"],
            Agent::Codex => &[".codex"],
            Agent::OpenCode => &[".config/opencode", ".opencode"],
            Agent::Gemini => &[".gemini/settings.json"],
            Agent::Antigravity => &[".gemini/antigravity"],
            Agent::Cursor => &[".cursor"],
        }
    }

    fn commands(self) -> &'static [&'static str] {
        match self {
            Agent::Claude => &["claude"],
            Agent::Codex => &["codex"],
            Agent::OpenCode => &["opencode"],
            Agent::Gemini => &["gemini"],
            Agent::Antigravity => &["agy", "antigravity"],
            Agent::Cursor => &["cursor", "cursor-agent"],
        }
    }

    /// The user config under `~` that lists this agent's MCP servers, and the key that
    /// holds them. OpenCode's is the user one, not the repo's `opencode.json`, because
    /// the entry carries this machine's binary path.
    fn mcp_file(self) -> (&'static str, &'static str) {
        match self {
            Agent::Claude => (".claude.json", "mcpServers"),
            Agent::Codex => (".codex/config.toml", "mcp_servers"),
            Agent::Gemini => (".gemini/settings.json", "mcpServers"),
            Agent::Antigravity => (".gemini/config/mcp_config.json", "mcpServers"),
            Agent::Cursor => (".cursor/mcp.json", "mcpServers"),
            Agent::OpenCode => (".config/opencode/opencode.json", "mcp"),
        }
    }

    /// The variable that moves this agent's config folder, which its own CLI follows.
    fn config_dir_var(self) -> Option<&'static str> {
        match self {
            Agent::Claude => Some("CLAUDE_CONFIG_DIR"),
            Agent::Codex => Some("CODEX_HOME"),
            _ => None,
        }
    }

    /// [`Agent::mcp_config`] with the environment read through `env`.
    pub(super) fn mcp_config_with(self, home: &Path, env: impl Fn(&str) -> Option<OsString>) -> (PathBuf, &'static str) {
        let (file, key) = self.mcp_file();
        let moved = self.config_dir_var().and_then(env).filter(|dir| !dir.is_empty());
        match (moved, Path::new(file).file_name()) {
            (Some(dir), Some(name)) => (PathBuf::from(dir).join(name), key),
            _ => (home.join(file), key),
        }
    }

    /// The config file that lists this agent's MCP servers, and the key that holds them.
    pub fn mcp_config(self, home: &Path) -> (PathBuf, &'static str) {
        self.mcp_config_with(home, |var| std::env::var_os(var))
    }

    /// This agent's `knot` MCP entry, from a JSON config that may carry comments.
    fn knot_entry(self, home: &Path) -> Option<serde_json::Value> {
        let (path, key) = self.mcp_config(home);
        let json = jsonc::parse(&std::fs::read_to_string(path).ok()?)?;
        json.get(key)?.get("knot").cloned()
    }

    /// Whether this agent's user config under `home` has a `knot` MCP entry. A missing or
    /// unreadable file counts as not wired.
    pub fn knot_wired(self, home: &Path) -> bool {
        if self == Agent::Codex {
            return toml_has_knot(&self.mcp_config(home).0);
        }
        self.knot_entry(home).is_some()
    }

    /// The binary the `knot` MCP entry runs: a string, or the first item of OpenCode's
    /// command list. `None` when there is no entry or no readable command.
    pub fn knot_command(self, home: &Path) -> Option<String> {
        if self == Agent::Codex {
            return toml_knot_command(&self.mcp_config(home).0);
        }
        let command = self.knot_entry(home)?.get("command")?.clone();
        let first = command.as_array().and_then(|list| list.first()).cloned().unwrap_or(command);
        first.as_str().map(String::from)
    }

    /// Whether this agent looks installed: its folder under `home`, or its command on
    /// `path` (the `PATH` value, passed in so tests do not depend on the machine).
    pub fn detected(self, home: &Path, path: Option<&OsStr>) -> bool {
        if self.home_markers().iter().any(|m| home.join(m).exists()) {
            return true;
        }
        let Some(path) = path else {
            return false;
        };
        std::env::split_paths(path).any(|dir| self.commands().iter().any(|c| has_command(&dir, c)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for agent in Agent::ALL {
            assert_eq!(Agent::from_id(agent.id()), Some(agent));
        }
        assert_eq!(Agent::from_id("vim"), None);
    }

    #[test]
    fn detects_by_home_folder_or_command_on_path() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        assert!(!Agent::Claude.detected(home.path(), None));

        std::fs::create_dir(home.path().join(".claude")).unwrap();
        assert!(Agent::Claude.detected(home.path(), None));

        let path = std::env::join_paths([bin.path()]).unwrap();
        assert!(!Agent::Antigravity.detected(home.path(), Some(&path)));
        std::fs::write(bin.path().join("agy"), "").unwrap();
        assert!(Agent::Antigravity.detected(home.path(), Some(&path)));
    }

    fn put(home: &Path, rel: &str, text: &str) {
        let file = home.join(rel);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    fn wired(home: &Path) -> Vec<Agent> {
        Agent::ALL.into_iter().filter(|a| a.knot_wired(home)).collect()
    }

    #[test]
    fn knot_wired_reads_each_agents_config() {
        let home = tempfile::tempdir().unwrap();
        assert!(wired(home.path()).is_empty());

        put(home.path(), ".cursor/mcp.json", r#"{"mcpServers":{"knot":{"command":"knot"}}}"#);
        assert_eq!(wired(home.path()), vec![Agent::Cursor]);

        put(home.path(), ".claude.json", r#"{"mcpServers":{"knot":{}}}"#);
        put(home.path(), ".gemini/settings.json", r#"{"mcpServers":{"knot":{}}}"#);
        put(home.path(), ".gemini/config/mcp_config.json", r#"{"mcpServers":{"knot":{}}}"#);
        put(home.path(), ".config/opencode/opencode.json", r#"{"mcp":{"knot":{}}}"#);
        put(home.path(), ".codex/config.toml", "[mcp_servers.knot]\ncommand = \"knot\"\n");
        assert_eq!(wired(home.path()), Agent::ALL.to_vec());
    }

    #[test]
    fn knot_wired_ignores_other_servers_and_broken_files() {
        let home = tempfile::tempdir().unwrap();
        put(home.path(), ".cursor/mcp.json", r#"{"mcpServers":{"other":{}}}"#);
        put(home.path(), ".claude.json", "{ not json");
        put(home.path(), ".config/opencode/opencode.json", r#"{"mcp":{"knot-ish":{}}}"#);
        put(home.path(), ".codex/config.toml", "[mcp_servers.knotty]\n# [mcp_servers.knot]\n");
        assert!(wired(home.path()).is_empty());
    }

    #[test]
    fn moved_config_folders_are_followed() {
        let home = Path::new("/home/maya");
        let env = |var: &str| match var {
            "CODEX_HOME" => Some("/work/codex".into()),
            "CLAUDE_CONFIG_DIR" => Some("/work/claude".into()),
            _ => Some("/never".into()),
        };

        assert_eq!(Agent::Codex.mcp_config_with(home, env), (PathBuf::from("/work/codex/config.toml"), "mcp_servers"));
        assert_eq!(Agent::Claude.mcp_config_with(home, env), (PathBuf::from("/work/claude/.claude.json"), "mcpServers"));
        assert_eq!(Agent::Cursor.mcp_config_with(home, env).0, PathBuf::from("/home/maya/.cursor/mcp.json"));
        let unset = |_: &str| Some(std::ffi::OsString::new());
        assert_eq!(Agent::Codex.mcp_config_with(home, unset).0, PathBuf::from("/home/maya/.codex/config.toml"), "empty is unset");
    }

    #[test]
    fn knot_command_reads_the_entry_each_agent_keeps() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(Agent::Cursor.knot_command(home.path()), None);

        put(home.path(), ".cursor/mcp.json", "{\n  // mine\n  \"mcpServers\": {\"knot\": {\"command\": \"/a/knot\"}}\n}");
        put(home.path(), ".config/opencode/opencode.json", r#"{"mcp":{"knot":{"type":"local","command":["/b/knot"]}}}"#);
        put(home.path(), ".codex/config.toml", "[other]\ncommand = \"x\"\n\n[mcp_servers.knot]\nargs = []\ncommand = \"C:\\\\knot.exe\"\n");

        assert_eq!(Agent::Cursor.knot_command(home.path()).as_deref(), Some("/a/knot"));
        assert!(Agent::Cursor.knot_wired(home.path()), "comments allowed");
        assert_eq!(Agent::OpenCode.knot_command(home.path()).as_deref(), Some("/b/knot"));
        assert_eq!(Agent::Codex.knot_command(home.path()).as_deref(), Some("C:\\knot.exe"));
    }

    #[test]
    fn a_folder_named_like_the_command_is_not_the_command() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        std::fs::create_dir(bin.path().join("codex")).unwrap();
        let path = std::env::join_paths([bin.path()]).unwrap();
        assert!(!Agent::Codex.detected(home.path(), Some(&path)));
    }
}
