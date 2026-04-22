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
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
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
        Some("logs") => {
            let follow = args.any(|a| a == "--follow" || a == "-f");
            cli_logs(follow).await
        }
        Some("init") => cli_init(),
        Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(cmd) => {
            eprintln!(
                "[KNOT] ERROR: Unknown command '{cmd}'. Available: recall, commit, status, logs, init"
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
    println!("  knot logs [--follow|-f]                 View activity log");
    println!("  knot init                               Register binary directory in PATH");
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

// ── PATH helpers ──────────────────────────────────────────────────────────────

fn is_in_path(dir: &Path) -> bool {
    let sep = if cfg!(windows) { ';' } else { ':' };
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    std::env::var("PATH")
        .unwrap_or_default()
        .split(sep)
        .any(|p| {
            let p = PathBuf::from(p);
            p == dir || p.canonicalize().ok().as_deref() == Some(canonical.as_path())
        })
}

fn warn_if_not_in_path() {
    if let Ok(bin) = std::env::current_exe() {
        if let Some(dir) = bin.parent() {
            if !is_in_path(dir) {
                eprintln!("[KNOT] WARN:  Knot is not in your PATH. Run 'knot init' to fix this.");
            }
        }
    }
}

// ── CLI commands ──────────────────────────────────────────────────────────────

fn log_path_for(data_dir: &str) -> PathBuf {
    PathBuf::from(data_dir).join("activity.log")
}

async fn cli_recall(query: &str, limit: usize) -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=error".into());
    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());
    logging::init(&filter, Some(&log_path_for(&data_dir)));
    warn_if_not_in_path();
    let engine = StorageEngine::new(&data_dir).await?;
    let results = engine.recall(query, limit.min(20)).await?;
    if !results.is_empty() {
        print!("{}", format_recall_cli(&results));
    }
    Ok(())
}

async fn cli_commit(session_id: &str, project_id: &str) -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=error".into());
    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());
    logging::init(&filter, Some(&log_path_for(&data_dir)));
    warn_if_not_in_path();
    let engine = StorageEngine::new(&data_dir).await?;
    let report = engine.commit_session(session_id, project_id).await?;
    print!("{}", format_commit_cli(&report));
    Ok(())
}

async fn cli_status() -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=error".into());
    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());
    logging::init(&filter, Some(&log_path_for(&data_dir)));
    warn_if_not_in_path();
    let engine = StorageEngine::new(&data_dir).await?;
    let status = engine.knot_status().await?;
    print!("{}", format_status_cli(&status));
    Ok(())
}

async fn cli_logs(follow: bool) -> Result<()> {
    // No file logging here - avoid the log growing from reading itself.
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=error".into());
    logging::init(&filter, None);
    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());
    let log_path = log_path_for(&data_dir);

    if !log_path.exists() {
        println!("[KNOT] INFO:  No activity log at {}", log_path.display());
        return Ok(());
    }

    if follow {
        let file = std::fs::File::open(&log_path)?;
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::End(0))?;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
                Ok(_) => print!("{line}"),
                Err(e) => return Err(e.into()),
            }
        }
    } else {
        let content = std::fs::read_to_string(&log_path)?;
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(100);
        for line in &lines[start..] {
            println!("{line}");
        }
        Ok(())
    }
}

fn cli_init() -> Result<()> {
    let bin = std::env::current_exe()?;
    let bin_dir = bin
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine binary directory"))?;

    if is_in_path(bin_dir) {
        println!("[KNOT] INFO:  {} is already in PATH.", bin_dir.display());
        return Ok(());
    }

    register_path(bin_dir)?;
    println!("[KNOT] SUCCESS: Path registered. Restart your terminal to use 'knot' globally.");
    Ok(())
}

#[cfg(unix)]
fn register_path(bin_dir: &Path) -> Result<()> {
    let home = std::env::var("HOME")?;
    let shell = std::env::var("SHELL").unwrap_or_default();

    let (config, line) = if shell.ends_with("zsh") {
        (
            PathBuf::from(&home).join(".zshrc"),
            format!("\nexport PATH=\"$PATH:{}\"  # added by knot init\n", bin_dir.display()),
        )
    } else if shell.ends_with("fish") {
        (
            PathBuf::from(&home).join(".config/fish/config.fish"),
            format!("\nfish_add_path {}  # added by knot init\n", bin_dir.display()),
        )
    } else {
        (
            PathBuf::from(&home).join(".bashrc"),
            format!("\nexport PATH=\"$PATH:{}\"  # added by knot init\n", bin_dir.display()),
        )
    };

    let existing = std::fs::read_to_string(&config).unwrap_or_default();
    if existing.contains("# added by knot init") {
        println!("[KNOT] INFO:  PATH entry already present in {}", config.display());
        return Ok(());
    }

    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config)?
        .write_all(line.as_bytes())?;

    println!("[KNOT] INFO:  Appended PATH entry to {}", config.display());
    Ok(())
}

#[cfg(windows)]
fn register_path(bin_dir: &Path) -> Result<()> {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME"))?;
    let profile = PathBuf::from(&home)
        .join("Documents")
        .join("PowerShell")
        .join("Microsoft.PowerShell_profile.ps1");

    let existing = std::fs::read_to_string(&profile).unwrap_or_default();
    if existing.contains("# added by knot init") {
        println!("[KNOT] INFO:  PATH entry already present in {}", profile.display());
        return Ok(());
    }

    if let Some(parent) = profile.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = format!(
        "\n$env:PATH += \";{}\"  # added by knot init\n",
        bin_dir.display()
    );
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&profile)?
        .write_all(line.as_bytes())?;

    println!("[KNOT] INFO:  Appended PATH entry to {}", profile.display());
    Ok(())
}

// ── MCP server ────────────────────────────────────────────────────────────────

async fn run_mcp_server() -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=info".into());
    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());

    let is_new = !Path::new(&data_dir).exists();
    std::fs::create_dir_all(&data_dir)?;

    logging::init(&filter, Some(&log_path_for(&data_dir)));

    if is_new {
        eprintln!("[KNOT] INFO:  Initialized new memory vault at {}", data_dir);
    }

    warn_if_not_in_path();
    tracing::info!("v{} data_dir={data_dir}", env!("CARGO_PKG_VERSION"));

    let engine = StorageEngine::new(&data_dir).await?;
    let session_id = Uuid::new_v4().to_string();
    tracing::info!("session={session_id}");

    let _ = std::fs::write(format!("{data_dir}/.current_session"), &session_id);

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

// ── Formatters ────────────────────────────────────────────────────────────────

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
