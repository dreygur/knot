//! `knot hook <event>`: what Claude Code's hooks run. Reads the hook's JSON on stdin
//! and answers in the JSON shape Claude Code adds to the model's context (plain stdout
//! from a tool hook only reaches the debug log).

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::ask::events::{self, Event, EventKind};
use crate::ask::{flatten, session};
use crate::engine::learnings::{active_learnings, RepoLearning};
use crate::shell::classify::{classify, Plan};

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
    for l in &shown {
        events::learning_shown(root, session_id, &l.file_path, l.frontmatter.cost_minutes, l.frontmatter.cost_tokens);
    }
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

fn pre_tool_json(fields: serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::json!({ "hookEventName": "PreToolUse" });
    out.as_object_mut().expect("object").extend(fields.as_object().expect("object").clone());
    serde_json::json!({ "hookSpecificOutput": out })
}

/// What the PreToolUse hook answers for a classified Bash command; `None` lets it run
/// untouched. `already_denied` is true when this python edit was blocked before in the session; it then runs.
pub(super) fn pre_tool_output(plan: &Plan, command: &str, bin: &Path, swap: bool, already_denied: bool) -> Option<serde_json::Value> {
    match plan {
        // "allow" is safe because classify admits only read-only searches of repo source.
        Plan::Grep { .. } | Plan::SedRange { .. } if swap => {
            let rewritten = format!("{} shell -- {}", shell_words::quote(&bin.to_string_lossy()), shell_words::quote(command));
            Some(pre_tool_json(serde_json::json!({
                "permissionDecision": "allow",
                "updatedInput": { "command": rewritten },
            })))
        }
        Plan::PythonWrite { file } if !already_denied => Some(pre_tool_json(serde_json::json!({
            "permissionDecision": "deny",
            "permissionDecisionReason": format!(
                "Knot: this edits {file} with a python script, which rewrites it without you seeing the result. \
                 Use your Edit tool instead. If you really need the script, run the same command again."
            ),
        }))),
        _ => None,
    }
}

fn log_event(root: &Path, session_id: &str, kind: EventKind, command: &str) {
    events::append(
        root,
        &Event {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            session: session_id.to_string(),
            kind,
            tool: "Bash".into(),
            command: Some(flatten(command, 200)),
            bytes_before: 0,
            bytes_after: 0,
            saved_tokens: 0,
            learning: None,
            cost_minutes: None,
            cost_tokens: None,
        },
    );
}

/// Whether `root` is the repo Claude Code was opened in (`project_dir`, from
/// `CLAUDE_PROJECT_DIR`). The pre-tool hook is installed in one repo's local settings,
/// so a command run in another (an `--add-dir` folder, a nested repo) is not its business.
fn in_project(root: &Path, project_dir: Option<&Path>) -> bool {
    project_dir.is_some_and(|p| !p.as_os_str().is_empty() && crate::ask::repo_root(p) == root)
}

/// The PreToolUse hook: swap grep/sed over code for `knot shell`, block python code edits once.
/// `None` (also on any failure) lets the command run as it would without Knot.
fn pre_tool(
    json: &serde_json::Value,
    root: &Path,
    cwd: &Path,
    project_dir: Option<&Path>,
    session_id: &str,
    swap: bool,
) -> Option<serde_json::Value> {
    if !in_project(root, project_dir) || json.pointer("/tool_name").and_then(|v| v.as_str()) != Some("Bash") {
        return None;
    }
    let command = json.pointer("/tool_input/command").and_then(|v| v.as_str())?;
    let bin = std::env::current_exe().ok()?;
    let plan = classify(command, root, cwd);
    let first_python = matches!(plan, Plan::PythonWrite { .. }) && session::deny_once(root, session_id, command);
    let already_denied = matches!(plan, Plan::PythonWrite { .. }) && !first_python;
    if matches!(plan, Plan::PythonWrite { .. }) {
        log_event(root, session_id, if first_python { EventKind::EditBlocked } else { EventKind::Override }, command);
    }
    pre_tool_output(&plan, command, &bin, swap, already_denied)
}

pub(super) async fn run(event: &str, flags: &[String]) -> Result<()> {
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
    let project_dir = std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from);
    if event == "pre-tool" && !in_project(&root, project_dir.as_deref()) {
        return Ok(()); // another repo's hook writes nothing here
    }

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
        "pre-tool" => {
            let cwd = if cwd.as_os_str().is_empty() { PathBuf::from(".") } else { cwd };
            let swap = !flags.iter().any(|f| f == "--no-swap");
            if let Some(out) = pre_tool(&json, &root, &cwd, project_dir.as_deref(), &session_id, swap) {
                println!("{out}");
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
    fn shown_learnings_are_logged_once_committed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let repo = git2::Repository::init(root).unwrap();
        let learnings = root.join(".knot/learnings");
        std::fs::create_dir_all(&learnings).unwrap();
        std::fs::write(
            learnings.join("2026-10-09_keep.md"),
            "---\nid: 6f1c1d0e-2b1a-4c55-9a53-0a1b2c3d4e5f\ntitle: Keep it\nauthor: Maya\ndate: 2026-10-09\n\
             touches:\n  - src/a.rs\ncost_minutes: 12\ncost_tokens: 27000\n---\n\n## Decided\nKeep it.\n",
        )
        .unwrap();

        // Untracked: the session that wrote it would be crediting itself
        assert!(learnings_for_file(root, &root.join("src/a.rs"), "writer").is_some());
        assert!(events::all(root).is_empty());

        let sig = git2::Signature::now("Maya", "m@x").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(".knot/learnings/2026-10-09_keep.md")).unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "add learning", &tree, &[]).unwrap();

        let text = learnings_for_file(root, &root.join("src/a.rs"), "sess");
        assert!(text.is_some());
        // A second look in the same session shows nothing new, so logs nothing new
        assert!(learnings_for_file(root, &root.join("src/a.rs"), "sess").is_none());

        let logged = events::all(root);
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].kind, EventKind::Learning);
        assert_eq!(logged[0].learning.as_deref(), Some("2026-10-09_keep"));
        assert_eq!(logged[0].cost_tokens, Some(27_000));
        assert_eq!(logged[0].session, "sess");
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

