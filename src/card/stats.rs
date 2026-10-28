//! The numbers on the share card, totalled from every listed repo's event log.

use crate::ask::events::{self, Event, EventKind};
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

const WINDOW_DAYS: i64 = 30;
/// Fewer days of history than this draw a per-session figure instead of a line.
const MIN_TREND_DAYS: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct CardStats {
    /// Graph answers + reused learnings.
    pub tokens_saved: u64,
    /// 0.0..=1.0; `None` when nothing was read before.
    pub less_to_read: Option<f64>,
    pub graph: Tile,
    pub learnings: LearningTile,
    pub label: String,
    pub trend: Trend,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tile {
    pub tokens: u64,
    pub count: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LearningTile {
    pub count: u64,
    pub tokens: u64,
    pub minutes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trend {
    /// One total per calendar day, from the first day with events to today.
    Daily(Vec<u64>),
    /// Tokens saved per session, for a user without enough days of history.
    PerSession(u64),
    /// Tokens saved in all, when savings were logged outside any known session.
    Total(u64),
}

/// Total the event logs of `repos` (repo roots). Missing repos and logs are skipped.
pub fn collect(repos: &[PathBuf], now: DateTime<Local>) -> CardStats {
    let start = now - Duration::days(WINDOW_DAYS);
    let mut all: Vec<Event> = repos
        .iter()
        .flat_map(|root| events::all(root))
        // A timestamp that is not a real local time, or lies in the future, is corrupt: skip it
        .filter(|e| e.ts_ms <= now.timestamp_millis() && Local.timestamp_millis_opt(e.ts_ms).earliest().is_some())
        .collect();
    all.sort_by_key(|e| e.ts_ms);

    let first = all.first().map(|e| e.ts_ms);
    let capped = first.is_some_and(|ts| ts <= start.timestamp_millis());
    if capped {
        all.retain(|e| e.ts_ms >= start.timestamp_millis());
    }

    let mut graph = Tile::default();
    let (mut before, mut after) = (0u64, 0u64);
    let mut sessions: HashSet<&str> = HashSet::new();
    let mut per_day: HashMap<NaiveDate, u64> = HashMap::new();
    // Latest event per learning id wins (the log is sorted, so later overwrites earlier)
    let mut learnings: HashMap<&str, &Event> = HashMap::new();

    for e in &all {
        if !e.session.is_empty() {
            sessions.insert(&e.session);
        }
        if e.kind != EventKind::Answer {
            if let Some(id) = e.learning.as_deref() {
                learnings.insert(id, e);
            }
            continue;
        }
        let saved = e.saved_tokens as u64;
        graph.tokens = graph.tokens.saturating_add(saved);
        graph.count = graph.count.saturating_add(1);
        before = before.saturating_add(e.bytes_before);
        after = after.saturating_add(e.bytes_after);
        add(per_day.entry(day_of(e.ts_ms)).or_default(), saved);
    }

    let mut learned = LearningTile::default();
    for e in learnings.values() {
        let tokens = e.cost_tokens.unwrap_or(0);
        learned.count += 1;
        learned.tokens = learned.tokens.saturating_add(tokens);
        learned.minutes = learned.minutes.saturating_add(e.cost_minutes.unwrap_or(0));
        add(per_day.entry(day_of(e.ts_ms)).or_default(), tokens);
    }

    let tokens_saved = graph.tokens.saturating_add(learned.tokens);
    let today = now.date_naive();
    let first_day = all.first().map(|e| day_of(e.ts_ms));
    let trend = match first_day {
        Some(day) if per_day_with_events(&all) >= MIN_TREND_DAYS => {
            // At most 30 entries; anything older folds into the first so the series still sums
            let series_start = day.max(today - Duration::days(WINDOW_DAYS - 1));
            let days = (today - series_start).num_days().max(0);
            let mut series: Vec<u64> = (0..=days).map(|i| per_day.get(&(series_start + Duration::days(i))).copied().unwrap_or(0)).collect();
            for (_, tokens) in per_day.iter().filter(|(d, _)| **d < series_start) {
                add(&mut series[0], *tokens);
            }
            Trend::Daily(series)
        }
        // Savings logged without a session would read as "0 per session"; show the total
        _ if sessions.is_empty() && tokens_saved > 0 => Trend::Total(tokens_saved),
        _ => Trend::PerSession(if sessions.is_empty() { 0 } else { tokens_saved / sessions.len() as u64 }),
    };

    CardStats {
        tokens_saved,
        less_to_read: (before > 0).then(|| (1.0 - after as f64 / before as f64).clamp(0.0, 1.0)),
        graph,
        learnings: learned,
        label: label(capped, first_day, today),
        trend,
    }
}

fn add(total: &mut u64, tokens: u64) {
    *total = total.saturating_add(tokens);
}

fn per_day_with_events(events: &[Event]) -> usize {
    events.iter().map(|e| day_of(e.ts_ms)).collect::<HashSet<_>>().len()
}

fn day_of(ts_ms: i64) -> NaiveDate {
    Local.timestamp_millis_opt(ts_ms).earliest().map_or(NaiveDate::MIN, |t| t.date_naive())
}

/// `capped` wins over `first_day`: history older than the window reads "last 30 days"
/// even when nothing falls inside it.
fn label(capped: bool, first_day: Option<NaiveDate>, today: NaiveDate) -> String {
    match first_day {
        _ if capped => format!("last {WINDOW_DAYS} days"),
        None => "first day".to_string(),
        Some(day) => {
            let days = (today - day).num_days().max(1);
            format!("since {} · {days} {}", day.format("%b %-d"), if days == 1 { "day" } else { "days" })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap()
    }

    fn ts(days_ago: i64) -> i64 {
        (now() - Duration::days(days_ago)).timestamp_millis()
    }

    fn event(kind: EventKind, days_ago: i64, saved: usize) -> Event {
        Event {
            ts_ms: ts(days_ago),
            session: "s1".to_string(),
            kind,
            tool: "t".to_string(),
            bytes_before: 0,
            bytes_after: 0,
            saved_tokens: saved,
            learning: None,
            cost_minutes: None,
            cost_tokens: None,
        }
    }

    fn learning(id: &str, days_ago: i64, tokens: Option<u64>) -> Event {
        let mut e = event(EventKind::Learning, days_ago, 0);
        e.learning = Some(id.to_string());
        e.cost_tokens = tokens;
        e
    }

    fn repo_with(events: &[Event]) -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        for e in events {
            events::append(repo.path(), e);
        }
        repo
    }

    #[test]
    fn headline_is_the_sum_of_the_tiles() {
        let repo = repo_with(&[
            event(EventKind::Answer, 1, 2_000),
            event(EventKind::Answer, 1, 600),
            learning("a", 1, Some(27_000)),
            learning("a", 1, Some(27_000)),
            learning("b", 1, None),
        ]);
        let stats = collect(&[repo.path().to_path_buf()], now());
        assert_eq!(stats.tokens_saved, 29_600);
        assert_eq!(stats.learnings.count, 2);
        assert_eq!(stats.learnings.tokens, 27_000);
        assert_eq!(stats.graph, Tile { tokens: 2_600, count: 2 });
    }

    #[test]
    fn less_to_read_uses_answers_only() {
        let mut answer = event(EventKind::Answer, 1, 10);
        answer.bytes_before = 1_000;
        answer.bytes_after = 250;
        let mut noisy = learning("a", 1, Some(5));
        noisy.bytes_before = 9_999;
        let repo = repo_with(&[answer, noisy]);
        assert_eq!(collect(&[repo.path().to_path_buf()], now()).less_to_read, Some(0.75));
    }

    #[test]
    fn label_for_a_new_user() {
        let repo = repo_with(&[event(EventKind::Answer, 7, 10)]);
        assert_eq!(collect(&[repo.path().to_path_buf()], now()).label, "since Oct 21 · 7 days");
    }

    #[test]
    fn label_after_thirty_days() {
        let repo = repo_with(&[event(EventKind::Answer, 40, 1_000), event(EventKind::Answer, 2, 10)]);
        let stats = collect(&[repo.path().to_path_buf()], now());
        assert_eq!(stats.label, "last 30 days");
        assert_eq!(stats.tokens_saved, 10);
    }

    #[test]
    fn no_events_is_first_day_and_zeros() {
        let stats = collect(&[], now());
        assert_eq!(stats.label, "first day");
        assert_eq!(stats.tokens_saved, 0);
        assert_eq!(stats.less_to_read, None);
        assert_eq!(stats.graph, Tile::default());
        assert_eq!(stats.learnings, LearningTile::default());
        assert_eq!(stats.trend, Trend::PerSession(0));
    }

    #[test]
    fn trend_needs_three_days() {
        let two = repo_with(&[event(EventKind::Answer, 1, 100), event(EventKind::Answer, 0, 300)]);
        assert_eq!(collect(&[two.path().to_path_buf()], now()).trend, Trend::PerSession(400));

        let three = repo_with(&[
            event(EventKind::Answer, 4, 100),
            event(EventKind::Answer, 2, 300),
            event(EventKind::Answer, 0, 50),
        ]);
        assert_eq!(collect(&[three.path().to_path_buf()], now()).trend, Trend::Daily(vec![100, 0, 300, 0, 50]));
    }

    #[test]
    fn gone_repo_is_skipped() {
        let repo = repo_with(&[event(EventKind::Answer, 1, 100)]);
        let gone = tempfile::tempdir().unwrap().path().join("nope");
        let stats = collect(&[gone, repo.path().to_path_buf()], now());
        assert_eq!(stats.tokens_saved, 100);
    }

    #[test]
    fn bad_lines_are_skipped() {
        let repo = repo_with(&[event(EventKind::Answer, 1, 100)]);
        let log = crate::ask::state_dir(repo.path()).join("events.jsonl");
        let mut file = std::fs::OpenOptions::new().append(true).open(log).unwrap();
        writeln!(file, "not json").unwrap();
        events::append(repo.path(), &event(EventKind::Answer, 1, 50));
        assert_eq!(collect(&[repo.path().to_path_buf()], now()).tokens_saved, 150);
    }

    #[test]
    fn old_events_beyond_the_tail_still_count() {
        let mut oldest = event(EventKind::Answer, 20, 1_000);
        oldest.tool = "x".repeat(100);
        let mut filler = event(EventKind::Answer, 1, 1);
        filler.tool = "y".repeat(1_000);
        let mut batch = vec![oldest];
        batch.extend(std::iter::repeat_with(|| filler.clone()).take(2_500));
        let repo = repo_with(&batch);
        let stats = collect(&[repo.path().to_path_buf()], now());
        assert_eq!(stats.tokens_saved, 1_000 + 2_500);
        assert_eq!(stats.graph.count, 2_501);
    }

    #[test]
    fn future_and_bad_timestamps_are_skipped() {
        let base = [event(EventKind::Answer, 4, 100), event(EventKind::Answer, 2, 300), event(EventKind::Answer, 0, 50)];
        let repo = repo_with(&base);
        let clean = collect(&[repo.path().to_path_buf()], now());
        for ts_ms in [ts(-3), ts(-4000), i64::MAX, i64::MIN] {
            let mut bad = event(EventKind::Answer, 0, 9_999);
            bad.ts_ms = ts_ms;
            events::append(repo.path(), &bad);
            let mut bad = learning("z", 0, Some(9_999));
            bad.ts_ms = ts_ms;
            events::append(repo.path(), &bad);
        }
        let stats = collect(&[repo.path().to_path_buf()], now());
        assert_eq!(stats, clean);
        assert!(matches!(&stats.trend, Trend::Daily(d) if d.len() <= 30));
    }

    #[test]
    fn daily_trend_has_at_most_thirty_entries_and_sums() {
        let repo = repo_with(&[
            event(EventKind::Answer, 40, 1),
            event(EventKind::Answer, 30, 100),
            event(EventKind::Answer, 10, 200),
            event(EventKind::Answer, 0, 300),
        ]);
        let stats = collect(&[repo.path().to_path_buf()], now());
        let Trend::Daily(days) = stats.trend else { panic!("expected daily") };
        assert_eq!(days.len(), 30);
        assert_eq!(days.iter().sum::<u64>(), stats.tokens_saved);
    }

    #[test]
    fn one_day_label_at_zero_elapsed_days() {
        let repo = repo_with(&[event(EventKind::Answer, 0, 10)]);
        assert_eq!(collect(&[repo.path().to_path_buf()], now()).label, "since Oct 28 · 1 day");
    }

    #[test]
    fn learning_shown_long_ago_and_yesterday_counts_once() {
        let repo = repo_with(&[learning("a", 40, Some(1_000)), learning("a", 1, Some(2_000))]);
        let stats = collect(&[repo.path().to_path_buf()], now());
        assert_eq!(stats.learnings.count, 1);
        assert_eq!(stats.learnings.tokens, 2_000);
    }

    #[test]
    fn same_learning_in_two_repos_counts_once() {
        let a = repo_with(&[learning("x", 3, Some(500))]);
        let b = repo_with(&[learning("x", 1, Some(700))]);
        let stats = collect(&[a.path().to_path_buf(), b.path().to_path_buf()], now());
        assert_eq!(stats.learnings.count, 1);
        assert_eq!(stats.learnings.tokens, 700);
    }

    #[test]
    fn sessions_with_only_learnings_count_toward_per_session() {
        let mut other = learning("a", 1, Some(1_000));
        other.session = "s2".to_string();
        let repo = repo_with(&[event(EventKind::Answer, 1, 200), other]);
        assert_eq!(collect(&[repo.path().to_path_buf()], now()).trend, Trend::PerSession(600));
    }

    #[test]
    fn savings_with_no_session_show_the_total() {
        let mut answer = event(EventKind::Answer, 1, 300);
        answer.session = String::new();
        let mut shown = learning("a", 1, Some(700));
        shown.session = String::new();
        let repo = repo_with(&[answer, shown]);
        let stats = collect(&[repo.path().to_path_buf()], now());
        assert_eq!(stats.tokens_saved, 1_000);
        assert_eq!(stats.trend, Trend::Total(1_000));
    }

    #[test]
    fn history_all_older_than_the_window_still_reads_last_30_days() {
        let repo = repo_with(&[event(EventKind::Answer, 40, 1_000), event(EventKind::Answer, 35, 10)]);
        let stats = collect(&[repo.path().to_path_buf()], now());
        assert_eq!(stats.label, "last 30 days");
        assert_eq!(stats.tokens_saved, 0);
    }

    #[test]
    fn huge_figures_saturate_instead_of_overflowing() {
        let repo = repo_with(&[
            event(EventKind::Answer, 4, usize::MAX - 1),
            event(EventKind::Answer, 2, 10),
            event(EventKind::Answer, 0, usize::MAX),
            learning("a", 0, Some(u64::MAX)),
        ]);
        let stats = collect(&[repo.path().to_path_buf()], now());
        assert_eq!(stats.tokens_saved, u64::MAX);
        assert_eq!(stats.graph.tokens, u64::MAX);
        assert!(matches!(&stats.trend, Trend::Daily(_)));
        let view = crate::card::render::CardView { stats, repo_name: None, agents: vec![] };
        crate::card::render::render_png(&view).unwrap();
    }

    #[test]
    fn less_to_read_is_never_negative() {
        let mut answer = event(EventKind::Answer, 1, 0);
        answer.bytes_before = 100;
        answer.bytes_after = 400;
        let repo = repo_with(&[answer]);
        assert_eq!(collect(&[repo.path().to_path_buf()], now()).less_to_read, Some(0.0));
    }
}
