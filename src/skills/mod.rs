use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillStep {
    pub description: String,
    pub command: String,
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillNode {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub prerequisites: Vec<String>,
    pub steps: Vec<SkillStep>,
    pub verification_command: String,
    pub success_count: i32,
    pub utility_score: f32,
    pub related_node_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SkillNode {
    pub fn new(
        name: String,
        description: String,
        prerequisites: Vec<String>,
        steps: Vec<SkillStep>,
        verification_command: String,
        related_node_id: Option<Uuid>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            description,
            prerequisites,
            steps,
            verification_command,
            success_count: 0,
            utility_score: 0.5,
            related_node_id,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn increment_success(&mut self) {
        self.success_count += 1;
        self.utility_score = (self.utility_score + 0.05).min(1.0);
        self.updated_at = Utc::now();
    }

    pub fn is_dry_run_passed(&self, variables: &[(String, String)]) -> bool {
        for prereq in &self.prerequisites {
            let expanded = interpolate(prereq, variables);
            if !path_or_command_exists(&expanded) {
                eprintln!("[KNOT] DRY-RUN FAIL: prerequisite '{}' not found", expanded);
                return false;
            }
        }
        true
    }
}

fn path_or_command_exists(s: &str) -> bool {
    if s.starts_with("cmd:") {
        // Use Command::new("which") directly - no shell, no injection surface.
        let cmd = s.trim_start_matches("cmd:").trim();
        std::process::Command::new("which")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else {
        std::path::Path::new(s).exists()
    }
}

/// Shell-escape a value for safe embedding in `sh -c` command strings.
/// Wraps in single quotes and escapes embedded single quotes via `'\''`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Plain interpolation - for display strings and descriptions only.
pub fn interpolate(template: &str, variables: &[(String, String)]) -> String {
    let mut result = template.to_string();
    for (key, value) in variables {
        let pattern = format!("{{{{{}}}}}", key);
        result = result.replace(&pattern, value);
    }
    result
}

/// Shell-safe interpolation for commands passed to `sh -c`.
/// Variable values are single-quote escaped to prevent shell injection.
pub fn interpolate_for_shell(template: &str, variables: &[(String, String)]) -> String {
    let mut result = template.to_string();
    for (key, value) in variables {
        let pattern = format!("{{{{{}}}}}", key);
        result = result.replace(&pattern, &shell_quote(value));
    }
    result
}

pub fn interpolate_steps(steps: &[SkillStep], variables: &[(String, String)]) -> Vec<SkillStep> {
    steps
        .iter()
        .map(|s| SkillStep {
            description: interpolate(&s.description, variables),
            command: interpolate_for_shell(&s.command, variables),
            working_dir: s.working_dir.clone(),
        })
        .collect()
}
