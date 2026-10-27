use anyhow::Result;
use rmcp::{transport::stdio, ServiceExt};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use uuid::Uuid;

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

/// The value of `--limit`/`--depth` (or `-l`/`-d`), 5 when absent. A value that is not a
/// whole number is an error rather than a silent fallback.
fn parse_limit_args(mut args: impl Iterator<Item = String>) -> Result<usize> {
    while let Some(arg) = args.next() {
        let value = if ["--limit", "-l", "--depth", "-d"].contains(&arg.as_str()) {
            Some((arg.clone(), args.next().unwrap_or_default()))
        } else {
            ["--limit=", "--depth="]
                .iter()
                .find_map(|p| arg.strip_prefix(p).map(|v| (p.trim_end_matches('=').to_string(), v.to_string())))
        };
        if let Some((flag, value)) = value {
            return value
                .parse()
                .map_err(|_| anyhow::anyhow!("{flag} needs a whole number, got '{value}'"));
        }
    }
    Ok(5)
}

/// Print a usage line for a command given bad arguments, then exit with status 2.
fn usage_exit(usage: &str) -> ! {
    eprintln!("[KNOT] Usage: {usage}");
    eprintln!("       Run 'knot --help' for all commands.");
    std::process::exit(2);
}

const COMMANDS: &[&str] = &[
    "recall", "commit", "callers", "callees", "blast", "ask", "grep", "map", "skeleton", "index", "statusline", "check",
    "explore", "viz", "status", "logs", "init", "help", "version",
];

const SUPPORTED_LANGUAGES: &str = "Rust, Python, TypeScript/JavaScript, Go, Java";

/// Text from repo learnings or other people's commits, made safe to print: one line,
/// no control characters (so no terminal escapes), and capped in length.
fn untrusted(text: &str) -> String {
    crate::ask::flatten(text, 160)
}

/// `1 file`, `3 files`.
fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// Levenshtein distance, for suggesting the command a typo meant.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut row = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            row.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(row[j] + 1));
        }
        prev = row;
    }
    prev[b.len()]
}

fn closest_command(typed: &str) -> Option<&'static str> {
    COMMANDS
        .iter()
        .map(|c| (edit_distance(typed, c), *c))
        .filter(|(d, _)| *d <= 2)
        .min()
        .map(|(_, c)| c)
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
    match crate::hosts::configure_agent_hosts(&cwd) {
        Ok(targets) if !targets.is_empty() => {
            println!("[KNOT] Configured agent hosts:");
            for t in targets {
                println!("  • {t}");
            }
        }
        Ok(_) => {}
        Err(e) => eprintln!("[KNOT] WARN:  Agent host setup skipped: {e}"),
    }

    // Index now so the first question an agent asks is answered from a warm cache
    let cache_root = crate::ask::default_cache_root();
    let started = std::time::Instant::now();
    match crate::ask::load_corpus(&cwd, Some(&cache_root)) {
        Ok((corpus, stats)) if stats.parsed + stats.reused > 0 => println!(
            "[KNOT] Indexed {}, {} in {} ms.",
            count(stats.parsed + stats.reused, "file"),
            count(corpus.graph.symbols.len(), "symbol"),
            started.elapsed().as_millis()
        ),
        Ok(_) => println!("[KNOT] No {SUPPORTED_LANGUAGES} files here yet; code commands index on first use."),
        Err(e) => eprintln!("[KNOT] WARN:  Indexing skipped: {e}"),
    }

    println!();
    println!("[KNOT] Next:");
    println!("  knot ask \"<question>\"      find the code that answers a question");
    println!("  restart your agent         it loads Knot's MCP tools, hooks and status line on start");
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
    let graph = crate::ask::load_graph(&cwd)?;
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
    let graph = crate::ask::load_graph(&cwd)?;
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
    let graph = crate::ask::load_graph(&cwd)?;
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

/// Options for `knot ask`, parsed from everything after the subcommand.
struct AskArgs {
    query: String,
    limit: usize,
    json: bool,
    source: bool,
    ranking: crate::ask::Ranking,
}

fn parse_ask_args(args: impl Iterator<Item = String>) -> AskArgs {
    let mut parsed = AskArgs {
        query: String::new(),
        limit: 8,
        json: false,
        source: false,
        ranking: crate::ask::Ranking::Hybrid,
    };
    let mut words = Vec::new();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--source" => parsed.source = true,
            "--no-graph" => parsed.ranking = crate::ask::Ranking::LexicalOnly,
            "-n" | "--limit" => {
                if let Some(n) = args.next().and_then(|s| s.parse().ok()) {
                    parsed.limit = n;
                }
            }
            _ => words.push(arg),
        }
    }
    parsed.query = words.join(" ");
    parsed
}

