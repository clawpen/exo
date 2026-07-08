//! Lightweight multi-agent orchestration.
//!
//! This module keeps Exo's "fast/light" principle: orchestration is just a
//! small state machine that turns one prime directive into scoped prompts for
//! specialized agents, records their reports, and decides the next prompt until
//! the goal is satisfied or blocked.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Top-level objective supplied by a human/operator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrimeDirective {
    pub objective: String,
    #[serde(default)]
    pub success_criteria: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,
}

fn default_max_rounds() -> u32 {
    12
}

/// A specialized agent slot in the orchestration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRole {
    pub id: String,
    pub name: String,
    pub specialty: String,
}

/// Work assigned to an agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTask {
    pub id: String,
    pub agent_id: String,
    pub prompt: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub status: TaskStatus,
    pub attempts: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Blocked,
}

/// Report produced by an agent after receiving a prompt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentReport {
    pub task_id: String,
    pub status: TaskStatus,
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub followups: Vec<String>,
    /// Success criteria this agent explicitly claims to have satisfied.
    ///
    /// This is the strongest completion signal: when present, the coordinator
    /// matches these against the directive's `success_criteria` instead of
    /// guessing from the free-text summary.
    #[serde(default)]
    pub satisfied_criteria: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrchestrationState {
    pub directive: PrimeDirective,
    pub agents: Vec<AgentRole>,
    pub tasks: Vec<AgentTask>,
    pub reports: Vec<AgentReport>,
    pub round: u32,
    pub status: OrchestrationStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationStatus {
    Running,
    Succeeded,
    Blocked,
    Failed,
}

/// Decision returned by the coordinator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrchestratorDecision {
    PromptAgent { task: AgentTask },
    Succeeded { summary: String },
    Blocked { reason: String },
}

pub struct Orchestrator {
    state: OrchestrationState,
    ready_queue: VecDeque<String>,
}

impl Orchestrator {
    pub fn new(directive: PrimeDirective, agents: Vec<AgentRole>) -> Self {
        let mut tasks = vec![];
        let mut ready_queue = VecDeque::new();
        for (idx, agent) in agents.iter().enumerate() {
            let id = format!("task-{}", idx + 1);
            let prompt = build_agent_prompt(&directive, agent, &[]);
            tasks.push(AgentTask {
                id: id.clone(),
                agent_id: agent.id.clone(),
                prompt,
                depends_on: vec![],
                status: TaskStatus::Pending,
                attempts: 0,
            });
            ready_queue.push_back(id);
        }

        Self {
            state: OrchestrationState {
                directive,
                agents,
                tasks,
                reports: vec![],
                round: 0,
                status: OrchestrationStatus::Running,
            },
            ready_queue,
        }
    }

    /// Rebuild a coordinator from previously persisted state.
    ///
    /// This is the mechanical half of "resume": pending tasks are re-queued in
    /// order, and any task left `Running` (an interrupted/crashed step) is
    /// treated as pending again so it can be retried on the next drive. If the
    /// previous coordinator ended blocked/failed, failed/blocked tasks are also
    /// re-queued once so a caller can resume with a fixed executor or updated
    /// environment. Reports and round counters are preserved so success
    /// detection and audit history stay intact.
    pub fn from_state(mut state: OrchestrationState) -> Self {
        let mut ready_queue = VecDeque::new();
        let should_retry_terminal_tasks = matches!(
            state.status,
            OrchestrationStatus::Blocked | OrchestrationStatus::Failed
        );
        for task in &mut state.tasks {
            if task.status == TaskStatus::Running {
                // The previous run stopped mid-flight; make it runnable again.
                task.status = TaskStatus::Pending;
                if task.attempts > 0 {
                    task.attempts -= 1;
                }
            }
            if should_retry_terminal_tasks
                && matches!(task.status, TaskStatus::Failed | TaskStatus::Blocked)
            {
                task.status = TaskStatus::Pending;
            }
            if task.status == TaskStatus::Pending {
                ready_queue.push_back(task.id.clone());
            }
        }
        if state.status != OrchestrationStatus::Succeeded {
            state.status = OrchestrationStatus::Running;
        }
        Self { state, ready_queue }
    }

