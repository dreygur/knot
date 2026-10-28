//! Decide whether a Bash command is one Knot can answer better than the shell.
//!
//! Anything this is not sure about is `PassThrough`: a later hook auto-approves
//! the swapped commands, so a wrong guess would run something nobody approved.

use crate::ask::grep::GrepOptions;
use crate::ast::resolve::{is_skipped_file, ALWAYS_SKIPPED_DIRS};
use crate::ast::SupportedLang;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Answer with Knot's grep. `word` means the pattern must match whole words.
    Grep { opts: GrepOptions, word: bool },
    /// Answer with a line range of a source file.
    SedRange { file: String, start: usize, end: usize },
    /// A Python one-off that writes a source file; Knot blocks these.
    PythonWrite { file: String },
    PassThrough,
}

pub fn classify(command: &str, root: &Path, cwd: &Path) -> Plan {
    let command = command.trim();
    if command.starts_with("KNOT_RAW=1") {
        return Plan::PassThrough;
    }
    let plan = if let Some(at) = command.find("<<") {
        classify_heredoc(command, at, root, cwd)
    } else if has_unquoted_meta(command) {
        None
    } else {
        shell_words::split(command).ok().and_then(|tokens| classify_tokens(&tokens, root, cwd))
    };
    plan.unwrap_or(Plan::PassThrough)
}

fn classify_tokens(tokens: &[String], root: &Path, cwd: &Path) -> Option<Plan> {
    let args: Vec<&str> = tokens.iter().map(String::as_str).collect();
    match args.as_slice() {
        [prog @ ("grep" | "egrep" | "fgrep" | "rg"), rest @ ..] => classify_grep(prog, rest, root, cwd),
        ["sed", rest @ ..] => classify_sed(rest, root, cwd),
        ["python" | "python3", "-c", script] => python_write(script, root, cwd),
        _ => None,
    }
}

/// True when the text outside quotes could do more than name arguments: pipes,
/// redirects, substitutions, globs, expansions, or a second command.
fn has_unquoted_meta(s: &str) -> bool {
    let (mut single, mut double, mut escaped) = (false, false, false);
    for c in s.chars() {
        if escaped {
            escaped = false;
        } else if single {
            single = c != '\'';
        } else if double {
            match c {
                '"' => double = false,
                '\\' => escaped = true,
                '$' | '`' => return true,
                _ => {}
            }
        } else {
            match c {
                '\'' => single = true,
                '"' => double = true,
                '|' | ';' | '&' | '<' | '>' | '$' | '`' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '~'
                | '!' | '#' | '\\' | '\n' | '\r' => return true,
                _ => {}
            }
        }
    }
    false
}

/// Resolve `path` against `cwd`; it must exist and sit inside `root`.
/// Returns the canonical path and its repo-relative form (empty for the root).
fn resolve(path: &str, root: &Path, cwd: &Path) -> Option<(PathBuf, String)> {
    let root = root.canonicalize().ok()?;
    let full = cwd.join(path).canonicalize().ok()?;
    let rel = full.strip_prefix(&root).ok()?;
    let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
    Some((full, rel))
}

/// A repo-relative source file Knot reads, or `None`.
fn source_file(path: &str, root: &Path, cwd: &Path) -> Option<String> {
    let (full, rel) = resolve(path, root, cwd)?;
    (full.is_file() && SupportedLang::from_path(&full).is_some() && !unindexed(root, &rel)).then_some(rel)
}

/// Whether Knot's index leaves out the repo-relative `rel`: it sits in a hidden or
/// always-skipped folder, is a file the index skips by name, or git ignores it.
/// Knot would find nothing there that grep does.
fn unindexed(root: &Path, rel: &str) -> bool {
    if rel.is_empty() {
        return false;
    }
    if rel.split('/').any(|c| c.starts_with('.') || ALWAYS_SKIPPED_DIRS.contains(&c)) {
        return true;
    }
    if rel.rsplit('/').next().is_some_and(is_skipped_file) {
        return true;
    }
    git2::Repository::open(root).is_ok_and(|repo| repo.is_path_ignored(rel).unwrap_or(true))
}

