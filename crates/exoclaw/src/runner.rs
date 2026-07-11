//! Orchestration runner.
//!
//! Drives the [`Orchestrator`] state machine by actually executing agent
//! prompts and feeding structured reports back until the prime directive is
//! satisfied or blocked. Execution is abstracted behind [`AgentExecutor`] so it
//! stays light and testable: a real deployment can spawn local Exo agents as
//! processes, while tests can use scripted/built-in executors.

use crate::orchestrator::{
    AgentReport, AgentTask, OrchestrationStatus, Orchestrator, OrchestratorDecision, TaskStatus,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// Executes one agent prompt and returns that agent's report.
pub trait AgentExecutor {
    fn execute(&mut self, task: &AgentTask) -> Result<AgentReport>;
}

/// Observes runner progress for durable logs/checkpoints.
pub trait RunObserver {
    fn on_task_prompted(&mut self, _task: &AgentTask) -> Result<()> {
        Ok(())
    }

    fn on_agent_report(
        &mut self,
        _task: &AgentTask,
        _report: &AgentReport,
        _state: &crate::orchestrator::OrchestrationState,
    ) -> Result<()> {
        Ok(())
    }

    fn on_run_finished(&mut self, _outcome: &RunOutcome) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct NoopRunObserver;

impl RunObserver for NoopRunObserver {}

/// Outcome of a full orchestration run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunOutcome {
    pub status: OrchestrationStatus,
    pub rounds: u32,
    pub message: String,
    /// Aggregated token usage across all agent reports that reported usage.
    #[serde(default)]
    pub usage: crate::orchestrator::TokenUsage,
}

/// Drive the orchestrator to a terminal state.
///
/// `max_steps` is a safety bound independent of the directive's `max_rounds`;
/// it prevents runaway loops if an executor keeps producing follow-ups.
pub fn run_to_completion(
    orchestrator: &mut Orchestrator,
    executor: &mut dyn AgentExecutor,
    max_steps: u32,
) -> Result<RunOutcome> {
    let mut observer = NoopRunObserver;
    run_to_completion_with_observer(orchestrator, executor, max_steps, &mut observer)
}

/// Drive the orchestrator to a terminal state and emit progress to an observer.
pub fn run_to_completion_with_observer(
    orchestrator: &mut Orchestrator,
    executor: &mut dyn AgentExecutor,
    max_steps: u32,
    observer: &mut dyn RunObserver,
) -> Result<RunOutcome> {
    let mut steps = 0u32;
    loop {
        match orchestrator.next() {
            OrchestratorDecision::Succeeded { summary } => {
                let outcome = RunOutcome {
                    status: OrchestrationStatus::Succeeded,
                    rounds: orchestrator.state().round,
                    message: summary,
                    usage: aggregate_usage(orchestrator.state().reports.iter().filter_map(|r| r.usage)),
                };
                observer.on_run_finished(&outcome)?;
                return Ok(outcome);
            }
            OrchestratorDecision::Blocked { reason } => {
                let outcome = RunOutcome {
                    status: orchestrator.state().status,
                    rounds: orchestrator.state().round,
                    message: reason,
                    usage: aggregate_usage(orchestrator.state().reports.iter().filter_map(|r| r.usage)),
                };
                observer.on_run_finished(&outcome)?;
                return Ok(outcome);
            }
            OrchestratorDecision::PromptAgent { task } => {
                steps += 1;
                if steps > max_steps {
                    let outcome = RunOutcome {
                        status: OrchestrationStatus::Blocked,
                        rounds: orchestrator.state().round,
                        message: format!("runner exceeded max steps ({})", max_steps),
                        usage: aggregate_usage(orchestrator.state().reports.iter().filter_map(|r| r.usage)),
                    };
                    observer.on_run_finished(&outcome)?;
                    return Ok(outcome);
                }
                observer.on_task_prompted(&task)?;
                let report = match executor.execute(&task) {
                    Ok(mut report) => {
                        // Executors may forget to echo the task id; enforce it.
                        report.task_id = task.id.clone();
                        report
                    }
                    Err(e) => AgentReport {
                        task_id: task.id.clone(),
                        status: TaskStatus::Failed,
                        summary: format!("executor error: {}", e),
                        artifacts: vec![],
                        followups: vec![],
                        satisfied_criteria: vec![],
                        usage: None,
                    },
                };
                orchestrator.record_report(report.clone());
                observer.on_agent_report(&task, &report, orchestrator.state())?;
            }
        }
    }
}

fn aggregate_usage(usages: impl Iterator<Item = crate::orchestrator::TokenUsage>) -> crate::orchestrator::TokenUsage {
    usages.fold(crate::orchestrator::TokenUsage::default(), |acc, u| acc + u)
}

/// Built-in executor that marks each task succeeded with a synthetic summary.
///
/// Useful for wiring, dry runs, and tests where no live agent/LLM is available.
/// The summary intentionally echoes each success-criterion-relevant keyword by
/// including the agent specialty and prompt-derived hints.
pub struct BuiltinExecutor {
    /// Optional per-agent summary overrides keyed by `agent_id`.
    pub summaries: HashMap<String, String>,
}

impl BuiltinExecutor {
    pub fn new() -> Self {
        Self {
            summaries: HashMap::new(),
        }
    }

    pub fn with_summary(mut self, agent_id: &str, summary: &str) -> Self {
        self.summaries
            .insert(agent_id.to_string(), summary.to_string());
        self
    }
}

impl Default for BuiltinExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentExecutor for BuiltinExecutor {
    fn execute(&mut self, task: &AgentTask) -> Result<AgentReport> {
        let summary = self
            .summaries
            .get(&task.agent_id)
            .cloned()
            .unwrap_or_else(|| format!("{} completed", task.agent_id));
        Ok(AgentReport {
            task_id: task.id.clone(),
            status: TaskStatus::Succeeded,
            summary,
            artifacts: vec![],
            followups: vec![],
            satisfied_criteria: vec![format!("{} completed", task.agent_id)],
            usage: None,
        })
    }
}

/// Payload written to an agent process' stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPrompt {
    pub task_id: String,
    pub agent_id: String,
    pub prompt: String,
}

