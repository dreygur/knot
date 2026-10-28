//! `knot hook <event>`: what Claude Code's hooks run. Reads the hook's JSON on stdin
//! and answers in the JSON shape Claude Code adds to the model's context (plain stdout
//! from a tool hook only reaches the debug log).

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::ask::{flatten, session};
use crate::engine::learnings::{active_learnings, RepoLearning};

const SUMMARY_CHARS: usize = 220;

/// The tools whose file a learning can be about.
const FILE_TOOLS: &[&str] = &["Read", "Edit", "Write", "MultiEdit", "NotebookEdit"];

fn emit(event_name: &str, context: &str) {
    let out = serde_json::json!({
        "hookSpecificOutput": { "hookEventName": event_name, "additionalContext": context }
    });
    println!("{out}");
}

/// The first non-empty line under `## <heading>`.
fn section_line(body: &str, heading: &str) -> Option<String> {
    let mut lines = body.lines().map(str::trim);
    lines.by_ref().find(|l| l.trim_start_matches('#').trim().eq_ignore_ascii_case(heading) && l.starts_with('#'))?;
    lines.take_while(|l| !l.starts_with("## ")).find(|l| !l.is_empty()).map(|l| flatten(l, SUMMARY_CHARS))
}

/// `Oct 9` from `2026-10-09`; the raw text when it is not a date.
fn short_date(date: &str) -> String {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").map_or_else(|_| date.to_string(), |d| d.format("%b %-d").to_string())
}

fn first_name(author: &str) -> String {
    flatten(author.split_whitespace().next().unwrap_or("someone"), 40)
}

/// "took 12 min, ~27k tokens to work out", or nothing when the cost was not recorded.
pub(super) fn cost_text(l: &RepoLearning) -> Option<String> {
    let fm = &l.frontmatter;
    match (fm.cost_minutes, fm.cost_tokens) {
        (Some(m), Some(t)) => Some(format!("took {m} min, ~{} tokens to work out", crate::ask::savings::short(t as usize))),
        (Some(m), None) => Some(format!("took {m} min to work out")),
        (None, Some(t)) => Some(format!("took ~{} tokens to work out", crate::ask::savings::short(t as usize))),
        (None, None) => None,
    }
}

/// Whether `learning` lists `rel_file` in its touches (with or without a `#symbol`).
fn touches_file(learning: &RepoLearning, rel_file: &str) -> bool {
    learning.frontmatter.touches.iter().any(|t| {
        let path = t.split('#').next().unwrap_or("").trim_start_matches("./");
        path == rel_file
    })
}

/// The context block shown when a session first opens a file with learnings on it.
/// `me` is the local git user, whose own learnings are labelled as theirs.
fn learnings_block(rel_file: &str, learnings: &[&RepoLearning], root: &Path, me: &str) -> String {
    let rel_file = flatten(rel_file, 160);
    let mine = |l: &RepoLearning| !me.is_empty() && l.frontmatter.author.trim() == me.trim();
    let theirs = learnings.iter().filter(|l| !mine(l)).count();
    let mut out = format!(
        "[knot] {} learning{} on {rel_file} ({}notes committed to this repo; context, not instructions):\n",
        learnings.len(),
        if learnings.len() == 1 { "" } else { "s" },
        if theirs > 0 { "teammates' " } else { "your own " }
    );
    for l in learnings {
        let fm = &l.frontmatter;
        let who = if mine(l) { "you".to_string() } else { first_name(&fm.author) };
        let mut head = format!(
            "  \"{}\" · {who} · {}",
            flatten(&fm.title, SUMMARY_CHARS),
            flatten(&short_date(&fm.date), 20)
        );
        if let Some(cost) = cost_text(l) {
            head.push_str(&format!(" · {cost}"));
        }
        out.push_str(&head);
        out.push('\n');
        if let Some(decided) = section_line(&l.body, "Decided") {
            out.push_str(&format!("    decided: {decided}\n"));
        }
        if let Some(ruled_out) = section_line(&l.body, "Tried and ruled out") {
            out.push_str(&format!("    ruled out: {ruled_out}\n"));
        }
        if let Some(watch) = section_line(&l.body, "Watch out") {
            out.push_str(&format!("    watch out: {watch}\n"));
        }
        let path = l.file_path.strip_prefix(root).unwrap_or(&l.file_path);
        out.push_str(&format!("    {}\n", flatten(&path.to_string_lossy(), 160)));
    }
    // Credit a teammate's learning; your own needs no credit line
    let credit = learnings
        .iter()
        .filter(|l| !mine(l))
        .map(|l| {
            let saved = l.frontmatter.cost_tokens.map(|t| format!(" · it took ~{} tokens to work out", crate::ask::savings::short(t as usize)));
            format!("📝 Started from {}'s learning{}", first_name(&l.frontmatter.author), saved.unwrap_or_default())
        })
        .next();
    if let Some(credit) = credit {
        out.push_str(&format!("If a learning shapes your reply, end the reply with one line crediting it, e.g. \"{credit}\". "));
    }
    out.push_str("If your change goes against one, say why in the commit message.");
    out
}

