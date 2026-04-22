#![allow(dead_code)]

mod engine;
mod hooks;
mod jitv;
mod logging;
mod memory;
mod skills;
mod tools;
mod utils;

use anyhow::Result;
use engine::{CommitReport, RecallResult, StatusReport, StorageEngine};
use rmcp::{transport::stdio, ServiceExt};
use std::path::Path;
use tools::KnotServer;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("recall") => {
            let query = args.next().unwrap_or_default();
            if query.is_empty() {
                eprintln!("Usage: knot recall <query> [--limit N]");
                std::process::exit(1);
            }
            let limit = parse_limit_args(args);
            cli_recall(&query, limit).await
        }
        Some("commit") => {
            let session_id = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("Usage: knot commit <session_id> <project_id>"))?;
            let project_id = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("Usage: knot commit <session_id> <project_id>"))?;
            cli_commit(&session_id, &project_id).await
        }
        Some("status") => cli_status().await,
        Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(cmd) => {
            eprintln!(
                "[KNOT] ERROR: Unknown command '{cmd}'. Available: recall, commit, status"
            );
            std::process::exit(1);
        }
        None => run_mcp_server().await,
    }
}

fn print_help() {
    println!("knot v{}", env!("CARGO_PKG_VERSION"));
    println!("Persistent memory pool MCP server");
    println!();
    println!("USAGE:");
    println!("  knot                                    Start MCP server (stdio)");
    println!("  knot recall <query> [--limit N]         Semantic search");
    println!("  knot commit <session_id> <project_id>   Commit session to project");
    println!("  knot status                             Vault health check");
}

fn parse_limit_args(mut args: impl Iterator<Item = String>) -> usize {
    while let Some(arg) = args.next() {
        if arg == "--limit" || arg == "-l" {
            if let Some(n) = args.next().and_then(|s| s.parse().ok()) {
                return n;
            }
        } else if let Some(n) = arg.strip_prefix("--limit=").and_then(|s| s.parse().ok()) {
            return n;
        }
    }
    5
}

async fn cli_recall(query: &str, limit: usize) -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=error".into());
    logging::init(&filter);
    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());
    let engine = StorageEngine::new(&data_dir).await?;
    let results = engine.recall(query, limit.min(20)).await?;
    if !results.is_empty() {
        print!("{}", format_recall_cli(&results));
    }
    Ok(())
}

async fn cli_commit(session_id: &str, project_id: &str) -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=error".into());
    logging::init(&filter);
    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());
    let engine = StorageEngine::new(&data_dir).await?;
    let report = engine.commit_session(session_id, project_id).await?;
    print!("{}", format_commit_cli(&report));
    Ok(())
}

async fn cli_status() -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=error".into());
    logging::init(&filter);
    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());
    let engine = StorageEngine::new(&data_dir).await?;
    let status = engine.knot_status().await?;
    print!("{}", format_status_cli(&status));
    Ok(())
}

async fn run_mcp_server() -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=info".into());
    logging::init(&filter);

    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());

    let is_new = !Path::new(&data_dir).exists();
    std::fs::create_dir_all(&data_dir)?;

    if is_new {
        eprintln!("[KNOT] INFO:  Initialized new memory vault at {}", data_dir);
    }

    tracing::info!("v{} data_dir={data_dir}", env!("CARGO_PKG_VERSION"));

    let engine = StorageEngine::new(&data_dir).await?;
    let session_id = Uuid::new_v4().to_string();
    tracing::info!("session={session_id}");

    // Persist session ID so the Stop hook can call `knot commit`
    let _ = std::fs::write(format!("{data_dir}/.current_session"), &session_id);

    // Register Claude Code hooks (idempotent — silent if already in place)
    if let Ok(bin_path) = std::env::current_exe() {
        match hooks::register(&data_dir, &bin_path) {
            Ok(true) => eprintln!("[KNOT] INFO:  Hooks registered in ~/.claude/settings.json"),
            Ok(false) => {}
            Err(e) => eprintln!("[KNOT] WARN:  Hook registration skipped: {e}"),
        }
    }

    let server = KnotServer::new(engine, session_id);
    let running = server.serve(stdio()).await?;
    running.waiting().await?;

    Ok(())
}

fn format_recall_cli(results: &[RecallResult]) -> String {
    let mut out = format!(
        "[KNOT] {} relevant memor{}:\n\n",
        results.len(),
        if results.len() == 1 { "y" } else { "ies" }
    );
    for (i, r) in results.iter().enumerate() {
        let stale = if r.is_stale { " [STALE]" } else { "" };
        let preview: String = r.annotated_content.chars().take(120).collect();
        out.push_str(&format!(
            "{}. [{}]{} score={:.2}\n   {}\n\n",
            i + 1,
            r.node.scope.scope_type(),
            stale,
            r.node.utility_score,
            preview,
        ));
    }
    out
}

fn format_commit_cli(r: &CommitReport) -> String {
    format!(
        "[KNOT] commit: {} promoted, {} rejected (project={})\n",
        r.promoted_count(),
        r.rejected_count(),
        r.project_id,
    )
}

fn format_status_cli(r: &StatusReport) -> String {
    format!(
        "[KNOT] status  L1(session)={} L2(project)={} L3(global)={} skills={} ghosts={} db={}\n",
        r.l1_nodes, r.l2_nodes, r.l3_nodes, r.skills, r.ghost_count, r.db_health,
    )
}

fn resolve_data_dir() -> String {
    std::env::var("HOME")
        .map(|h| format!("{h}/.knot"))
        .unwrap_or_else(|_| ".knot".into())
}
