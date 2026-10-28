mod agent;
mod jsonc;
pub mod mcp;

pub use agent::{Agent, Target};

use agent::{ANTIGRAVITY_SKILL, CURSOR_RULE};
use crate::utils::{is_symlink, refuse_symlink, write_atomic};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// The one full text of Knot's instructions. The plugin skill, the Antigravity skill
/// and the block in every agent's instruction file all come from it, so a rule is
/// written once and the copies never drift.
const SKILL_MD: &str = include_str!("../../skills/knot/SKILL.md");
const START_MARKER: &str = "<!-- knot:start -->";
const END_MARKER: &str = "<!-- knot:end -->";
/// Without `alwaysApply`, Cursor treats a rule as one the agent may pick or skip.
const CURSOR_FRONTMATTER: &str = "---\ndescription: Knot code graph and team learnings\nalwaysApply: true\n---";

/// The skill without its frontmatter: what an instruction file needs.
fn skill_body() -> &'static str {
    SKILL_MD
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(SKILL_MD, |(_, body)| body)
        .trim()
}

/// The skill body one heading level down, so the block sits under the H1 of the file it
/// lands in. SKILL.md has no code fences, so every line that opens with `#` is a heading.
fn block_body() -> String {
    skill_body().lines().map(|l| if l.starts_with('#') { format!("#{l}") } else { l.to_string() }).collect::<Vec<_>>().join("\n")
}

/// The skill body between the markers a re-run finds and replaces.
fn protocol_block() -> String {
    format!("{START_MARKER}\n{}\n{END_MARKER}", block_body())
}

/// The file to write for the instruction file at `path`. A cloned repo can commit it as
/// a link, so a link is followed only to a file inside `repo_root` (`CLAUDE.md ->
/// AGENTS.md`); one that points out of the repo or at nothing is refused.
fn repo_file(repo_root: &Path, path: &Path) -> Result<PathBuf> {
    if !is_symlink(path) {
        return Ok(path.to_path_buf());
    }
    let Ok(real) = path.canonicalize() else {
        anyhow::bail!("{} is a link to nothing; Knot does not write through it", path.display());
    };
    if !real.starts_with(repo_root.canonicalize()?) {
        anyhow::bail!("{} links outside the repo; Knot does not write through it", path.display());
    }
    Ok(real)
}

fn upsert_block_into_file(repo_root: &Path, path: &Path, block: &str) -> Result<()> {
    let path = repo_file(repo_root, path)?;
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => anyhow::bail!("cannot read {}: {e}", path.display()),
    };

    let updated = crate::utils::upsert_block(&existing, block, START_MARKER, END_MARKER);
    write_atomic(&path, updated.trim_start())
}

pub fn install_cursor_rule(repo_root: &Path) -> Result<PathBuf> {
    let rules = repo_root.join(".cursor").join("rules");
    let target = rules.join("knot.mdc");
    // A linked folder on the way would carry the rule out of the repo
    refuse_symlink(&repo_root.join(".cursor"))?;
    refuse_symlink(&rules)?;
    refuse_symlink(&target)?;
    fs::create_dir_all(&rules)?;
    // The rule is Knot's own file, so it is rewritten whole: one frontmatter, one block
    write_atomic(&target, &format!("{CURSOR_FRONTMATTER}\n\n{}\n", protocol_block()))?;
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
    write_atomic(&target, SKILL_MD)?;
    Ok(target)
}

fn write_target(target: Target, repo_root: &Path, home: &Path) -> Result<()> {
    match target {
        CURSOR_RULE => install_cursor_rule(repo_root).map(drop),
        ANTIGRAVITY_SKILL => install_antigravity_skill(home).map(drop),
        Target::Repo(path) => upsert_block_into_file(repo_root, &repo_root.join(path), &protocol_block()),
        Target::Home(path) => anyhow::bail!("no writer for ~/{path}"),
    }
}

