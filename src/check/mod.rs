pub mod remote;

pub use remote::{
    detect_remote_collisions, start_branch_monitor, BranchMonitorHandle, RemoteCheckReport,
    RemoteCollision,
};

use anyhow::{Context, Result};
use git2::{DiffOptions, Repository};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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

/// Words too generic to identify a ruled-out approach on their own.
const STOP_WORDS: &[&str] = &[
    "the", "and", "that", "this", "with", "from", "under", "caused", "causes", "into", "have", "been",
    "were", "when", "then", "than", "also", "only", "because", "instead", "still", "even", "would",
    "could", "should", "does", "doesn", "didn", "about", "after", "before", "every", "using", "used",
    "uses", "make", "made", "some", "more", "most", "much", "many", "other", "they", "them", "their",
    "there", "these", "those", "which", "while", "where", "what", "will", "just", "like", "each",
    "code", "async", "await", "function", "method", "class", "struct", "const", "static", "return",
    "self", "true", "false", "none", "null", "string", "value", "values", "data", "file", "files",
    "line", "lines", "test", "tests", "error", "errors", "result", "type", "types", "call", "calls",
    "load", "time", "work", "works", "slow", "fast", "fails", "failed", "break", "breaks", "approach",
];

const MIN_WORD_LEN: usize = 4;
const MIN_WORDS_FOR_CONFLICT: usize = 2;

impl CheckReport {
    pub fn conflict_count(&self) -> usize {
        self.findings.iter().filter(|f| f.kind == FindingKind::Conflict).count()
    }
}

/// Return the text under a markdown heading, up to the next `## ` heading.
fn extract_section<'a>(body: &'a str, heading: &str) -> Option<&'a str> {
    let wanted = heading.trim_start_matches('#').trim().to_lowercase();
    let mut offset = 0;
    let mut start = None;

    for line in body.split_inclusive('\n') {
        let trimmed = line.trim();
        if let Some(start_idx) = start {
            if trimmed.starts_with("## ") {
                return Some(body[start_idx..offset].trim());
            }
        } else if trimmed.starts_with('#') && trimmed.trim_start_matches('#').trim().to_lowercase() == wanted {
            start = Some(offset + line.len());
        }
        offset += line.len();
    }

    start.map(|s| body[s..].trim())
}

/// A single ruled-out statement, split into the signals used to match diff lines.
struct RuledOutPhrase {
    text: String,
    code_terms: Vec<String>,
    words: Vec<String>,
}

fn normalize_word(w: &str) -> String {
    let lower = w.to_lowercase();
    match lower.strip_suffix('s') {
        Some(stem) if stem.len() >= MIN_WORD_LEN && !stem.ends_with('s') => stem.to_string(),
        _ => lower,
    }
}

fn split_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| !w.is_empty())
}

fn significant_words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = split_words(text)
        .filter(|w| w.len() >= MIN_WORD_LEN && !w.chars().all(|c| c.is_ascii_digit()))
        .map(normalize_word)
        .filter(|w| !STOP_WORDS.contains(&w.as_str()))
        .collect();
    words.sort();
    words.dedup();
    words
}

/// Backticked spans name the exact code that was ruled out, e.g. `Spinlock::new`.
fn backticked_terms(text: &str) -> Vec<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .map(|t| t.trim().to_lowercase())
        .filter(|t| t.len() >= 3)
        .collect()
}

fn extract_ruled_out_phrases(section_text: &str) -> Vec<RuledOutPhrase> {
    section_text
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '*']).trim())
        .filter(|l| l.len() >= MIN_WORD_LEN && !l.starts_with('#'))
        .map(|l| RuledOutPhrase {
            text: l.to_string(),
            code_terms: backticked_terms(l),
            words: significant_words(&l.replace('`', " ")),
        })
        .collect()
}

fn is_comment_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with('#') || t.starts_with("/*") || t.starts_with('*') || t.starts_with("--")
}

fn contains_term(lower_line: &str, term: &str) -> bool {
    let bytes = lower_line.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    lower_line.match_indices(term).any(|(idx, _)| {
        let before_ok = idx == 0 || !is_ident(bytes[idx - 1]) || !is_ident(term.as_bytes()[0]);
        let end = idx + term.len();
        let after_ok = end >= bytes.len() || !is_ident(bytes[end]) || !is_ident(term.as_bytes()[term.len() - 1]);
        before_ok && after_ok
    })
}

