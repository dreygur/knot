mod agent;

pub use agent::{Agent, Target};

use agent::{ANTIGRAVITY_SKILL, CURSOR_RULE};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// The one full text of Knot's instructions. The plugin skill, the Antigravity skill
/// and the block in every agent's instruction file all come from it, so a rule is
/// written once and the copies never drift.
const SKILL_MD: &str = include_str!("../../skills/knot/SKILL.md");
const START_MARKER: &str = "<!-- knot:start -->";
const END_MARKER: &str = "<!-- knot:end -->";

/// The skill without its frontmatter: what an instruction file needs.
fn skill_body() -> &'static str {
    SKILL_MD
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(SKILL_MD, |(_, body)| body)
        .trim()
}

/// The skill body between the markers a re-run finds and replaces.
fn protocol_block() -> String {
    format!("{START_MARKER}\n{}\n{END_MARKER}", skill_body())
}

fn upsert_block_into_file(path: &Path, block: &str) -> Result<bool> {
    let existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let updated = if let (Some(s), Some(e)) = (existing.find(START_MARKER), existing.find(END_MARKER)) {
        if s < e {
            let after_end = e + END_MARKER.len();
            format!("{}{}{}", &existing[..s], block.trim(), &existing[after_end..])
        } else {
            format!("{existing}\n{block}\n")
        }
    } else {
        format!("{existing}\n{block}\n")
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, updated.trim_start())?;
    Ok(true)
}

pub fn install_cursor_rule(repo_root: &Path) -> Result<PathBuf> {
    let target = repo_root.join(".cursor").join("rules").join("knot.mdc");
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&target, protocol_block())?;
    Ok(target)
}

pub fn install_antigravity_skill(home_dir: &Path) -> Result<PathBuf> {
    let target = home_dir
        .join(".gemini")
        .join("skills")
        .join("knot")
        .join("SKILL.md");

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&target, SKILL_MD)?;
    Ok(target)
}

fn write_target(target: Target, repo_root: &Path, home: &Path) -> Result<()> {
    match target {
        CURSOR_RULE => install_cursor_rule(repo_root).map(drop),
        ANTIGRAVITY_SKILL => install_antigravity_skill(home).map(drop),
        Target::Repo(path) => upsert_block_into_file(&repo_root.join(path), &protocol_block()).map(drop),
        Target::Home(path) => anyhow::bail!("no writer for ~/{path}"),
    }
}

/// Write Knot's instructions for `agents` and nothing else. A file two of them read
/// (AGENTS.md for Codex and OpenCode) is written once.
pub fn configure_agents(repo_root: &Path, home: &Path, agents: &[Agent]) -> Result<()> {
    let mut written = std::collections::HashSet::new();
    for target in agents.iter().flat_map(|a| a.targets()) {
        if written.insert(*target) {
            write_target(*target, repo_root, home)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// The Knot text between the markers in `content`.
    fn block_in(content: &str) -> &str {
        let start = content.find(START_MARKER).expect("a start marker") + START_MARKER.len();
        let end = content.find(END_MARKER).expect("an end marker");
        content[start..end].trim()
    }

    #[test]
    fn writes_only_the_picked_agents_files() -> Result<()> {
        let repo = tempdir()?;
        let home = tempdir()?;
        configure_agents(repo.path(), home.path(), &[Agent::Claude, Agent::Codex])?;

        assert!(repo.path().join("CLAUDE.md").is_file());
        assert!(repo.path().join("AGENTS.md").is_file());
        assert!(!repo.path().join("GEMINI.md").exists());
        assert!(!repo.path().join(".cursor").exists());
        assert!(!home.path().join(".gemini").exists());

        let content = fs::read_to_string(repo.path().join("AGENTS.md"))?;
        assert!(content.contains("<!-- knot:start -->"));
        assert!(content.contains("knot callers"));
        assert!(content.contains("Never take a package version from memory"), "{content}");
        Ok(())
    }

    #[test]
    fn every_agent_gets_the_same_text_the_skill_has() -> Result<()> {
        let repo = tempdir()?;
        let home = tempdir()?;
        configure_agents(repo.path(), home.path(), &Agent::ALL)?;

        let body = skill_body();
        assert!(body.starts_with("# Knot") && !body.contains("name: knot"), "frontmatter left out: {body}");
        for file in ["CLAUDE.md", "AGENTS.md", "GEMINI.md", ".cursor/rules/knot.mdc"] {
            let content = fs::read_to_string(repo.path().join(file))?;
            assert_eq!(block_in(&content), body, "{file}");
        }
        assert_eq!(fs::read_to_string(home.path().join(".gemini/skills/knot/SKILL.md"))?, SKILL_MD);
        Ok(())
    }

    #[test]
    fn rerun_updates_the_block_in_place_and_keeps_other_text() -> Result<()> {
        let repo = tempdir()?;
        let home = tempdir()?;
        fs::write(repo.path().join("AGENTS.md"), "# House rules\n")?;
        configure_agents(repo.path(), home.path(), &[Agent::Codex, Agent::OpenCode])?;
        configure_agents(repo.path(), home.path(), &[Agent::Codex])?;

        let content = fs::read_to_string(repo.path().join("AGENTS.md"))?;
        assert_eq!(content.matches("<!-- knot:start -->").count(), 1);
        assert!(content.starts_with("# House rules"));
        Ok(())
    }

    #[test]
    fn an_old_block_is_replaced_not_doubled() -> Result<()> {
        let repo = tempdir()?;
        let home = tempdir()?;
        let old = "# House rules\n\n<!-- knot:start -->\n## Knot: code graph and team learnings\n\nFind code before opening files.\n<!-- knot:end -->\n\n## Footer\n";
        fs::write(repo.path().join("CLAUDE.md"), old)?;

        configure_agents(repo.path(), home.path(), &[Agent::Claude])?;

        let content = fs::read_to_string(repo.path().join("CLAUDE.md"))?;
        assert_eq!(content.matches(START_MARKER).count(), 1, "{content}");
        assert_eq!(content.matches(END_MARKER).count(), 1, "{content}");
        assert!(!content.contains("Find code before opening files."), "{content}");
        assert_eq!(block_in(&content), skill_body());
        assert!(content.starts_with("# House rules") && content.contains("## Footer"), "{content}");
        Ok(())
    }

    #[test]
    fn antigravity_also_gets_its_machine_wide_skill() -> Result<()> {
        let repo = tempdir()?;
        let home = tempdir()?;
        configure_agents(repo.path(), home.path(), &[Agent::Antigravity, Agent::Cursor])?;

        assert!(repo.path().join("GEMINI.md").is_file());
        assert!(repo.path().join(".cursor/rules/knot.mdc").is_file());
        let skill = fs::read_to_string(home.path().join(".gemini/skills/knot/SKILL.md"))?;
        assert!(skill.contains("Never take a package version from memory"), "{skill}");
        Ok(())
    }
}