    pub fn state(&self) -> &OrchestrationState {
        &self.state
    }

    /// Get the next agent prompt or terminal decision.
    pub fn next(&mut self) -> OrchestratorDecision {
        if self.goal_satisfied() {
            self.state.status = OrchestrationStatus::Succeeded;
            return OrchestratorDecision::Succeeded {
                summary: "All success criteria are covered by agent reports.".to_string(),
            };
        }
        if self.state.round >= self.state.directive.max_rounds {
            self.state.status = OrchestrationStatus::Blocked;
            return OrchestratorDecision::Blocked {
                reason: format!(
                    "maximum rounds ({}) reached",
                    self.state.directive.max_rounds
                ),
            };
        }

        while let Some(task_id) = self.ready_queue.pop_front() {
            let Some(idx) = self.state.tasks.iter().position(|task| task.id == task_id) else {
                continue;
            };
            if self.state.tasks[idx].status != TaskStatus::Pending {
                continue;
            }
            if !dependencies_satisfied(&self.state.tasks[idx], &self.state.reports) {
                continue;
            }
            let task = &mut self.state.tasks[idx];
            task.status = TaskStatus::Running;
            task.attempts += 1;
            let task = task.clone();
            self.state.round += 1;
            return OrchestratorDecision::PromptAgent { task };
        }

        self.state.status = OrchestrationStatus::Blocked;
        OrchestratorDecision::Blocked {
            reason: "no pending runnable agent tasks remain".to_string(),
        }
    }

    /// Record an agent report and schedule follow-up prompts if needed.
    pub fn record_report(&mut self, report: AgentReport) {
        if let Some(task) = self.task_mut(&report.task_id) {
            task.status = report.status;
        }

        let followups = report.followups.clone();
        self.state.reports.push(report.clone());

        if report.status == TaskStatus::Failed && !self.retry_existing(&report.task_id) {
            self.state.status = OrchestrationStatus::Blocked;
        }

        for followup in followups {
            if let Some(agent) = self.choose_agent_for_followup(&followup) {
                let id = format!("task-{}", self.state.tasks.len() + 1);
                let prompt = build_agent_prompt(
                    &self.state.directive,
                    agent,
                    &[format!("Follow-up requested: {}", followup)],
                );
                self.state.tasks.push(AgentTask {
                    id: id.clone(),
                    agent_id: agent.id.clone(),
                    prompt,
                    depends_on: vec![report.task_id.clone()],
                    status: TaskStatus::Pending,
                    attempts: 0,
                });
                self.ready_queue.push_back(id);
            }
        }
    }

    fn task_mut(&mut self, id: &str) -> Option<&mut AgentTask> {
        self.state.tasks.iter_mut().find(|task| task.id == id)
    }

    fn retry_existing(&mut self, task_id: &str) -> bool {
        let Some(task) = self.task_mut(task_id) else {
            return false;
        };
        if task.attempts >= 2 {
            return false;
        }
        task.status = TaskStatus::Pending;
        self.ready_queue.push_front(task_id.to_string());
        true
    }

    fn choose_agent_for_followup(&self, followup: &str) -> Option<&AgentRole> {
        let lower = followup.to_lowercase();
        self.state
            .agents
            .iter()
            .find(|agent| lower.contains(&agent.specialty.to_lowercase()))
            .or_else(|| self.state.agents.first())
    }

    fn goal_satisfied(&self) -> bool {
        if self.state.directive.success_criteria.is_empty() {
            return self
                .state
                .tasks
                .iter()
                .all(|task| task.status == TaskStatus::Succeeded);
        }

        self.state
            .directive
            .success_criteria
            .iter()
            .all(|criterion| criterion_satisfied_by_reports(criterion, &self.state.reports))
    }
}

