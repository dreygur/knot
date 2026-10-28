pub mod remote;

pub use remote::{
    changed_symbols, detect_remote_collisions, start_branch_monitor, symbol_clashes, BranchMonitorHandle,
    RemoteCollision, SymbolClash,
};

use anyhow::{Context, Result};
use git2::{DiffOptions, Repository};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::engine::learnings::{scan_repo_learnings, touch_path, RepoLearning};
use crate::utils::count;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FindingKind {
    Conflict,
    Warning,
    Follows,
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
    /// Short id of the learning, which a commit message cites to override it.
    #[serde(default)]
    pub learning_id: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub date: String,
    /// The learning's first decided line, the thing the change goes against.
    #[serde(default)]
    pub decision: String,
    /// For a pinned value: "defaultRetryMax from 4 to 10".
    #[serde(default)]
    pub change: Option<String>,
    /// The commit whose message overrides this learning, when one does.
    #[serde(default)]
    pub overridden_by: Option<String>,
    /// Short id of a learning added in this change that replaces this one. It retires
    /// the old learning only once merged, so the conflict still counts here.
    #[serde(default)]
    pub replaced_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReport {
    pub ok: bool,
    pub checked_files: Vec<String>,
    pub findings: Vec<CheckFinding>,
    #[serde(default)]
    pub learnings_checked: usize,
}

fn short_date(date: &str) -> String {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").map_or_else(|_| date.to_string(), |d| d.format("%b %-d").to_string())
}

/// The report as people read it. Text from learnings is untrusted repo content, so it
/// is flattened to single lines with no control characters.
pub fn format_report(report: &CheckReport) -> String {
    use crate::ask::flatten;
    let mut out = format!(
        "checking {} against {}\n",
        count(report.checked_files.len(), "changed file"),
        count(report.learnings_checked, "learning")
    );

    for f in &report.findings {
        let mark = match (f.kind, &f.overridden_by) {
            (_, Some(_)) => "↳",
            (FindingKind::Conflict, _) => "✗",
            _ => "!",
        };
        let what = match &f.change {
            Some(change) => format!("changes {}", flatten(change, 120)),
            None => flatten(&f.evidence, 160),
        };
        out.push_str(&format!("\n{mark} {}:{}  {what}\n", flatten(&f.file, 120), f.line));
        let decision = if f.decision.is_empty() { &f.learning_title } else { &f.decision };
        let who = if f.author.is_empty() {
            String::new()
        } else {
            format!("{} · {}: ", flatten(&f.author, 40), flatten(&short_date(&f.date), 20))
        };
        out.push_str(&format!("  {who}{}\n", flatten(decision, 220)));
        let id = if f.learning_id.is_empty() { String::new() } else { format!(" · knot {}", f.learning_id) };
        out.push_str(&format!("  .knot/learnings/{}{id}\n", flatten(&f.learning_file, 120)));
        if let Some(commit) = &f.overridden_by {
            out.push_str(&format!("  overridden in {commit}, which says why\n"));
        }
        if let Some(new) = &f.replaced_by {
            out.push_str(&format!("  replaced by knot {new} in this change; that counts once it is merged\n"));
        }
    }

    let conflicts = report.conflict_count();
    let warnings = report.findings.len() - conflicts;
    out.push('\n');
    if conflicts > 0 {
        let cite = report
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::Conflict)
            .map(|f| format!(" (\"Overrides knot {}: <why>\")", f.learning_id))
            .unwrap_or_default();
        out.push_str(&format!(
            "· {} · fix it, or say why in the commit message{cite}\n",
            count(conflicts, "conflict")
        ));
    } else {
        out.push_str(&format!("· no conflicts{}\n", if warnings > 0 { format!(" · {}", count(warnings, "note")) } else { String::new() }));
    }
    out
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

/// `NAME = VALUE` pairs a decision pins, e.g. "Keep `defaultRetryMax = 4`".
fn pinned_values(decided: &str) -> Vec<(String, String)> {
    static PIN: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"([A-Za-z_][A-Za-z0-9_]*)\s*:?=\s*([A-Za-z0-9_.\-]+)").unwrap()
    });
    PIN.captures_iter(decided)
        .filter(|c| !decided[c.get(0).unwrap().end()..].starts_with('='))
        .map(|c| (c[1].to_string(), c[2].trim_end_matches('.').to_string()))
        .collect()
}

