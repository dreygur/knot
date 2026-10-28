use std::io::IsTerminal;
use std::path::Path;

use anyhow::{bail, Result};

use super::picker::{self, Picker, Row};
use super::{count, register_path, PathChange, SUPPORTED_LANGUAGES};
use crate::hosts::mcp::{self, Registered};
use crate::hosts::{Agent, Target};

const USAGE: &str = "knot init [--agents <id,id...>] [--no-swaps] [-y|--yes]";
const PICKER_TITLE: &str = "knot init · pick the agents you use:";
const LABEL_WIDTH: usize = 12;
const DONE: char = '✓';
const NOTE: char = '·';
const FAILED: char = '✗';

/// Which agents to wire, as the command line asked.
#[derive(Debug, PartialEq, Eq)]
enum Selection {
    /// No flag: the picker on a terminal, the detected agents otherwise.
    Ask,
    /// `--yes`: every agent found on this machine, no prompt.
    Detected,
    /// `--agents`: exactly these.
    Named(Vec<Agent>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Terminal {
    Interactive,
    Piped,
}

fn known_ids() -> String {
    Agent::ALL.map(Agent::id).join(", ")
}

fn parse_ids(value: &str, into: &mut Vec<Agent>) -> Result<()> {
    for id in value.split(',').map(str::trim).filter(|id| !id.is_empty()) {
        let Some(agent) = Agent::from_id(id) else {
            bail!("unknown agent '{id}'; known: {}", known_ids());
        };
        if !into.contains(&agent) {
            into.push(agent);
        }
    }
    Ok(())
}

/// The agent selection and whether swaps go on: yes unless `--no-swaps` was given.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<(Selection, bool)> {
    let mut args = args.into_iter().peekable();
    let mut named: Option<Vec<Agent>> = None;
    let mut yes = false;
    let mut swaps = true;

    while let Some(arg) = args.next() {
        if arg == "-y" || arg == "--yes" {
            yes = true;
            continue;
        }
        if arg == "--no-swaps" {
            swaps = false;
            continue;
        }
        // Swaps used to be opt-in; scripts that still ask for them keep working
        if arg == "--swaps" {
            continue;
        }
        // `--agents claude,codex`, `--agents claude codex` and `--agents=claude` all work
        let value = if arg == "--agents" {
            let mut values = Vec::new();
            while let Some(next) = args.next_if(|a| !a.starts_with('-')) {
                values.push(next);
            }
            values.join(",")
        } else if let Some(value) = arg.strip_prefix("--agents=") {
            value.to_string()
        } else {
            bail!("unknown option '{arg}'\nusage: {USAGE}");
        };
        let list = named.get_or_insert_with(Vec::new);
        parse_ids(&value, list)?;
        if list.is_empty() {
            bail!("--agents needs at least one id; known: {}", known_ids());
        }
    }

    let selection = match (named, yes) {
        (Some(list), _) => Selection::Named(list),
        (None, true) => Selection::Detected,
        (None, false) => Selection::Ask,
    };
    Ok((selection, swaps))
}

/// What wiring `agent` writes: repo files first, then anything under `~`.
fn summary(agent: Agent) -> String {
    let repo: Vec<String> = agent
        .targets()
        .iter()
        .filter(|t| matches!(t, Target::Repo(_)))
        .map(|t| t.display())
        .collect();
    let home = agent.targets().iter().filter(|t| matches!(t, Target::Home(_))).map(|t| format!("+ {}", t.display()));
    std::iter::once(repo.join(", ")).chain(home).collect::<Vec<_>>().join(" · ")
}

/// Where `agent` lists its MCP servers, as the person reads it.
fn mcp_place(agent: Agent) -> String {
    format!("~/{}", agent.mcp_config().0)
}

/// Every row writes outside the repo, since each agent's MCP entry is in its user config.
fn picker_rows(found: &[(Agent, bool)]) -> Vec<Row> {
    found
        .iter()
        .map(|&(agent, detected)| Row {
            label: agent.id().to_string(),
            detected,
            summary: format!("{} · + MCP {}", summary(agent), mcp_place(agent)),
            machine_wide: true,
        })
        .collect()
}

/// The extra picker row for per-repo shell swaps; it starts checked unless `--no-swaps` was given.
fn swaps_row() -> Row {
    Row {
        label: "swaps".to_string(),
        detected: true,
        summary: "Claude Code only · grep and sed over code answered by knot, python code edits stopped · .claude/settings.local.json".to_string(),
        machine_wide: false,
    }
}

/// The picker: one row per agent, then the swaps row, checked unless `swaps` is off.
fn agent_picker(found: &[(Agent, bool)], swaps: bool) -> Picker {
    let mut rows = picker_rows(found);
    rows.push(swaps_row());
    let mut picker = Picker::new(PICKER_TITLE, rows);
    if !swaps {
        picker.uncheck(found.len());
    }
    picker
}

/// The agents to wire and whether to turn swaps on, or `None` when the person cancelled the picker.
fn choose(
    selection: Selection,
    swaps: bool,
    found: &[(Agent, bool)],
    terminal: Terminal,
) -> Result<Option<(Vec<Agent>, bool)>> {
    let detected = || found.iter().filter(|(_, d)| *d).map(|(a, _)| *a).collect();
    match (selection, terminal) {
        (Selection::Named(list), _) => Ok(Some((list, swaps))),
        (Selection::Detected, _) | (Selection::Ask, Terminal::Piped) => Ok(Some((detected(), swaps))),
        (Selection::Ask, Terminal::Interactive) => {
            Ok(picker::run(agent_picker(found, swaps))?.map(|rows| {
                let on = rows.contains(&found.len());
                (rows.into_iter().filter(|&i| i < found.len()).map(|i| found[i].0).collect(), on)
            }))
        }
    }
}

/// Turn the swap hook on for this repo, as one report line. It needs Claude Code, the one
/// agent whose hooks Knot writes.
fn swaps_line(root: &Path, agents: &[Agent], bin: &Path) -> String {
    if !agents.contains(&Agent::Claude) {
        return line(NOTE, "swaps", "needs Claude Code; pick claude to turn them on");
    }
    match crate::hooks::register_repo(root, bin) {
        Ok(crate::hooks::SwapInstall::Installed) => line(
            DONE,
            "swaps",
            "grep and sed over code answered by knot, python code edits stopped · this repo only",
        ),
        Ok(crate::hooks::SwapInstall::Deferred { to }) => {
            line(NOTE, "swaps", &format!("another hook already rewrites Bash ({to}); python code edits stopped only"))
        }
        Err(e) => line(FAILED, "swaps", &format!("{e}")),
    }
}

fn line(mark: char, label: &str, detail: &str) -> String {
    format!("{mark} {label:<LABEL_WIDTH$}  {detail}").trim_end().to_string()
}

fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Register Knot's MCP server with `agent`, as one report line.
fn mcp_line(agent: Agent, home: &Path, bin: &Path, run: mcp::Runner) -> String {
    let place = mcp_place(agent);
    let (mark, detail) = match mcp::register(agent, home, bin, run) {
        Registered::Added => (DONE, format!("added to {place}")),
        Registered::AlreadyThere => (DONE, format!("already in {place}")),
        Registered::NoCli(command) => (NOTE, format!("no {} command found; run: {command}", agent.id())),
        Registered::ByHand(what) => (NOTE, what),
        Registered::Failed(e) => (FAILED, format!("not added: {e}")),
    };
    line(mark, "mcp", &format!("{} · {detail}", agent.id()))
}

/// Write the picked agents' files, register Knot's MCP server with each, and index the
/// repo, returning one report line each. The index line is always last.
fn wire(root: &Path, home: &Path, agents: &[Agent], bin: &Path, run: mcp::Runner) -> Result<Vec<String>> {
    crate::hosts::configure_agents(root, home, agents)?;
    let mut lines: Vec<String> = agents.iter().map(|a| line(DONE, a.id(), &summary(*a))).collect();
    if agents.is_empty() {
        lines.push(line(NOTE, "agents", "none picked · knot init --agents <id> wires one by name"));
    }
    lines.extend(agents.iter().map(|a| mcp_line(*a, home, bin, run)));

    // Index now so the first question an agent asks is answered from a warm cache
    let started = std::time::Instant::now();
    lines.push(match crate::ask::load_corpus(root, Some(&crate::ask::index_root(root))) {
        Ok((corpus, stats)) if stats.parsed + stats.reused > 0 => line(
            DONE,
            "index",
            &format!(
                "{}, {} in {} ms · .knot/cache/",
                count(stats.parsed + stats.reused, "file"),
                count(corpus.graph.symbols.len(), "symbol"),
                started.elapsed().as_millis()
            ),
        ),
        Ok(_) => line(NOTE, "index", &format!("no {SUPPORTED_LANGUAGES} files yet; code commands index on first use")),
        Err(e) => line(FAILED, "index", &format!("{e}; code commands index on first use")),
    });
    Ok(lines)
}

fn next_steps(agents: &[Agent]) -> Vec<String> {
    let mut steps = Vec::new();
    if !agents.is_empty() {
        steps.push(format!("{NOTE} restart your agent so a new session picks up knot"));
    }
    steps.push(format!("{NOTE} knot ask \"<question>\" finds the code that answers it"));
    steps
}

/// `knot init`: pick agents, then add knot to PATH, write only those agents'
/// instructions, and index the repo into `.knot/cache/`. Cancelling writes nothing.
pub(super) fn run(args: impl IntoIterator<Item = String>) -> Result<()> {
    let (selection, swaps) = parse_args(args)?;
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine the home directory"))?;
    let path = std::env::var_os("PATH");
    let found: Vec<(Agent, bool)> = Agent::ALL.into_iter().map(|a| (a, a.detected(&home, path.as_deref()))).collect();
    let terminal = if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
        Terminal::Interactive
    } else {
        Terminal::Piped
    };