/// Whether Knot's index answers for every file a recursive grep reads under `dir`:
/// some source file is indexed and none is skipped by name, like a bundled `*.min.js`.
/// Walks with the index's own rules, so an ignored `target/` is neither counted nor walked.
fn index_covers(dir: &Path) -> bool {
    let walker = ignore::WalkBuilder::new(dir)
        .follow_links(false)
        .filter_entry(|e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            !(is_dir && e.depth() > 0 && ALWAYS_SKIPPED_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
        })
        .build();

    let mut source = false;
    for e in walker.flatten() {
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if is_skipped_file(&e.file_name().to_string_lossy()) {
            return false;
        }
        source |= SupportedLang::from_path(e.path()).is_some();
    }
    source
}

fn classify_grep(prog: &str, args: &[&str], root: &Path, cwd: &Path) -> Option<Plan> {
    let is_rg = prog == "rg";
    let (mut recursive, mut ignore_case, mut word, mut extended) = (false, false, false, prog == "egrep");
    let mut fixed = prog == "fgrep";
    let mut operands: Vec<&str> = Vec::new();
    let mut flags_done = false;
    for &arg in args {
        if flags_done || !arg.starts_with('-') || arg == "-" {
            operands.push(arg);
        } else if arg == "--" {
            flags_done = true;
        } else if arg.starts_with("--") {
            return None;
        } else {
            for flag in arg[1..].chars() {
                match (flag, is_rg) {
                    ('i', _) => ignore_case = true,
                    ('F', _) => fixed = true,
                    ('w', _) => word = true,
                    ('n', _) => {}
                    ('r' | 'R' | 'H' | 'E', false) => {
                        recursive |= flag != 'H' && flag != 'E';
                        extended |= flag == 'E';
                    }
                    _ => return None,
                }
            }
        }
    }
    let (pattern, path) = match operands.as_slice() {
        [pattern] => (*pattern, None),
        [pattern, path] => (*pattern, Some(*path)),
        _ => return None,
    };
    // Rust regex is not POSIX basic syntax; only swap patterns both read alike.
    if pattern.is_empty() || (!is_rg && !fixed && !extended && pattern.contains(['\\', '+', '?', '|', '(', ')', '{', '}'])) {
        return None;
    }
    if path.is_none() && !is_rg && !recursive {
        return None; // grep with no path reads stdin
    }
    if path.is_some_and(|p| p.starts_with('-')) {
        return None; // `-` is stdin; anything else dashed is an option after `--`
    }
    let (full, rel) = resolve(path.unwrap_or("."), root, cwd)?;
    if unindexed(root, &rel) {
        return None;
    }
    if full.is_dir() {
        // Plain grep refuses directories; and a tree with no source has nothing for Knot to search.
        if (!is_rg && !recursive) || !index_covers(&full) {
            return None;
        }
    } else if SupportedLang::from_path(&full).is_none() {
        return None;
    }
    let within = (!rel.is_empty()).then_some(rel);
    Some(Plan::Grep { opts: GrepOptions { pattern: pattern.to_string(), ignore_case, fixed, within }, word })
}

fn classify_sed(args: &[&str], root: &Path, cwd: &Path) -> Option<Plan> {
    let ["-n", range, file] = args else { return None };
    if file.starts_with('-') {
        return None; // `-fx.rs` is an option to sed, not a file
    }
    let (start, end) = range.strip_suffix('p')?.split_once(',')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(start) || !digits(end) {
        return None;
    }
    let (start, end) = (start.parse::<usize>().ok()?, end.parse::<usize>().ok()?);
    if start < 1 || start > end {
        return None;
    }
    Some(Plan::SedRange { file: source_file(file, root, cwd)?, start, end })
}

