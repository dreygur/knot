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
    /// Graph answers + swaps + reused learnings.
    pub tokens_saved: u64,
    /// 0.0..=1.0; `None` when nothing was read before.
    pub less_to_read: Option<f64>,
    pub graph: Tile,
    pub learnings: LearningTile,
    pub swaps: Tile,
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
}

/// Total the event logs of `repos` (repo roots). Missing repos and logs are skipped.
pub fn collect(repos: &[PathBuf], now: DateTime<Local>) -> CardStats {
    let start = now - Duration::days(WINDOW_DAYS);
    let mut all: Vec<Event> = repos
        .iter()
        .flat_map(|root| events::all(root))
        .filter(|e| matches!(e.kind, EventKind::Answer | EventKind::Swap | EventKind::Learning))
        .collect();
    all.sort_by_key(|e| e.ts_ms);

    let first = all.first().map(|e| e.ts_ms);
    let capped = first.is_some_and(|ts| ts <= start.timestamp_millis());
    if capped {
        all.retain(|e| e.ts_ms >= start.timestamp_millis());
    }

    let mut graph = Tile::default();
    let mut swaps = Tile::default();
    let (mut before, mut after) = (0u64, 0u64);
    let mut sessions: HashSet<&str> = HashSet::new();
    let mut per_day: HashMap<NaiveDate, u64> = HashMap::new();
    // Latest event per learning id wins (the log is sorted, so later overwrites earlier)
    let mut learnings: HashMap<&str, &Event> = HashMap::new();

    for e in &all {
        let tile = match e.kind {
            EventKind::Answer => &mut graph,
            EventKind::Swap => &mut swaps,
            _ => {
                if let Some(id) = e.learning.as_deref() {
                    learnings.insert(id, e);
                }
                continue;
            }
        };
        tile.tokens += e.saved_tokens as u64;
        tile.count += 1;
        before += e.bytes_before;
        after += e.bytes_after;
        if !e.session.is_empty() {
            sessions.insert(&e.session);
        }
        *per_day.entry(day_of(e.ts_ms)).or_default() += e.saved_tokens as u64;
    }

    let mut learned = LearningTile::default();
    for e in learnings.values() {
        let tokens = e.cost_tokens.unwrap_or(0);
        learned.count += 1;
        learned.tokens += tokens;
        learned.minutes += e.cost_minutes.unwrap_or(0);
        *per_day.entry(day_of(e.ts_ms)).or_default() += tokens;
    }

    let tokens_saved = graph.tokens + swaps.tokens + learned.tokens;
    let today = now.date_naive();
    let first_day = all.first().map(|e| day_of(e.ts_ms));
    let trend = match first_day {
        Some(day) if per_day_with_events(&all) >= MIN_TREND_DAYS => {
            let days = (today - day).num_days().max(0);
            Trend::Daily((0..=days).map(|i| per_day.get(&(day + Duration::days(i))).copied().unwrap_or(0)).collect())
        }
        _ => Trend::PerSession(if sessions.is_empty() { 0 } else { tokens_saved / sessions.len() as u64 }),
    };

    CardStats {
        tokens_saved,
        less_to_read: (before > 0).then(|| 1.0 - after as f64 / before as f64),
        graph,
        learnings: learned,
        swaps,
        label: label(capped, first_day, today),
        trend,
    }
}

fn per_day_with_events(events: &[Event]) -> usize {
    events.iter().map(|e| day_of(e.ts_ms)).collect::<HashSet<_>>().len()
}

fn day_of(ts_ms: i64) -> NaiveDate {
    Local.timestamp_millis_opt(ts_ms).single().map_or(NaiveDate::MIN, |t| t.date_naive())
}

fn label(capped: bool, first_day: Option<NaiveDate>, today: NaiveDate) -> String {
    match first_day {
        None => "first day".to_string(),
        Some(_) if capped => format!("last {WINDOW_DAYS} days"),
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
            command: None,
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
    fn headline_is_the_sum_of_the_three_tiles() {
        let repo = repo_with(&[
            event(EventKind::Answer, 1, 2_000),
            event(EventKind::Answer, 1, 600),
            event(EventKind::Swap, 1, 400),
            learning("a", 1, Some(27_000)),
            learning("a", 1, Some(27_000)),
            learning("b", 1, None),
        ]);
        let stats = collect(&[repo.path().to_path_buf()], now());
        assert_eq!(stats.tokens_saved, 30_000);
        assert_eq!(stats.learnings.count, 2);
        assert_eq!(stats.learnings.tokens, 27_000);
        assert_eq!(stats.graph, Tile { tokens: 2_600, count: 2 });
        assert_eq!(stats.swaps, Tile { tokens: 400, count: 1 });
    }

    #[test]
    fn less_to_read_uses_answers_and_swaps_only() {
        let mut answer = event(EventKind::Answer, 1, 10);
        answer.bytes_before = 1_000;
        answer.bytes_after = 250;
        let mut noisy = learning("a", 1, Some(5));
        noisy.bytes_before = 9_999;
        let repo = repo_with(&[answer, noisy]);
        assert_eq!(collect(&[repo.path().to_path_buf()], now()).less_to_read, Some(0.75));
    }

    #[test]
    fn python_blocks_are_ignored() {
        let repo = repo_with(&[event(EventKind::Answer, 2, 100)]);
        let before = collect(&[repo.path().to_path_buf()], now());
        let mut blocked = event(EventKind::EditBlocked, 9, 500);
        blocked.bytes_before = 100;
        events::append(repo.path(), &blocked);
        events::append(repo.path(), &event(EventKind::Override, 9, 500));
        assert_eq!(collect(&[repo.path().to_path_buf()], now()), before);
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
        assert_eq!(stats.swaps, Tile::default());
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
        oldest.command = Some("x".repeat(100));
        let mut filler = event(EventKind::Answer, 1, 1);
        filler.command = Some("y".repeat(1_000));
        let mut batch = vec![oldest];
        batch.extend(std::iter::repeat_with(|| filler.clone()).take(2_500));
        let repo = repo_with(&batch);
        let stats = collect(&[repo.path().to_path_buf()], now());
        assert_eq!(stats.tokens_saved, 1_000 + 2_500);
        assert_eq!(stats.graph.count, 2_501);
    }
}