    let Some((agents, swaps)) = choose(selection, swaps, &found, terminal)? else {
        eprintln!("{NOTE} cancelled, nothing written");
        return Ok(());
    };

    let bin = std::env::current_exe()?;
    let bin_dir = bin.parent().ok_or_else(|| anyhow::anyhow!("cannot determine the binary's directory"))?;
    if let PathChange::Added { place, activate } = register_path(bin_dir)? {
        println!("{}", line(DONE, "PATH", &format!("{} added to {place} · run {activate}", tilde(bin_dir, &home))));
    }

    let root = crate::ask::current_repo_root()?;
    let mut lines = wire(&root, &home, &agents, &bin, &mcp::run_cli)?;
    crate::card::repos::register(&home, &root);
    if swaps {
        lines.insert(lines.len() - 1, swaps_line(&root, &agents, &bin));
    }
    for l in lines.into_iter().chain(next_steps(&agents)) {
        println!("{l}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_agent_lists_in_every_spelling() {
        assert_eq!(parse_args(args(&[])).unwrap(), (Selection::Ask, true), "swaps are on unless turned off");
        assert_eq!(parse_args(args(&["-y", "--swaps"])).unwrap(), (Selection::Detected, true));
        assert_eq!(parse_args(args(&["-y", "--no-swaps"])).unwrap(), (Selection::Detected, false));
        let both = (Selection::Named(vec![Agent::Claude, Agent::Codex]), true);
        assert_eq!(parse_args(args(&["--agents", "claude,codex"])).unwrap(), both);
        assert_eq!(parse_args(args(&["--agents", "claude", "codex", "--yes"])).unwrap(), both);
        assert_eq!(parse_args(args(&["--agents=claude,claude,codex"])).unwrap(), both);
    }

    #[test]
    fn rejects_unknown_agents_and_options() {
        let err = parse_args(args(&["--agents", "vim"])).unwrap_err().to_string();
        assert!(err.contains("unknown agent 'vim'") && err.contains("claude, codex"), "{err}");
        assert!(parse_args(args(&["--agents"])).unwrap_err().to_string().contains("at least one id"));
        assert!(parse_args(args(&["--all"])).unwrap_err().to_string().contains("unknown option '--all'"));
    }

    #[test]
    fn without_a_terminal_only_detected_agents_are_wired() {
        let found = [(Agent::Claude, true), (Agent::Codex, false), (Agent::Cursor, true)];
        assert_eq!(choose(Selection::Ask, false, &found, Terminal::Piped).unwrap(), Some((vec![Agent::Claude, Agent::Cursor], false)));
        assert_eq!(choose(Selection::Detected, true, &found, Terminal::Interactive).unwrap(), Some((vec![Agent::Claude, Agent::Cursor], true)));
        let named = Selection::Named(vec![Agent::Codex]);
        assert_eq!(choose(named, false, &found, Terminal::Interactive).unwrap(), Some((vec![Agent::Codex], false)));
    }

    #[test]
    fn the_swaps_row_starts_checked_unless_turned_off() {
        let found = [(Agent::Claude, true), (Agent::Codex, false)];
        assert_eq!(agent_picker(&found, true).picked(), vec![0, 2]);
        assert_eq!(agent_picker(&found, false).picked(), vec![0]);
        let (_, swaps) = parse_args(args(&[])).unwrap();
        assert_eq!(agent_picker(&found, swaps).picked(), vec![0, 2], "no flag: swaps checked");
    }

    #[test]
    fn summaries_name_machine_wide_writes() {
        assert_eq!(summary(Agent::Claude), "CLAUDE.md");
        assert_eq!(summary(Agent::Antigravity), "GEMINI.md · + ~/.gemini/skills/knot/SKILL.md");
    }

    fn no_claude(_: &str, _: &[String]) -> mcp::Ran {
        mcp::Ran::Missing
    }

    #[test]
    fn every_picked_agent_gets_one_mcp_line() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let ran = |program: &str, _: &[String]| if program == "codex" { mcp::Ran::Done } else { mcp::Ran::Missing };

        let lines = wire(repo.path(), home.path(), &Agent::ALL, Path::new("/bin/knot"), &ran).unwrap();

        let mcp: Vec<&str> = lines.iter().map(String::as_str).filter(|l| l.contains(" mcp ")).collect();
        assert_eq!(
            mcp,
            vec![
                "· mcp           claude · no claude command found; run: claude mcp add -s user knot -- /bin/knot",
                "✓ mcp           codex · added to ~/.codex/config.toml",
                "✓ mcp           opencode · added to ~/.config/opencode/opencode.json",
                "✓ mcp           gemini · added to ~/.gemini/settings.json",
                "✓ mcp           antigravity · added to ~/.gemini/config/mcp_config.json",
                "✓ mcp           cursor · added to ~/.cursor/mcp.json",
            ]
        );
        assert!(lines.last().unwrap().contains(" index "), "the index line stays last: {lines:?}");

        let again = wire(repo.path(), home.path(), &[Agent::Cursor], Path::new("/bin/knot"), &ran).unwrap();
        assert_eq!(again[1], "✓ mcp           cursor · already in ~/.cursor/mcp.json");
    }