/// Executor that runs an external command per prompt.
///
/// The command is executed via `sh -c <template>`. The agent receives:
/// - the prompt JSON (`AgentPrompt`) on stdin
/// - `EXO_TASK_ID` / `EXO_AGENT_ID` environment variables
///
/// and must print an `AgentReport` as JSON on stdout (last non-empty line).
pub struct CommandAgentExecutor {
    template: String,
    per_agent: HashMap<String, String>,
}

impl CommandAgentExecutor {
    pub fn new(template: impl Into<String>) -> Self {
        Self {
            template: template.into(),
            per_agent: HashMap::new(),
        }
    }

    /// Override the command for a specific agent id.
    pub fn with_agent_command(mut self, agent_id: &str, template: &str) -> Self {
        self.per_agent
            .insert(agent_id.to_string(), template.to_string());
        self
    }

    fn command_for(&self, agent_id: &str) -> &str {
        self.per_agent.get(agent_id).unwrap_or(&self.template)
    }
}

impl AgentExecutor for CommandAgentExecutor {
    fn execute(&mut self, task: &AgentTask) -> Result<AgentReport> {
        let template = self.command_for(&task.agent_id).to_string();
        let prompt = AgentPrompt {
            task_id: task.id.clone(),
            agent_id: task.agent_id.clone(),
            prompt: task.prompt.clone(),
        };
        let prompt_json = serde_json::to_string(&prompt)?;

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&template)
            .env("EXO_TASK_ID", &task.id)
            .env("EXO_AGENT_ID", &task.agent_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn agent command for {}", task.agent_id))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt_json.as_bytes())?;
            stdin.write_all(b"\n")?;
        }

        let mut stdout = String::new();
        if let Some(mut out) = child.stdout.take() {
            out.read_to_string(&mut stdout)?;
        }
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            err.read_to_string(&mut stderr)?;
        }
        let status = child.wait()?;

        for line in stdout
            .lines()
            .rev()
            .map(|l| l.trim())
            .filter(|l| l.starts_with('{'))
        {
            if let Ok(mut report) = serde_json::from_str::<AgentReport>(line) {
                report.task_id = task.id.clone();
                return Ok(report);
            }
        }

        // No structured report: treat exit status as the signal.
        let ok = status.success();
        Ok(AgentReport {
            task_id: task.id.clone(),
            status: if ok {
                TaskStatus::Succeeded
            } else {
                TaskStatus::Failed
            },
            summary: if ok {
                format!("{} completed (exit 0)", task.agent_id)
            } else {
                format!(
                    "{} failed (exit {:?}): {}",
                    task.agent_id,
                    status.code(),
                    stderr.trim()
                )
            },
            artifacts: vec![],
            followups: vec![],
            satisfied_criteria: vec![],
            usage: None,
        })
    }
}

/// Executor that spawns each agent through the `exo` CLI.
///
/// This is a thin wrapper around [`CommandAgentExecutor`], but it constructs a
/// safe `exo run ...` command from structured options so users don't need to
/// hand-roll a shell template for the common case.
#[derive(Debug, Clone)]
pub struct ExoAgentExecutor {
    pub exo_bin: String,
    pub backend: String,
    pub image: String,
    pub agent_command: Vec<String>,
    pub volumes: Vec<(String, String)>,
    pub secrets: Vec<String>,
    pub sandbox: Option<String>,
}

impl ExoAgentExecutor {
    pub fn new(agent_command: Vec<String>) -> Self {
        Self {
            exo_bin: "exo".to_string(),
            backend: "native".to_string(),
            image: "host".to_string(),
            agent_command,
            volumes: vec![],
            secrets: vec![],
            sandbox: None,
        }
    }

