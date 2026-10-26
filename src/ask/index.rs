use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::tokenize::tokenize;
use crate::ast::SymbolNode;

/// Fields per symbol, in this order: name, signature, doc comment, path, body.
const FIELD_COUNT: usize = 5;

/// How much a match in each field counts. A name or doc comment states what the
/// symbol is for; the body only shows what it touches.
const FIELD_WEIGHTS: [f64; FIELD_COUNT] = [3.0, 1.5, 2.0, 1.0, 0.5];

/// Length normalization per field: bodies vary wildly in size, names barely do.
const FIELD_B: [f64; FIELD_COUNT] = [0.3, 0.5, 0.6, 0.3, 0.75];

const K1: f64 = 1.2;
const MAX_DOC_LINES: usize = 40;
const DOCSTRING_SCAN_LINES: usize = 3;

/// Comment and annotation prefixes that may sit directly above a definition.
const COMMENT_PREFIXES: &[&str] = &["///", "//!", "//", "/**", "/*", "*/", "*", "#", "\"\"\"", "'''"];
const ANNOTATION_PREFIXES: &[&str] = &["#[", "#![", "@"];

/// Searchable text of one symbol, split into fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolDoc {
    pub id: String,
    pub is_test: bool,
    fields: [Vec<String>; FIELD_COUNT],
}

#[derive(Debug, Clone)]
struct Posting {
    doc: usize,
    tf: [u32; FIELD_COUNT],
}

/// BM25F index over every symbol in a code graph.
pub struct LexicalIndex {
    docs: Vec<SymbolDoc>,
    postings: HashMap<String, Vec<Posting>>,
    avg_len: [f64; FIELD_COUNT],
}

/// Comment block directly above `start_line` (1-indexed), skipping attributes and
/// decorators that sit between the comment and the definition.
fn leading_comment(lines: &[&str], start_line: usize) -> (String, bool) {
    let mut collected = Vec::new();
    let mut annotated_test = false;
    let mut idx = start_line.saturating_sub(1);

    while idx > 0 && collected.len() < MAX_DOC_LINES {
        idx -= 1;
        let trimmed = lines[idx].trim();
        if ANNOTATION_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            annotated_test |= trimmed.contains("test");
            continue;
        }
        if trimmed.is_empty() || !COMMENT_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            break;
        }
        collected.push(trimmed);
    }

    collected.reverse();
    (collected.join("\n"), annotated_test)
}

/// Python docstring: a triple quoted string opening within the first body lines.
fn docstring(lines: &[&str], span: (usize, usize)) -> String {
    let body_start = span.0;
    let scan_end = (body_start + DOCSTRING_SCAN_LINES).min(span.1).min(lines.len());
    let Some(open) = (body_start..scan_end).find(|&i| {
        let t = lines[i].trim_start();
        t.starts_with("\"\"\"") || t.starts_with("'''") || t.starts_with("r\"\"\"")
    }) else {
        return String::new();
    };

    let mut text = Vec::new();
    for (offset, line) in lines[open..span.1.min(lines.len())].iter().enumerate() {
        text.push(line.trim());
        let quotes = line.matches("\"\"\"").count() + line.matches("'''").count();
        if (offset == 0 && quotes >= 2) || (offset > 0 && quotes >= 1) {
            break;
        }
    }
    text.join("\n")
}

fn is_test_symbol(sym: &SymbolNode, annotated_test: bool) -> bool {
    let bare = sym.name.rsplit(['.', ':']).next().unwrap_or(&sym.name);
    let file = sym.file.to_lowercase();
    annotated_test
        || bare.starts_with("test_")
        || bare.starts_with("Test")
        || file.starts_with("test/")
        || file.starts_with("tests/")
        || file.contains("/test/")
        || file.contains("/tests/")
        || file.contains(".test.")
        || file.contains(".spec.")
        || file.rsplit('/').next().is_some_and(|f| f.starts_with("test_"))
}

pub(super) fn build_doc(sym: &SymbolNode, lines: &[&str]) -> SymbolDoc {
    let (comment, annotated_test) = leading_comment(lines, sym.span.0);
    let doc_text = format!("{comment}\n{}", docstring(lines, sym.span));

    let body_start = sym.span.0.min(lines.len());
    let body_end = sym.span.1.min(lines.len());
    let body = lines.get(body_start..body_end).map(|b| b.join("\n")).unwrap_or_default();

    let path = sym.file.rsplit_once('.').map_or(sym.file.as_str(), |(stem, _)| stem);

    SymbolDoc {
        id: sym.id.clone(),
        is_test: is_test_symbol(sym, annotated_test),
        fields: [
            tokenize(&sym.name),
            tokenize(&sym.signature),
            tokenize(&doc_text),
            tokenize(path),
            tokenize(&body),
        ],
    }
}

