//! `knot card`: a picture of what Knot saved across the repos it has worked in.

pub mod clipboard;
pub mod render;
pub mod repos;
pub mod stats;

use crate::hosts::Agent;
use anyhow::{bail, Result};
use chrono::{DateTime, Local, NaiveDate};
use render::CardView;
use std::path::{Path, PathBuf};

const USAGE: &str = "knot card [--repo] [--out <path>]";

struct Args {
    repo: bool,
    out: Option<PathBuf>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut parsed = Args { repo: false, out: None };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--repo" => parsed.repo = true,
            "--out" => match args.next() {
                Some(path) => parsed.out = Some(PathBuf::from(path)),
                None => bail!("--out needs a path. Usage: {USAGE}"),
            },
            _ => bail!("unknown argument '{arg}'. Usage: {USAGE}"),
        }
    }
    Ok(parsed)
}

/// `~/Downloads/knot-card-YYYY-MM-DD.png`, or the same name in `cwd` with no Downloads folder.
fn default_path(home: &Path, today: NaiveDate, cwd: &Path) -> PathBuf {
    let downloads = home.join("Downloads");
    let dir = if downloads.is_dir() { downloads } else { cwd.to_path_buf() };
    dir.join(format!("knot-card-{}.png", today.format("%Y-%m-%d")))
}

/// The agents whose user config already holds Knot, by name.
fn wired_agents(home: &Path) -> Vec<String> {
    Agent::ALL.into_iter().filter(|a| a.knot_wired(home)).map(|a| a.display().to_string()).collect()
}

/// List the repo we are in, so history written before the list existed still counts.
/// Best-effort and silent; outside a repo there is nothing to list.
pub fn register_current(home: &Path, repo: Option<&Path>) {
    if let Some(root) = repo {
        repos::register(home, root);
    }
}

/// What the card shows: every listed repo, or just `repo` (named by its folder).
fn build_view(home: &Path, repo: Option<&Path>, now: DateTime<Local>) -> CardView {
    let (repos, repo_name) = match repo {
        Some(root) => (vec![root.to_path_buf()], root.file_name().map(|n| n.to_string_lossy().into_owned())),
        None => (repos::list(home), None),
    };
    CardView { stats: stats::collect(&repos, now), repo_name, agents: wired_agents(home) }
}

/// Draw the card to `out`, then try `copy`. A failed copy is reported on the second line
/// and never fails the command; a failed draw or write does.
fn make_card(view: &CardView, out: &Path, copy: &dyn Fn(&Path) -> Result<()>) -> Result<[String; 2]> {
    let png = render::render_png(view)?;
    std::fs::write(out, png).map_err(|e| anyhow::anyhow!("cannot write {}: {e}", out.display()))?;
    // Absolute, so a relative `--out -x.png` can never read as an option to the copy tool
    let absolute = std::fs::canonicalize(out).unwrap_or_else(|_| out.to_path_buf());
    let second = match copy(&absolute) {
        Ok(()) => "✓ copied to clipboard".to_string(),
        Err(e) => format!("· couldn't copy to the clipboard ({e}); the file is saved"),
    };
    Ok([out.display().to_string(), second])
}