fn line_words(line: &str) -> HashSet<String> {
    let mut words = HashSet::new();
    for token in split_words(line) {
        // Split snake_case and CamelCase so `spin_lock` and `SpinLock` both expose `spin` and `lock`.
        words.insert(normalize_word(token));
        for part in token.split('_') {
            let mut current = String::new();
            for c in part.chars() {
                if c.is_uppercase() && !current.is_empty() {
                    words.insert(normalize_word(&current));
                    current.clear();
                }
                current.push(c);
            }
            if !current.is_empty() {
                words.insert(normalize_word(&current));
            }
        }
    }
    words
}

/// Match one added line against a ruled-out phrase.
///
/// A backticked term, two distinct significant words, or one word that also names the
/// learning's subject in its title is a conflict. Any other single word is only a warning.
fn match_phrase(
    added_line: &str,
    phrase: &RuledOutPhrase,
    title_words: &[String],
) -> Option<(FindingKind, String)> {
    let lower_added = added_line.to_lowercase();

    if let Some(term) = phrase.code_terms.iter().find(|t| contains_term(&lower_added, t)) {
        return Some((FindingKind::Conflict, format!("Ruled out: `{term}` from '{}'", phrase.text)));
    }

    let words = line_words(added_line);
    let matched: Vec<&str> = phrase
        .words
        .iter()
        .filter(|w| words.contains(*w))
        .map(|w| w.as_str())
        .collect();

    match matched.len() {
        0 => None,
        n if n >= MIN_WORDS_FOR_CONFLICT || title_words.iter().any(|w| w == matched[0]) => Some((
            FindingKind::Conflict,
            format!("Ruled out: {} from '{}'", matched.join(", "), phrase.text),
        )),
        _ => Some((
            FindingKind::Warning,
            format!("Mentions '{}' from ruled-out '{}'", matched[0], phrase.text),
        )),
    }
}

fn check_hunk_against_learning(
    file: &str,
    added_lines: &[(usize, String)],
    learning: &RepoLearning,
    findings: &mut Vec<CheckFinding>,
) {
    let Some(ruled_out_text) = extract_section(&learning.body, "Tried and ruled out") else {
        return;
    };

    let phrases = extract_ruled_out_phrases(ruled_out_text);
    let title_words = significant_words(&learning.frontmatter.title);
    for (line_num, added_line) in added_lines {
        if is_comment_line(added_line) {
            continue;
        }

        // Report the strongest match per line: a conflict beats a warning.
        let best = phrases
            .iter()
            .filter_map(|p| match_phrase(added_line, p, &title_words))
            .min_by_key(|(kind, _)| if *kind == FindingKind::Conflict { 0 } else { 1 });

        if let Some((kind, evidence)) = best {
            findings.push(CheckFinding {
                kind,
                file: file.to_string(),
                line: *line_num,
                learning_title: learning.frontmatter.title.clone(),
                learning_file: learning.file_path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                evidence,
                diff_snippet: added_line.trim().to_string(),
            });
        }
    }
}

/// Normalize a `touches` entry to a repo-relative path without its symbol suffix.
fn touch_path(touch: &str, repo_root: &Path) -> String {
    let path = touch.split('#').next().unwrap_or("");
    let path = match path.find("::") {
        Some(idx) if path[..idx].contains('.') => &path[..idx],
        _ => path,
    };
    let path = path.replace('\\', "/");
    let root = repo_root.to_string_lossy().replace('\\', "/");
    let rel = path.strip_prefix(&root).unwrap_or(&path);
    rel.trim_start_matches("./").trim_start_matches('/').to_string()
}

fn learning_applies(learning: &RepoLearning, rel_path: &str, repo_root: &Path) -> bool {
    let touches = &learning.frontmatter.touches;
    touches.is_empty()
        || touches.iter().any(|t| {
            let tp = touch_path(t, repo_root);
            !tp.is_empty()
                && (rel_path == tp || rel_path.starts_with(&format!("{}/", tp.trim_end_matches('/'))))
        })
}

