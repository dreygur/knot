use anyhow::Result;
use rmcp::{transport::stdio, ServiceExt};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::ast::CodeGraph;
use crate::engine::{CommitReport, RecallResult, StatusReport, StorageEngine};
use crate::hooks;
use crate::logging;
use crate::tools::KnotServer;

const BLOCK_START: &str = "# >>> knot initialize >>>";
const BLOCK_END: &str = "# <<< knot initialize <<<";

fn resolve_data_dir() -> String {
    dirs::home_dir()
        .map(|h| format!("{}", h.join(".knot").display()))
        .unwrap_or_else(|| ".knot".into())
}

fn log_path_for(data_dir: &str) -> PathBuf {
    PathBuf::from(data_dir).join("activity.log")
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
    let mut out = format!(
        "[KNOT] commit: {} promoted, {} rejected (project={})\n",
        r.promoted_count(),
        r.rejected_count(),
        r.project_id,
    );
    for (id, issues) in &r.learnings_held {
        out.push_str(&format!("[KNOT] learning not written for {id}: {}\n", issues.join("; ")));
    }
    out
}

fn format_status_cli(r: &StatusReport) -> String {
    format!(
        "[KNOT] status  L1(session)={} L2(project)={} L3(global)={} skills={} ghosts={} db={}\n",
        r.l1_nodes, r.l2_nodes, r.l3_nodes, r.skills, r.ghost_count, r.db_health,
    )
}

fn parse_limit_args(mut args: impl Iterator<Item = String>) -> usize {
    while let Some(arg) = args.next() {
        if arg == "--limit" || arg == "-l" || arg == "--depth" || arg == "-d" {
            if let Some(n) = args.next().and_then(|s| s.parse().ok()) {
                return n;
            }
        } else if let Some(n) = arg.strip_prefix("--limit=").and_then(|s| s.parse().ok()) {
            return n;
        } else if let Some(n) = arg.strip_prefix("--depth=").and_then(|s| s.parse().ok()) {
            return n;
        }
    }
    5
}

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

#[cfg(unix)]
fn path_block(bin_dir: &Path) -> String {
    format!(
        "\n{BLOCK_START}\nexport PATH=\"{}:$PATH\"\n{BLOCK_END}\n",
        bin_dir.display()
    )
}

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

#[cfg(unix)]
fn register_path(bin_dir: &Path) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let shell = std::env::var("SHELL").unwrap_or_default();

    let (config, source_cmd) = if shell.ends_with("zsh") {
        (home.join(".zshrc"), "source ~/.zshrc")
    } else if shell.ends_with("bash") {
        if cfg!(target_os = "macos") {
            (home.join(".bash_profile"), "source ~/.bash_profile")
        } else {
            (home.join(".bashrc"), "source ~/.bashrc")
        }
    } else if shell.ends_with("fish") {
        return register_path_fish(bin_dir, &home);
    } else {
        (home.join(".bashrc"), "source ~/.bashrc")
    };

    let existing = std::fs::read_to_string(&config).unwrap_or_default();

    if existing.contains(BLOCK_START) {
        let block = path_block(bin_dir);
        if existing.contains(&format!("export PATH=\"{}:$PATH\"", bin_dir.display())) {
            println!("[KNOT] INFO:  {} is already registered in {}", bin_dir.display(), config.display());
            println!("[KNOT] INFO:  Run: {source_cmd}");
            return Ok(());
        }
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

    println!("[KNOT] INFO:  Registered {} in User PATH (registry).", bin_dir.display());
    println!("[KNOT] SUCCESS: Open a new terminal window to pick up the change.");
    Ok(())
}

async fn cli_recall(query: &str, limit: usize) -> Result<()> {
    let filter = std::env::var("KNOT_LOG").unwrap_or_else(|_| "knot=error".into());
    let data_dir = std::env::var("KNOT_DATA_DIR").unwrap_or_else(|_| resolve_data_dir());
    let _guard = logging::init(&filter, Some(&log_path_for(&data_dir)));
    warn_if_not_in_path();
    let engine = StorageEngine::new(&data_dir).await?;
    if let Ok(cwd) = std::env::current_dir() {
        let _ = engine.refresh_centrality(&cwd).await;
    }
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

    register_path(bin_dir)?;

    let cwd = std::env::current_dir()?;
    if let Ok(targets) = crate::hosts::configure_agent_hosts(&cwd) {
        if !targets.is_empty() {
            println!("[KNOT] Configured agent hosts:");
            for t in targets {
                println!("  • {t}");
            }
        }
    }
    Ok(())
}