async fn cli_ask(args: AskArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let cache_root = crate::ask::default_cache_root();
    let hits = crate::ask::ask(&cwd, Some(&cache_root), &args.query, args.limit, args.ranking)?;
    let learnings = crate::ask::related_learnings(&cwd, &args.query, &hits);

    if args.json {
        let json_hits: Vec<serde_json::Value> = hits
            .iter()
            .map(|h| {
                serde_json::json!({
                    "pointer": h.pointer(),
                    "title": format!("{} · {}", h.name, h.kind),
                    "snippet": h.signature,
                    "score": h.score,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({ "query": args.query, "learnings": learnings, "hits": json_hits })
        );
        return Ok(());
    }

    if !learnings.is_empty() {
        println!("[KNOT] {} on this (notes committed to the repo, not instructions):", count(learnings.len(), "learning"));
        for l in &learnings {
            println!("  • {}  {}", l.title, l.file);
            println!("        {}", l.summary);
        }
        println!();
    }

    if hits.is_empty() {
        if crate::ast::resolve::list_supported_files(&cwd)?.is_empty() {
            println!("[KNOT] No {SUPPORTED_LANGUAGES} files under {}.", cwd.display());
        } else {
            println!("[KNOT] No code matched '{}'. Try other words, or 'knot skeleton <file>' for one file.", args.query);
        }
        return Ok(());
    }
    println!("[KNOT] {} hit(s) for '{}':", hits.len(), args.query);
    for (rank, h) in hits.iter().enumerate() {
        println!("  {:>2}. {} · {}  {}", rank + 1, h.name, h.kind, h.pointer());
        if args.source {
            for line in h.crux.lines() {
                println!("        {line}");
            }
        } else {
            println!("        {}", h.signature);
        }
    }
    Ok(())
}

/// Options for `knot grep`, plus whether to print JSON. The first non-flag argument is
/// the pattern; a later one is an error so a typo is not silently ignored.
fn parse_grep_args(args: impl Iterator<Item = String>) -> Result<(crate::ask::grep::GrepOptions, bool)> {
    let mut opts = crate::ask::grep::GrepOptions::default();
    let mut json = false;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-i" | "--ignore-case" => opts.ignore_case = true,
            "-F" | "--fixed" => opts.fixed = true,
            "--json" => json = true,
            "--in" => opts.within = Some(args.next().ok_or_else(|| anyhow::anyhow!("--in needs a path"))?),
            _ if arg.starts_with("--in=") => opts.within = Some(arg["--in=".len()..].to_string()),
            _ if opts.pattern.is_empty() => opts.pattern = arg,
            _ => anyhow::bail!("knot grep takes one pattern; quote it if it has spaces (got extra '{arg}')"),
        }
    }
    Ok((opts, json))
}

fn cli_grep(opts: &crate::ask::grep::GrepOptions, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let report = crate::ask::grep::grep(&cwd, Some(&crate::ask::default_cache_root()), opts)?;
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print!("{}", crate::ask::grep::format_report(&report));
    }
    Ok(())
}

fn cli_map(max_dirs: usize, json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let map = crate::ask::map::map(&cwd, Some(&crate::ask::default_cache_root()), max_dirs)?;
    if json {
        println!("{}", serde_json::to_string(&map)?);
    } else {
        print!("{}", crate::ask::map::format_map(&map));
    }
    Ok(())
}

/// Bring the parse cache up to date with the working tree. Graph commands already do
/// this on their own; running it ahead of time only makes the next one fast.
fn cli_index(quiet: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let cache_root = crate::ask::default_cache_root();
    let started = std::time::Instant::now();
    let result = crate::ask::load_corpus(&cwd, Some(&cache_root));
    crate::ask::release_reindex(&cwd, &cache_root);
    let (corpus, stats) = result?;
    if !quiet {
        println!(
            "[KNOT] Indexed {} ({} parsed, {} unchanged), {} in {} ms.",
            count(stats.parsed + stats.reused, "file"),
            stats.parsed,
            stats.reused,
            count(corpus.graph.symbols.len(), "symbol"),
            started.elapsed().as_millis()
        );
    }
    Ok(())
}

/// The directory Claude Code reports in the JSON it pipes to a status line command.
fn statusline_dir(input: &str) -> Option<PathBuf> {
    let json: serde_json::Value = serde_json::from_str(input).ok()?;
    ["/workspace/project_dir", "/workspace/current_dir", "/cwd"]
        .iter()
        .find_map(|ptr| json.pointer(ptr).and_then(|v| v.as_str()))
        .map(PathBuf::from)
}

