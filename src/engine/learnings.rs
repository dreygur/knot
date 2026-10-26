use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::memory::KnowledgeNode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningFrontmatter {
    pub id: Uuid,
    pub title: String,
    pub author: String,
    pub date: String,
    pub touches: Vec<String>,
    pub tags: Vec<String>,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoLearning {
    pub frontmatter: LearningFrontmatter,
    pub body: String,
    pub file_path: PathBuf,
}

fn sanitize_slug(text: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;

    for c in text.chars() {
        if c.is_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('_');
            prev_dash = true;
        }
    }

    slug.trim_matches('_').to_string()
}

fn format_frontmatter(f: &LearningFrontmatter) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("id: {}\n", f.id));
    out.push_str(&format!("title: \"{}\"\n", f.title.replace('"', "\\\"")));
    out.push_str(&format!("author: \"{}\"\n", f.author));
    out.push_str(&format!("date: \"{}\"\n", f.date));
    out.push_str(&format!("scope: \"{}\"\n", f.scope));

    if !f.touches.is_empty() {
        out.push_str("touches:\n");
        for t in &f.touches {
            out.push_str(&format!("  - \"{t}\"\n"));
        }
    }

    if !f.tags.is_empty() {
        out.push_str("tags:\n");
        for tag in &f.tags {
            out.push_str(&format!("  - \"{tag}\"\n"));
        }
    }

    out.push_str("---\n");
    out
}

fn parse_yaml_frontmatter(content: &str) -> Option<(LearningFrontmatter, String)> {
    if !content.starts_with("---") {
        return None;
    }

    let after_first = &content[3..];
    let end_idx = after_first.find("---")?;
    let yaml_str = &after_first[..end_idx].trim();
    let body = after_first[end_idx + 3..].trim().to_string();

    let mut id = None;
    let mut title = String::new();
    let mut author = "unknown".to_string();
    let mut date = Utc::now().format("%Y-%m-%d").to_string();
    let mut scope = "project".to_string();
    let mut touches = Vec::new();
    let mut tags = Vec::new();

    let mut in_touches = false;
    let mut in_tags = false;

    for line in yaml_str.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("touches:") {
            in_touches = true;
            in_tags = false;
            continue;
        }
        if trimmed.starts_with("tags:") {
            in_tags = true;
            in_touches = false;
            continue;
        }

        if in_touches {
            if trimmed.starts_with("- ") {
                let item = trimmed.trim_start_matches("- ").trim().trim_matches('"');
                touches.push(item.to_string());
                continue;
            } else if !trimmed.starts_with('#') {
                in_touches = false;
            }
        }

        if in_tags {
            if trimmed.starts_with("- ") {
                let item = trimmed.trim_start_matches("- ").trim().trim_matches('"');
                tags.push(item.to_string());
                continue;
            } else if !trimmed.starts_with('#') {
                in_tags = false;
            }
        }

        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim();
            let val = v.trim().trim_matches('"');
            match key {
                "id" => id = Uuid::parse_str(val).ok(),
                "title" => title = val.to_string(),
                "author" => author = val.to_string(),
                "date" => date = val.to_string(),
                "scope" => scope = val.to_string(),
                _ => {}
            }
        }
    }

    let fm = LearningFrontmatter {
        id: id.unwrap_or_else(Uuid::new_v4),
        title,
        author,
        date,
        touches,
        tags,
        scope,
    };

    Some((fm, body))
}

/// Phrases that state why a decision was made, not only what was decided.
const CAUSAL_MARKERS: &[&str] = &[
    "because", "since ", "so that", "caused", "causes", "due to", "otherwise", "to avoid",
    "to prevent", "which means", "led to", "leads to", "results in", "resulted in", "breaks",
    "fails", "failed", "instead of", "the reason",
];

