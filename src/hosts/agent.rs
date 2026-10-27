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

    #[test]
    fn a_folder_named_like_the_command_is_not_the_command() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        std::fs::create_dir(bin.path().join("codex")).unwrap();
        let path = std::env::join_paths([bin.path()]).unwrap();
        assert!(!Agent::Codex.detected(home.path(), Some(&path)));
    }
}