    #[test]
    fn wire_writes_only_picked_agents_and_indexes_into_the_repo() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("lib.rs"), "pub fn alpha() {}\n").unwrap();

        let lines = wire(repo.path(), home.path(), &[Agent::Claude], Path::new("/bin/knot"), &no_claude).unwrap();

        assert_eq!(lines[0], "✓ claude        CLAUDE.md");
        assert_eq!(lines[1], "· mcp           claude · no claude command found; run: claude mcp add -s user knot -- /bin/knot");
        assert!(lines[2].starts_with("✓ index         1 file, 1 symbol in "), "{}", lines[2]);
        assert!(lines[2].ends_with(" ms · .knot/cache/"), "{}", lines[2]);
        assert!(repo.path().join("CLAUDE.md").is_file());
        assert!(!repo.path().join("AGENTS.md").exists());
        assert!(repo.path().join(".knot/cache/index.json").is_file());
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0, "nothing written to the home folder");
    }

    #[test]
    fn wire_with_no_agents_still_indexes_and_says_so() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let lines = wire(repo.path(), home.path(), &[], Path::new("/bin/knot"), &no_claude).unwrap();
        assert_eq!(lines[0], "· agents        none picked · knot init --agents <id> wires one by name");
        assert!(lines[1].starts_with("· index         no "), "{}", lines[1]);
        assert_eq!(next_steps(&[]).len(), 1, "no restart hint when nothing was wired");
    }
}
