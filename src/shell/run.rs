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
/// A swapped grep shows at most this many places (enclosing definitions)...
const MAX_GROUPS: usize = 10;
/// ...and at most this many matching lines in each.
const MAX_GROUP_LINES: usize = 3;

/// The text to print for `command`. Fails when Knot has nothing better than the shell's
/// own output: not a swap, nothing Knot indexes searched, or a reply no smaller or no
/// wider than raw. The caller then runs the command with the shell, and no swap is logged.
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
            learning: None,
            cost_minutes: None,
            cost_tokens: None,
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
    if report.files_searched == 0 {
        // An ignored or unindexed path: "no matches" would be a lie, so let grep answer
        bail!("no file Knot indexes is under the searched path");
    }
    let bytes_before: u64 = report
        .groups
        .iter()
        .flat_map(|g| g.lines.iter().map(move |l| (g, l)))
        .map(|(g, l)| (g.file.len() + l.line.to_string().len() + l.text.len() + 3) as u64)
        .sum();
    let mut groups: Vec<grep::GrepGroup> = report.groups.iter().take(MAX_GROUPS).cloned().collect();
    for g in &mut groups {
        g.lines.truncate(MAX_GROUP_LINES);
    }
    let shown: usize = groups.iter().map(|g| g.lines.len()).sum();
    let mut out = format!(
        "[knot] ran `knot grep` in place of {tool}: {shown} of {} matches, grouped by function. Raw output: KNOT_RAW=1 {command}\n\
         '{}': {} hit(s) in {} place(s) across {} file(s) (searched {} indexed files)\n{}",
        report.hits,
        report.pattern,
        report.hits,
        report.groups.len(),
        report.files_matched,
        report.files_searched,
        grep::format_groups(&groups)
    );
    if shown < report.hits {
        let _ = writeln!(out, "\n+{} more matches; raw output: KNOT_RAW=1 {command}", report.hits - shown);
    }
    if out.len() as u64 >= bytes_before {
        bail!("Knot's answer is no smaller than {tool}'s own");
    }
    Ok((out, bytes_before))
}