pub fn run(args: impl IntoIterator<Item = String>) -> Result<()> {
    let args = parse_args(args)?;
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine the home directory"))?;
    let now = Local::now();
    let here = crate::ask::current_repo_root().ok();
    register_current(&home, here.as_deref());
    let root = if args.repo { Some(crate::ask::current_repo_root()?) } else { None };
    let out = match args.out {
        Some(out) => out,
        None => default_path(&home, now.date_naive(), &std::env::current_dir()?),
    };
    let view = build_view(&home, root.as_deref(), now);
    for line in make_card(&view, &out, &clipboard::copy_png)? {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ask::events::{self, Event, EventKind};
    use chrono::TimeZone;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap()
    }

    fn answer_event(saved: usize) -> Event {
        Event {
            ts_ms: (now() - chrono::Duration::days(1)).timestamp_millis(),
            session: "s1".to_string(),
            kind: EventKind::Answer,
            tool: "ask".to_string(),
            command: None,
            bytes_before: 4000,
            bytes_after: 400,
            saved_tokens: saved,
            learning: None,
            cost_minutes: None,
            cost_tokens: None,
        }
    }

    fn repo_named(parent: &Path, name: &str, saved: usize) -> PathBuf {
        let root = parent.join(name);
        std::fs::create_dir_all(&root).unwrap();
        events::append(&root, &answer_event(saved));
        root
    }

    #[test]
    fn default_path_is_downloads_then_cwd() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        assert_eq!(default_path(home.path(), day, cwd.path()), cwd.path().join("knot-card-2026-10-28.png"));
        std::fs::create_dir(home.path().join("Downloads")).unwrap();
        assert_eq!(default_path(home.path(), day, cwd.path()), home.path().join("Downloads/knot-card-2026-10-28.png"));
    }

    #[test]
    fn clipboard_failure_still_writes_the_file() {
        let home = tempfile::tempdir().unwrap();
        let out = home.path().join("card.png");
        let view = build_view(home.path(), None, now());
        let lines = make_card(&view, &out, &|_| bail!("no clipboard tool")).unwrap();
        assert!(out.metadata().unwrap().len() > 0);
        assert_eq!(lines[0], out.display().to_string());
        assert_eq!(lines[1], "· couldn't copy to the clipboard (no clipboard tool); the file is saved");
    }

    #[test]
    fn a_copied_card_says_so() {
        let home = tempfile::tempdir().unwrap();
        let out = home.path().join("card.png");
        let view = build_view(home.path(), None, now());
        let lines = make_card(&view, &out, &|_| Ok(())).unwrap();
        assert_eq!(lines[1], "✓ copied to clipboard");
    }

    #[test]
    fn a_missing_output_folder_is_an_error() {
        let home = tempfile::tempdir().unwrap();
        let out = home.path().join("nope/card.png");
        let view = build_view(home.path(), None, now());
        assert!(make_card(&view, &out, &|_| Ok(())).is_err());
        assert!(!home.path().join("nope").exists());
    }

    #[test]
    fn repo_flag_limits_to_this_repo() {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mine = repo_named(work.path(), "mine", 100);
        let other = repo_named(work.path(), "other", 5000);
        repos::register(home.path(), &mine);
        repos::register(home.path(), &other);

        let all = build_view(home.path(), None, now());
        assert_eq!(all.stats.graph.tokens, 5100);
        assert_eq!(all.repo_name, None);

        let one = build_view(home.path(), Some(&mine), now());
        assert_eq!(one.stats.graph.tokens, 100);
        assert_eq!(one.repo_name.as_deref(), Some("mine"));
    }

    #[test]
    fn a_repo_with_older_history_is_listed_by_the_run_path() {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        // events.jsonl already exists, so the log write never registered it
        let root = repo_named(work.path(), "old", 700);
        assert!(repos::list(home.path()).is_empty());
        assert_eq!(build_view(home.path(), None, now()).stats.graph.tokens, 0);

        register_current(home.path(), Some(&root));
        assert_eq!(build_view(home.path(), None, now()).stats.graph.tokens, 700);

        // outside a repo it is a no-op
        let other_home = tempfile::tempdir().unwrap();
        register_current(other_home.path(), None);
        assert!(repos::list(other_home.path()).is_empty());
    }

    #[test]
    fn the_copier_gets_an_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("card.png");
        let view = build_view(dir.path(), None, now());
        let seen = std::cell::RefCell::new(PathBuf::new());
        make_card(&view, &out, &|p| {
            *seen.borrow_mut() = p.to_path_buf();
            Ok(())
        })
        .unwrap();
        assert!(seen.borrow().is_absolute());
    }

    #[test]
    fn args_parse() {
        let a = parse_args(["--repo".to_string(), "--out".to_string(), "x.png".to_string()]).unwrap();
        assert!(a.repo);
        assert_eq!(a.out, Some(PathBuf::from("x.png")));
        assert!(parse_args(["--out".to_string()]).is_err());
        assert!(parse_args(["--bogus".to_string()]).is_err());
    }
}