pub fn check_git_diff(repo_root: &Path, base_ref: Option<&str>) -> Result<CheckReport> {
    let repo = Repository::discover(repo_root).context("Failed to open git repository")?;
    // Diff paths are relative to the work tree, so learnings and touches must be too.
    let work_root = repo.workdir().unwrap_or(repo_root).to_path_buf();
    let learnings = scan_repo_learnings(&work_root)?;

    let mut diff_opts = DiffOptions::new();
    diff_opts
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);

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
            .filter(|l| learning_applies(l, &rel_path, &work_root))
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

    fn spinlock_learning(ruled_out: &str) -> RepoLearning {
        RepoLearning {
            frontmatter: LearningFrontmatter {
                id: Uuid::new_v4(),
                title: "Avoid raw spinlocks in async tasks".into(),
                author: "Rakib".into(),
                date: "2026-10-26".into(),
                touches: vec!["src/lock.rs".into()],
                tags: vec![],
                scope: "project".into(),
            },
            body: format!("\n## Decided\nUse tokio::sync::Mutex instead.\n\n## Tried and ruled out\n{ruled_out}\n"),
            file_path: PathBuf::from(".knot/learnings/2026_10_26_avoid_spinlocks.md"),
        }
    }

    fn run(learning: &RepoLearning, lines: &[&str]) -> Vec<CheckFinding> {
        let added: Vec<(usize, String)> = lines.iter().enumerate().map(|(i, l)| (i + 1, l.to_string())).collect();
        let mut findings = Vec::new();
        check_hunk_against_learning("src/lock.rs", &added, learning, &mut findings);
        findings
    }

    #[test]
    fn test_conflict_detection_on_ruled_out_approach() {
        let learning = spinlock_learning("Spinlock in async code caused thread starvation under load.");
        let findings = run(&learning, &["let lock = Spinlock::new();", "// lock in async context"]);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::Conflict);
        assert_eq!(findings[0].line, 1);
        assert!(findings[0].evidence.contains("Ruled out"));
    }

    #[test]
    fn test_common_words_do_not_conflict() {
        let learning = spinlock_learning("Spinlock in async code caused thread starvation under load.");
        let findings = run(
            &learning,
            &["let cfg = reload_config();", "async fn read_code() {}", "// spinlock removed"],
        );

        assert!(findings.is_empty(), "unexpected findings: {findings:?}");
    }

    #[test]
    fn test_backticked_term_conflicts() {
        let learning = spinlock_learning("- `parking_lot::Mutex` held across .await deadlocked the pool.");
        let findings = run(&learning, &["use parking_lot::Mutex;", "let m = tokio::sync::Mutex::new(0);"]);

        let conflicts: Vec<_> = findings.iter().filter(|f| f.kind == FindingKind::Conflict).collect();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].line, 1);
        // A bare `Mutex` on its own only shares one word with the phrase.
        assert!(findings.iter().all(|f| f.line == 1 || f.kind == FindingKind::Warning));
    }

    #[test]
    fn test_single_unrelated_word_is_warning() {
        let learning = spinlock_learning("Busy polling caused thread starvation.");
        let findings = run(&learning, &["let pool = ThreadPool::new(4);"]);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::Warning);
    }

    #[test]
    fn test_two_words_conflict() {
        let learning = spinlock_learning("Busy polling caused thread starvation.");
        let findings = run(&learning, &["loop { busy_poll(); } // polling"]);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::Conflict);
    }

    #[test]
    fn test_extract_section_stops_at_next_heading() {
        let body = "## Decided\nA\n\n## Tried and ruled out\nB\n\n## Watch out\nC\n";
        assert_eq!(extract_section(body, "## Tried and ruled out"), Some("B"));
        assert_eq!(extract_section("## Ünïcode\nx", "missing"), None);
    }

    #[test]
    fn test_touches_match_absolute_and_symbol_paths() {
        let root = Path::new("/repo");
        let mut learning = spinlock_learning("x");
        learning.frontmatter.touches = vec!["/repo/src/lock.rs#acquire".into()];
        assert!(learning_applies(&learning, "src/lock.rs", root));
        assert!(!learning_applies(&learning, "src/lock_other.rs", root));

        learning.frontmatter.touches = vec!["src/engine/".into()];
        assert!(learning_applies(&learning, "src/engine/graph.rs", root));
        assert!(!learning_applies(&learning, "src/engineering.rs", root));

        learning.frontmatter.touches = vec!["src/auth.rs::validate_jwt".into()];
        assert!(learning_applies(&learning, "src/auth.rs", root));
    }
}