/// The value a code line assigns to `name` (`name = 10`, `name := 10`, `const name: u8 = 10;`).
fn assigned_value(line: &str, name: &str) -> Option<String> {
    let at = line.match_indices(name).find(|(i, _)| {
        let before = line[..*i].chars().last();
        let after = line[i + name.len()..].chars().next();
        !before.is_some_and(|c| c.is_alphanumeric() || c == '_') && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
    })?;
    let rest = &line[at.0 + name.len()..];
    let (_, value) = rest.split_once('=')?;
    if value.starts_with('=') {
        return None;
    }
    // A trailing comment is not part of the value
    let value = ["//", "/*", " #"].iter().fold(value, |v, marker| v.split(marker).next().unwrap_or(v));
    let value = value.trim().trim_end_matches([';', ',']).trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string()
}

#[cfg(test)]
fn check_hunk_against_learning(
    file: &str,
    added_lines: &[(usize, String)],
    learning: &RepoLearning,
    findings: &mut Vec<CheckFinding>,
) {
    check_change_against_learning(file, added_lines, &[], learning, findings);
}

fn check_change_against_learning(
    file: &str,
    added_lines: &[(usize, String)],
    removed_lines: &[String],
    learning: &RepoLearning,
    findings: &mut Vec<CheckFinding>,
) {
    let decided = extract_section(&learning.body, "Decided").unwrap_or("");
    let finding = |kind, line: usize, evidence: String, snippet: &str, change: Option<String>| CheckFinding {
        kind,
        file: file.to_string(),
        line,
        learning_title: learning.frontmatter.title.clone(),
        learning_file: learning.file_path.file_name().unwrap_or_default().to_string_lossy().to_string(),
        evidence,
        diff_snippet: snippet.trim().to_string(),
        learning_id: learning.frontmatter.id.to_string()[..8].to_string(),
        author: learning.frontmatter.author.clone(),
        date: learning.frontmatter.date.clone(),
        decision: first_line(decided),
        change,
        overridden_by: None,
        replaced_by: None,
    };

    // A decision that pins a value is matched exactly: a different value is a conflict.
    let pins = pinned_values(decided);
    let mut pinned_lines = HashSet::new();
    for (line_num, added_line) in added_lines.iter().filter(|(_, l)| !is_comment_line(l)) {
        for (name, pinned) in &pins {
            let Some(new_value) = assigned_value(added_line, name) else { continue };
            if &new_value == pinned {
                continue;
            }
            let old_value = removed_lines
                .iter()
                .find_map(|r| assigned_value(r, name))
                .unwrap_or_else(|| pinned.clone());
            let change = format!("{name} from {old_value} to {new_value}");
            findings.push(finding(
                FindingKind::Conflict,
                *line_num,
                format!("Decided {name} = {pinned}"),
                added_line,
                Some(change),
            ));
            pinned_lines.insert(*line_num);
        }
    }

    let Some(ruled_out_text) = extract_section(&learning.body, "Tried and ruled out") else {
        return;
    };
    let phrases = extract_ruled_out_phrases(ruled_out_text);
    let title_words = significant_words(&learning.frontmatter.title);
    for (line_num, added_line) in added_lines {
        if is_comment_line(added_line) || pinned_lines.contains(line_num) {
            continue;
        }

        // Report the strongest match per line: a conflict beats a warning.
        let best = phrases
            .iter()
            .filter_map(|p| match_phrase(added_line, p, &title_words))
            .min_by_key(|(kind, _)| if *kind == FindingKind::Conflict { 0 } else { 1 });

        if let Some((kind, evidence)) = best {
            findings.push(finding(kind, *line_num, evidence, added_line, None));
        }
    }
}

/// Commit messages between `base` and HEAD, newest first, with their short ids.
fn commit_messages(repo: &Repository, base: &str) -> Vec<(String, String)> {
    let Ok(base_oid) = repo.revparse_single(base).and_then(|o| o.peel_to_commit()).map(|c| c.id()) else {
        return Vec::new();
    };
    let Ok(mut walk) = repo.revwalk() else { return Vec::new() };
    if walk.push_head().is_err() || walk.hide(base_oid).is_err() {
        return Vec::new();
    }
    walk.filter_map(|oid| oid.ok())
        .filter_map(|oid| repo.find_commit(oid).ok())
        .map(|c| (c.id().to_string()[..7].to_string(), c.message().unwrap_or("").to_string()))
        .collect()
}