fn criterion_satisfied_by_reports(criterion: &str, reports: &[AgentReport]) -> bool {
    let normalized_criterion = normalize_text(criterion);
    if normalized_criterion.is_empty() {
        return false;
    }

    reports
        .iter()
        .filter(|report| report.status == TaskStatus::Succeeded)
        .any(|report| {
            report
                .satisfied_criteria
                .iter()
                .any(|claimed| criteria_equivalent(&normalized_criterion, claimed))
                || normalize_text(&report.summary).contains(&normalized_criterion)
        })
}

fn criteria_equivalent(normalized_criterion: &str, claimed: &str) -> bool {
    let normalized_claim = normalize_text(claimed);
    !normalized_claim.is_empty()
        && (normalized_claim == normalized_criterion
            || normalized_claim.contains(normalized_criterion)
            || normalized_criterion.contains(&normalized_claim))
}

fn normalize_text(value: &str) -> String {
    value
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn dependencies_satisfied(task: &AgentTask, reports: &[AgentReport]) -> bool {
    task.depends_on.iter().all(|dep| {
        reports
            .iter()
            .any(|report| &report.task_id == dep && report.status == TaskStatus::Succeeded)
    })
}

fn build_agent_prompt(
    directive: &PrimeDirective,
    agent: &AgentRole,
    extra_context: &[String],
) -> String {
    let mut prompt = String::new();
    prompt.push_str("Prime directive:\n");
    prompt.push_str(&directive.objective);
    prompt.push_str("\n\nYour role:\n");
    prompt.push_str(&format!("{} — {}\n", agent.name, agent.specialty));

    if !directive.success_criteria.is_empty() {
        prompt.push_str("\nSuccess criteria:\n");
        for item in &directive.success_criteria {
            prompt.push_str(&format!("- {}\n", item));
        }
    }
    if !directive.constraints.is_empty() {
        prompt.push_str("\nConstraints:\n");
        for item in &directive.constraints {
            prompt.push_str(&format!("- {}\n", item));
        }
    }
    if !extra_context.is_empty() {
        prompt.push_str("\nContext:\n");
        for item in extra_context {
            prompt.push_str(&format!("- {}\n", item));
        }
    }
    prompt.push_str(
        "\nReturn a concise report with: status, what you did, artifacts, blockers, and follow-up prompts for other agents if needed.",
    );
    prompt
}

/// Useful default agent set for local Exo projects.
pub fn default_agent_roles() -> Vec<AgentRole> {
    vec![
        AgentRole {
            id: "planner".to_string(),
            name: "Planner".to_string(),
            specialty: "planning".to_string(),
        },
        AgentRole {
            id: "builder".to_string(),
            name: "Builder".to_string(),
            specialty: "implementation".to_string(),
        },
        AgentRole {
            id: "verifier".to_string(),
            name: "Verifier".to_string(),
            specialty: "verification".to_string(),
        },
    ]
}

/// Simple helper to summarize task counts by status.
pub fn status_counts(state: &OrchestrationState) -> HashMap<&'static str, usize> {
    let mut map = HashMap::new();
    for task in &state.tasks {
        let key = match task.status {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Succeeded => "succeeded",
            TaskStatus::Failed => "failed",
            TaskStatus::Blocked => "blocked",
        };
        *map.entry(key).or_insert(0) += 1;
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directive() -> PrimeDirective {
        PrimeDirective {
            objective: "Ship a lightweight Exo agent workflow".to_string(),
            success_criteria: vec![
                "planning complete".to_string(),
                "implementation complete".to_string(),
                "verification complete".to_string(),
            ],
            constraints: vec!["keep daemon light".to_string()],
            max_rounds: 10,
        }
    }

    #[test]
    fn creates_initial_prompts_for_default_agents() {
        let orch = Orchestrator::new(directive(), default_agent_roles());
        assert_eq!(orch.state().tasks.len(), 3);
        assert!(orch.state().tasks[0].prompt.contains("Prime directive"));
        assert!(orch.state().tasks[0].prompt.contains("keep daemon light"));
    }

    #[test]
    fn records_reports_and_detects_success() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        for i in 1..=3 {
            let decision = orch.next();
            let task_id = match decision {
                OrchestratorDecision::PromptAgent { task } => task.id,
                _ => panic!("expected prompt"),
            };
            let summary = match i {
                1 => "planning complete",
                2 => "implementation complete",
                _ => "verification complete",
            };
            orch.record_report(AgentReport {
                task_id,
                status: TaskStatus::Succeeded,
                summary: summary.to_string(),
                artifacts: vec![],
                followups: vec![],
                satisfied_criteria: vec![],
            });
        }
        assert!(matches!(
            orch.next(),
            OrchestratorDecision::Succeeded { .. }
        ));
    }

    #[test]
    fn summary_mentions_do_not_cross_satisfy_other_criteria() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = match orch.next() {
            OrchestratorDecision::PromptAgent { task } => task.id,
            _ => panic!("expected prompt"),
        };
        orch.record_report(AgentReport {
            task_id,
            status: TaskStatus::Succeeded,
            summary: "planning complete; builder will handle implementation and verifier will handle verification".to_string(),
            artifacts: vec![],
            followups: vec![],
            satisfied_criteria: vec![],
        });

        assert!(!matches!(
            orch.next(),
            OrchestratorDecision::Succeeded { .. }
        ));
    }

    #[test]
    fn explicit_satisfied_criteria_can_complete_goal() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = match orch.next() {
            OrchestratorDecision::PromptAgent { task } => task.id,
            _ => panic!("expected prompt"),
        };
        orch.record_report(AgentReport {
            task_id,
            status: TaskStatus::Succeeded,
            summary: "I completed the whole checklist.".to_string(),
            artifacts: vec![],
            followups: vec![],
            satisfied_criteria: vec![
                "planning complete".to_string(),
                "implementation complete".to_string(),
                "verification complete".to_string(),
            ],
        });

        assert!(matches!(
            orch.next(),
            OrchestratorDecision::Succeeded { .. }
        ));
    }

    #[test]
    fn failed_task_gets_one_retry() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = match orch.next() {
            OrchestratorDecision::PromptAgent { task } => task.id,
            _ => panic!("expected prompt"),
        };
        orch.record_report(AgentReport {
            task_id: task_id.clone(),
            status: TaskStatus::Failed,
            summary: "temporary failure".to_string(),
            artifacts: vec![],
            followups: vec![],
            satisfied_criteria: vec![],
        });
        let retry_id = match orch.next() {
            OrchestratorDecision::PromptAgent { task } => task.id,
            other => panic!("expected retry, got {:?}", other),
        };
        assert_eq!(retry_id, task_id);
    }

    #[test]
    fn followups_create_dependent_tasks() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = match orch.next() {
            OrchestratorDecision::PromptAgent { task } => task.id,
            _ => panic!("expected prompt"),
        };
        orch.record_report(AgentReport {
            task_id: task_id.clone(),
            status: TaskStatus::Succeeded,
            summary: "planning complete".to_string(),
            artifacts: vec![],
            followups: vec!["implementation should add IPC".to_string()],
            satisfied_criteria: vec![],
        });
        assert!(orch.state().tasks.iter().any(|task| {
            task.depends_on == vec![task_id.clone()] && task.prompt.contains("Follow-up requested")
        }));
    }

    #[test]
    fn from_state_requeues_pending_and_interrupted_tasks() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let first = match orch.next() {
            OrchestratorDecision::PromptAgent { task } => task,
            _ => panic!("expected prompt"),
        };
        orch.record_report(AgentReport {
            task_id: first.id,
            status: TaskStatus::Succeeded,
            summary: "planning complete".to_string(),
            artifacts: vec![],
            followups: vec![],
            satisfied_criteria: vec![],
        });
        let second = match orch.next() {
            OrchestratorDecision::PromptAgent { task } => task,
            _ => panic!("expected prompt"),
        };

        let mut resumed = Orchestrator::from_state(orch.state().clone());
        let next = match resumed.next() {
            OrchestratorDecision::PromptAgent { task } => task,
            other => panic!("expected resumed prompt, got {:?}", other),
        };
        assert_eq!(next.id, second.id);
        assert_eq!(next.attempts, 1);
    }
}