/// Resolve a path from a learning file to an absolute path inside `repo_root`.
///
/// Learning files come from anyone who can push, so `..`, absolute paths elsewhere
/// and committed symlinks pointing out of the repo are all rejected. The deepest
/// existing ancestor is canonicalized, which resolves a symlinked directory even
/// when the file under it does not exist yet.
pub fn contained_path(repo_root: &Path, path: &str) -> Option<PathBuf> {
    let candidate = Path::new(path);
    if candidate.components().any(|c| c == std::path::Component::ParentDir) {
        return None;
    }

    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        repo_root.join(candidate)
    };
    let root = fs::canonicalize(repo_root).ok()?;

    // `exists()` follows links, so a dangling symlink would look like a missing file
    // and be appended unresolved; any link entry at all among the missing parts is refused.
    let mut existing = joined.as_path();
    let mut missing = Vec::new();
    while !existing.exists() {
        if fs::symlink_metadata(existing).is_ok() {
            return None;
        }
        missing.push(existing.file_name()?);
        existing = existing.parent()?;
    }
    let mut resolved = fs::canonicalize(existing).ok()?;
    resolved.extend(missing.iter().rev());

    resolved.starts_with(&root).then_some(resolved)
}

/// Quality gate for team learnings committed to Git.
///
/// A learning must say why (a causal statement or a "Tried and ruled out" section) and
/// every `touches` entry must point at an existing file and, when given, an existing symbol.
/// Returns the problems found; an empty list means the learning passes.
pub fn quality_issues(content: &str, touches: &[String], repo_root: &Path) -> Vec<String> {
    let mut issues = Vec::new();
    let lower = content.to_lowercase();

    let has_ruled_out = lower
        .split("## tried and ruled out")
        .nth(1)
        .map(|rest| !rest.split("\n## ").next().unwrap_or("").trim().is_empty())
        .unwrap_or(false);
    if !has_ruled_out && !CAUSAL_MARKERS.iter().any(|m| lower.contains(m)) {
        issues.push("no causal why: say why (e.g. \"because ...\") or add a \"## Tried and ruled out\" section".into());
    }

    for touch in touches {
        let (path, symbol) = crate::jitv::parse_target(touch);
        let Some(abs) = contained_path(repo_root, path) else {
            issues.push(format!("touches '{touch}': path is outside the repository"));
            continue;
        };
        if !abs.exists() {
            issues.push(format!("touches '{touch}': path does not exist"));
            continue;
        }
        if let Some(sym) = symbol {
            let target = format!("{}#{sym}", abs.to_string_lossy());
            if let Some(err) = crate::jitv::target_error(&target) {
                issues.push(format!("touches '{touch}': {err}"));
            }
        }
    }

    issues
}

pub fn learnings_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".knot").join("learnings")
}

pub fn read_learning_file(path: &Path) -> Result<RepoLearning> {
    let content = fs::read_to_string(path).context("Failed to read learning file")?;
    let (frontmatter, body) = parse_yaml_frontmatter(&content)
        .context("Invalid learning file format: missing frontmatter")?;

    Ok(RepoLearning {
        frontmatter,
        body,
        file_path: path.to_path_buf(),
    })
}

pub fn write_learning_file(
    repo_root: &Path,
    node: &KnowledgeNode,
    author: &str,
) -> Result<PathBuf> {
    let dir = learnings_dir(repo_root);
    fs::create_dir_all(&dir)?;

    let date_str = node.created_at.format("%Y-%m-%d").to_string();
    let title_line = node.content.lines().next().unwrap_or("learning").trim();
    let slug = sanitize_slug(title_line);
    let filename = format!("{date_str}_{slug}.md");
    let file_path = dir.join(filename);

    let touches = match &node.verification_path {
        Some(p) => vec![p.clone()],
        None => Vec::new(),
    };

    let fm = LearningFrontmatter {
        id: node.id,
        title: title_line.to_string(),
        author: author.to_string(),
        date: date_str,
        touches,
        tags: node.tags.clone(),
        scope: node.scope.scope_type().to_string(),
    };

    let frontmatter_str = format_frontmatter(&fm);
    let full_content = format!("{frontmatter_str}\n{}\n", node.content);

    fs::write(&file_path, full_content)?;
    Ok(file_path)
}