#[cfg(test)]
mod pre_tool_tests {
    use super::*;
    use crate::shell::classify::{classify, Plan};

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn foo() {}\n").unwrap();
        dir
    }

    fn command_of(out: &serde_json::Value) -> &str {
        out.pointer("/hookSpecificOutput/updatedInput/command").and_then(|v| v.as_str()).unwrap()
    }

    #[test]
    fn grep_is_rewritten_to_knot_shell() {
        let dir = repo();
        let plan = classify("grep -rn foo src", dir.path(), dir.path());
        let out = pre_tool_output(&plan, "grep -rn foo src", Path::new("/bin/knot"), true, false).unwrap();
        assert_eq!(command_of(&out), "/bin/knot shell -- 'grep -rn foo src'");
        assert_eq!(out.pointer("/hookSpecificOutput/permissionDecision").unwrap(), "allow");
        assert_eq!(out.pointer("/hookSpecificOutput/hookEventName").unwrap(), "PreToolUse");
    }

    #[test]
    fn swap_off_leaves_grep_alone() {
        let dir = repo();
        let plan = classify("grep -rn foo src", dir.path(), dir.path());
        assert!(pre_tool_output(&plan, "grep -rn foo src", Path::new("/bin/knot"), false, false).is_none());
    }

    #[test]
    fn raw_prefix_is_never_rewritten() {
        let dir = repo();
        let cmd = "KNOT_RAW=1 grep -rn foo src";
        let plan = classify(cmd, dir.path(), dir.path());
        assert!(pre_tool_output(&plan, cmd, Path::new("/bin/knot"), true, false).is_none());
    }

    fn bash(command: &str) -> serde_json::Value {
        serde_json::json!({ "tool_name": "Bash", "tool_input": { "command": command } })
    }

    fn decision(out: &serde_json::Value) -> &str {
        out.pointer("/hookSpecificOutput/permissionDecision").and_then(|v| v.as_str()).unwrap()
    }

    #[test]
    fn hook_acts_only_in_the_repo_claude_code_opened() {
        let outer = repo();
        git2::Repository::init(outer.path()).unwrap();
        let inner_dir = outer.path().join("inner");
        std::fs::create_dir_all(inner_dir.join("src")).unwrap();
        std::fs::write(inner_dir.join("src/b.rs"), "fn foo() {}\n").unwrap();
        git2::Repository::init(&inner_dir).unwrap();
        let inner = crate::ask::repo_root(&inner_dir);
        let python = "python3 -c \"open('src/b.rs','w').write('x')\"";

        for project in [None, Some(outer.path())] {
            assert!(pre_tool(&bash("grep -rn foo src"), &inner, &inner, project, "s1", true).is_none(), "{project:?}");
            assert!(pre_tool(&bash(python), &inner, &inner, project, "s1", true).is_none(), "{project:?}");
        }
        assert!(!crate::ask::state_dir(&inner).exists(), "nothing written for another repo's hook");

        let here = pre_tool(&bash("grep -rn foo src"), &inner, &inner, Some(&inner_dir), "s1", true).unwrap();
        assert_eq!(decision(&here), "allow");
    }

    #[test]
    fn a_deferred_hook_never_swaps_but_still_blocks_python_writes() {
        let dir = repo();
        let root = crate::ask::repo_root(dir.path());
        let project = Some(dir.path());
        for cmd in ["grep -rn foo src", "sed -n '1,1p' src/a.rs"] {
            assert!(pre_tool(&bash(cmd), &root, &root, project, "s1", false).is_none(), "{cmd}");
        }
        let python = "python3 -c \"open('src/a.rs','w').write('x')\"";
        let out = pre_tool(&bash(python), &root, &root, project, "s1", false).unwrap();
        assert_eq!(decision(&out), "deny");
    }

    #[test]
    fn python_write_is_denied_once_then_allowed() {
        let dir = repo();
        let root = dir.path();
        let cmd = "python3 -c \"open('src/a.rs','w').write('x')\"";
        let plan = classify(cmd, root, root);
        assert!(matches!(plan, Plan::PythonWrite { .. }), "{plan:?}");

        let seen_before = !session::deny_once(root, "s1", cmd);
        let out = pre_tool_output(&plan, cmd, Path::new("/bin/knot"), true, seen_before).unwrap();
        assert_eq!(out.pointer("/hookSpecificOutput/permissionDecision").unwrap(), "deny");
        assert!(out.pointer("/hookSpecificOutput/permissionDecisionReason").unwrap().as_str().unwrap().contains("src/a.rs"));

        let seen_again = !session::deny_once(root, "s1", cmd);
        assert!(pre_tool_output(&plan, cmd, Path::new("/bin/knot"), true, seen_again).is_none());
    }
}