impl LexicalIndex {
    pub(super) fn from_docs(docs: Vec<SymbolDoc>) -> Self {
        let mut postings: HashMap<String, Vec<Posting>> = HashMap::new();
        let mut total_len = [0usize; FIELD_COUNT];

        for (doc_idx, doc) in docs.iter().enumerate() {
            let mut tf: HashMap<&str, [u32; FIELD_COUNT]> = HashMap::new();
            for (field, tokens) in doc.fields.iter().enumerate() {
                total_len[field] += tokens.len();
                for token in tokens {
                    tf.entry(token.as_str()).or_default()[field] += 1;
                }
            }
            for (token, counts) in tf {
                postings.entry(token.to_string()).or_default().push(Posting { doc: doc_idx, tf: counts });
            }
        }

        let n = docs.len().max(1) as f64;
        let avg_len = total_len.map(|len| (len as f64 / n).max(1.0));
        Self { docs, postings, avg_len }
    }

    pub fn doc(&self, idx: usize) -> &SymbolDoc {
        &self.docs[idx]
    }

    /// BM25F scores for `query`, as (doc index, score), best first.
    pub fn search(&self, query: &str) -> Vec<(usize, f64)> {
        let mut terms = tokenize(query);
        terms.sort();
        terms.dedup();

        let n = self.docs.len() as f64;
        let mut scores: HashMap<usize, f64> = HashMap::new();

        for term in &terms {
            let Some(list) = self.postings.get(term) else {
                continue;
            };
            let df = list.len() as f64;
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();

            for posting in list {
                let doc = &self.docs[posting.doc];
                let weighted_tf: f64 = (0..FIELD_COUNT)
                    .map(|f| {
                        let len_ratio = doc.fields[f].len() as f64 / self.avg_len[f];
                        let norm = 1.0 - FIELD_B[f] + FIELD_B[f] * len_ratio;
                        FIELD_WEIGHTS[f] * posting.tf[f] as f64 / norm
                    })
                    .sum();
                *scores.entry(posting.doc).or_default() += idf * weighted_tf / (K1 + weighted_tf);
            }
        }

        let mut ranked: Vec<(usize, f64)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        ranked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: &str, name: &str, doc_text: &str, body: &str) -> SymbolDoc {
        SymbolDoc {
            id: id.into(),
            is_test: false,
            fields: [tokenize(name), Vec::new(), tokenize(doc_text), tokenize(id), tokenize(body)],
        }
    }

    #[test]
    fn name_and_doc_outrank_body_mentions() {
        let index = LexicalIndex::from_docs(vec![
            doc("a.rs::caller", "caller", "", "resolve_redirects(); resolve_redirects(); log redirect"),
            doc("b.rs::resolve_redirects", "resolve_redirects", "Follow redirects until done", "loop {}"),
            doc("c.rs::unrelated", "unrelated", "", "nothing"),
        ]);
        let ranked = index.search("follow http redirects");
        assert_eq!(index.doc(ranked[0].0).id, "b.rs::resolve_redirects");
        assert!(ranked.iter().all(|(i, _)| index.doc(*i).id != "c.rs::unrelated"));
    }

    #[test]
    fn leading_comment_skips_attributes_and_flags_tests() {
        let src = "/// Normalizes ranks.\n#[inline]\npub fn norm() {}\n\n#[test]\nfn checks() {}\n";
        let lines: Vec<&str> = src.lines().collect();
        assert_eq!(leading_comment(&lines, 3), ("/// Normalizes ranks.".to_string(), false));
        assert!(leading_comment(&lines, 6).1);
    }

    #[test]
    fn python_docstring_is_found() {
        let src = "def send(self):\n    \"\"\"Send a prepared request.\n\n    Returns a Response.\"\"\"\n    pass\n";
        let lines: Vec<&str> = src.lines().collect();
        let text = docstring(&lines, (1, 5));
        assert!(text.contains("Send a prepared request"));
        assert!(text.contains("Returns a Response"));
        assert!(!text.contains("pass"));
    }
}