pub fn scan_repo_learnings(repo_root: &Path) -> Result<Vec<RepoLearning>> {
    let dir = learnings_dir(repo_root);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut learnings = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
            if let Ok(learning) = read_learning_file(&path) {
                learnings.push(learning);
            }
        }
    }

    learnings.sort_by(|a, b| b.frontmatter.date.cmp(&a.frontmatter.date));
    Ok(learnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryScope;
    use tempfile::tempdir;

    #[test]
    fn test_quality_gate_requires_causal_why() {
        let dir = tempdir().unwrap();
        assert!(!quality_issues("Use WAL mode.", &[], dir.path()).is_empty());
        assert!(quality_issues("Use WAL mode because readers block the writer.", &[], dir.path()).is_empty());
        assert!(quality_issues(
            "Use WAL.\n\n## Tried and ruled out\nDELETE journal mode.",
            &[],
            dir.path()
        )
        .is_empty());
        assert!(!quality_issues("Use WAL.\n\n## Tried and ruled out\n\n## Watch out\nx", &[], dir.path()).is_empty());
    }

    #[test]
    fn test_quality_gate_validates_touched_symbols() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/db.rs"), "pub fn open_pool() {}\n").unwrap();
        let why = "Pool opens lazily because startup must stay fast.";

        assert!(quality_issues(why, &["src/db.rs#open_pool".into()], dir.path()).is_empty());
        assert!(quality_issues(why, &["src/db.rs".into()], dir.path()).is_empty());

        let missing_sym = quality_issues(why, &["src/db.rs#close_pool".into()], dir.path());
        assert!(missing_sym[0].contains("close_pool"));
        let missing_file = quality_issues(why, &["src/gone.rs".into()], dir.path());
        assert!(missing_file[0].contains("does not exist"));

        #[cfg(unix)]
        {
            let outside = tempdir().unwrap();
            std::os::unix::fs::symlink(outside.path().join("later"), dir.path().join("dangling")).unwrap();
            assert_eq!(contained_path(dir.path(), "dangling/id_rsa"), None);
            assert_eq!(contained_path(dir.path(), "dangling"), None);
        }

        for escape in ["../outside.rs", "/etc/hosts"] {
            let issues = quality_issues(why, &[escape.into()], dir.path());
            assert!(issues[0].contains("outside the repository"), "{escape}: {issues:?}");
        }
    }

    #[test]
    fn test_write_and_read_learning_markdown() -> Result<()> {
        let dir = tempdir()?;
        let node_id = Uuid::new_v4();
        let node = KnowledgeNode {
            id: node_id,
            content: "SQLite WAL mode prevents writer blocking\n\n## Decided\nEnable WAL mode.".into(),
            tags: vec!["sqlite".into(), "concurrency".into()],
            verification_path: Some("src/db.rs#init_db".into()),
            content_hash: Some("abc123hash".into()),
            utility_score: 0.8,
            scope: MemoryScope::Project("knot".into()),
            is_stale: false,
            parent_id: None,
            origin_agent: Some("agent-1".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let saved_path = write_learning_file(dir.path(), &node, "Rakib")?;
        assert!(saved_path.exists());

        let read = read_learning_file(&saved_path)?;
        assert_eq!(read.frontmatter.id, node_id);
        assert_eq!(read.frontmatter.author, "Rakib");
        assert_eq!(read.frontmatter.touches, vec!["src/db.rs#init_db"]);
        assert!(read.body.contains("## Decided"));

        let scanned = scan_repo_learnings(dir.path())?;
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].frontmatter.id, node_id);

        Ok(())
    }
}
