//! Registers Knot's MCP server with each agent `knot init` wires, in that agent's user
//! config, so a new session has the tools without a manual step.

use anyhow::Result;
use serde_json::json;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::Agent;
use crate::hooks::{read_settings, refuse_symlink, write_atomic};

const SERVER_NAME: &str = "knot";
/// An agent CLI still running by then is waiting on something, most likely a prompt
/// nobody will answer, so it is stopped.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_EVERY: Duration = Duration::from_millis(50);
/// OpenCode merges this next to `opencode.json`; Knot cannot edit it without losing
/// its comments, so it only looks for an entry there.
const OPENCODE_JSONC: &str = ".config/opencode/opencode.jsonc";

/// How an agent's own CLI ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ran {
    Done,
    /// The command is not installed.
    Missing,
    Failed(String),
}

/// Runs an agent's CLI. Tests pass a fake, so no real CLI ever runs from them.
pub type Runner<'a> = &'a dyn Fn(&str, &[String]) -> Ran;

/// What registering Knot with one agent did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registered {
    Added,
    /// A `knot` entry was already there, and was left as it is.
    AlreadyThere,
    /// The agent's CLI is not installed; the command to run once it is.
    NoCli(String),
    Failed(String),
}

/// Run `program args` with no stdin, so a CLI that would prompt fails instead of
/// hanging `knot init`, and stop it after [`COMMAND_TIMEOUT`].
pub fn run_cli(program: &str, args: &[String]) -> Ran {
    let spawned = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ran::Missing,
        Err(e) => return Ran::Failed(format!("{program}: {e}")),
    };

    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ran::Done,
            Ok(Some(status)) => return Ran::Failed(format!("{program} exited with {status}")),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_EVERY),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Ran::Failed(format!("{program} did not finish in {} s", COMMAND_TIMEOUT.as_secs()));
            }
            Err(e) => return Ran::Failed(format!("{program}: {e}")),
        }
    }
}

/// The server entry an agent's JSON config takes.
fn entry(agent: Agent, bin: &str) -> serde_json::Value {
    match agent {
        Agent::OpenCode => json!({"type": "local", "command": [bin], "enabled": true}),
        _ => json!({"command": bin}),
    }
}

fn in_opencode_jsonc(home: &Path) -> bool {
    std::fs::read_to_string(home.join(OPENCODE_JSONC)).is_ok_and(|text| text.contains(&format!("\"{SERVER_NAME}\"")))
}

/// Add Knot to an agent's JSON config under `home`. A file that does not parse, is not
/// an object or is a symlink is refused, so a hand-edited config is never lost.
fn add_to_json(agent: Agent, home: &Path, bin: &str) -> Result<Registered> {
    let (file, key) = agent.mcp_config();
    let path = home.join(file);
    refuse_symlink(&path)?;
    let mut cfg = read_settings(&path)?;

    match cfg.get(key) {
        None => cfg[key] = json!({}),
        Some(servers) if servers.is_object() => {}
        Some(_) => anyhow::bail!("{}'s \"{key}\" is not an object; Knot leaves it alone", path.display()),
    }
    if cfg[key].get(SERVER_NAME).is_some() || (agent == Agent::OpenCode && in_opencode_jsonc(home)) {
        return Ok(Registered::AlreadyThere);
    }
    cfg[key][SERVER_NAME] = entry(agent, bin);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut content = serde_json::to_string_pretty(&cfg)?;
    content.push('\n');
    write_atomic(&path, &content)?;
    Ok(Registered::Added)
}

/// Add Knot through the agent's own CLI, which owns its config format. Both CLIs
/// would replace or refuse an existing entry, so one already there is left alone first.
fn add_with_cli(agent: Agent, home: &Path, bin: &str, run: Runner) -> Registered {
    if agent.knot_wired(home) {
        return Registered::AlreadyThere;
    }
    let (program, scope): (&str, &[&str]) = match agent {
        Agent::Claude => ("claude", &["-s", "user"]),
        _ => ("codex", &[]),
    };
    let mut args: Vec<String> = ["mcp", "add"].into_iter().chain(scope.iter().copied()).map(String::from).collect();
    args.extend([SERVER_NAME.to_string(), "--".to_string(), bin.to_string()]);

    match run(program, &args) {
        Ran::Done => Registered::Added,
        Ran::Missing => Registered::NoCli(format!("{program} {}", args.join(" "))),
        Ran::Failed(e) => Registered::Failed(e),
    }
}