/// A `touches` entry as a path relative to `repo_root`, which it may be written under.
fn relative_touch(touch: &str, repo_root: &Path) -> String {
    let path = touch_path(touch);
    let root = repo_root.to_string_lossy().replace('\\', "/");
    let rel = path.strip_prefix(&root).unwrap_or(&path);
    rel.trim_start_matches("./").trim_start_matches('/').to_string()
}

fn learning_applies(learning: &RepoLearning, rel_path: &str, repo_root: &Path) -> bool {
    let touches = &learning.frontmatter.touches;
    touches.is_empty()
        || touches.iter().any(|t| {
            let tp = relative_touch(t, repo_root);
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

    // A learning added in this same change records it; checking the change against it
    // would flag the decision the learning was written to explain.
    let paths_where = |keep: &dyn Fn(git2::Delta) -> bool| -> HashSet<std::path::PathBuf> {
        diff.deltas()
            .filter(|d| keep(d.status()))
            .filter_map(|d| d.new_file().path().map(|p| work_root.join(p)))
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
            .collect()
    };
    let added_here = paths_where(&|s| s == git2::Delta::Added || s == git2::Delta::Untracked);
    let changed_here = paths_where(&|_| true);
    let canonical = |l: &RepoLearning| std::fs::canonicalize(&l.file_path).unwrap_or_else(|_| l.file_path.clone());
    let (added, learnings): (Vec<RepoLearning>, Vec<RepoLearning>) =
        learnings.into_iter().partition(|l| added_here.contains(&canonical(l)));

    // A replacement only retires a learning once it is merged: otherwise a branch could
    // silence the check on itself with a note, or an edit to one, that replaces the
    // learning it breaks. So only learnings this change leaves alone can retire others.
    let retired: HashSet<uuid::Uuid> = learnings
        .iter()
        .filter(|l| !changed_here.contains(&canonical(l)))
        .filter_map(|l| l.frontmatter.replaces)
        .collect();
    let pending: HashMap<String, String> = added
        .iter()
        .chain(learnings.iter().filter(|l| changed_here.contains(&canonical(l))))
        .filter_map(|l| {
            let old = l.frontmatter.replaces?.to_string()[..8].to_string();
            Some((old, l.frontmatter.id.to_string()[..8].to_string()))
        })
        .collect();
    let learnings: Vec<RepoLearning> =
        learnings.into_iter().filter(|l| !retired.contains(&l.frontmatter.id)).collect();

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
        let mut removed_lines = Vec::new();
        if let Ok(Some(patch)) = git2::Patch::from_diff(&diff, i) {
            for h in 0..patch.num_hunks() {
                let Ok((_hunk, num_lines)) = patch.hunk(h) else { continue };
                for l in 0..num_lines {
                    let Ok(line) = patch.line_in_hunk(h, l) else { continue };
                    let content = String::from_utf8_lossy(line.content()).trim().to_string();
                    match line.origin() {
                        '+' => added_lines.push((line.new_lineno().unwrap_or(1) as usize, content)),
                        '-' => removed_lines.push(content),
                        _ => {}
                    }
                }
            }
        }

        for l in relevant_learnings {
            check_change_against_learning(&rel_path, &added_lines, &removed_lines, l, &mut findings);
        }
    }

    // "Overrides knot 3f2a9c1e" in a commit on the branch acknowledges that learning:
    // the conflict stays visible but no longer fails the check.
    if let Some(base) = base_ref {
        let messages = commit_messages(&repo, base);
        for f in findings.iter_mut().filter(|f| f.kind == FindingKind::Conflict && !f.learning_id.is_empty()) {
            let cited = format!("overrides knot {}", f.learning_id).to_lowercase();
            if let Some((commit, _)) = messages.iter().find(|(_, msg)| msg.to_lowercase().contains(&cited)) {
                f.kind = FindingKind::Warning;
                f.overridden_by = Some(commit.clone());
            }
        }
    }

    for f in &mut findings {
        f.replaced_by = pending.get(&f.learning_id).cloned();
    }

    let ok = !findings.iter().any(|f| f.kind == FindingKind::Conflict);

    Ok(CheckReport {
        ok,
        checked_files,
        findings,
        learnings_checked: learnings.len(),
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
                ..Default::default()
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

    fn retry_learning() -> RepoLearning {
        RepoLearning {
            frontmatter: LearningFrontmatter {
                title: "Keep the default retry count at 4".into(),
                author: "Maya".into(),
                date: "2026-10-09".into(),
                touches: vec!["client.go#defaultRetryMax".into()],
                ..Default::default()
            },
            body: "## Decided\nKeep `defaultRetryMax = 4`: ten retries with the default backoff can hold one Do() call for over an hour.\n"
                .into(),
            file_path: PathBuf::from(".knot/learnings/2026-10-09_keep.md"),
        }
    }

    #[test]
    fn pinned_values_and_assignments_are_read_from_prose_and_code() {
        assert_eq!(pinned_values("Keep `defaultRetryMax = 4`: ten retries"), vec![("defaultRetryMax".into(), "4".into())]);
        assert!(pinned_values("Compare with a == b only").is_empty());
        assert_eq!(assigned_value("\tdefaultRetryMax     = 10", "defaultRetryMax"), Some("10".into()));
        assert_eq!(assigned_value("const MAX_RETRY: u32 = 7;", "MAX_RETRY"), Some("7".into()));
        assert_eq!(assigned_value("if defaultRetryMax == 4 {", "defaultRetryMax"), None);
        assert_eq!(assigned_value("myDefaultRetryMax = 9", "defaultRetryMax"), None, "whole names only");
    }

    #[test]
    fn changing_a_pinned_value_is_a_conflict_with_from_and_to() {
        let mut findings = Vec::new();
        let added = vec![(51, "defaultRetryMax     = 10".to_string())];
        let removed = vec!["defaultRetryMax     = 4".to_string()];
        check_change_against_learning("client.go", &added, &removed, &retry_learning(), &mut findings);

        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert_eq!(f.kind, FindingKind::Conflict);
        assert_eq!(f.change.as_deref(), Some("defaultRetryMax from 4 to 10"));
        assert_eq!((f.author.as_str(), f.date.as_str()), ("Maya", "2026-10-09"));
        assert!(f.decision.starts_with("Keep `defaultRetryMax = 4`"), "{}", f.decision);

        let mut same = Vec::new();
        check_change_against_learning("client.go", &[(51, "defaultRetryMax = 4 // doc".into())], &[], &retry_learning(), &mut same);
        assert!(same.is_empty(), "keeping the pinned value is fine");
    }

    #[test]
    fn a_commit_citing_the_learning_overrides_the_conflict() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let repo = Repository::init(root)?;
        let sig = git2::Signature::now("Kumar", "k@example.com")?;
        let commit_all = |msg: &str| -> Result<()> {
            let mut index = repo.index()?;
            index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
            index.write()?;
            let tree = repo.find_tree(index.write_tree()?)?;
            let parents: Vec<git2::Commit> = repo.head().ok().and_then(|h| h.peel_to_commit().ok()).into_iter().collect();
            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parent_refs)?;
            Ok(())
        };

        let learning = retry_learning();
        std::fs::create_dir_all(root.join(".knot/learnings"))?;
        let id = learning.frontmatter.id.to_string();
        std::fs::write(
            root.join(".knot/learnings/2026-10-09_keep.md"),
            format!("---\nid: {id}\ntitle: \"{}\"\nauthor: \"Maya\"\ndate: \"2026-10-09\"\ntouches:\n  - \"client.go#defaultRetryMax\"\n---\n{}", learning.frontmatter.title, learning.body),
        )?;
        std::fs::write(root.join("client.go"), "package r\n\nconst (\n\tdefaultRetryMax = 4\n)\n")?;
        commit_all("base")?;
        let base = repo.head()?.peel_to_commit()?.id().to_string();

        std::fs::write(root.join("client.go"), "package r\n\nconst (\n\tdefaultRetryMax = 10\n)\n")?;
        commit_all("Raise retries")?;
        let report = check_git_diff(root, Some(&base))?;
        assert!(!report.ok);
        assert_eq!(report.findings[0].change.as_deref(), Some("defaultRetryMax from 4 to 10"));

        std::fs::write(root.join("client.go"), "package r\n\nconst (\n\tdefaultRetryMax = 11\n)\n")?;
        commit_all(&format!("Raise retries for the batch importer\n\nOverrides knot {}: imports run offline.", &id[..8]))?;
        let report = check_git_diff(root, Some(&base))?;
        assert!(report.ok, "{:?}", report.findings);
        assert!(report.findings[0].overridden_by.is_some());

        // A learning added on the branch records the branch's own change: it is not checked against it
        std::fs::write(
            root.join(".knot/learnings/2026-10-28_eleven.md"),
            "---\ntitle: \"Default retries are now 11\"\nauthor: \"Kumar\"\ntouches:\n  - \"client.go#defaultRetryMax\"\n---\n\
             ## Decided\n`defaultRetryMax = 11` for the importer.\n\n## Tried and ruled out\nRaising the default retry count to 10.\n",
        )?;
        commit_all("Record why")?;
        let report = check_git_diff(root, Some(&base))?;
        assert_eq!(report.learnings_checked, 1, "only the pre-existing learning is checked");
        assert!(report.findings.iter().all(|f| f.author == "Maya"), "{:?}", report.findings);
        Ok(())
    }

    #[test]
    fn a_replacement_retires_the_old_learning_only_once_merged() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let repo = Repository::init(root)?;
        let sig = git2::Signature::now("Kumar", "k@example.com")?;
        let commit_all = |msg: &str| -> Result<String> {
            let mut index = repo.index()?;
            index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
            index.write()?;
            let tree = repo.find_tree(index.write_tree()?)?;
            let parents: Vec<git2::Commit> = repo.head().ok().and_then(|h| h.peel_to_commit().ok()).into_iter().collect();
            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            Ok(repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parent_refs)?.to_string())
        };
        let set_retries = |n: u32| std::fs::write(root.join("client.go"), format!("package r\n\nconst (\n\tdefaultRetryMax = {n}\n)\n"));

        let maya = retry_learning();
        let maya_id = maya.frontmatter.id.to_string();
        std::fs::create_dir_all(root.join(".knot/learnings"))?;
        std::fs::write(
            root.join(".knot/learnings/2026-10-09_keep.md"),
            format!("---\nid: {maya_id}\ntitle: \"{}\"\nauthor: \"Maya\"\ndate: \"2026-10-09\"\ntouches:\n  - \"client.go#defaultRetryMax\"\n---\n{}", maya.frontmatter.title, maya.body),
        )?;
        set_retries(4)?;
        let base = commit_all("base")?;

        // On the branch, the replacement does not yet retire Maya's learning
        let kumar_id = uuid::Uuid::new_v4().to_string();
        std::fs::write(
            root.join(".knot/learnings/2026-10-28_ten.md"),
            format!("---\nid: {kumar_id}\ntitle: \"Default retries are 10\"\nauthor: \"Kumar\"\ndate: \"2026-10-28\"\nreplaces: {maya_id}\ntouches:\n  - \"client.go#defaultRetryMax\"\n---\n\
             ## Decided\nKeep `defaultRetryMax = 10`, because the importer runs offline.\n"),
        )?;
        set_retries(10)?;
        commit_all("Raise retries")?;
        let report = check_git_diff(root, Some(&base))?;
        assert!(!report.ok, "{:?}", report.findings);
        assert_eq!(report.findings[0].author, "Maya");
        assert_eq!(report.findings[0].replaced_by.as_deref(), Some(&kumar_id[..8]));

        // Once it is on the base branch, only Kumar's learning is checked
        let merged = repo.head()?.peel_to_commit()?.id().to_string();
        set_retries(12)?;
        let report = check_git_diff(root, Some(&merged))?;
        assert_eq!(report.learnings_checked, 1);
        assert!(!report.ok);
        assert!(report.findings.iter().all(|f| f.author == "Kumar"), "{:?}", report.findings);

        // Editing a merged learning to replace another does not retire it on that branch either
        set_retries(10)?;
        let tightened = std::fs::read_to_string(root.join(".knot/learnings/2026-10-28_ten.md"))?
            .replace(&format!("replaces: {maya_id}\n"), "");
        std::fs::write(root.join(".knot/learnings/2026-10-28_ten.md"), &tightened)?;
        commit_all("Drop the replacement")?;
        std::fs::write(
            root.join(".knot/learnings/2026-10-28_ten.md"),
            tightened.replace("date: \"2026-10-28\"\n", &format!("date: \"2026-10-28\"\nreplaces: {maya_id}\n")),
        )?;
        set_retries(9)?;
        let unreplaced = repo.head()?.peel_to_commit()?.id().to_string();
        let report = check_git_diff(root, Some(&unreplaced))?;
        assert!(report.findings.iter().any(|f| f.author == "Maya"), "{:?}", report.findings);
        Ok(())
    }
}
