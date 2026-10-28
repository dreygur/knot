//! Spot a Bash command that edits code with a python one-off. The python block denies
//! it once, so the agent uses its Edit tool and sees the change.
//!
//! Anything this is not sure about is left alone: a false match blocks a command the
//! agent needed.

use crate::ast::SupportedLang;
use std::path::Path;

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

/// Parse a leading `'..'` or `".."` literal (no escapes); returns it and the rest.
fn string_literal(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let quote = s.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let body = &s[1..];
    let end = body.find(quote)?;
    let text = &body[..end];
    (!text.contains('\\')).then(|| (text.to_string(), &body[end + 1..]))
}

/// The repo relative source file `path` names. It may not exist yet: a write can create it.
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

/// The first source file the script writes: via `open(.., mode)` or `Path(..).write_text`.
/// Every candidate is checked, so a write to a log file does not hide one to code.
fn python_write(script: &str, root: &Path, cwd: &Path) -> Option<String> {
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
    opens.chain(path_writes).find_map(|p| writable_source(&p, root, cwd))
}

/// `python3 - <<'X'` / `python3 <<X`: the lines up to the closing `X` are the script.
fn heredoc_write(command: &str, at: usize, root: &Path, cwd: &Path) -> Option<String> {
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

/// The repo source file `command` writes with a python one-off (`python3 -c` or a
/// heredoc), or `None` for anything else.
pub fn python_edit(command: &str, root: &Path, cwd: &Path) -> Option<String> {
    let command = command.trim();
    if let Some(at) = command.find("<<") {
        return heredoc_write(command, at, root, cwd);
    }
    if has_unquoted_meta(command) {
        return None;
    }
    let tokens = shell_words::split(command).ok()?;
    match tokens.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["python" | "python3", "-c", script] => python_write(script, root, cwd),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.path().join("app.log"), "log\n").unwrap();
        dir
    }

    fn edit(dir: &tempfile::TempDir, cmd: &str) -> Option<String> {
        python_edit(cmd, dir.path(), dir.path())
    }

    #[test]
    fn finds_python_writes_to_code() {
        let dir = repo();
        let write = Some("src/a.rs".to_string());
        assert_eq!(edit(&dir, "python3 -c \"open('src/a.rs','w').write('x')\""), write);
        assert_eq!(edit(&dir, "python3 -c \"open('src/a.rs', 'r+')\""), write);
        let heredoc = "python3 - <<'PY'\nfrom pathlib import Path\nPath('src/a.rs').write_text('x')\nPY";
        assert_eq!(edit(&dir, heredoc), write);
        assert_eq!(edit(&dir, "python3 <<EOF\nopen('src/a.rs', 'a').write('x')\nEOF\n"), write);
        assert_eq!(edit(&dir, "python3 -c \"open('app.log','w'); open('src/a.rs','w')\""), write);
        assert_eq!(edit(&dir, "python3 -c \"open('src/a.rs','w'); open('app.log','w')\""), write);
        assert_eq!(
            edit(&dir, "python3 -c \"from pathlib import Path; Path('app.log').write_text('x'); open('src/a.rs','w')\""),
            write
        );
        assert_eq!(edit(&dir, "python3 -c \"open('app.log','w'); Path('src/a.rs').write_text('x')\""), write);
        assert_eq!(edit(&dir, "python3 -c \"open('src/new.rs','w').write('x')\""), Some("src/new.rs".to_string()));
        assert_eq!(edit(&dir, "python3 -c \"open('src/a.rs', mode='w')\""), write);
        assert_eq!(edit(&dir, "python3 -c \"open('src/a.rs','x')\""), write);
    }

    #[test]
    fn leaves_everything_else_alone() {
        let dir = repo();
        for cmd in [
            "python3 -c \"open('nodir/new.rs','w')\"",
            "python3 -c \"open('../new.rs','w')\"",
            // Writing a non code file is none of our business; neither is a heredoc left open or followed by more
            "python3 -c \"open('app.log','w')\"",
            "python3 <<EOF\nopen('src/a.rs','w')",
            "python3 <<EOF\nopen('src/a.rs','w')\nEOF\nrm -rf x",
            "python3 -c \"print(open('src/a.rs').read())\"",
            "python3 tests.py",
            "grep -rn foo src",
            "sed -i 's/a/b/' src/a.rs",
        ] {
            assert_eq!(edit(&dir, cmd), None, "{cmd}");
        }
    }
}