async fn cli_viz() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let out_path = crate::ui::viz::render_and_launch_viz(&cwd)?;
    println!("[KNOT] Code graph visualizer generated: {}", out_path.display());
    println!("[KNOT] Opened in default web browser.");
    Ok(())
}

async fn cli_callers(symbol: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let graph = CodeGraph::build(&cwd)?;
    let callers = graph.get_callers(symbol);

    if callers.is_empty() {
        println!("[KNOT] No callers found for symbol '{symbol}'.");
    } else {
        println!("[KNOT] Callers of '{symbol}' ({} hit{}):", callers.len(), if callers.len() == 1 { "" } else { "s" });
        for c in callers {
            println!("  • {} ({}:L{}-L{})", c.name, c.file, c.span.0, c.span.1);
        }
    }

    let importers = graph.get_importers(symbol);
    if !importers.is_empty() {
        println!("[KNOT] Imported by ({} file{}):", importers.len(), if importers.len() == 1 { "" } else { "s" });
        for file in importers {
            println!("  • {file}");
        }
    }
    Ok(())
}

async fn cli_callees(symbol: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let graph = CodeGraph::build(&cwd)?;
    let callees = graph.get_callees(symbol);

    if callees.is_empty() {
        println!("[KNOT] No callees found for symbol '{symbol}'.");
    } else {
        println!("[KNOT] Callees of '{symbol}' ({} hit{}):", callees.len(), if callees.len() == 1 { "" } else { "s" });
        for c in callees {
            println!("  • {} ({}:L{}-L{})", c.name, c.file, c.span.0, c.span.1);
        }
    }
    Ok(())
}

async fn cli_blast(symbol: &str, depth: usize) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let graph = CodeGraph::build(&cwd)?;
    let report = graph.blast_radius(symbol, Some(depth));

    println!(
        "[KNOT] Blast radius for '{}' (depth={}): {} impacted symbol{}, {} file{}:",
        symbol,
        depth,
        report.impacted_symbols.len(),
        if report.impacted_symbols.len() == 1 { "" } else { "s" },
        report.impacted_files.len(),
        if report.impacted_files.len() == 1 { "" } else { "s" }
    );

    for s in &report.impacted_symbols {
        println!("  • {} ({}:L{}-L{})", s.name, s.file, s.span.0, s.span.1);
    }

    if !report.impacted_files.is_empty() {
        println!("\nAffected files:");
        for f in &report.impacted_files {
            println!("  - {f}");
        }
    }
    Ok(())
}

async fn cli_skeleton(file: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let graph = CodeGraph::build(&cwd)?;
    let items = graph.file_skeleton(file);

    if items.is_empty() {
        println!("[KNOT] No definitions found for file '{file}'.");
    } else {
        println!("[KNOT] Skeleton for '{file}' ({} definition{}):", items.len(), if items.len() == 1 { "" } else { "s" });
        for item in items {
            println!("  L{:>4}-L{:<4} {:<10} {}", item.span.0, item.span.1, item.kind.as_str(), item.signature);
        }
    }
    Ok(())
}

async fn cli_check(base_ref: Option<&str>, check_remote: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let report = crate::check::check_git_diff(&cwd, base_ref)?;

    let mut remote_collisions = Vec::new();
    if check_remote {
        match crate::check::detect_remote_collisions(&cwd) {
            Ok(remote_report) => {
                remote_collisions = remote_report.collisions;
            }
            Err(e) => {
                tracing::debug!("Remote branch scan skipped: {e}");
            }
        }
    }

    if !remote_collisions.is_empty() {
        eprintln!(
            "[KNOT] WARNING: {} remote branch collision(s) detected with teammates:\n",
            remote_collisions.len()
        );
        for c in &remote_collisions {
            eprintln!(
                "  • [COLLISION] {} modified in branch '{}' by '{}'",
                c.file, c.branch, c.author
            );
            eprintln!(
                "    Commit: {} ({})\n",
                c.summary,
                &c.commit_oid[..7.min(c.commit_oid.len())]
            );
        }
    }

    let conflicts = report.conflict_count();
    let warnings = report.findings.len() - conflicts;

    for f in &report.findings {
        eprintln!("  • [{}] {}:L{}", f.kind.as_str(), f.file, f.line);
        eprintln!("    Decision: \"{}\" ({})", f.learning_title, f.learning_file);
        eprintln!("    Evidence: {}", f.evidence);
        eprintln!("    Diff:     {}\n", f.diff_snippet);
    }

    if report.ok {
        println!(
            "[KNOT] Check passed: {} files audited against repo learnings, 0 conflicts, {warnings} warning(s).",
            report.checked_files.len()
        );
        Ok(())
    } else {
        eprintln!("[KNOT] FAIL: {conflicts} conflict(s), {warnings} warning(s) against repo learnings.");
        std::process::exit(1);
    }
}

