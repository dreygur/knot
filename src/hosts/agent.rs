use std::ffi::OsStr;
use std::path::Path;

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
    pub fn mcp_config(self) -> (&'static str, &'static str) {
        match self {
            Agent::Claude => (".claude.json", "mcpServers"),
            Agent::Codex => (".codex/config.toml", "mcp_servers"),
            Agent::Gemini => (".gemini/settings.json", "mcpServers"),
            Agent::Antigravity => (".gemini/config/mcp_config.json", "mcpServers"),
            Agent::Cursor => (".cursor/mcp.json", "mcpServers"),
            Agent::OpenCode => (".config/opencode/opencode.json", "mcp"),
        }
    }

    /// Whether this agent's user config under `home` has a `knot` MCP entry. A missing or
    /// unreadable file counts as not wired.
    pub fn knot_wired(self, home: &Path) -> bool {
        let (file, key) = self.mcp_config();
        if self == Agent::Codex {
            return toml_has_knot(&home.join(file));
        }
        let Ok(text) = std::fs::read_to_string(home.join(file)) else { return false };
        serde_json::from_str::<serde_json::Value>(&text).is_ok_and(|json| json.get(key).and_then(|s| s.get("knot")).is_some())
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
    fn a_folder_named_like_the_command_is_not_the_command() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        std::fs::create_dir(bin.path().join("codex")).unwrap();
        let path = std::env::join_paths([bin.path()]).unwrap();
        assert!(!Agent::Codex.detected(home.path(), Some(&path)));
    }
}
