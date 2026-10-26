use anyhow::{Context, Result};
use git2::{DiffOptions, Repository};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::engine::learnings::{scan_repo_learnings, RepoLearning};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FindingKind {
    Conflict,
    Warning,
    Follows,
}

impl FindingKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            FindingKind::Conflict => "CONFLICT",
            FindingKind::Warning => "WARNING",
            FindingKind::Follows => "FOLLOWS",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckFinding {
    pub kind: FindingKind,
    pub file: String,
    pub line: usize,
    pub learning_title: String,
    pub learning_file: String,
    pub evidence: String,
    pub diff_snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReport {
    pub ok: bool,
    pub checked_files: Vec<String>,
    pub findings: Vec<CheckFinding>,
}

fn extract_section<'a>(body: &'a str, heading: &str) -> Option<&'a str> {
    let lower_body = body.to_lowercase();
    let lower_heading = heading.to_lowercase();
    let idx = lower_body.find(&lower_heading)?;
    let start = idx + heading.len();
    let rest = &body[start..];

    let end = rest.find("\n## ").unwrap_or(rest.len());
    Some(rest[..end].trim())
}

fn extract_key_phrases(section_text: &str) -> Vec<String> {
    section_text
        .lines()
        .map(|l| l.trim().trim_start_matches('-').trim_start_matches('*').trim())
        .filter(|l| l.len() >= 4 && !l.starts_with('#'))
        .map(|l| l.to_lowercase())
        .collect()
}

fn is_stop_word(w: &str) -> bool {
    matches!(
        w,
        "the" | "and" | "that" | "this" | "with" | "from" | "under" | "caused" | "into" | "have" | "been" | "were"
    )
}

fn check_hunk_against_learning(
    file: &str,
    added_lines: &[(usize, String)],
    learning: &RepoLearning,
    findings: &mut Vec<CheckFinding>,
) {
    let ruled_out = extract_section(&learning.body, "## Tried and ruled out")
        .or_else(|| extract_section(&learning.body, "Tried and ruled out"));

    if let Some(ruled_out_text) = ruled_out {
        let phrases = extract_key_phrases(ruled_out_text);
        for (line_num, added_line) in added_lines {
            let lower_added = added_line.to_lowercase();
            for phrase in &phrases {
                let significant_words: Vec<&str> = phrase
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .filter(|w| w.len() >= 4 && !is_stop_word(w))
                    .collect();

                let matched_word = significant_words.iter().find(|w| lower_added.contains(**w));
                if let Some(w) = matched_word {
                    findings.push(CheckFinding {
                        kind: FindingKind::Conflict,
                        file: file.to_string(),
                        line: *line_num,
                        learning_title: learning.frontmatter.title.clone(),
                        learning_file: learning.file_path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                        evidence: format!("Ruled out: '{w}' from '{phrase}'"),
                        diff_snippet: added_line.trim().to_string(),
                    });
                    break;
                }
            }
        }
    }
}

pub fn check_git_diff(repo_root: &Path, base_ref: Option<&str>) -> Result<CheckReport> {
    let repo = Repository::discover(repo_root).context("Failed to open git repository")?;
    let learnings = scan_repo_learnings(repo_root)?;

    let mut diff_opts = DiffOptions::new();
    diff_opts.include_untracked(true);

    let diff = match base_ref {
        Some(base) => {
            let obj = repo.revparse_single(base)?;
            let tree = obj.peel_to_tree()?;
            repo.diff_tree_to_workdir_with_index(Some(&tree), Some(&mut diff_opts))?
        }
        None => {
            let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
            repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut diff_opts))?
        }
    };

    let mut checked_files = Vec::new();
    let mut findings = Vec::new();

    let num_deltas = diff.deltas().len();
    for i in 0..num_deltas {
        let delta = diff.get_delta(i).context("Invalid diff delta")?;
        let new_file = delta.new_file();
        let rel_path = new_file.path().unwrap_or(Path::new("")).to_string_lossy().replace('\\', "/");

        if rel_path.is_empty() {
            continue;
        }

        checked_files.push(rel_path.clone());

        let relevant_learnings: Vec<&RepoLearning> = learnings
            .iter()
            .filter(|l| {
                l.frontmatter.touches.is_empty()
                    || l.frontmatter.touches.iter().any(|t| t.starts_with(&rel_path) || rel_path.starts_with(t.split('#').next().unwrap_or("")))
            })
            .collect();

        if relevant_learnings.is_empty() {
            continue;
        }

        let mut added_lines = Vec::new();
        if let Ok(patch) = git2::Patch::from_diff(&diff, i) {
            if let Some(patch) = patch {
                for h in 0..patch.num_hunks() {
                    if let Ok((_hunk, num_lines)) = patch.hunk(h) {
                        for l in 0..num_lines {
                            if let Ok(line) = patch.line_in_hunk(h, l) {
                                if line.origin() == '+' {
                                    let content = String::from_utf8_lossy(line.content()).trim().to_string();
                                    let line_no = line.new_lineno().unwrap_or(1) as usize;
                                    added_lines.push((line_no, content));
                                }
                            }
                        }
                    }
                }
            }
        }

        for l in relevant_learnings {
            check_hunk_against_learning(&rel_path, &added_lines, l, &mut findings);
        }
    }

    let ok = !findings.iter().any(|f| f.kind == FindingKind::Conflict);

    Ok(CheckReport {
        ok,
        checked_files,
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::learnings::LearningFrontmatter;
    use std::path::PathBuf;
    use uuid::Uuid;

    #[test]
    fn test_conflict_detection_on_ruled_out_approach() {
        let learning = RepoLearning {
            frontmatter: LearningFrontmatter {
                id: Uuid::new_v4(),
                title: "Avoid raw spinlocks in async tasks".into(),
                author: "Rakib".into(),
                date: "2026-10-26".into(),
                touches: vec!["src/lock.rs".into()],
                tags: vec![],
                scope: "project".into(),
            },
            body: r#"
## Decided
Use tokio::sync::Mutex instead.

## Tried and ruled out
Spinlock in async code caused thread starvation under load.
"#
            .into(),
            file_path: PathBuf::from(".knot/learnings/2026_10_26_avoid_spinlocks.md"),
        };

        let added_lines = vec![
            (42, "let lock = Spinlock::new();".into()),
            (43, "// lock in async context".into()),
        ];

        let mut findings = Vec::new();
        check_hunk_against_learning("src/lock.rs", &added_lines, &learning, &mut findings);

        assert!(!findings.is_empty());
        assert!(findings.iter().any(|f| f.kind == FindingKind::Conflict));
        assert!(findings[0].evidence.contains("Ruled out"));
    }
}
