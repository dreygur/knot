//! Starts the real binary as an MCP server over stdio and checks the tool list a
//! client sees, so an rmcp or schemars upgrade can't rename a tool or drop a parameter.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);
const PROTOCOL_VERSION: &str = "2024-11-05";
const INITIALIZE_ID: u64 = 1;
const TOOLS_LIST_ID: u64 = 2;

/// Every tool and its input properties, sorted, as v0.4.1 published them.
const EXPECTED_TOOLS: &[(&str, &[&str])] = &[
    ("commit_session", &["project_id"]),
    ("delete_skill", &["force", "skill_name"]),
    ("delete_wisdom", &["node_id"]),
    ("execute_skill", &["skill_name", "variables"]),
    ("forget_node", &["node_id"]),
    ("jit_verify", &["node_id"]),
    ("knot_ask", &["limit", "question", "source"]),
    ("knot_check", &["base_ref", "check_remote"]),
    ("knot_file_api", &["file"]),
    ("knot_grep", &["fixed", "ignore_case", "pattern", "within"]),
    ("knot_map", &[]),
    ("knot_status", &[]),
    ("knot_trace_calls", &["depth", "direction", "symbol"]),
    ("link_nodes", &["edge_type", "source_id", "target_id"]),
    ("list_nodes", &["scope_id", "scope_type", "tag_filter"]),
    ("prune_ghosts", &[]),
    ("recall_memory", &["full_content", "limit", "query"]),
    ("recall_skills", &["query"]),
    (
        "save_skill",
        &["description", "name", "prerequisites", "related_node_id", "steps", "verification_command"],
    ),
    (
        "save_wisdom",
        &[
            "command_exit_code",
            "content",
            "origin_agent",
            "parent_id",
            "project_id",
            "scope",
            "tags",
            "verification_path",
        ],
    ),
];

/// Runs the server against throwaway dirs, so it never touches the real vault or
/// ~/.claude/settings.json, from a cwd outside any git repo.
fn start_server(sandbox: &std::path::Path) -> Child {
    let cwd = sandbox.join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    Command::new(env!("CARGO_BIN_EXE_knot"))
        .current_dir(&cwd)
        .env("HOME", sandbox.join("home"))
        .env("KNOT_DATA_DIR", sandbox.join("data"))
        .env_remove("KNOT_READ_ONLY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn knot")
}

/// Reads stdout on its own thread, one JSON message per line, so a hung server
/// fails the test on a timeout instead of blocking it forever.
fn read_messages(child: &mut Child) -> Receiver<Value> {
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str(&line) else { continue };
            if tx.send(message).is_err() {
                break;
            }
        }
    });
    rx
}

fn send(stdin: &mut ChildStdin, message: Value) {
    writeln!(stdin, "{message}").unwrap();
    stdin.flush().unwrap();
}

fn response(messages: &Receiver<Value>, id: u64) -> Value {
    loop {
        let message = messages.recv_timeout(RESPONSE_TIMEOUT).expect("server response");
        if message["id"] == id {
            return message;
        }
    }
}

fn property_names(tool: &Value) -> Vec<String> {
    let schema = &tool["inputSchema"];
    assert_eq!(schema["type"], "object", "{tool}");
    let mut names: Vec<String> = schema["properties"]
        .as_object()
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn lists_every_tool_over_stdio() {
    let sandbox = tempfile::tempdir().unwrap();
    let mut child = start_server(sandbox.path());
    let messages = read_messages(&mut child);
    let mut stdin = child.stdin.take().unwrap();

    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": INITIALIZE_ID, "method": "initialize", "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "mcp_stdio_test", "version": "0"},
        }}),
    );
    let init = response(&messages, INITIALIZE_ID);
    send(&mut stdin, json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    send(&mut stdin, json!({"jsonrpc": "2.0", "id": TOOLS_LIST_ID, "method": "tools/list", "params": {}}));
    let list = response(&messages, TOOLS_LIST_ID);
    child.kill().ok();
    child.wait().ok();

    assert_eq!(init["result"]["serverInfo"]["name"], "knot", "{init}");
    assert!(init["result"]["capabilities"]["tools"].is_object(), "{init}");
    assert!(init["result"]["instructions"].as_str().is_some_and(|i| i.contains("`knot_ask`")), "{init}");

    let mut tools: Vec<&Value> = list["result"]["tools"].as_array().expect("tools array").iter().collect();
    tools.sort_by_key(|t| t["name"].as_str().unwrap_or_default().to_string());
    let listed: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    let expected: Vec<&str> = EXPECTED_TOOLS.iter().map(|(name, _)| *name).collect();
    assert_eq!(listed, expected);

    for (tool, (name, props)) in tools.iter().zip(EXPECTED_TOOLS) {
        assert!(tool["description"].as_str().is_some_and(|d| !d.is_empty()), "{name} has no description");
        assert_eq!(property_names(tool), *props, "{name} parameters changed");
    }
}
