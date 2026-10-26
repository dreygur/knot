use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

const KNOT_PROTOCOL_BLOCK: &str = r#"
<!-- knot:start -->
## Knot: Code Graph & Memory Vault

Before starting any task, check for prior context and callers:
- Call `recall_memory` to check for prior context, decisions, and lessons.
- Run `knot callers <symbol>` or call `knot_trace_calls` to see what depends on a function before changing it.
- Run `knot blast <symbol>` before refactoring to discover the full blast radius.
- Run `knot skeleton <file>` or call `knot_file_api` to skim signatures (~10x token savings vs reading the full file).
- Run `knot check` before submitting a commit or PR to verify diffs against recorded team learnings.
- After completing non-trivial tasks or fixing bugs, call `save_wisdom` with `verification_path="file#symbol"` to persist the lesson.
- Call `commit_session` to promote verified session wisdom into the project permanently.
<!-- knot:end -->
"#;

const ANTIGRAVITY_SKILL_MD: &str = r#"---
name: knot
description: Codebase context graph, callers, blast radius, and verified memory pool for coding agents.
version: 1.0.0
---

# Knot Code Graph and Memory

Use Knot to explore code structure and retrieve verified memories:
- Query callers: `knot callers <symbol>`
- Query callees: `knot callees <symbol>`
- Analyze blast radius: `knot blast <symbol> --depth 3`
- Inspect file skeleton: `knot skeleton <file>`
- Audit changes before commit: `knot check`
- Recall prior learnings: `recall_memory(query="...")`
- Persist verified learnings: `save_wisdom(content="...", verification_path="file#symbol")`
"#;

fn upsert_block_into_file(path: &Path, block: &str) -> Result<bool> {
    let existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let start_marker = "<!-- knot:start -->";
    let end_marker = "<!-- knot:end -->";

    let updated = if let (Some(s), Some(e)) = (existing.find(start_marker), existing.find(end_marker)) {
        if s < e {
            let after_end = e + end_marker.len();
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
    fs::write(&target, KNOT_PROTOCOL_BLOCK.trim())?;
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
    fs::write(&target, ANTIGRAVITY_SKILL_MD.trim())?;
    Ok(target)
}

pub fn configure_agent_hosts(repo_root: &Path) -> Result<Vec<String>> {
    let mut configured = Vec::new();

    // 1. AGENTS.md (Codex, OpenCode, universal CLIs)
    let agents_file = repo_root.join("AGENTS.md");
    if upsert_block_into_file(&agents_file, KNOT_PROTOCOL_BLOCK)? {
        configured.push("AGENTS.md".into());
    }

    // 2. GEMINI.md (Antigravity IDE)
    let gemini_file = repo_root.join("GEMINI.md");
    if upsert_block_into_file(&gemini_file, KNOT_PROTOCOL_BLOCK)? {
        configured.push("GEMINI.md".into());
    }

    // 3. CLAUDE.md (Claude Code)
    let claude_file = repo_root.join("CLAUDE.md");
    if upsert_block_into_file(&claude_file, KNOT_PROTOCOL_BLOCK)? {
        configured.push("CLAUDE.md".into());
    }

    // 4. Cursor rule (.cursor/rules/knot.mdc)
    if let Ok(_) = install_cursor_rule(repo_root) {
        configured.push(".cursor/rules/knot.mdc".into());
    }

    // 5. Global Antigravity skill
    if let Some(home) = dirs::home_dir() {
        if let Ok(_) = install_antigravity_skill(&home) {
            configured.push("~/.gemini/skills/knot/SKILL.md".into());
        }
    }

    Ok(configured)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_host_protocol_injection() -> Result<()> {
        let dir = tempdir()?;
        let configured = configure_agent_hosts(dir.path())?;
        assert!(configured.contains(&"AGENTS.md".to_string()));
        assert!(configured.contains(&"GEMINI.md".to_string()));
        assert!(configured.contains(&"CLAUDE.md".to_string()));

        let content = fs::read_to_string(dir.path().join("AGENTS.md"))?;
        assert!(content.contains("<!-- knot:start -->"));
        assert!(content.contains("knot callers"));

        // Idempotency check: running again updates in place
        let reconfigured = configure_agent_hosts(dir.path())?;
        assert!(reconfigured.contains(&"AGENTS.md".to_string()));
        let content2 = fs::read_to_string(dir.path().join("AGENTS.md"))?;
        assert_eq!(content2.matches("<!-- knot:start -->").count(), 1);

        Ok(())
    }
}