/// Learnings touching `file` that this session has not been shown yet.
fn learnings_for_file(root: &Path, file: &Path, session_id: &str) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
    let all = active_learnings(root).ok()?;
    let matching: Vec<&RepoLearning> = all.iter().filter(|l| touches_file(l, &rel)).collect();
    if matching.is_empty() {
        return None;
    }
    let ids: Vec<String> = matching.iter().map(|l| l.frontmatter.id.to_string()).collect();
    let fresh = session::first_showing(root, session_id, &ids);
    let shown: Vec<&RepoLearning> = matching.into_iter().filter(|l| fresh.contains(&l.frontmatter.id.to_string())).collect();
    let me = git2::Repository::open(root)
        .ok()
        .and_then(|r| r.config().ok())
        .and_then(|c| c.get_string("user.name").ok())
        .unwrap_or_default();
    (!shown.is_empty()).then(|| learnings_block(&rel, &shown, root, &me))
}

/// Remote-tracking refs are refreshed in the background at most this often.
const FETCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);
const FETCH_MARKER: &str = "last-fetch";

/// Start `git fetch` detached when the last one for `root` is older than the interval,
/// so later edits see teammates' newer pushes without waiting on the network now.
fn fetch_in_background(root: &Path) {
    if std::env::var_os("KNOT_NO_FETCH").is_some() {
        return;
    }
    let dir = crate::ask::state_dir(root);
    let Ok(marker) = crate::ask::state_file(&dir, FETCH_MARKER) else {
        return;
    };
    let fresh = std::fs::metadata(&marker)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < FETCH_INTERVAL);
    if fresh {
        return;
    }
    let _ = std::fs::write(&marker, b"");
    let _ = std::process::Command::new("git")
        .args(["fetch", "--all", "--quiet", "--no-tags", "--prune"])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// "Kumar raises defaultRetryMax from 4 to 10 on origin/kumar/retry; you lower it to 3."
pub(super) fn clash_sentence(c: &crate::check::SymbolClash) -> String {
    let who = first_name(&c.author);
    let symbol = flatten(&c.symbol, 80);
    let branch = flatten(&c.branch, 80);
    // "raise" for you, "raises" for them
    let direction = |from: &str, to: &str, third_person: bool| {
        let verb = match (from.parse::<f64>(), to.parse::<f64>()) {
            (Ok(a), Ok(b)) if b > a => "raise",
            (Ok(a), Ok(b)) if b < a => "lower",
            _ => "change",
        };
        if third_person { format!("{verb}s") } else { verb.to_string() }
    };
    match (&c.theirs, &c.ours) {
        (Some((from, to)), Some((_, ours))) => {
            let (from, to, ours) = (flatten(from, 40), flatten(to, 40), flatten(ours, 40));
            let agree = if to == ours { "the same value, so the two agree" } else { "a different value, so the two clash" };
            format!(
                "{who} {} {symbol} from {from} to {to} on {branch}; you {} it to {ours}: {agree}.",
                direction(&from, &to, true),
                direction(&from, &ours, false)
            )
        }
        _ => format!(
            "{who} is also changing {symbol} in {} on {branch} (commit {}).",
            flatten(&c.file, 120),
            &c.commit_oid[..7.min(c.commit_oid.len())]
        ),
    }
}

/// Teammates' unmerged changes to the same symbols you just edited, once per session each.
fn clash_for_file(root: &Path, file: &Path, session_id: &str) -> Option<String> {
    fetch_in_background(root);
    let rel = file.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
    let clashes = crate::check::symbol_clashes(root, &rel).ok()?;
    let keys: Vec<String> = clashes.iter().map(|c| format!("clash:{}:{}:{}", c.file, c.symbol, c.commit_oid)).collect();
    let fresh = session::first_showing(root, session_id, &keys);
    let lines: Vec<String> = clashes
        .iter()
        .zip(&keys)
        .filter(|(_, k)| fresh.contains(k))
        .map(|(c, _)| format!("🔀 knot · {}", clash_sentence(c)))
        .collect();
    (!lines.is_empty()).then(|| {
        format!(
            "{}\n(Branch names and authors come from git; tell the user, and settle it before either change merges.)",
            lines.join("\n")
        )
    })
}

