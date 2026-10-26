//! Retrieval eval: scores `trail ask` and `knot ask` on the same questions.
//!
//! Each question names the symbol(s) that answer it. A hit is a returned
//! `file:Lstart-Lend` pointer whose span overlaps that symbol's definition, so
//! file-only results never score. Spans come from parsing the corpus, not from
//! either tool's index.
//!
//!   cargo run --release --example eval -- [--engine trail|knot|knot-lexical]...
//!       [--corpus NAME] [--questions eval/holdout.json]

use anyhow::{bail, Context, Result};
use knot::ast::{extract_file_symbols, SupportedLang};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const QUESTIONS_PATH: &str = "eval/questions.json";
const RESULT_LIMIT: usize = 10;
const TOP_K: usize = 5;
const ENGINES: &[&str] = &["trail", "knot"];

#[derive(Deserialize)]
struct Corpus {
    path: String,
}

#[derive(Deserialize)]
struct Expect {
    file: String,
    symbol: String,
}

#[derive(Deserialize)]
struct Question {
    corpus: String,
    q: String,
    expect: Vec<Expect>,
}

#[derive(Deserialize)]
struct QuestionSet {
    corpora: BTreeMap<String, Corpus>,
    questions: Vec<Question>,
}

#[derive(Deserialize)]
struct AskHit {
    pointer: String,
}

#[derive(Deserialize)]
struct AskOutput {
    hits: Vec<AskHit>,
}

struct Span {
    file: String,
    lines: (usize, usize),
}

#[derive(Default)]
struct Score {
    asked: usize,
    hit_at_1: usize,
    hit_at_k: usize,
    file_at_k: usize,
    reciprocal_rank: f64,
    latencies: Vec<Duration>,
    failures: usize,
}

/// `src/a.rs:L10-L20` -> (`src/a.rs`, (10, 20)); file-only pointers yield `None`.
fn parse_pointer(pointer: &str) -> Option<(String, (usize, usize))> {
    let (file, range) = pointer.rsplit_once(":L")?;
    let (start, end) = match range.split_once("-L") {
        Some((s, e)) => (s.parse().ok()?, e.parse().ok()?),
        None => {
            let line = range.parse().ok()?;
            (line, line)
        }
    };
    Some((file.to_string(), (start, end)))
}

fn bare_name(name: &str) -> &str {
    name.rsplit(['.', ':']).next().unwrap_or(name)
}

/// Definition span of `expect.symbol` in its file, found by parsing the file.
fn resolve_span(corpus_root: &Path, expect: &Expect) -> Result<Span> {
    let path = corpus_root.join(&expect.file);
    let lang = SupportedLang::from_path(&path).with_context(|| format!("unsupported file {}", expect.file))?;
    let source = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let symbols = extract_file_symbols(&expect.file, &source, lang)?;

    let found = symbols
        .iter()
        .find(|s| s.name == expect.symbol || bare_name(&s.name) == expect.symbol)
        .with_context(|| format!("symbol {} not found in {}", expect.symbol, expect.file))?;
    Ok(Span { file: expect.file.clone(), lines: found.span })
}

fn overlaps(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

/// 1-based rank of the first pointer that lands on any expected span.
fn first_hit_rank(pointers: &[String], spans: &[Span]) -> Option<usize> {
    pointers.iter().position(|p| {
        let Some((file, lines)) = parse_pointer(p) else {
            return false;
        };
        spans.iter().any(|s| s.file == file && overlaps(s.lines, lines))
    })
    .map(|i| i + 1)
}

/// Whether any of the top `TOP_K` pointers names an expected file, spans or not.
/// Reported separately so file-only answers get visible partial credit.
fn file_in_top_k(pointers: &[String], spans: &[Span]) -> bool {
    pointers.iter().take(TOP_K).any(|p| {
        let file = p.rsplit_once(":L").map_or(p.as_str(), |(f, _)| f);
        spans.iter().any(|s| s.file == file)
    })
}

fn engine_command(engine: &str, query: &str, corpus_root: &Path) -> Command {
    let limit = RESULT_LIMIT.to_string();
    match engine {
        "trail" => {
            let mut cmd = Command::new("trail");
            cmd.args(["ask", query, "--json", "--no-refresh", "-n", &limit]).arg(corpus_root);
            cmd
        }
        knot_variant => {
            let knot = std::env::current_dir().unwrap_or_default().join("target/release/knot");
            let mut cmd = Command::new(knot);
            cmd.args(["ask", query, "--json", "-n", &limit]).current_dir(corpus_root);
            if knot_variant == "knot-lexical" {
                cmd.arg("--no-graph");
            }
            cmd
        }
    }
}

fn ask(engine: &str, query: &str, corpus_root: &Path) -> Result<(Vec<String>, Duration)> {
    let started = Instant::now();
    let output = engine_command(engine, query, corpus_root)
        .output()
        .with_context(|| format!("run {engine}"))?;
    let elapsed = started.elapsed();

    if !output.status.success() {
        bail!("{engine} exited {}: {}", output.status, String::from_utf8_lossy(&output.stderr).trim());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout.find('{').context("no JSON in output")?;
    let parsed: AskOutput = serde_json::from_str(&stdout[json_start..]).context("parse ask JSON")?;
    Ok((parsed.hits.into_iter().map(|h| h.pointer).collect(), elapsed))
}

fn median(mut values: Vec<Duration>) -> Duration {
    if values.is_empty() {
        return Duration::ZERO;
    }
    values.sort();
    values[values.len() / 2]
}

struct Args {
    engines: Vec<String>,
    corpus: Option<String>,
    questions: String,
}

fn parse_args() -> Args {
    let mut parsed = Args { engines: Vec::new(), corpus: None, questions: QUESTIONS_PATH.to_string() };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--engine" => parsed.engines.extend(args.next()),
            "--corpus" => parsed.corpus = args.next(),
            "--questions" => parsed.questions = args.next().unwrap_or(parsed.questions),
            other => eprintln!("[!] ignoring argument {other}"),
        }
    }
    if parsed.engines.is_empty() {
        parsed.engines = ENGINES.iter().map(|e| e.to_string()).collect();
    }
    parsed
}