/// Register Knot's MCP server at `bin` with `agent`, never touching an existing entry.
pub fn register(agent: Agent, home: &Path, bin: &Path, run: Runner) -> Registered {
    let bin = bin.to_string_lossy();
    match agent {
        Agent::Claude | Agent::Codex => add_with_cli(agent, home, &bin, run),
        _ => add_to_json(agent, home, &bin).unwrap_or_else(|e| Registered::Failed(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const BIN: &str = "/opt/knot/knot";

    fn no_cli(_: &str, _: &[String]) -> Ran {
        panic!("this agent needs no CLI");
    }

    fn read(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn json_agents_get_a_knot_entry_in_their_user_config() {
        let home = tempfile::tempdir().unwrap();
        for agent in [Agent::Gemini, Agent::Antigravity, Agent::Cursor, Agent::OpenCode] {
            assert_eq!(register(agent, home.path(), Path::new(BIN), &no_cli), Registered::Added, "{agent:?}");
            assert!(agent.knot_wired(home.path()), "{agent:?}");
        }
        assert_eq!(read(&home.path().join(".cursor/mcp.json")), json!({"mcpServers": {"knot": {"command": BIN}}}));
        assert_eq!(
            read(&home.path().join(".config/opencode/opencode.json")),
            json!({"mcp": {"knot": {"type": "local", "command": [BIN], "enabled": true}}})
        );
        assert!(home.path().join(".gemini/settings.json").is_file());
        assert!(home.path().join(".gemini/config/mcp_config.json").is_file());
    }

    #[test]
    fn other_settings_and_an_existing_knot_entry_are_kept() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(".gemini/settings.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"theme":"dark","mcpServers":{"other":{"command":"x"}}}"#).unwrap();

        assert_eq!(register(Agent::Gemini, home.path(), Path::new(BIN), &no_cli), Registered::Added);
        let cfg = read(&file);
        assert_eq!(cfg["theme"], "dark");
        assert_eq!(cfg["mcpServers"]["other"]["command"], "x");

        // A second run, or a knot entry the person set up by hand, is never rewritten
        std::fs::write(&file, r#"{"mcpServers":{"knot":{"command":"/mine/knot"}}}"#).unwrap();
        assert_eq!(register(Agent::Gemini, home.path(), Path::new(BIN), &no_cli), Registered::AlreadyThere);
        assert_eq!(read(&file)["mcpServers"]["knot"]["command"], "/mine/knot");
    }

    #[test]
    fn a_knot_entry_in_opencode_jsonc_counts_as_there() {
        let home = tempfile::tempdir().unwrap();
        let jsonc = home.path().join(OPENCODE_JSONC);
        std::fs::create_dir_all(jsonc.parent().unwrap()).unwrap();
        std::fs::write(&jsonc, "{\n  // mine\n  \"mcp\": { \"knot\": { \"type\": \"local\" } }\n}\n").unwrap();

        assert_eq!(register(Agent::OpenCode, home.path(), Path::new(BIN), &no_cli), Registered::AlreadyThere);
        assert!(!home.path().join(".config/opencode/opencode.json").exists());
    }

    #[test]
    fn broken_or_odd_configs_are_refused_and_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(".cursor/mcp.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();

        for text in ["{ not json", "[]", r#"{"mcpServers":[]}"#] {
            std::fs::write(&file, text).unwrap();
            let got = register(Agent::Cursor, home.path(), Path::new(BIN), &no_cli);
            assert!(matches!(got, Registered::Failed(_)), "{text}: {got:?}");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_is_not_written_through() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("victim.json");
        std::fs::write(&target, "{}").unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        std::os::unix::fs::symlink(&target, home.path().join(".cursor/mcp.json")).unwrap();

        let got = register(Agent::Cursor, home.path(), Path::new(BIN), &no_cli);

        assert!(matches!(&got, Registered::Failed(e) if e.contains("symlink")), "{got:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{}");
    }

    #[test]
    fn claude_and_codex_go_through_their_own_cli() {
        let home = tempfile::tempdir().unwrap();
        let calls = RefCell::new(Vec::new());
        let record = |program: &str, args: &[String]| {
            calls.borrow_mut().push(format!("{program} {}", args.join(" ")));
            Ran::Done
        };

        assert_eq!(register(Agent::Claude, home.path(), Path::new(BIN), &record), Registered::Added);
        assert_eq!(register(Agent::Codex, home.path(), Path::new(BIN), &record), Registered::Added);

        assert_eq!(*calls.borrow(), vec![format!("claude mcp add -s user knot -- {BIN}"), format!("codex mcp add knot -- {BIN}")]);
    }

    #[test]
    fn an_existing_cli_entry_is_not_added_again() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".claude.json"), r#"{"mcpServers":{"knot":{"command":"/x"}}}"#).unwrap();
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::fs::write(home.path().join(".codex/config.toml"), "[mcp_servers.knot]\ncommand = \"/x\"\n").unwrap();

        for agent in [Agent::Claude, Agent::Codex] {
            assert_eq!(register(agent, home.path(), Path::new(BIN), &no_cli), Registered::AlreadyThere);
        }
    }

    #[test]
    fn a_missing_or_failing_cli_is_reported() {
        let home = tempfile::tempdir().unwrap();
        let missing = |_: &str, _: &[String]| Ran::Missing;
        let failing = |_: &str, _: &[String]| Ran::Failed("codex exited with exit status: 1".into());

        assert_eq!(
            register(Agent::Claude, home.path(), Path::new(BIN), &missing),
            Registered::NoCli(format!("claude mcp add -s user knot -- {BIN}"))
        );
        assert_eq!(
            register(Agent::Codex, home.path(), Path::new(BIN), &failing),
            Registered::Failed("codex exited with exit status: 1".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_cli_gives_no_stdin_and_reports_missing_commands() {
        assert_eq!(run_cli("knot-no-such-command-anywhere", &[]), Ran::Missing);
        // `cat` with no stdin ends at once instead of waiting for input
        assert_eq!(run_cli("cat", &[]), Ran::Done);
        assert!(matches!(run_cli("false", &[]), Ran::Failed(_)));
    }
}