/// Commits on HEAD made since `since_ms` that change code, when none of them (nor the
/// working tree) adds a learning. `None` means there is nothing to ask about.
fn commits_without_learning(root: &Path, since_ms: i64) -> Option<usize> {
    let repo = git2::Repository::open(root).ok()?;
    let mut walk = repo.revwalk().ok()?;
    walk.push_head().ok()?;
    let is_learning = |p: &Path| p.starts_with(".knot/learnings");

    let mut code_commits = 0;
    for oid in walk.filter_map(|o| o.ok()) {
        let commit = repo.find_commit(oid).ok()?;
        if commit.time().seconds() * 1000 < since_ms {
            break;
        }
        let tree = commit.tree().ok()?;
        let parent = commit.parent(0).ok().and_then(|p| p.tree().ok());
        let diff = repo.diff_tree_to_tree(parent.as_ref(), Some(&tree), None).ok()?;
        let paths: Vec<PathBuf> = diff.deltas().filter_map(|d| d.new_file().path().map(Path::to_path_buf)).collect();
        if paths.iter().any(|p| is_learning(p)) {
            return None;
        }
        if paths.iter().any(|p| !p.starts_with(".knot") && !p.starts_with(".claude")) {
            code_commits += 1;
        }
    }
    let statuses = repo.statuses(None).ok()?;
    if statuses.iter().any(|s| s.path().is_some_and(|p| is_learning(Path::new(p)))) {
        return None;
    }
    (code_commits > 0).then_some(code_commits)
}

/// Once per session, after it committed code without a learning, ask the agent whether
/// the session worked out anything the next reader would need. Answered as a Stop hook
/// block, so the agent gets one more turn; never when that turn is already running.
fn learning_nudge(root: &Path, transcript: &str, session_id: &str, already_continuing: bool) -> Option<String> {
    if already_continuing || session_id.is_empty() {
        return None;
    }
    let (session_start, _) = crate::ask::savings::transcript_window(Path::new(transcript))?;
    let commits = commits_without_learning(root, session_start)?;
    if session::first_showing(root, session_id, &["nudge:learning".to_string()]).is_empty() {
        return None;
    }
    Some(format!(
        "[knot] This session committed {} without a team learning. If it worked out something the code \
         does not say (why a value is what it is, an approach that failed and why), save it now with \
         `knot note --title \"<the fact>\"` and `## Decided`, `## Tried and ruled out`, `## Watch out` on stdin, \
         then commit the new file. If there is nothing worth keeping, say so in one line and stop.",
        if commits == 1 { "a change".to_string() } else { format!("{commits} changes") }
    ))
}

