//! Claude Code's context and plan usage, as its status line last reported them. Hooks
//! never receive these numbers, so the status line saves them per session and the
//! PostToolUse hook reads them back to warn before a limit ends the session.

use serde::{Deserialize, Serialize};
use std::path::Path;

const USAGE_PREFIX: &str = "usage-";

/// One status line reading. A field Claude Code did not send stays `None`: the rate
/// limits come only on Pro and Max plans, after the first response, and either window
/// can be missing.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour_pct: Option<f64>,
    /// Unix epoch seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour_resets_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day_pct: Option<f64>,
    pub ts_ms: i64,
}

fn file_name(session_id: &str) -> String {
    format!("{USAGE_PREFIX}{}.json", super::session::safe_id(session_id))
}

fn number(json: &serde_json::Value, ptr: &str) -> Option<f64> {
    json.pointer(ptr).and_then(serde_json::Value::as_f64)
}

/// The usage in status line `json`, or `None` when it carries none.
fn from_statusline(json: &serde_json::Value, ts_ms: i64) -> Option<Usage> {
    let usage = Usage {
        context_pct: number(json, "/context_window/used_percentage"),
        five_hour_pct: number(json, "/rate_limits/five_hour/used_percentage"),
        five_hour_resets_at: number(json, "/rate_limits/five_hour/resets_at").map(|s| s as i64),
        seven_day_pct: number(json, "/rate_limits/seven_day/used_percentage"),
        ts_ms,
    };
    let any = usage.context_pct.is_some() || usage.five_hour_pct.is_some() || usage.seven_day_pct.is_some();
    any.then_some(usage)
}

/// Save `usage` for `session_id`, only in a repo Knot already keeps state for. Silent
/// on failure: a status line has nowhere to report it.
pub fn record(root: &Path, session_id: &str, usage: &Usage) {
    let dir = super::state_dir(root);
    if session_id.is_empty() || !dir.is_dir() {
        return;
    }
    let Ok(path) = super::state_file(&dir, &file_name(session_id)) else {
        return;
    };
    if let Ok(json) = serde_json::to_vec(usage) {
        let _ = std::fs::write(path, json);
    }
}

/// Save what the status line JSON `json` says about the session's usage.
pub fn record_statusline(root: &Path, json: &serde_json::Value, ts_ms: i64) {
    let Some(session_id) = json.get("session_id").and_then(|v| v.as_str()) else {
        return;
    };
    if let Some(usage) = from_statusline(json, ts_ms) {
        record(root, session_id, &usage);
    }
}

/// The usage the status line last saved for `session_id` in `root`.
pub fn load(root: &Path, session_id: &str) -> Option<Usage> {
    let dir = super::state_dir(root);
    let name = file_name(session_id);
    if session_id.is_empty() || !super::state_file_readable(&dir, &name) {
        return None;
    }
    serde_json::from_slice(&std::fs::read(dir.join(name)).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repo Knot keeps state for, as after `knot init`.
    fn knot_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(crate::ask::state_dir(dir.path())).unwrap();
        dir
    }

    #[test]
    fn records_context_and_rate_limits_from_the_status_line() {
        let repo = knot_repo();
        let json = serde_json::json!({
            "session_id": "s1",
            "context_window": { "used_percentage": 42.5 },
            "rate_limits": {
                "five_hour": { "used_percentage": 71, "resets_at": 1_793_212_800 },
                "seven_day": { "used_percentage": 12.0, "resets_at": 1_793_600_000 }
            }
        });

        record_statusline(repo.path(), &json, 5_000);

        let usage = load(repo.path(), "s1").unwrap();
        assert_eq!(
            usage,
            Usage {
                context_pct: Some(42.5),
                five_hour_pct: Some(71.0),
                five_hour_resets_at: Some(1_793_212_800),
                seven_day_pct: Some(12.0),
                ts_ms: 5_000,
            }
        );
        assert!(load(repo.path(), "s2").is_none(), "each session has its own");
    }

    #[test]
    fn missing_fields_stay_absent() {
        let repo = knot_repo();
        let dir = crate::ask::state_dir(repo.path());

        record_statusline(repo.path(), &serde_json::json!({ "session_id": "s1", "context_window": { "used_percentage": null } }), 1);
        assert!(!dir.join(file_name("s1")).exists(), "nothing to record, nothing written");

        record_statusline(repo.path(), &serde_json::json!({ "session_id": "s1", "context_window": { "used_percentage": 30 } }), 1);
        let saved = std::fs::read_to_string(dir.join(file_name("s1"))).unwrap();
        assert_eq!(saved, r#"{"context_pct":30.0,"ts_ms":1}"#);
    }

    #[test]
    fn writes_nothing_in_a_repo_without_knot_state() {
        let dir = tempfile::tempdir().unwrap();
        let json = serde_json::json!({ "session_id": "s1", "context_window": { "used_percentage": 30 } });
        record_statusline(dir.path(), &json, 1);
        assert!(!crate::ask::state_dir(dir.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn never_writes_through_a_symlinked_cache() {
        let repo = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".knot")).unwrap();
        std::os::unix::fs::symlink(target.path(), repo.path().join(".knot/cache")).unwrap();

        record(repo.path(), "s1", &Usage { context_pct: Some(90.0), ..Default::default() });

        assert_eq!(std::fs::read_dir(target.path()).unwrap().count(), 0);
        assert!(load(repo.path(), "s1").is_none());
    }
}
