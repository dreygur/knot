use super::{DeleteSkillResult, ExecutionResult, StepResult, StorageEngine, VerificationOutput};
use crate::skills::{SkillNode, SkillStep};
use anyhow::Result;
use std::process::Command;
use uuid::Uuid;

impl StorageEngine {
    pub async fn save_skill(
        &self,
        name: String,
        description: String,
        prerequisites: Vec<String>,
        steps: Vec<SkillStep>,
        verification_command: String,
        related_node_id: Option<Uuid>,
    ) -> Result<SkillNode> {
        let skill = SkillNode::new(
            name,
            description,
            prerequisites,
            steps,
            verification_command,
            related_node_id,
        );
        self.graph.insert_skill(&skill).await?;
        tracing::info!(
            "[KNOT] Saved skill '{}' (score={:.2})",
            skill.name,
            skill.utility_score
        );
        Ok(skill)
    }

    pub async fn execute_skill(
        &self,
        name: &str,
        variables: Vec<(String, String)>,
    ) -> Result<ExecutionResult> {
        let Some(skill) = self.graph.get_skill(name).await? else {
            return Err(anyhow::anyhow!("Skill '{}' not found", name));
        };

        if !skill.is_dry_run_passed(&variables) {
            return Ok(ExecutionResult {
                success: false,
                step_results: vec![],
                verification_output: None,
                detail: "Dry-run check failed".into(),
            });
        }

        let expanded_steps = crate::skills::interpolate_steps(&skill.steps, &variables);
        let mut step_results = Vec::new();

        for step in &expanded_steps {
            let output = if let Some(ref wd) = step.working_dir {
                Command::new("sh")
                    .args(["-c", &step.command])
                    .current_dir(wd)
                    .output()
            } else {
                Command::new("sh").args(["-c", &step.command]).output()
            };

            step_results.push(match output {
                Ok(o) => StepResult {
                    command: step.command.clone(),
                    success: o.status.success(),
                    stdout: String::from_utf8_lossy(&o.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&o.stderr).to_string(),
                },
                Err(e) => StepResult {
                    command: step.command.clone(),
                    success: false,
                    stdout: String::new(),
                    stderr: e.to_string(),
                },
            });
        }

        let all_steps_passed = step_results.iter().all(|r| r.success);
        let verification_output: Option<VerificationOutput> = if all_steps_passed {
            let cmd = crate::skills::interpolate_for_shell(&skill.verification_command, &variables);
            match Command::new("sh").args(["-c", &cmd]).output() {
                Ok(o) => Some(VerificationOutput {
                    success: o.status.success(),
                    stdout: String::from_utf8_lossy(&o.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&o.stderr).to_string(),
                }),
                Err(e) => Some(VerificationOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: e.to_string(),
                }),
            }
        } else {
            None
        };

        if let Some(ref vo) = verification_output {
            if vo.success {
                self.graph.increment_skill_success(name).await?;
                tracing::info!(
                    "[KNOT] SUCCESS: Skill '{}' executed. Utility score incremented.",
                    name
                );
            }
        }

        let verification_passed = verification_output
            .as_ref()
            .map(|v| v.success)
            .unwrap_or(false);
        Ok(ExecutionResult {
            success: all_steps_passed && verification_passed,
            step_results,
            verification_output,
            detail: if all_steps_passed {
                "All steps executed"
            } else {
                "Some steps failed"
            }
            .into(),
        })
    }

    pub async fn recall_skills(&self, query: &str) -> Result<Vec<SkillNode>> {
        let all = self.graph.list_skills().await?;
        let query_lower = query.to_lowercase();
        Ok(all
            .into_iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&query_lower)
                    || s.description.to_lowercase().contains(&query_lower)
            })
            .collect())
    }

    /// Delete a skill by name. Blocked if success_count > 10 and force is false.
    pub async fn delete_skill(&self, name: &str, force: bool) -> Result<DeleteSkillResult> {
        let Some(success_count) = self.graph.get_skill_success_count(name).await? else {
            return Ok(DeleteSkillResult::NotFound);
        };
        if success_count > 10 && !force {
            return Ok(DeleteSkillResult::HighUtilityBlocked { success_count });
        }
        self.graph.delete_skill_by_name(name).await?;
        tracing::info!(
            "[KNOT] Deleted skill '{}' (success_count={})",
            name,
            success_count
        );
        Ok(DeleteSkillResult::Deleted)
    }
}
