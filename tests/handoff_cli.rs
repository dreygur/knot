//! Runs `knot handoff` as an agent would, in a throwaway repo with its own HOME.
#![cfg(unix)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const NOTE: &str = "# Cap retries at four\n\n## Done\n6d1fbf3 adds the cap.\n\n## Next step\nWire the test.\n";

fn knot(repo: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_knot"))
        .args(args)
        .current_dir(repo)
        .env("HOME", repo.join("home"))
        .env("KNOT_DATA_DIR", repo.join("data"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn knot");
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn save_show_clear_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();

    let saved = knot(repo, &["handoff", "save"], NOTE);
    assert!(saved.status.success(), "{}", String::from_utf8_lossy(&saved.stderr));
    assert_eq!(std::fs::read_to_string(repo.join(".knot/cache/handoff.md")).unwrap(), NOTE);

    let shown = knot(repo, &["handoff", "show"], "");
    assert!(shown.status.success());
    assert_eq!(stdout(&shown), NOTE);

    assert!(knot(repo, &["handoff", "clear"], "").status.success());
    assert!(!repo.join(".knot/cache/handoff.md").exists());
    assert!(stdout(&knot(repo, &["handoff", "show"], "")).contains("No handoff"));
}

#[test]
fn an_empty_note_or_a_bad_action_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!knot(dir.path(), &["handoff", "save"], "  \n").status.success());
    assert_eq!(knot(dir.path(), &["handoff", "keep"], "").status.code(), Some(2));
    assert!(!dir.path().join(".knot/cache/handoff.md").exists());
}

#[test]
fn a_symlinked_cache_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".knot")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), dir.path().join(".knot/cache")).unwrap();

    let out = knot(dir.path(), &["handoff", "save"], NOTE);

    assert!(!out.status.success());
    assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0, "nothing lands where the link points");
}
