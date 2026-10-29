//! Files a query names, and the files tied to them by name: a test and the code it
//! tests. A stack trace names the failing test, a diff names the edited file, and
//! the answer is usually on the other side of that pair, which shares no words with
//! the query.

/// Characters that end a path inside prose, logs and diffs.
const PATH_BREAKS: &[char] = &['"', '\'', '`', '(', ')', '[', ']', '{', '}', ',', ';', '<', '>', '='];
/// Prefixes diffs and shells put before a path.
const PATH_PREFIXES: &[&str] = &["a/", "b/", "./"];
/// A bare file name matching more files than this says nothing about which one.
const MAX_SAME_NAME: usize = 3;
/// Name parts that mark a test file, removed to find the code it tests.
const TEST_PREFIX: &str = "test_";
const TEST_SUFFIXES: &[&str] = &["_test", "_tests", "_spec"];
const CAMEL_TEST_SUFFIXES: &[&str] = &["Tests", "Test"];
const TEST_INFIXES: &[&str] = &[".test.", ".spec."];

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `gate_test.go:67:5` gives `gate_test.go`; `b/src/x.rs` gives `src/x.rs`.
fn clean(token: &str) -> &str {
    let mut token = token.trim_end_matches([':', '.']);
    while let Some((head, tail)) = token.rsplit_once(':') {
        if tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        token = head;
    }
    PATH_PREFIXES.iter().fold(token, |t, p| t.strip_prefix(p).unwrap_or(t))
}

/// The stem a file shares with its test or with the code it tests: `gate_test.go`,
/// `test_gate.py`, `gate.test.ts`, `GateTest.java` and `gate.go` all give `gate`.
fn shared_stem(path: &str) -> String {
    let name = file_name(path);
    let mut stem = name.split('.').next().unwrap_or(name);
    stem = stem.strip_prefix(TEST_PREFIX).unwrap_or(stem);
    for suffix in TEST_SUFFIXES.iter().chain(CAMEL_TEST_SUFFIXES) {
        if stem.len() > suffix.len() && stem.ends_with(suffix) {
            stem = &stem[..stem.len() - suffix.len()];
            break;
        }
    }
    stem.to_lowercase()
}

fn is_test_name(path: &str) -> bool {
    let name = file_name(path);
    let stem = name.split('.').next().unwrap_or(name);
    TEST_INFIXES.iter().any(|i| name.contains(i))
        || stem.starts_with(TEST_PREFIX)
        || TEST_SUFFIXES.iter().chain(CAMEL_TEST_SUFFIXES).any(|s| stem.len() > s.len() && stem.ends_with(s))
}

/// Repository files the query names, by relative path or by a file name that only a
/// few files carry.
pub fn named_files<'a>(query: &str, files: &'a [String]) -> Vec<&'a str> {
    let mut named: Vec<&str> = Vec::new();
    for token in query.split(|c: char| c.is_whitespace() || PATH_BREAKS.contains(&c)) {
        let token = clean(token);
        if !token.contains('.') || token.ends_with('.') {
            continue;
        }
        let suffix = format!("/{token}");
        let matches: Vec<&str> =
            files.iter().map(String::as_str).filter(|f| *f == token || f.ends_with(&suffix)).collect();
        if matches.len() > MAX_SAME_NAME {
            continue;
        }
        for file in matches {
            if !named.contains(&file) {
                named.push(file);
            }
        }
    }
    named
}

fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Files that share `named`'s stem on the other side of the test line: the tests of
/// a source file, or the source a test covers. Ones in the same folder win; elsewhere
/// (Python keeps tests apart) only when few files share the stem.
pub fn counterparts<'a>(named: &str, files: &'a [String]) -> Vec<&'a str> {
    let stem = shared_stem(named);
    let named_is_test = is_test_name(named);
    let all: Vec<&str> = files
        .iter()
        .map(String::as_str)
        .filter(|f| *f != named && is_test_name(f) != named_is_test && shared_stem(f) == stem)
        .collect();
    let near: Vec<&str> = all.iter().copied().filter(|f| folder(f) == folder(named)).collect();
    if !near.is_empty() {
        return near;
    }
    if all.len() > MAX_SAME_NAME {
        return Vec::new();
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_and_source_share_a_stem_in_every_naming_style() {
        for path in ["gate/gate_test.go", "tests/test_gate.py", "src/gate.test.ts", "src/gate.spec.js", "GateTest.java", "gate.go"] {
            assert_eq!(shared_stem(path), "gate", "{path}");
        }
        assert_eq!(shared_stem("latest.go"), "latest", "a word ending in test is not a test");
        assert!(!is_test_name("latest.go") && is_test_name("GateTests.java"));
    }

    #[test]
    fn names_come_out_of_traces_and_diff_headers() {
        let repo = files(&["gate/gate_test.go", "src/x.rs", "README.md"]);
        assert_eq!(named_files("    gate_test.go:67: expected", &repo), vec!["gate/gate_test.go"]);
        assert_eq!(named_files("--- a/src/x.rs\n+++ b/src/x.rs", &repo), vec!["src/x.rs"]);
        assert!(named_files("nothing here, version 1.2 ends.", &repo).is_empty());
    }

    #[test]
    fn a_name_too_many_files_carry_names_none() {
        let repo = files(&["a/mod.rs", "b/mod.rs", "c/mod.rs", "d/mod.rs"]);
        assert!(named_files("see mod.rs", &repo).is_empty());
        assert_eq!(named_files("see c/mod.rs", &repo), vec!["c/mod.rs"]);
    }

    #[test]
    fn counterparts_cross_the_test_line_only() {
        let repo = files(&["gate/gate.go", "gate/gate_test.go", "gate/gate_extra.go", "other/gate.go"]);
        assert_eq!(counterparts("gate/gate_test.go", &repo), vec!["gate/gate.go"], "same folder wins");
        assert_eq!(counterparts("gate/gate.go", &repo), vec!["gate/gate_test.go"]);

        let python = files(&["src/pkg/sessions.py", "tests/test_sessions.py"]);
        assert_eq!(counterparts("src/pkg/sessions.py", &python), vec!["tests/test_sessions.py"]);
    }
}