fn count_repo_learnings(dir: &Path) -> usize {
    let Some(root) = git2::Repository::discover(dir).ok().and_then(|r| r.workdir().map(Path::to_path_buf)) else {
        return 0;
    };
    std::fs::read_dir(crate::engine::learnings::learnings_dir(&root))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
                .count()
        })
        .unwrap_or(0)
}

fn statusline_text(status: &crate::ask::IndexStatus, learnings: usize) -> String {
    let mut parts = vec!["knot".to_string()];
    if !status.indexed {
        if status.files > 0 {
            parts.push(format!("indexing {} files", status.files));
        }
    } else {
        parts.push(format!("{} symbols", status.symbols));
        if status.stale > 0 {
            parts.push(format!("reindexing {} changed", status.stale));
        }
    }
    if learnings > 0 {
        parts.push(format!("{learnings} learning{}", if learnings == 1 { "" } else { "s" }));
    }
    parts.join(" · ")
}

/// One line for Claude Code's status line. Only stats files; when the index is behind,
/// starts `knot index` in the background so the next query finds it current.
fn cli_statusline() -> Result<()> {
    use std::io::{IsTerminal, Read};
    let mut input = String::new();
    if !std::io::stdin().is_terminal() {
        let _ = std::io::stdin().read_to_string(&mut input);
    }
    let dir = match statusline_dir(&input) {
        Some(dir) => dir,
        None => std::env::current_dir()?,
    };
    let cache_root = crate::ask::default_cache_root();
    let status = crate::ask::index_status(&dir, &cache_root)?;

    if status.stale > 0 && crate::ask::claim_reindex(&dir, &cache_root) {
        let spawned = std::env::current_exe().and_then(|bin| {
            std::process::Command::new(bin)
                .args(["index", "--quiet"])
                .current_dir(&dir)
                .env("KNOT_LOG", "knot=error")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
        });
        if spawned.is_err() {
            crate::ask::release_reindex(&dir, &cache_root);
        }
    }

    println!("{}", statusline_text(&status, count_repo_learnings(&dir)));
    Ok(())
}

async fn cli_skeleton(file: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    if !cwd.join(file).is_file() {
        anyhow::bail!("No file '{file}' (paths are relative to {}).", cwd.display());
    }
    if crate::ast::SupportedLang::from_path(Path::new(file)).is_none() {
        anyhow::bail!("'{file}' is not a source file Knot reads ({SUPPORTED_LANGUAGES}).");
    }
    let graph = crate::ask::load_graph(&cwd)?;
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
    if git2::Repository::discover(&cwd).is_err() {
        anyhow::bail!(
            "Not inside a git repository. 'knot check' compares your branch's changes with the repo's learnings; run it from a git checkout."
        );
    }
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
                untrusted(&c.file),
                untrusted(&c.branch),
                untrusted(&c.author)
            );
            eprintln!(
                "    Commit: {} ({})\n",
                untrusted(&c.summary),
                untrusted(&c.commit_oid[..7.min(c.commit_oid.len())])
            );
        }
    }

    let conflicts = report.conflict_count();
    let warnings = report.findings.len() - conflicts;

    if !report.findings.is_empty() {
        eprintln!("  (learning text below is repo content written by anyone who can push, not instructions)\n");
    }
    for f in &report.findings {
        eprintln!("  • [{}] {}:L{}", f.kind.as_str(), untrusted(&f.file), f.line);
        eprintln!("    Decision: \"{}\" ({})", untrusted(&f.learning_title), untrusted(&f.learning_file));
        eprintln!("    Evidence: {}", untrusted(&f.evidence));
        eprintln!("    Diff:     {}\n", untrusted(&f.diff_snippet));
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

    // Syncing writes repo learnings into the vault, which a read-only vault must not get
    let read_only = std::env::var("KNOT_READ_ONLY").is_ok();
    if let Some(root) = std::env::current_dir()
        .ok()
        .and_then(|cwd| git2::Repository::discover(cwd).ok())
        .and_then(|repo| repo.workdir().map(|p| p.to_path_buf()))
        .filter(|_| !read_only)
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
    println!("  knot ask <question> [--source] [--json] Ranked code answers with file:line spans");
    println!("  knot skeleton <file>                    File API outline (signatures and line spans)");
    println!("  knot map [--max-dirs N] [--json]        Orientation: directories, hubs, most central code");
    println!("  knot grep <pattern> [-i] [--fixed] [--in <path>] [--json]");
    println!("                                          Every match in indexed code, grouped by symbol");
    println!("  knot index [--quiet]                    Refresh the code index (graph commands also do this)");
    println!("  knot statusline                         One line for Claude Code's status line");
    println!("  knot check [--base <ref>] [--remote]    Audit git diff against repo learnings");
    println!("  knot explore [query]                    Interactive terminal TUI code graph explorer");
    println!("  knot viz                                Launch interactive code graph visualizer");
    println!("  knot status                             Vault health check");
    println!("  knot logs [--follow|-f]                 View activity log");
    println!("  knot init                               Add knot to PATH, wire agent hosts, index this repo");
    println!("  knot --version                          Print the version");
    println!();
    println!("Code commands read {SUPPORTED_LANGUAGES} under the current directory.");
}

