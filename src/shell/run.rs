//! Run a command the hook swapped: answer it with Knot and say what ran instead.

use super::{classify, Plan};
use crate::ask::events::{self, Event, EventKind};
use crate::ask::{grep, savings};
use anyhow::{bail, Context, Result};
use std::fmt::Write as _;
use std::path::Path;

/// A sed range wider than this gets the file's definitions instead of its lines.
const MAX_RANGE_LINES: usize = 400;
/// How far a range edge moves to take in the definition it cuts.
const MAX_SNAP_LINES: usize = 60;

/// The text to print for `command`. Fails for a command that is not a swap; the caller
/// runs those with the shell.
pub fn run(command: &str, root: &Path, cwd: &Path) -> Result<String> {
    let (out, tool, bytes_before) = match classify(command, root, cwd) {
        Plan::Grep { opts, word } => {
            let tool = if command.trim_start().starts_with("rg") { "rg" } else { "grep" };
            let (out, before) = run_grep(command, tool, opts, word, root)?;
            (out, tool, before)
        }
        Plan::SedRange { file, start, end } => {
            let (out, before) = run_sed(command, &file, start, end, root)?;
            (out, "sed", before)
        }
        _ => bail!("'{command}' is not a command Knot answers"),
    };
    let bytes_after = out.len() as u64;
    events::append(
        root,
        &Event {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            session: events::current_session(root),
            kind: EventKind::Swap,
            tool: tool.to_string(),
            command: Some(command.to_string()),
            bytes_before,
            bytes_after,
            saved_tokens: savings::estimate_tokens(bytes_before).saturating_sub(savings::estimate_tokens(bytes_after)),
        },
    );
    Ok(out)
}

fn run_grep(command: &str, tool: &str, mut opts: grep::GrepOptions, word: bool, root: &Path) -> Result<(String, u64)> {
    if word {
        // The grep builder escapes a fixed pattern, \b included, so escape here instead.
        let inner = if opts.fixed { regex::escape(&opts.pattern) } else { opts.pattern.clone() };
        opts.pattern = format!(r"\b(?:{inner})\b");
        opts.fixed = false;
    }
    let report = grep::grep(root, Some(&crate::ask::index_root(root)), &opts)?;
    let shown: usize = report.groups.iter().map(|g| g.lines.len()).sum();
    let bytes_before = report
        .groups
        .iter()
        .flat_map(|g| g.lines.iter().map(move |l| (g, l)))
        .map(|(g, l)| (g.file.len() + l.line.to_string().len() + l.text.len() + 3) as u64)
        .sum();
    let out = format!(
        "[knot] ran `knot grep` in place of {tool}: {shown} of {} matches, grouped by function. Raw output: KNOT_RAW=1 {command}\n{}",
        report.hits,
        grep::format_report(&report)
    );
    Ok((out, bytes_before))
}