/// `python3 - <<'X'` / `python3 <<X`: the lines up to the closing `X` are the script.
fn classify_heredoc(command: &str, at: usize, root: &Path, cwd: &Path) -> Option<Plan> {
    let (head, rest) = command.split_at(at);
    let (marker, body) = rest[2..].split_once('\n')?;
    let delim = marker.trim().trim_matches(|c| c == '\'' || c == '"');
    if delim.is_empty() || !delim.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || has_unquoted_meta(head) {
        return None;
    }
    if !matches!(shell_words::split(head).ok()?.iter().map(String::as_str).collect::<Vec<_>>()[..], ["python" | "python3"] | ["python" | "python3", "-"]) {
        return None;
    }
    let mut script = String::new();
    let mut lines = body.lines();
    for line in lines.by_ref() {
        if line == delim {
            return if lines.all(|l| l.trim().is_empty()) { python_write(&script, root, cwd) } else { None };
        }
        script.push_str(line);
        script.push('\n');
    }
    None // never closed
}

/// The first source file the script writes: via `open(.., mode)` or `Path(..).write_text`.
/// Every candidate is checked, so a write to a log file does not hide one to code.
fn python_write(script: &str, root: &Path, cwd: &Path) -> Option<Plan> {
    let opens = script.match_indices("open(").filter_map(|(i, m)| {
        let (path, after) = string_literal(&script[i + m.len()..])?;
        let after = after.trim_start().strip_prefix(',')?.trim_start();
        let after = after.strip_prefix("mode").map_or(after, |r| r.trim_start().strip_prefix('=').unwrap_or(after));
        let (mode, _) = string_literal(after)?;
        mode.contains(['w', 'a', 'x', '+']).then_some(path)
    });
    let path_writes = [".write_text(", ".write_bytes("].into_iter().flat_map(|call| script.match_indices(call)).filter_map(
        |(i, _)| {
            let before = script[..i].strip_suffix(')')?;
            let quote = before.chars().last().filter(|c| matches!(c, '\'' | '"'))?;
            let inner = &before[..before.len() - 1];
            let start = inner.rfind(quote)?;
            inner[..start].ends_with('(').then(|| inner[start + 1..].to_string())
        },
    );
    let file = opens.chain(path_writes).find_map(|p| writable_source(&p, root, cwd))?;
    Some(Plan::PythonWrite { file })
}