fn run_sed(command: &str, file: &str, start: usize, end: usize, root: &Path) -> Result<(String, u64)> {
    let text = std::fs::read_to_string(root.join(file)).with_context(|| format!("cannot read {file}"))?;
    let lines: Vec<&str> = text.lines().collect();
    let end = end.min(lines.len());
    if start > end {
        bail!("lines {start}-{end} are past the end of {file}"); // raw sed prints nothing; let it
    }
    let bytes_before: u64 = lines[start - 1..end].iter().map(|l| l.len() as u64 + 1).sum();
    // Without an index the lines still print, just not widened.
    let items = crate::ask::load_graph(root).map(|g| g.file_skeleton(file)).unwrap_or_default();

    let mut out = String::new();
    if end - start + 1 > MAX_RANGE_LINES {
        let inside: Vec<_> = items.iter().filter(|i| i.span.0 <= end && i.span.1 >= start).collect();
        if inside.is_empty() {
            bail!("no definitions in lines {start}-{end} of {file}"); // the notice alone would hide the lines
        }
        let _ = writeln!(
            out,
            "[knot] {} lines is a lot to read; here are the definitions in that range. Ask for a narrower range, or raw output: KNOT_RAW=1 {command}",
            end - start + 1
        );
        for item in inside {
            let _ = writeln!(out, "  L{:>4}-L{:<4} {:<10} {}", item.span.0, item.span.1, item.kind.as_str(), item.signature);
        }
        if out.len() as u64 >= bytes_before {
            bail!("the definitions in lines {start}-{end} of {file} are no shorter than the lines");
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
    if (new_start, new_end) == (start, end) {
        bail!("lines {start}-{end} of {file} cut no definition"); // a notice would only add bytes
    }
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

    /// `functions` functions with five long lines each that mention `alpha`.
    fn many(dir: &tempfile::TempDir, functions: usize) {
        let body: String = (0..functions)
            .map(|f| {
                let lines: String = (0..5).map(|l| format!("    let alpha_{f}_{l} = \"alpha value with a longer tail of text\";\n")).collect();
                format!("fn f{f}() {{\n{lines}}}\n\n")
            })
            .collect();
        std::fs::write(dir.path().join("src/many.rs"), body).unwrap();
    }

    fn swaps(dir: &tempfile::TempDir) -> Vec<Event> {
        events::recent(dir.path()).into_iter().filter(|e| e.kind == EventKind::Swap).collect()
    }

    #[test]
    fn grep_swap_says_what_ran_and_how_to_get_raw() {
        let dir = repo();
        many(&dir, 4);
        let out = run("grep -rn alpha src", dir.path(), dir.path()).unwrap();
        let first = out.lines().next().unwrap();
        assert!(first.starts_with("[knot] ran `knot grep` in place of grep:"), "{first}");
        assert!(first.contains("KNOT_RAW=1 grep -rn alpha src"), "{first}");
    }

    #[test]
    fn broad_grep_is_capped_and_smaller_than_raw() {
        let dir = repo();
        std::fs::remove_file(dir.path().join("src/a.rs")).unwrap();
        many(&dir, 40);
        let out = run("grep -rn alpha src", dir.path(), dir.path()).unwrap();
        let shown = out.lines().filter(|l| l.starts_with("  L")).count();
        assert_eq!(shown, MAX_GROUPS * MAX_GROUP_LINES, "{out}");
        let hits = 40 * 5;
        assert!(out.lines().next().unwrap().contains(&format!("{shown} of {hits} matches")), "{out}");
        assert!(out.ends_with(&format!("+{} more matches; raw output: KNOT_RAW=1 grep -rn alpha src\n", hits - shown)), "{out}");
        let swap = &swaps(&dir)[0];
        assert!(swap.bytes_after * 3 < swap.bytes_before, "{swap:?}");
    }

    #[test]
    fn grep_no_smaller_than_raw_is_left_to_the_shell() {
        let dir = repo();
        assert!(run("grep -rn alpha src", dir.path(), dir.path()).is_err());
        assert!(run("grep -rn nothing_matches src", dir.path(), dir.path()).is_err());
        assert!(swaps(&dir).is_empty(), "a command the shell runs is not a swap");
    }

    #[test]
    fn grep_over_files_knot_does_not_index_is_left_to_the_shell() {
        let dir = repo();
        // `.ignore` keeps vendor/ out of the index but not out of git, so classify lets it by
        std::fs::write(dir.path().join(".ignore"), "vendor/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("vendor/lib")).unwrap();
        std::fs::write(dir.path().join("vendor/lib/x.rs"), "fn needle() {}\n").unwrap();
        assert!(matches!(classify("grep -rn needle vendor", dir.path(), dir.path()), Plan::Grep { .. }));
        assert!(run("grep -rn needle vendor", dir.path(), dir.path()).is_err());
        assert!(swaps(&dir).is_empty());
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
    fn sed_range_that_widens_nothing_is_left_to_the_shell() {
        let dir = repo();
        assert!(run("sed -n '1,5p' src/a.rs", dir.path(), dir.path()).is_err(), "already whole definitions");
        assert!(run("sed -n '6,6p' src/a.rs", dir.path(), dir.path()).is_err(), "between definitions");
        assert!(swaps(&dir).is_empty());
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
    fn huge_sed_range_without_definitions_is_left_to_the_shell() {
        let dir = repo();
        let filler = "// filler\n".repeat(450);
        std::fs::write(dir.path().join("src/a.rs"), format!("fn alpha() {{}}\n{filler}")).unwrap();
        assert!(run("sed -n '10,440p' src/a.rs", dir.path(), dir.path()).is_err());
        assert!(swaps(&dir).is_empty());
    }

    #[test]
    fn sed_range_past_the_end_of_the_file_is_left_to_the_shell() {
        let dir = repo();
        assert!(run("sed -n '500,510p' src/a.rs", dir.path(), dir.path()).is_err());
    }

    #[test]
    fn each_swap_is_logged() {
        let dir = repo();
        many(&dir, 4);
        run("grep -rn alpha src", dir.path(), dir.path()).unwrap();
        let swaps = swaps(&dir);
        assert_eq!(swaps.len(), 1);
        assert_eq!(swaps[0].tool, "grep");
        assert_eq!(swaps[0].command.as_deref(), Some("grep -rn alpha src"));
        assert!(swaps[0].bytes_before > swaps[0].bytes_after, "{:?}", swaps[0]);
    }
}
