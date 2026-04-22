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
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tools::KnotServer;
use uuid::Uuid;

const BLOCK_START: &str = "# >>> knot initialize >>>";
const BLOCK_END: &str = "# <<< knot initialize <<<";

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
                eprintln!(
                    "[KNOT] WARN:  Knot is not in your PATH. Run 'knot init' to enable global access."
                );
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
    let _guard = logging::init(&filter, Some(&log_path_for(&data_dir)));
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
    let _guard = logging::init(&filter, Some(&log_path_for(&data_dir)));
    warn_if_not_in_path();
    let engine = StorageEngine::new(&data_dir).await?;
    let report = engine.commit_session(session_id, project_id).await?;
    print!("{}", format_commit_cli(&report));
    Ok(())
}

async fn cli_status() -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=error".into());
    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());
    let _guard = logging::init(&filter, Some(&log_path_for(&data_dir)));
    warn_if_not_in_path();
    let engine = StorageEngine::new(&data_dir).await?;
    let status = engine.knot_status().await?;
    print!("{}", format_status_cli(&status));
    Ok(())
}

async fn cli_logs(follow: bool) -> Result<()> {
    // Pass None to avoid the log growing from reading itself.
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=error".into());
    let _guard = logging::init(&filter, None);
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

    register_path(bin_dir)
}

// ── PATH registration ─────────────────────────────────────────────────────────

/// Build the managed block content for POSIX shells.
#[cfg(unix)]
fn path_block(bin_dir: &Path) -> String {
    format!(
        "\n{BLOCK_START}\nexport PATH=\"{}:$PATH\"\n{BLOCK_END}\n",
        bin_dir.display()
    )
}

/// Replace or append the managed block in `content`, returning the new string.
fn inject_block(content: &str, block: &str) -> String {
    if let (Some(s), Some(e)) = (content.find(BLOCK_START), content.find(BLOCK_END)) {
        if s < e {
            let after_end = e + BLOCK_END.len();
            return format!("{}{}{}", &content[..s], block.trim_start_matches('\n'), &content[after_end..]);
        }
    }
    format!("{content}{block}")
}

#[cfg(unix)]
fn register_path(bin_dir: &Path) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let shell = std::env::var("SHELL").unwrap_or_default();

    let (config, source_cmd) = if shell.ends_with("zsh") {
        (home.join(".zshrc"), "source ~/.zshrc")
    } else if shell.ends_with("bash") {
        // Prefer .bash_profile on macOS (login shell), .bashrc on Linux.
        if cfg!(target_os = "macos") {
            (home.join(".bash_profile"), "source ~/.bash_profile")
        } else {
            (home.join(".bashrc"), "source ~/.bashrc")
        }
    } else if shell.ends_with("fish") {
        // fish uses a different config and syntax - handled separately.
        return register_path_fish(bin_dir, &home);
    } else {
        (home.join(".bashrc"), "source ~/.bashrc")
    };

    let existing = std::fs::read_to_string(&config).unwrap_or_default();

    if existing.contains(BLOCK_START) {
        // Block present - check if path inside it is already correct.
        let block = path_block(bin_dir);
        if existing.contains(&format!("export PATH=\"{}:$PATH\"", bin_dir.display())) {
            println!("[KNOT] INFO:  {} is already registered in {}", bin_dir.display(), config.display());
            println!("[KNOT] INFO:  Run: {source_cmd}");
            return Ok(());
        }
        // Path changed (binary moved) - update in place.
        let updated = inject_block(&existing, &block);
        std::fs::write(&config, updated)?;
        println!("[KNOT] INFO:  Updated PATH entry in {}", config.display());
    } else {
        if let Some(parent) = config.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let block = path_block(bin_dir);
        let updated = inject_block(&existing, &block);
        std::fs::write(&config, updated)?;
        println!("[KNOT] INFO:  Registered {} in {}", bin_dir.display(), config.display());
    }

    println!("[KNOT] SUCCESS: Run the following to activate immediately:");
    println!("         {source_cmd}");
    Ok(())
}

#[cfg(unix)]
fn register_path_fish(bin_dir: &Path, home: &Path) -> Result<()> {
    let config = home.join(".config/fish/config.fish");
    let entry = format!("fish_add_path \"{}\"", bin_dir.display());
    let existing = std::fs::read_to_string(&config).unwrap_or_default();

    if existing.contains(&entry) {
        println!("[KNOT] INFO:  {} is already registered in {}", bin_dir.display(), config.display());
        println!("[KNOT] INFO:  Run: source ~/.config/fish/config.fish");
        return Ok(());
    }

    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let updated = format!(
        "{existing}\n{BLOCK_START}\n{entry}\n{BLOCK_END}\n"
    );
    std::fs::write(&config, updated)?;
    println!("[KNOT] INFO:  Registered {} in {}", bin_dir.display(), config.display());
    println!("[KNOT] SUCCESS: Run the following to activate immediately:");
    println!("         source ~/.config/fish/config.fish");
    Ok(())
}

#[cfg(windows)]
fn register_path(bin_dir: &Path) -> Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu.open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)?;
    let current: String = env.get_value("PATH").unwrap_or_default();

    let dir_str = bin_dir.to_string_lossy();
    if current.split(';').any(|p| p.trim() == dir_str.as_ref()) {
        println!("[KNOT] INFO:  {} is already in the User PATH.", bin_dir.display());
        return Ok(());
    }

    let new_path = if current.is_empty() {
        dir_str.into_owned()
    } else {
        format!("{current};{dir_str}")
    };
    env.set_value("PATH", &new_path)?;

    // Notify running processes of the environment change.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
            windows_sys::Win32::UI::WindowsAndMessaging::HWND_BROADCAST,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE,
            0,
            windows_sys::core::w!("Environment") as _,
            windows_sys::Win32::UI::WindowsAndMessaging::SMTO_ABORTIFHUNG,
            5000,
            std::ptr::null_mut(),
        );
    }

    println!("[KNOT] INFO:  Registered {} in User PATH (registry).", bin_dir.display());
    println!("[KNOT] SUCCESS: Open a new terminal window to pick up the change.");
    Ok(())
}

// ── MCP server ────────────────────────────────────────────────────────────────

async fn run_mcp_server() -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=info".into());
    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());

    let is_new = !Path::new(&data_dir).exists();
    std::fs::create_dir_all(&data_dir)?;

    let _guard = logging::init(&filter, Some(&log_path_for(&data_dir)));

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
    dirs::home_dir()
        .map(|h| format!("{}", h.join(".knot").display()))
        .unwrap_or_else(|| ".knot".into())
}