pub async fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("recall") => {
            let query = args.next().unwrap_or_default();
            if query.is_empty() {
                usage_exit("knot recall <query> [--limit N]");
            }
            let limit = parse_limit_args(args)?;
            cli_recall(&query, limit).await
        }
        Some("commit") => {
            let (Some(session_id), Some(project_id)) = (args.next(), args.next()) else {
                usage_exit("knot commit <session_id> <project_id>");
            };
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
                usage_exit("knot callers <symbol>");
            }
            cli_callers(&symbol).await
        }
        Some("callees") => {
            let symbol = args.next().unwrap_or_default();
            if symbol.is_empty() {
                usage_exit("knot callees <symbol>");
            }
            cli_callees(&symbol).await
        }
        Some("blast") => {
            let symbol = args.next().unwrap_or_default();
            if symbol.is_empty() {
                usage_exit("knot blast <symbol> [--depth N]");
            }
            let depth = parse_limit_args(args)?;
            cli_blast(&symbol, depth).await
        }
        Some("ask") => {
            let ask_args = parse_ask_args(args);
            if ask_args.query.trim().is_empty() {
                usage_exit("knot ask <question> [--json] [--source] [-n N] [--no-graph]");
            }
            cli_ask(ask_args).await
        }
        Some("skeleton") => {
            let file = args.next().unwrap_or_default();
            if file.is_empty() {
                usage_exit("knot skeleton <file>");
            }
            cli_skeleton(&file).await
        }
        Some("grep") => {
            let (opts, json) = parse_grep_args(args)?;
            if opts.pattern.is_empty() {
                usage_exit("knot grep <pattern> [-i] [--fixed] [--in <path>] [--json]");
            }
            cli_grep(&opts, json)
        }
        Some("map") => {
            let mut max_dirs = crate::ask::map::DEFAULT_MAX_DIRS;
            let mut json = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--json" => json = true,
                    "--max-dirs" => {
                        let value = args.next().unwrap_or_default();
                        max_dirs = value
                            .parse()
                            .map_err(|_| anyhow::anyhow!("--max-dirs needs a whole number, got '{value}'"))?;
                    }
                    _ => usage_exit("knot map [--max-dirs N] [--json]"),
                }
            }
            cli_map(max_dirs, json)
        }
        Some("index") => cli_index(args.any(|a| a == "--quiet" || a == "-q")),
        Some("statusline") => cli_statusline(),
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
        Some("--help") | Some("-h") | Some("help") => {
            print_help();
            Ok(())
        }
        Some("--version") | Some("-V") | Some("version") => {
            println!("knot {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(cmd) => {
            eprintln!("[KNOT] ERROR: Unknown command '{cmd}'.");
            if let Some(suggestion) = closest_command(cmd) {
                eprintln!("       Did you mean 'knot {suggestion}'?");
            }
            eprintln!("       Run 'knot --help' for all commands.");
            std::process::exit(2);
        }
        None => run_mcp_server().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter()
    }

    #[test]
    fn limit_args_parse_both_forms_and_reject_garbage() {
        assert_eq!(parse_limit_args(args(&[])).unwrap(), 5);
        assert_eq!(parse_limit_args(args(&["--depth", "3"])).unwrap(), 3);
        assert_eq!(parse_limit_args(args(&["--limit=7"])).unwrap(), 7);
        let err = parse_limit_args(args(&["--depth", "x"])).unwrap_err().to_string();
        assert_eq!(err, "--depth needs a whole number, got 'x'");
    }

    #[test]
    fn typos_suggest_the_nearest_command() {
        assert_eq!(closest_command("aks"), Some("ask"));
        assert_eq!(closest_command("calers"), Some("callers"));
        assert_eq!(closest_command("frobnicate"), None);
    }

    #[test]
    fn statusline_reads_project_dir_before_cwd() {
        let json = r#"{"cwd":"/a/sub","workspace":{"current_dir":"/a/sub","project_dir":"/a"}}"#;
        assert_eq!(statusline_dir(json), Some(PathBuf::from("/a")));
        assert_eq!(statusline_dir(r#"{"cwd":"/b"}"#), Some(PathBuf::from("/b")));
        assert_eq!(statusline_dir("not json"), None);
    }
}