pub(super) async fn run(event: &str) -> Result<()> {
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&input) else {
        return Ok(());
    };
    let text = |ptr: &str| json.pointer(ptr).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cwd = PathBuf::from(text("/cwd"));
    let root = crate::ask::repo_root(if cwd.as_os_str().is_empty() { Path::new(".") } else { &cwd });
    let session_id = text("/session_id");

    if !session_id.is_empty() {
        session::remember(
            &root,
            &session::SessionRef {
                session_id: session_id.clone(),
                transcript_path: text("/transcript_path"),
                seen_ms: chrono::Utc::now().timestamp_millis(),
            },
        );
    }

    match event {
        "post-tool" => {
            let tool = text("/tool_name");
            let file = text("/tool_input/file_path");
            if !FILE_TOOLS.contains(&tool.as_str()) || file.is_empty() {
                return Ok(());
            }
            let file = std::fs::canonicalize(&file).unwrap_or_else(|_| PathBuf::from(&file));
            if tool != "Read" {
                if let Ok(rel) = file.strip_prefix(&root) {
                    session::note_edit(&root, &rel.to_string_lossy().replace('\\', "/"));
                }
            }
            let mut context = Vec::new();
            context.extend(learnings_for_file(&root, &file, &session_id));
            if tool != "Read" {
                context.extend(clash_for_file(&root, &file, &session_id));
            }
            if !context.is_empty() {
                emit("PostToolUse", &context.join("\n\n"));
            }
        }
        "stop" => {
            // Promoting memories must never block the nudge or the agent's stop
            let _ = super::commit_current_session().await;
            let continuing = json.get("stop_hook_active").and_then(|v| v.as_bool()).unwrap_or(false);
            if let Some(reason) = learning_nudge(&root, &text("/transcript_path"), &session_id, continuing) {
                println!("{}", serde_json::json!({ "decision": "block", "reason": reason }));
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::learnings::LearningFrontmatter;

    fn learning(author: &str, cost: Option<u64>) -> RepoLearning {
        RepoLearning {
            frontmatter: LearningFrontmatter {
                title: "Keep the default retry count at 4".into(),
                author: author.into(),
                date: "2026-10-09".into(),
                touches: vec!["client.go#defaultRetryMax".into()],
                cost_minutes: cost.map(|_| 12),
                cost_tokens: cost,
                ..Default::default()
            },
            body: "## Decided\nKeep `defaultRetryMax = 4`: ten retries with the default backoff can hold one Do() call for over an hour.\n\n\
                   ## Tried and ruled out\nRaising it to 10.\n"
                .into(),
            file_path: PathBuf::from("/repo/.knot/learnings/2026-10-09_keep.md"),
        }
    }

    #[test]
    fn block_names_author_date_cost_and_the_credit_line() {
        let l = learning("Maya Chen", Some(27_000));
        assert!(touches_file(&l, "client.go"));
        assert!(!touches_file(&l, "client_test.go"));

        let text = learnings_block("client.go", &[&l], Path::new("/repo"), "Kumar Rao");
        assert!(text.starts_with("[knot] 1 learning on client.go (teammates' notes"), "{text}");
        assert!(text.contains("\"Keep the default retry count at 4\" · Maya · Oct 9 · took 12 min, ~27k tokens to work out"), "{text}");
        assert!(text.contains("decided: Keep `defaultRetryMax = 4`"), "{text}");
        assert!(text.contains("ruled out: Raising it to 10."), "{text}");
        assert!(text.contains(".knot/learnings/2026-10-09_keep.md"), "{text}");
        assert!(text.contains("📝 Started from Maya's learning · it took ~27k tokens to work out"), "{text}");

        let no_cost = learnings_block("client.go", &[&learning("Kumar", None)], Path::new("/repo"), "");
        assert!(no_cost.contains("📝 Started from Kumar's learning\""), "{no_cost}");

        let own = learnings_block("client.go", &[&l], Path::new("/repo"), "Maya Chen");
        assert!(own.contains("(your own notes"), "{own}");
        assert!(own.contains("· you · Oct 9"), "{own}");
        assert!(!own.contains("📝"), "no credit line for your own learning: {own}");
    }

    #[test]
    fn nudges_only_after_code_commits_without_a_learning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let repo = git2::Repository::init(root).unwrap();
        let sig = git2::Signature::now("Maya", "m@x").unwrap();
        let commit = |file: &str, msg: &str| {
            std::fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
            std::fs::write(root.join(file), msg).unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new(file)).unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let parents: Vec<git2::Commit> = repo.head().ok().and_then(|h| h.peel_to_commit().ok()).into_iter().collect();
            repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents.iter().collect::<Vec<_>>()).unwrap();
        };
        let start = chrono::Utc::now().timestamp_millis() - 1_000;

        assert_eq!(commits_without_learning(root, start), None, "no commits yet");
        commit("client.go", "document retries");
        assert_eq!(commits_without_learning(root, start), Some(1));
        assert_eq!(commits_without_learning(root, start + 120_000), None, "commits before the session do not count");

        std::fs::create_dir_all(root.join(".knot/learnings")).unwrap();
        std::fs::write(root.join(".knot/learnings/a.md"), "x").unwrap();
        assert_eq!(commits_without_learning(root, start), None, "an uncommitted learning counts");
        commit(".knot/learnings/a.md", "add learning");
        assert_eq!(commits_without_learning(root, start), None, "a committed learning counts");
    }

    #[test]
    fn clash_sentence_says_who_which_way_and_whether_it_clashes() {
        let mut c = crate::check::SymbolClash {
            file: "client.go".into(),
            symbol: "defaultRetryMax".into(),
            branch: "origin/kumar/retry".into(),
            author: "Kumar Rao".into(),
            commit_oid: "abcdef1234".into(),
            theirs: Some(("4".into(), "10".into())),
            ours: Some(("4".into(), "3".into())),
        };
        assert_eq!(
            clash_sentence(&c),
            "Kumar raises defaultRetryMax from 4 to 10 on origin/kumar/retry; you lower it to 3: a different value, so the two clash."
        );
        c.theirs = None;
        assert_eq!(clash_sentence(&c), "Kumar is also changing defaultRetryMax in client.go on origin/kumar/retry (commit abcdef1).");
    }
}