    /// Build the exact shell command used by [`CommandAgentExecutor`].
    pub fn command_template(&self) -> String {
        let mut parts = vec![
            shell_quote(&self.exo_bin),
            "run".to_string(),
            "--backend".to_string(),
            shell_quote(&self.backend),
            "--rm".to_string(),
            "-e".to_string(),
            "EXO_TASK_ID=\"$EXO_TASK_ID\"".to_string(),
            "-e".to_string(),
            "EXO_AGENT_ID=\"$EXO_AGENT_ID\"".to_string(),
        ];
        if let Some(sandbox) = &self.sandbox {
            parts.push("--sandbox".to_string());
            parts.push(shell_quote(sandbox));
        }
        for secret in &self.secrets {
            parts.push("--secret".to_string());
            parts.push(shell_quote(secret));
        }
        for (source, target) in &self.volumes {
            parts.push("-v".to_string());
            parts.push(shell_quote(&format!("{}:{}", source, target)));
        }
        parts.push(shell_quote(&self.image));
        parts.push("--".to_string());
        parts.extend(self.agent_command.iter().map(|arg| shell_quote(arg)));
        parts.join(" ")
    }
}

impl AgentExecutor for ExoAgentExecutor {
    fn execute(&mut self, task: &AgentTask) -> Result<AgentReport> {
        let mut executor = CommandAgentExecutor::new(self.command_template());
        executor.execute(task)
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::{default_agent_roles, PrimeDirective};

    fn directive() -> PrimeDirective {
        PrimeDirective {
            objective: "Ship a lightweight Exo agent workflow".to_string(),
            success_criteria: vec![
                "planner complete".to_string(),
                "builder complete".to_string(),
                "verifier complete".to_string(),
            ],
            constraints: vec!["keep daemon light".to_string()],
            max_rounds: 12,
        }
    }

    #[test]
    fn builtin_executor_runs_to_success() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let mut exec = BuiltinExecutor::new();
        let outcome = run_to_completion(&mut orch, &mut exec, 50).unwrap();
        assert_eq!(outcome.status, OrchestrationStatus::Succeeded);
    }

    struct FlakyExecutor {
        failed_once: bool,
    }

    impl AgentExecutor for FlakyExecutor {
        fn execute(&mut self, task: &AgentTask) -> Result<AgentReport> {
            if task.agent_id == "builder" && !self.failed_once {
                self.failed_once = true;
                return Ok(AgentReport {
                    task_id: task.id.clone(),
                    status: TaskStatus::Failed,
                    summary: "transient".to_string(),
                    artifacts: vec![],
                    followups: vec![],
                    satisfied_criteria: vec![],
                    usage: None,
                });
            }
            Ok(AgentReport {
                task_id: task.id.clone(),
                status: TaskStatus::Succeeded,
                summary: format!("{} complete", task.agent_id),
                artifacts: vec![],
                followups: vec![],
                satisfied_criteria: vec![format!("{} complete", task.agent_id)],
                usage: None,
            })
        }
    }

    #[test]
    fn runner_retries_then_succeeds() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let mut exec = FlakyExecutor { failed_once: false };
        let outcome = run_to_completion(&mut orch, &mut exec, 50).unwrap();
        assert_eq!(outcome.status, OrchestrationStatus::Succeeded);
    }

    #[test]
    fn command_executor_reads_report_json() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        // Each agent echoes a success report with a matching summary keyword.
        let template = r#"printf '{"task_id":"","status":"succeeded","summary":"%s complete","artifacts":[],"followups":[]}\n' "$EXO_AGENT_ID""#;
        let mut exec = CommandAgentExecutor::new(template);
        let outcome = run_to_completion(&mut orch, &mut exec, 50).unwrap();
        assert_eq!(outcome.status, OrchestrationStatus::Succeeded);
    }

    #[test]
    fn exo_executor_builds_structured_command() {
        let mut exec =
            ExoAgentExecutor::new(vec!["sh".to_string(), "-c".to_string(), "cat".to_string()]);
        exec.exo_bin = "/usr/local/bin/exo".to_string();
        exec.backend = "native".to_string();
        exec.image = "host".to_string();
        exec.sandbox = Some("off".to_string());
        exec.volumes
            .push(("workspace".to_string(), "/workspace".to_string()));
        exec.secrets.push("OPENAI_API_KEY".to_string());
        let cmd = exec.command_template();
        assert!(cmd.contains("'run'") || cmd.contains(" run "));
        assert!(cmd.contains("--backend 'native'"));
        assert!(cmd.contains("EXO_TASK_ID=\"$EXO_TASK_ID\""));
        assert!(cmd.contains("EXO_AGENT_ID=\"$EXO_AGENT_ID\""));
        assert!(cmd.contains("--sandbox 'off'"));
        assert!(cmd.contains("--secret 'OPENAI_API_KEY'"));
        assert!(cmd.contains("-v 'workspace:/workspace'"));
        assert!(cmd.contains("'host' -- 'sh' '-c' 'cat'"));
    }
}