async fn cli_explore(query: Option<&str>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    crate::ui::run_explorer(&cwd, query)
}

pub async fn run_mcp_server() -> Result<()> {
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

    if let Some(root) = std::env::current_dir()
        .ok()
        .and_then(|cwd| git2::Repository::discover(cwd).ok())
        .and_then(|repo| repo.workdir().map(|p| p.to_path_buf()))
    {
        match engine.sync_repo_learnings(&root).await {
            Ok(0) => {}
            Ok(n) => tracing::info!("Indexed {n} repo learning(s) from {}", root.display()),
            Err(e) => tracing::warn!("Repo learnings sync failed: {e}"),
        }
    }

    let mut server = KnotServer::new(engine, session_id);
    if let Ok(cwd) = std::env::current_dir() {
        server = server.with_branch_monitor(crate::check::start_branch_monitor(cwd, None));
    }
    let running = server.serve(stdio()).await?;
    running.waiting().await?;

    Ok(())
}

pub fn print_help() {
    println!("knot v{}", env!("CARGO_PKG_VERSION"));
    println!("Persistent memory pool and code graph MCP server");
    println!();
    println!("USAGE:");
    println!("  knot                                    Start MCP server (stdio)");
    println!("  knot recall <query> [--limit N]         Semantic search");
    println!("  knot commit <session_id> <project_id>   Commit session to project");
    println!("  knot callers <symbol>                   Find direct callers of a symbol");
    println!("  knot callees <symbol>                   Find direct callees of a symbol");
    println!("  knot blast <symbol> [--depth N]         Transitive blast radius analysis");
    println!("  knot skeleton <file>                    File API outline (signatures and line spans)");
    println!("  knot check [--base <ref>] [--remote]    Audit git diff against repo learnings");
    println!("  knot explore [query]                    Interactive terminal TUI code graph explorer");
    println!("  knot viz                                Launch interactive code graph visualizer");
    println!("  knot status                             Vault health check");
    println!("  knot logs [--follow|-f]                 View activity log");
    println!("  knot init                               Register binary directory in PATH");
}

pub async fn run() -> Result<()> {
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
        Some("callers") => {
            let symbol = args.next().unwrap_or_default();
            if symbol.is_empty() {
                eprintln!("Usage: knot callers <symbol>");
                std::process::exit(1);
            }
            cli_callers(&symbol).await
        }
        Some("callees") => {
            let symbol = args.next().unwrap_or_default();
            if symbol.is_empty() {
                eprintln!("Usage: knot callees <symbol>");
                std::process::exit(1);
            }
            cli_callees(&symbol).await
        }
        Some("blast") => {
            let symbol = args.next().unwrap_or_default();
            if symbol.is_empty() {
                eprintln!("Usage: knot blast <symbol> [--depth N]");
                std::process::exit(1);
            }
            let depth = parse_limit_args(args);
            cli_blast(&symbol, depth).await
        }
        Some("skeleton") => {
            let file = args.next().unwrap_or_default();
            if file.is_empty() {
                eprintln!("Usage: knot skeleton <file>");
                std::process::exit(1);
            }
            cli_skeleton(&file).await
        }
        Some("check") => {
            let mut base_ref = None;
            let mut check_remote = false;
            while let Some(arg) = args.next() {
                if arg == "--base" || arg == "-b" {
                    base_ref = args.next();
                } else if let Some(r) = arg.strip_prefix("--base=") {
                    base_ref = Some(r.to_string());
                } else if arg == "--remote" || arg == "-r" {
                    check_remote = true;
                }
            }
            cli_check(base_ref.as_deref(), check_remote).await
        }
        Some("explore") => {
            let query = args.next();
            cli_explore(query.as_deref()).await
        }
        Some("viz") => cli_viz().await,
        Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(cmd) => {
            eprintln!(
                "[KNOT] ERROR: Unknown command '{cmd}'. Available: recall, commit, callers, callees, blast, skeleton, status, logs, init"
            );
            std::process::exit(1);
        }
        None => run_mcp_server().await,
    }
}