fn print_report(scores: &BTreeMap<(String, String), Score>) {
    println!("| engine | corpus | asked | hit@1 | hit@{TOP_K} | file@{TOP_K} | MRR@{RESULT_LIMIT} | median ms | failed |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for ((engine, corpus), s) in scores {
        let n = s.asked.max(1) as f64;
        println!(
            "| {engine} | {corpus} | {} | {:.0}% | {:.0}% | {:.0}% | {:.3} | {} | {} |",
            s.asked,
            100.0 * s.hit_at_1 as f64 / n,
            100.0 * s.hit_at_k as f64 / n,
            100.0 * s.file_at_k as f64 / n,
            s.reciprocal_rank / n,
            median(s.latencies.clone()).as_millis(),
            s.failures,
        );
    }

    let mut totals: BTreeMap<&str, Score> = BTreeMap::new();
    for ((engine, _), s) in scores {
        let total = totals.entry(engine.as_str()).or_default();
        total.asked += s.asked;
        total.hit_at_1 += s.hit_at_1;
        total.hit_at_k += s.hit_at_k;
        total.file_at_k += s.file_at_k;
        total.reciprocal_rank += s.reciprocal_rank;
        total.failures += s.failures;
    }
    for (engine, s) in totals {
        let n = s.asked.max(1) as f64;
        println!(
            "| **{engine}** | **all** | {} | {:.0}% | {:.0}% | {:.0}% | **{:.3}** | | {} |",
            s.asked,
            100.0 * s.hit_at_1 as f64 / n,
            100.0 * s.hit_at_k as f64 / n,
            100.0 * s.file_at_k as f64 / n,
            s.reciprocal_rank / n,
            s.failures,
        );
    }
}

fn main() -> Result<()> {
    let Args { engines, corpus: only_corpus, questions } = parse_args();
    let set: QuestionSet = serde_json::from_str(&std::fs::read_to_string(&questions)?)?;
    let mut scores: BTreeMap<(String, String), Score> = BTreeMap::new();

    for question in &set.questions {
        if only_corpus.as_ref().is_some_and(|c| c != &question.corpus) {
            continue;
        }
        let corpus = set.corpora.get(&question.corpus).context("unknown corpus")?;
        let root = PathBuf::from(&corpus.path);
        let spans: Vec<Span> = question.expect.iter().map(|e| resolve_span(&root, e)).collect::<Result<_>>()?;

        for engine in &engines {
            let score = scores.entry((engine.clone(), question.corpus.clone())).or_default();
            score.asked += 1;

            let (pointers, elapsed) = match ask(engine, &question.q, &root) {
                Ok(result) => result,
                Err(e) => {
                    score.failures += 1;
                    eprintln!("[-] {engine} {:?}: {e:#}", question.q);
                    continue;
                }
            };
            score.latencies.push(elapsed);

            let rank = first_hit_rank(&pointers, &spans);
            if rank == Some(1) {
                score.hit_at_1 += 1;
            }
            if rank.is_some_and(|r| r <= TOP_K) {
                score.hit_at_k += 1;
            }
            if file_in_top_k(&pointers, &spans) {
                score.file_at_k += 1;
            }
            score.reciprocal_rank += rank.map_or(0.0, |r| 1.0 / r as f64);
            match rank {
                None => eprintln!("[*] {engine} miss: {:?} top={:?}", question.q, pointers.first()),
                Some(r) if r > 1 => eprintln!("[*] {engine} rank {r}: {:?} top={:?}", question.q, pointers.first()),
                _ => {}
            }
        }
    }

    print_report(&scores);
    Ok(())
}