fn run_sed(command: &str, file: &str, start: usize, end: usize, root: &Path) -> Result<(String, u64)> {
    let text = std::fs::read_to_string(root.join(file)).with_context(|| format!("cannot read {file}"))?;
    let lines: Vec<&str> = text.lines().collect();
    let end = end.min(lines.len());
    if start > end {
        return Ok((String::new(), 0)); // raw sed prints nothing past the end either
    }
    let bytes_before: u64 = lines[start - 1..end].iter().map(|l| l.len() as u64 + 1).sum();
    // Without an index the lines still print, just not widened.
    let items = crate::ask::load_graph(root).map(|g| g.file_skeleton(file)).unwrap_or_default();

    let mut out = String::new();
    if end - start + 1 > MAX_RANGE_LINES {
        let _ = writeln!(
            out,
            "[knot] {} lines is a lot to read; here are the definitions in that range. Ask for a narrower range, or raw output: KNOT_RAW=1 {command}",
            end - start + 1
        );
        for item in items.iter().filter(|i| i.span.0 <= end && i.span.1 >= start) {
            let _ = writeln!(out, "  L{:>4}-L{:<4} {:<10} {}", item.span.0, item.span.1, item.kind.as_str(), item.signature);
        }
        return Ok((out, bytes_before));
    }
    // The innermost definition a range edge cuts is the one worth showing whole.
    let new_start = items
        .iter()
        .filter(|i| i.span.0 < start && start <= i.span.1 && start - i.span.0 <= MAX_SNAP_LINES)
        .map(|i| i.span.0)
        .max()
        .unwrap_or(start);
    let new_end = items
        .iter()
        .filter(|i| i.span.0 <= end && end < i.span.1 && i.span.1 - end <= MAX_SNAP_LINES)
        .map(|i| i.span.1)
        .min()
        .unwrap_or(end)
        .min(lines.len());
    let _ = writeln!(
        out,
        "[knot] showed lines {new_start}-{new_end} of {file}, widened to whole definitions. Raw output: KNOT_RAW=1 {command}"
    );
    for line in &lines[new_start - 1..new_end] {
        out.push_str(line);
        out.push('\n');
    }
    Ok((out, bytes_before))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/a.rs"),
            "fn alpha() {\n    let a = 1;\n    let b = 2;\n    let _ = a + b;\n}\n\nfn beta() {\n    alpha();\n}\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn grep_swap_says_what_ran_and_how_to_get_raw() {
        let dir = repo();
        let out = run("grep -rn alpha src", dir.path(), dir.path()).unwrap();
        let first = out.lines().next().unwrap();
        assert!(first.starts_with("[knot] ran `knot grep` in place of grep:"), "{first}");
        assert!(first.contains("KNOT_RAW=1 grep -rn alpha src"), "{first}");
    }

    #[test]
    fn sed_range_widens_to_whole_definitions() {
        let dir = repo();
        let out = run("sed -n '2,2p' src/a.rs", dir.path(), dir.path()).unwrap();
        assert!(out.starts_with("[knot] showed lines 1-5 of src/a.rs"), "{out}");
        assert!(out.contains("fn alpha() {\n") && out.ends_with("}\n"), "{out}");
        assert!(!out.contains("beta"));
    }

    #[test]
    fn huge_sed_range_returns_the_skeleton() {
        let dir = repo();
        let filler = "// filler\n".repeat(450);
        std::fs::write(dir.path().join("src/a.rs"), format!("fn alpha() {{\n    let a = 1;\n}}\n{filler}fn beta() {{}}\n")).unwrap();
        let out = run("sed -n '1,454p' src/a.rs", dir.path(), dir.path()).unwrap();
        assert!(out.contains("454 lines is a lot to read"), "{out}");
        assert!(out.contains("fn alpha") && out.contains("fn beta"), "{out}");
        assert!(!out.contains("let a = 1"));
    }

    #[test]
    fn sed_range_past_the_end_of_the_file_does_not_fail() {
        let dir = repo();
        let out = run("sed -n '500,510p' src/a.rs", dir.path(), dir.path()).unwrap();
        assert!(!out.contains("fn alpha"), "{out}");
    }

    #[test]
    fn each_swap_is_logged() {
        let dir = repo();
        let body: String = (0..80).map(|i| format!("    let alpha_{i} = \"alpha value number {i} with a longer tail of text\";\n")).collect();
        std::fs::write(dir.path().join("src/big.rs"), format!("fn big() {{\n{body}}}\n")).unwrap();
        run("grep -rn alpha src", dir.path(), dir.path()).unwrap();
        let swaps: Vec<_> = events::recent(dir.path()).into_iter().filter(|e| e.kind == EventKind::Swap).collect();
        assert_eq!(swaps.len(), 1);
        assert_eq!(swaps[0].tool, "grep");
        assert_eq!(swaps[0].command.as_deref(), Some("grep -rn alpha src"));
        assert!(swaps[0].bytes_before > swaps[0].bytes_after, "{:?}", swaps[0]);
    }
}
