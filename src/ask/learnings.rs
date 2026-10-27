//! Repo learnings that bear on a question, shown above the code that answers it.
//!
//! A learning is related when it `touches` a file among the code hits, or when the
//! question shares words with its title and body. Learnings are committed by anyone
//! who can push, so their text is flattened to one short line before it is shown.

use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

use super::{tokenize, AskHit};
use crate::engine::learnings::{scan_repo_learnings, RepoLearning};

const MAX_LEARNINGS: usize = 3;
const MAX_SUMMARY_CHARS: usize = 240;
/// Without a touched file, a learning needs this many distinct question words, at
/// least one of them in its title, so one shared common word does not pull it in.
const MIN_SHARED_WORDS: usize = 2;
const TITLE_WEIGHT: f64 = 0.5;
const BODY_WEIGHT: f64 = 0.2;

#[derive(Debug, Clone, Serialize)]
pub struct LearningHit {
    pub title: String,
    /// The learning file, relative to the repo root.
    pub file: String,
    /// Its decision in one line (the first line under "## Decided", else the first line).
    pub summary: String,
    /// Hit files this learning lists in `touches`.
    pub touched: Vec<String>,
    pub score: f64,
}

/// One line with no control characters, cut to `max` characters.
pub fn flatten(text: &str, max: usize) -> String {
    let flat = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match flat.char_indices().nth(max) {
        Some((cut, _)) => format!("{}...", &flat[..cut]),
        None => flat,
    }
}

fn summary_of(body: &str) -> String {
    let mut lines = body.lines().map(str::trim);
    let decided = lines
        .by_ref()
        .skip_while(|l| !l.eq_ignore_ascii_case("## decided"))
        .skip(1)
        .find(|l| !l.is_empty() && !l.starts_with('#'));
    let first = || body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'));
    flatten(decided.or_else(first).unwrap_or(""), MAX_SUMMARY_CHARS)
}

fn touch_path(touch: &str) -> &str {
    let path = touch.split_once('#').map_or(touch, |(p, _)| p);
    path.strip_prefix("./").unwrap_or(path)
}

fn score(learning: &RepoLearning, query: &HashSet<String>, hits: &[AskHit]) -> Option<(f64, Vec<String>)> {
    let mut touched = Vec::new();
    let mut touch_score = 0.0;
    for touch in &learning.frontmatter.touches {
        let path = touch_path(touch);
        if let Some(rank) = hits.iter().position(|h| h.file == path) {
            touch_score += 1.0 / (rank as f64 + 1.0);
            if !touched.iter().any(|t| t == path) {
                touched.push(path.to_string());
            }
        }
    }

    let title: HashSet<String> = tokenize(&learning.frontmatter.title).into_iter().collect();
    let body: HashSet<String> = tokenize(&learning.body).into_iter().collect();
    let in_title = query.intersection(&title).count();
    let in_body = query.iter().filter(|t| body.contains(*t) && !title.contains(*t)).count();
    let lexical = in_title >= 1 && in_title + in_body >= MIN_SHARED_WORDS;

    (touch_score > 0.0 || lexical).then(|| {
        let total = touch_score + TITLE_WEIGHT * in_title as f64 + BODY_WEIGHT * in_body as f64;
        (total, touched)
    })
}

/// Up to three repo learnings related to `query` and its code `hits`, best first.
pub fn related_learnings(repo_root: &Path, query: &str, hits: &[AskHit]) -> Vec<LearningHit> {
    let Ok(learnings) = scan_repo_learnings(repo_root) else {
        return Vec::new();
    };
    let query: HashSet<String> = tokenize(query).into_iter().collect();

    let mut related: Vec<LearningHit> = learnings
        .iter()
        .filter_map(|l| {
            let (score, touched) = score(l, &query, hits)?;
            let file = l.file_path.strip_prefix(repo_root).unwrap_or(&l.file_path);
            Some(LearningHit {
                title: flatten(&l.frontmatter.title, MAX_SUMMARY_CHARS),
                file: file.to_string_lossy().replace('\\', "/"),
                summary: summary_of(&l.body),
                touched,
                score,
            })
        })
        .collect();
    related.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.file.cmp(&b.file)));
    related.truncate(MAX_LEARNINGS);
    related
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_learning(root: &Path, name: &str, title: &str, touches: &[&str], body: &str) {
        let dir = root.join(".knot/learnings");
        std::fs::create_dir_all(&dir).unwrap();
        let touches: String = touches.iter().map(|t| format!("  - \"{t}\"\n")).collect();
        std::fs::write(
            dir.join(name),
            format!("---\ntitle: \"{title}\"\ndate: \"2026-10-27\"\ntouches:\n{touches}---\n{body}\n"),
        )
        .unwrap();
    }

    fn hit(file: &str) -> AskHit {
        AskHit {
            id: format!("{file}::f"),
            file: file.into(),
            name: "f".into(),
            kind: "function".into(),
            signature: String::new(),
            span: (1, 2),
            crux: String::new(),
            score: 1.0,
        }
    }

    #[test]
    fn learnings_touching_hit_files_or_sharing_words_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_learning(
            root,
            "pool.md",
            "Keep the sqlite pool at four connections",
            &["src/db.rs#open_pool"],
            "## Decided\nFour connections, because WAL readers block past that.\n\n## Tried and ruled out\nEight.",
        );
        write_learning(root, "retry.md", "Retry uploads with jitter", &["src/upload.rs"], "Backoff with jitter.");
        write_learning(root, "noise.md", "Unrelated note", &["src/other.rs"], "Nothing shared.");

        let found = related_learnings(root, "how is the database opened", &[hit("src/db.rs")]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Keep the sqlite pool at four connections");
        assert_eq!(found[0].file, ".knot/learnings/pool.md");
        assert_eq!(found[0].summary, "Four connections, because WAL readers block past that.");
        assert_eq!(found[0].touched, vec!["src/db.rs".to_string()]);

        let by_words = related_learnings(root, "retry failed uploads", &[]);
        assert_eq!(by_words.iter().map(|l| l.file.as_str()).collect::<Vec<_>>(), vec![".knot/learnings/retry.md"]);
    }

    #[test]
    fn one_shared_word_is_not_enough() {
        let dir = tempfile::tempdir().unwrap();
        write_learning(dir.path(), "a.md", "Retry uploads with jitter", &[], "Backoff.");
        assert!(related_learnings(dir.path(), "retry the parser", &[]).is_empty());
    }

    #[test]
    fn flatten_strips_control_characters_and_cuts() {
        assert_eq!(flatten("a\nb\x1b[31m  c", 10), "a b [31m c");
        assert_eq!(flatten("abcdef", 3), "abc...");
    }
}