/// Like `source_file`, but the file may not exist yet: a write can create it.
fn writable_source(path: &str, root: &Path, cwd: &Path) -> Option<String> {
    let full = cwd.join(path);
    let name = full.file_name()?;
    let parent = full.parent()?.canonicalize().ok()?;
    let target = parent.join(name);
    if target.is_dir() || SupportedLang::from_path(&target).is_none() {
        return None;
    }
    let rel = target.strip_prefix(root.canonicalize().ok()?).ok()?;
    Some(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
}

/// Parse a leading `'..'` or `".."` literal (no escapes); returns it and the rest.
fn string_literal(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let quote = s.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let body = &s[1..];
    let end = body.find(quote)?;
    let text = &body[..end];
    (!text.contains('\\')).then(|| (text.to_string(), &body[end + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("notes")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.path().join("notes/todo.txt"), "TODO\n").unwrap();
        std::fs::write(dir.path().join("app.log"), "log\n").unwrap();
        dir
    }

    fn plan(dir: &tempfile::TempDir, cmd: &str) -> Plan {
        classify(cmd, dir.path(), dir.path())
    }

    fn grep(pattern: &str, within: Option<&str>, ignore_case: bool, word: bool) -> Plan {
        let opts = GrepOptions {
            pattern: pattern.into(),
            ignore_case,
            within: within.map(String::from),
            ..Default::default()
        };
        Plan::Grep { opts, word }
    }

    #[test]
    fn swaps_grep_over_code() {
        let dir = repo();
        assert_eq!(plan(&dir, "grep -rn foo src"), grep("foo", Some("src"), false, false));
        assert_eq!(plan(&dir, "rg -i Foo"), grep("Foo", None, true, false));
        assert_eq!(plan(&dir, "grep -rnw foo ."), grep("foo", None, false, true));
        assert_eq!(plan(&dir, "grep -n foo src/a.rs"), grep("foo", Some("src/a.rs"), false, false));
        assert_eq!(plan(&dir, "grep -rn -E 'a|b' src"), grep("a|b", Some("src"), false, false));
        match plan(&dir, "fgrep -r a.b src") {
            Plan::Grep { opts, .. } => assert!(opts.fixed),
            other => panic!("{other:?}"),
        }
        let sub = dir.path().join("src");
        assert_eq!(classify("rg foo", dir.path(), &sub), grep("foo", Some("src"), false, false));
    }

    #[test]
    fn swaps_sed_ranges_on_source_files() {
        let dir = repo();
        assert_eq!(
            plan(&dir, "sed -n '10,40p' src/a.rs"),
            Plan::SedRange { file: "src/a.rs".into(), start: 10, end: 40 }
        );
        assert_eq!(plan(&dir, "sed -n '0,4p' src/a.rs"), Plan::PassThrough);
        assert_eq!(plan(&dir, "sed -n '9,4p' src/a.rs"), Plan::PassThrough);
    }

    #[test]
    fn blocks_python_writes_to_code() {
        let dir = repo();
        let write = Plan::PythonWrite { file: "src/a.rs".into() };
        assert_eq!(plan(&dir, "python3 -c \"open('src/a.rs','w').write('x')\""), write);
        assert_eq!(plan(&dir, "python3 -c \"open('src/a.rs', 'r+')\""), write);
        let heredoc = "python3 - <<'PY'\nfrom pathlib import Path\nPath('src/a.rs').write_text('x')\nPY";
        assert_eq!(plan(&dir, heredoc), write);
        assert_eq!(plan(&dir, "python3 <<EOF\nopen('src/a.rs', 'a').write('x')\nEOF\n"), write);
        assert_eq!(plan(&dir, "python3 -c \"open('app.log','w'); open('src/a.rs','w')\""), write);
        assert_eq!(plan(&dir, "python3 -c \"open('src/a.rs','w'); open('app.log','w')\""), write);
        assert_eq!(
            plan(&dir, "python3 -c \"from pathlib import Path; Path('app.log').write_text('x'); open('src/a.rs','w')\""),
            write
        );
        assert_eq!(
            plan(&dir, "python3 -c \"open('app.log','w'); Path('src/a.rs').write_text('x')\""),
            write
        );
        let new = Plan::PythonWrite { file: "src/new.rs".into() };
        assert_eq!(plan(&dir, "python3 -c \"open('src/new.rs','w').write('x')\""), new);
        assert_eq!(plan(&dir, "python3 -c \"open('src/a.rs', mode='w')\""), write);
        assert_eq!(plan(&dir, "python3 -c \"open('src/a.rs','x')\""), write);
        assert_eq!(plan(&dir, "python3 -c \"open('nodir/new.rs','w')\""), Plan::PassThrough);
        assert_eq!(plan(&dir, "python3 -c \"open('../new.rs','w')\""), Plan::PassThrough);
        // Writing a non-code file is none of our business; neither is a heredoc left open or followed by more.
        assert_eq!(plan(&dir, "python3 -c \"open('app.log','w')\""), Plan::PassThrough);
        assert_eq!(plan(&dir, "python3 <<EOF\nopen('src/a.rs','w')"), Plan::PassThrough);
        assert_eq!(plan(&dir, "python3 <<EOF\nopen('src/a.rs','w')\nEOF\nrm -rf x"), Plan::PassThrough);
    }

    #[test]
    fn pass_through_anything_not_understood() {
        let dir = repo();
        for cmd in [
            "grep -rn foo src | head",
            "grep -c foo src/a.rs",
            "grep -rl foo src",
            "grep -A3 foo src/a.rs",
            "sed -i 's/a/b/' src/a.rs",
            "sed -n '1,5p;9p' src/a.rs",
            "grep foo src/*.rs",
            "KNOT_RAW=1 grep -rn foo src",
            "python3 -c \"print(open('src/a.rs').read())\"",
            "python3 tests.py",
            "grep foo src",                  // directory without -r
            "grep foo",                      // stdin
            "grep -r foo -",
            "grep -r -- foo -x.rs",
            "sed -n 1,5p -fx.rs",
            "grep -rn foo src; rm -rf x",
            "grep -rn foo src > out.txt",
            "grep -rn \"$(whoami)\" src",
            "grep -rn foo src &",
            "grep -rn 'a|b' src",            // BRE: `|` is literal, Rust regex disagrees
            "grep -rn foo src\nrm -rf x",
            "grep -rn foo nope",
            "/usr/bin/grep -rn foo src",
        ] {
            assert_eq!(plan(&dir, cmd), Plan::PassThrough, "{cmd}");
        }
    }

    #[test]
    fn non_code_paths_pass_through() {
        let dir = repo();
        for cmd in ["grep -rn TODO notes", "grep foo app.log", "sed -n '1,5p' notes/todo.txt"] {
            assert_eq!(plan(&dir, cmd), Plan::PassThrough, "{cmd}");
        }
    }

    #[test]
    fn ignored_and_skipped_paths_pass_through() {
        let dir = repo();
        git2::Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "vendor/\nsrc/gen.rs\n").unwrap();
        for sub in ["vendor/lib", "target/debug", "node_modules/x", "reference/r", ".hidden"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
            std::fs::write(dir.path().join(sub).join("x.rs"), "fn needle() {}\n").unwrap();
        }
        std::fs::write(dir.path().join("src/gen.rs"), "fn needle() {}\n").unwrap();
        for cmd in [
            "grep -rn needle vendor",
            "grep -rn needle vendor/lib",
            "grep -n needle vendor/lib/x.rs",
            "grep -rn needle target",
            "rg needle node_modules/x",
            "grep -rn needle reference",
            "grep -rn needle .hidden",
            "grep -n needle src/gen.rs",
            "sed -n '1,1p' vendor/lib/x.rs",
            "sed -n '1,1p' src/gen.rs",
        ] {
            assert_eq!(plan(&dir, cmd), Plan::PassThrough, "{cmd}");
        }
        // Source outside the ignored folders still swaps
        assert_eq!(plan(&dir, "grep -rn needle src"), grep("needle", Some("src"), false, false));
    }

    #[test]
    fn a_tree_whose_only_source_is_ignored_passes_through() {
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "gen/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("pkg/gen")).unwrap();
        std::fs::create_dir_all(dir.path().join("pkg/target")).unwrap();
        std::fs::write(dir.path().join("pkg/gen/x.rs"), "fn needle() {}\n").unwrap();
        std::fs::write(dir.path().join("pkg/target/y.rs"), "fn needle() {}\n").unwrap();
        assert_eq!(plan(&dir, "grep -rn needle pkg"), Plan::PassThrough);
    }

    #[test]
    fn trees_with_minified_scripts_pass_through() {
        // The index skips `*.min.js`, so Knot's grep would miss a match there that grep finds.
        let dir = repo();
        std::fs::create_dir_all(dir.path().join("web/vendor")).unwrap();
        std::fs::write(dir.path().join("web/app.js"), "function app() {}\n").unwrap();
        std::fs::write(dir.path().join("web/vendor/sigma.min.js"), "function needle(){}\n").unwrap();
        for cmd in ["grep -rn needle web", "rg needle", "grep -n needle web/vendor/sigma.min.js", "sed -n '1,1p' web/vendor/sigma.min.js"] {
            assert_eq!(plan(&dir, cmd), Plan::PassThrough, "{cmd}");
        }
        assert_eq!(plan(&dir, "grep -rn needle src"), grep("needle", Some("src"), false, false));
    }

    #[test]
    fn paths_outside_the_repo_pass_through() {
        let dir = repo();
        let sub = dir.path().join("src");
        assert_eq!(plan(&dir, "grep -rn x /etc"), Plan::PassThrough);
        assert_eq!(classify("sed -n '1,5p' ../a.rs", dir.path(), &sub), Plan::PassThrough);
        assert_eq!(classify("grep -rn x ../..", dir.path(), &sub), Plan::PassThrough);
        // `..` from src is the repo root itself, which is fine.
        assert_eq!(classify("grep -rn x ..", dir.path(), &sub), grep("x", None, false, false));
    }
}