/// Write Knot's instructions for `agents` and nothing else. A file two of them read
/// (AGENTS.md for Codex and OpenCode) is written once. Returns the files Knot refused or
/// failed to write, with why; one bad file never stops the others.
pub fn configure_agents(repo_root: &Path, home: &Path, agents: &[Agent]) -> Vec<(Target, String)> {
    let mut written = std::collections::HashSet::new();
    let mut refused = Vec::new();
    for target in agents.iter().flat_map(|a| a.targets()) {
        if !written.insert(*target) {
            continue;
        }
        if let Err(e) = write_target(*target, repo_root, home) {
            refused.push((*target, e.to_string()));
        }
    }
    refused
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// [`configure_agents`] in a test that expects every file written.
    fn configure(repo: &Path, home: &Path, agents: &[Agent]) -> Result<()> {
        let refused = configure_agents(repo, home, agents);
        anyhow::ensure!(refused.is_empty(), "refused: {refused:?}");
        Ok(())
    }

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
        configure(repo.path(),home.path(), &[Agent::Claude, Agent::Codex])?;

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
        configure(repo.path(),home.path(), &Agent::ALL)?;

        let body = skill_body();
        assert!(body.starts_with("# Knot") && !body.contains("name: knot"), "frontmatter left out: {body}");
        for file in ["CLAUDE.md", "AGENTS.md", "GEMINI.md", ".cursor/rules/knot.mdc"] {
            let content = fs::read_to_string(repo.path().join(file))?;
            assert_eq!(block_in(&content), block_body(), "{file}");
            assert!(block_in(&content).starts_with("## Knot\n") && content.contains("\n### Which tool\n"), "{file}");
        }
        assert_eq!(fs::read_to_string(home.path().join(".gemini/skills/knot/SKILL.md"))?, SKILL_MD);
        Ok(())
    }

    #[test]
    fn rerun_updates_the_block_in_place_and_keeps_other_text() -> Result<()> {
        let repo = tempdir()?;
        let home = tempdir()?;
        fs::write(repo.path().join("AGENTS.md"), "# House rules\n")?;
        configure(repo.path(),home.path(), &[Agent::Codex, Agent::OpenCode])?;
        configure(repo.path(),home.path(), &[Agent::Codex])?;

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

        configure(repo.path(),home.path(), &[Agent::Claude])?;

        let content = fs::read_to_string(repo.path().join("CLAUDE.md"))?;
        assert_eq!(content.matches(START_MARKER).count(), 1, "{content}");
        assert_eq!(content.matches(END_MARKER).count(), 1, "{content}");
        assert!(!content.contains("Find code before opening files."), "{content}");
        assert_eq!(block_in(&content), block_body());
        assert!(content.starts_with("# House rules") && content.contains("## Footer"), "{content}");
        Ok(())
    }

    #[test]
    fn the_cursor_rule_always_applies_and_a_rerun_keeps_one_of_each() -> Result<()> {
        let repo = tempdir()?;
        let home = tempdir()?;
        configure(repo.path(),home.path(), &[Agent::Cursor])?;
        configure(repo.path(),home.path(), &[Agent::Cursor])?;

        let rule = fs::read_to_string(repo.path().join(".cursor/rules/knot.mdc"))?;
        assert!(rule.starts_with("---\ndescription: Knot code graph and team learnings\nalwaysApply: true\n---\n"), "{rule}");
        assert_eq!(rule.matches("alwaysApply").count(), 1, "{rule}");
        assert_eq!(rule.matches(START_MARKER).count(), 1, "{rule}");
        assert_eq!(block_in(&rule), block_body());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_antigravity_skill_is_not_written_through() -> Result<()> {
        let home = tempdir()?;
        let outside = tempdir()?;
        let victim = outside.path().join("victim");
        fs::write(&victim, "keep me")?;
        let skill = home.path().join(".gemini/skills/knot/SKILL.md");
        fs::create_dir_all(skill.parent().unwrap())?;
        std::os::unix::fs::symlink(&victim, &skill)?;

        let err = install_antigravity_skill(home.path()).unwrap_err().to_string();

        assert!(err.contains("symlink"), "{err}");
        assert_eq!(fs::read_to_string(&victim)?, "keep me");
        Ok(())
    }

    #[test]
    fn antigravity_also_gets_its_machine_wide_skill() -> Result<()> {
        let repo = tempdir()?;
        let home = tempdir()?;
        configure(repo.path(),home.path(), &[Agent::Antigravity, Agent::Cursor])?;

        assert!(repo.path().join("GEMINI.md").is_file());
        assert!(repo.path().join(".cursor/rules/knot.mdc").is_file());
        let skill = fs::read_to_string(home.path().join(".gemini/skills/knot/SKILL.md"))?;
        assert!(skill.contains("Never take a package version from memory"), "{skill}");
        Ok(())
    }
}
