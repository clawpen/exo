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
    /// The agent ran out of turns/tokens before finishing.
    ///
    /// Distinct from both `Succeeded` (it did NOT finish) and `Failed` (its work
    /// so far is real and worth keeping). The caller should resume it with a
    /// bigger budget rather than accept it or throw the work away.
    Incomplete,
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
    /// Optional token usage reported by the agent's LLM worker.
    #[serde(default)]
    pub usage: Option<TokenUsage>,
    /// Structured verdict, required from the inspector (verifier) role.
    ///
    /// The inspector also writes `verdict.json` to the workspace as the
    /// human-facing artifact; this field carries the same object through the
    /// report channel so the coordinator can validate and record it without
    /// knowing where the workspace lives.
    #[serde(default)]
    pub verdict: Option<InspectionVerdict>,
}

/// The inspector's verdict on worker output. Serialized kebab-case to match
/// the `verdict.json` contract ("pass", "fail-fixable", "fail-escalate").
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Output meets contract; route to the conductor for integration.
    Pass,
    /// Specific, fixable defects; retry the same worker with the diagnosis.
    FailFixable,
    /// The task was mis-specced or beyond the worker tier; the conductor
    /// must re-decompose rather than retry.
    FailEscalate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum VerdictConfidence {
    High,
    /// The inspector could not verify everything. Treated as fail-escalate
    /// by the conductor regardless of the verdict field.
    Low,
}

/// Assessment of one success criterion. `criterion` must be copied verbatim
/// from the directive; `evidence` must cite something checkable (a command run
/// and its result, or a file path and what was verified in it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CriterionAssessment {
    pub criterion: String,
    pub met: bool,
    #[serde(default)]
    pub evidence: String,
}

/// Where the verdict says the work should go next.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VerdictRouteHint {
    /// For fail-fixable: the diagnosis restated as an actionable worker task.
    #[serde(default)]
    pub retry_task: Option<String>,
    /// For fail-escalate: what the original spec got wrong.
    #[serde(default)]
    pub respec: Option<String>,
}

/// Structured verdict returned by the inspector role (see `verdict.json`
/// contract in Orchestre's docs/INSPECTOR-PROTOCOL.md).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InspectionVerdict {
    pub verdict: Verdict,
    #[serde(default)]
    pub criteria: Vec<CriterionAssessment>,
    #[serde(default)]
    pub diagnosis: Option<String>,
    #[serde(default)]
    pub route_hint: Option<VerdictRouteHint>,
    #[serde(default)]
    pub confidence: Option<VerdictConfidence>,
}

/// Token usage for a single agent prompt.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

impl std::ops::Add for TokenUsage {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            input_tokens: self.input_tokens + rhs.input_tokens,
            output_tokens: self.output_tokens + rhs.output_tokens,
        }
    }
}

impl std::iter::Sum for TokenUsage {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), |a, b| a + b)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrchestrationState {
    pub directive: PrimeDirective,
    pub agents: Vec<AgentRole>,
    pub tasks: Vec<AgentTask>,
    pub reports: Vec<AgentReport>,
    pub round: u32,
    pub status: OrchestrationStatus,
    /// Latest validated inspector verdict, if the verifier role has produced one.
    #[serde(default)]
    pub verdict: Option<InspectionVerdict>,
    /// Workspace directory the agents share. When set, the coordinator reads
    /// `<workspace>/verdict.json` as a fallback verdict channel for inspector
    /// reports that don't embed one — agents reliably write the file even when
    /// they forget the report field. Persisted so resume keeps working.
    #[serde(default)]
    pub workspace: Option<String>,
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
                verdict: None,
                workspace: None,
            },
            ready_queue,
        }
    }

    /// Set the shared workspace directory (enables the verdict.json fallback
    /// channel for inspector reports). Builder-style; call right after `new`.
    pub fn with_workspace(mut self, workspace: impl Into<String>) -> Self {
        self.state.workspace = Some(workspace.into());
        self
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
            // An Incomplete task ran out of budget mid-flight. It is always worth
            // resuming (unlike Failed/Blocked, which only requeue when the previous
            // run ended badly), because its partial work is real and the only thing
            // it needs is more turns.
            if task.status == TaskStatus::Incomplete {
                task.status = TaskStatus::Pending;
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
    pub fn record_report(&mut self, mut report: AgentReport) {
        // An agent that claims success but produced no summary, no artifacts, and
        // no satisfied criteria did nothing. Recording that as success is the most
        // dishonest thing this coordinator can do: it hides the failure, poisons
        // goal evaluation, and sends the caller hunting for a bug that isn't there.
        // Call it what it is and let the retry path deal with it.
        if is_empty_report(&report) {
            report.status = TaskStatus::Failed;
            report.summary = format!(
                "agent returned an empty report (no summary, artifacts, or satisfied criteria) \
                 - it most likely did nothing; check the agent's stdout/stderr logs"
            );
        } else if is_truncated_report(&report) {
            // The agent stopped because it ran out of turns, not because it was done.
            // Reporting that as success is how a half-finished job gets accepted.
            report.status = TaskStatus::Incomplete;
        } else if report.status == TaskStatus::Succeeded && self.is_inspector_task(&report.task_id) {
            // Inspector enforcement. A verification that claims success but carries
            // no structured verdict — or a verdict that violates the contract —
            // is the soft-review failure mode: it rubber-stamps work and teaches
            // the conductor to trust it. Same rule as the empty-report gate:
            // record it as the failure it is and let the retry path deal with it.
            //
            // Fallback channel: agents reliably *write* verdict.json to the
            // workspace even when they forget to embed the verdict in their
            // report JSON (observed in dogfooding). If the workspace is known,
            // read the file before declaring the verdict missing.
            if report.verdict.is_none() {
                report.verdict = self.load_verdict_file();
            }
            match &report.verdict {
                Some(verdict) => match validate_verdict(verdict, &self.state.directive) {
                    Ok(()) => self.state.verdict = Some(verdict.clone()),
                    Err(reason) => {
                        report.status = TaskStatus::Failed;
                        report.summary = format!(
                            "inspector verdict invalid: {}. The verdict must follow the \
                             verdict.json contract (see the inspector prompt).",
                            reason
                        );
                    }
                },
                None => {
                    report.status = TaskStatus::Failed;
                    report.summary =
                        "inspector produced no verdict: a successful verification must return \
                         a structured verdict object (and write verdict.json to the workspace)"
                            .to_string();
                }
            }
        }

        if let Some(task) = self.task_mut(&report.task_id) {
            task.status = report.status;
        }

        let followups = report.followups.clone();
        // Keep only the latest report for each task so retries don't leave stale failures in the log.
        self.state.reports.retain(|r| r.task_id != report.task_id);
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

    /// Is this task assigned to the inspector (verifier) role?
    fn is_inspector_task(&self, task_id: &str) -> bool {
        let Some(task) = self.state.tasks.iter().find(|t| t.id == task_id) else {
            return false;
        };
        self.state
            .agents
            .iter()
            .find(|a| a.id == task.agent_id)
            .map(is_inspector_role)
            .unwrap_or(false)
    }

    /// Read and parse `<workspace>/verdict.json`, if a workspace is configured
    /// and the file exists and parses. Missing/unparseable is None, not an
    /// error — the caller treats it as "no verdict" and fails the report.
    fn load_verdict_file(&self) -> Option<InspectionVerdict> {
        let workspace = self.state.workspace.as_ref()?;
        let path = std::path::Path::new(workspace).join("verdict.json");
        let contents = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&contents).ok()
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
        // When the run includes an inspector, only a validated pass verdict
        // completes the goal. Anything less lets workers self-certify: the
        // summary-substring fallback below is trivially satisfied by any
        // report that merely *quotes* a criterion (observed in dogfooding: a
        // planner's "builder should report 'code written'" completed the run
        // before the builder and verifier ever ran). The verdict pass already
        // cross-checks every criterion with evidence, so it subsumes the
        // criteria-coverage check. Runs without an inspector role keep the
        // old behavior.
        if self.state.agents.iter().any(is_inspector_role) {
            return matches!(
                self.state.verdict.as_ref().map(|v| v.verdict),
                Some(Verdict::Pass)
            );
        }

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

/// The inspector role: the cold-context verifier. Identified by id or specialty
/// so custom role sets keep working as long as one role owns "verification".
fn is_inspector_role(agent: &AgentRole) -> bool {
    agent.id == "verifier" || agent.specialty == "verification"
}

/// Check a verdict against the contract. Returns why it's invalid, for the
/// failure summary the coordinator records.
fn validate_verdict(verdict: &InspectionVerdict, directive: &PrimeDirective) -> Result<(), String> {
    match verdict.verdict {
        Verdict::Pass => {
            if directive.success_criteria.is_empty() {
                // No criteria to cross-check; a pass needs at least one evidenced
                // assessment to mean anything.
                if verdict.criteria.iter().all(|c| c.evidence.trim().is_empty()) {
                    return Err(
                        "pass verdict carries no evidence for any assessed criterion".to_string()
                    );
                }
                return Ok(());
            }
            for criterion in &directive.success_criteria {
                let normalized = normalize_text(criterion);
                let assessment = verdict
                    .criteria
                    .iter()
                    .find(|a| criteria_equivalent(&normalized, &a.criterion));
                match assessment {
                    None => {
                        return Err(format!(
                            "pass verdict does not assess criterion: {}",
                            criterion
                        ))
                    }
                    Some(a) if !a.met => {
                        return Err(format!(
                            "pass verdict marks criterion unmet: {}",
                            criterion
                        ))
                    }
                    Some(a) if a.evidence.trim().is_empty() => {
                        return Err(format!(
                            "criterion assessed without evidence: {}",
                            criterion
                        ))
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        Verdict::FailFixable => {
            let actionable = verdict
                .route_hint
                .as_ref()
                .and_then(|h| h.retry_task.as_ref())
                .map(|t| !t.trim().is_empty())
                .unwrap_or(false);
            if actionable {
                Ok(())
            } else {
                Err("fail-fixable verdict requires route_hint.retry_task".to_string())
            }
        }
        Verdict::FailEscalate => {
            let explained = verdict
                .route_hint
                .as_ref()
                .and_then(|h| h.respec.as_ref())
                .map(|t| !t.trim().is_empty())
                .unwrap_or(false);
            if explained {
                Ok(())
            } else {
                Err("fail-escalate verdict requires route_hint.respec".to_string())
            }
        }
    }
}

/// A report that claims success while carrying no evidence of work.
///
/// This is the signature of an agent that never ran, never reached the model, or
/// silently gave up. It must never be treated as success.
fn is_empty_report(report: &AgentReport) -> bool {    report.status == TaskStatus::Succeeded
        && report.summary.trim().is_empty()
        && report.artifacts.is_empty()
        && report.satisfied_criteria.is_empty()
}

/// A report from an agent that stopped on its turn/token budget rather than finishing.
///
/// Agents signal this in prose rather than structurally, so we match on the phrasing
/// they actually use. A false positive here is cheap (the caller resumes work that was
/// already done); a false negative is expensive (a half-finished job is accepted as
/// complete), so lean towards catching it.
fn is_truncated_report(report: &AgentReport) -> bool {
    if report.status != TaskStatus::Succeeded {
        return false;
    }
    let s = report.summary.to_lowercase();
    const CAP_HIT_PHRASES: [&str; 4] = [
        "may need more iterations",
        "need more iterations",
        "please continue if needed",
        "ran out of turns",
    ];
    CAP_HIT_PHRASES.iter().any(|p| s.contains(p))
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
    if is_inspector_role(agent) {
        return build_inspector_prompt(directive, agent, extra_context);
    }

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
        "\n\nWhen you produce code or structured output, write each file in its own \
         markdown code block preceded by a header line `### path/to/file.ext`. \
         You may create multiple files. Put a brief summary after the files.",
    );
    prompt.push_str(
        "\nReturn a concise report with: status, what you did, artifacts, blockers, and follow-up prompts for other agents if needed.",
    );
    prompt
}

/// Prompt template for the inspector role.
///
/// The inspector is the system's lie detector: a cold-context agent that
/// verifies worker output against the original intent and returns a structured
/// verdict, never vibes. Three properties are load-bearing and must survive any
/// edit of this template:
///
/// 1. Cold context is stated explicitly — the inspector has not seen the work
///    or any discussion of it, and must trust nothing except its own checks.
/// 2. Intent is injected verbatim (objective, criteria, constraints) — never a
///    paraphrase, so no information is lost at the handoff.
/// 3. Grading is evidence-gated — the inspector runs builds/tests/reads and
///    cites what it checked; "looks good" is not evidence.
fn build_inspector_prompt(
    directive: &PrimeDirective,
    agent: &AgentRole,
    extra_context: &[String],
) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are the Inspector. You did NOT do the work under review and have not seen \
         any prior discussion of it. You receive only the original objective, the success \
         criteria, the constraints, and the workspace containing the workers' artifacts. \
         Trust nothing in the worker reports except as pointers to evidence you check \
         yourself.\n",
    );

    prompt.push_str("\nObjective (verbatim):\n");
    prompt.push_str(&directive.objective);
    prompt.push('\n');

    if !directive.success_criteria.is_empty() {
        prompt.push_str(
            "\nSuccess criteria (verbatim — assess each one exactly as written; \
             do not paraphrase):\n",
        );
        for item in &directive.success_criteria {
            prompt.push_str(&format!("- {}\n", item));
        }
    }
    if !directive.constraints.is_empty() {
        prompt.push_str("\nConstraints (verbatim):\n");
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
        "\nMethod:\n\
         - Verify with evidence, not vibes. Use your tools: run the build, run the tests, \
         read the files. Each criterion needs checkable evidence: a command you ran and \
         its result, or a file path and what you verified in it.\n\
         - \"Looks good\" is not evidence. If you cannot verify a criterion, mark it \
         unmet and set confidence to \"low\".\n",
    );

    prompt.push_str(
        "\nVerdicts:\n\
         - \"pass\": every criterion met with evidence. The work is ready to integrate.\n\
         - \"fail-fixable\": the work is wrong in specific, fixable ways. Attach a \
         diagnosis and restate it as an actionable task for the worker \
         (route_hint.retry_task).\n\
         - \"fail-escalate\": the task itself was mis-specified, ambiguous, or missing \
         context — retrying the same worker will not help. Explain what the spec got \
         wrong (route_hint.respec).\n\
         Choose fail-escalate over fail-fixable when the failure is the spec's fault, \
         not the worker's.\n",
    );

    prompt.push_str(&format!(
        "\nOutput contract:\n\
         1. Write a file named verdict.json at the workspace root with this shape:\n\
         {{\n\
         \x20 \"verdict\": \"pass\" | \"fail-fixable\" | \"fail-escalate\",\n\
         \x20 \"criteria\": [{{\"criterion\": \"<verbatim>\", \"met\": true|false, \
         \"evidence\": \"...\"}}],\n\
         \x20 \"diagnosis\": \"...\",\n\
         \x20 \"route_hint\": {{\"retry_task\": \"...\", \"respec\": \"...\"}},\n\
         \x20 \"confidence\": \"high\" | \"low\"\n\
         }}\n\
         \x20 - criterion strings must be copied verbatim from the success criteria above.\n\
         \x20 - fail-fixable requires route_hint.retry_task; fail-escalate requires \
         route_hint.respec.\n\
         2. In your report JSON, include the same object under a \"verdict\" key, list \
         verdict.json in \"artifacts\", and do NOT claim any satisfied_criteria unless \
         the verdict is \"pass\" and you verified them yourself.\n\
         3. Return a concise report with: status, your verdict in one line, what you \
         checked, and blockers if any.\n",
    ));

    // The role line stays at the end so template changes don't bury the
    // assignment; keep agent.name/specialty visible for custom role sets.
    prompt.push_str(&format!(
        "\n(Your role slot: {} — {}.)\n",
        agent.name, agent.specialty
    ));
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
            TaskStatus::Incomplete => "incomplete",
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

    /// Regression: an agent that reports success with nothing to show did nothing.
    /// This exact shape (succeeded + empty summary + no artifacts + no criteria) is
    /// what silently wasted an hour of debugging, so pin it hard.
    #[test]
    fn empty_report_is_recorded_as_failure_not_success() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = match orch.next() {
            OrchestratorDecision::PromptAgent { task } => task.id,
            _ => panic!("expected prompt"),
        };

        orch.record_report(AgentReport {
            task_id: task_id.clone(),
            status: TaskStatus::Succeeded, // the agent *claims* success
            summary: String::new(),        // ...but shows no work
            artifacts: vec![],
            followups: vec![],
            satisfied_criteria: vec![],
            usage: None,
            verdict: None,
        });

        let report = orch
            .state()
            .reports
            .iter()
            .find(|r| r.task_id == task_id)
            .expect("report recorded");
        assert_eq!(
            report.status,
            TaskStatus::Failed,
            "an empty report must never be recorded as success"
        );
        assert!(
            report.summary.contains("empty report"),
            "the summary must say why: {}",
            report.summary
        );

        // The goal must not be considered satisfied off the back of it.
        assert!(!orch.goal_satisfied());
    }

    /// Regression: an agent that stops on its turn cap has NOT finished. This is the
    /// exact summary exo-agent emits, and it used to be recorded as `succeeded`,
    /// silently accepting a half-written subsystem as done.
    #[test]
    fn cap_hit_report_is_incomplete_not_success() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = match orch.next() {
            OrchestratorDecision::PromptAgent { task } => task.id,
            _ => panic!("expected prompt"),
        };

        orch.record_report(AgentReport {
            task_id: task_id.clone(),
            status: TaskStatus::Succeeded, // the agent claims success...
            summary: "I've completed the available actions but may need more iterations \
                      to finish. Please continue if needed."
                .to_string(), // ...while saying it ran out of turns
            artifacts: vec!["Sources/MechCore/MAHeatModel.m".to_string()],
            followups: vec![],
            satisfied_criteria: vec![],
            usage: None,
            verdict: None,
        });

        let report = orch
            .state()
            .reports
            .iter()
            .find(|r| r.task_id == task_id)
            .expect("report recorded");
        assert_eq!(
            report.status,
            TaskStatus::Incomplete,
            "hitting the turn cap must not be reported as success"
        );
        // Partial work is preserved, not discarded.
        assert_eq!(report.artifacts.len(), 1);
        assert!(!orch.goal_satisfied());
    }

    /// An Incomplete task must be resumable: its partial work is real, it just
    /// needs more budget.
    #[test]
    fn incomplete_task_is_requeued_on_resume() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = match orch.next() {
            OrchestratorDecision::PromptAgent { task } => task.id,
            _ => panic!("expected prompt"),
        };
        orch.record_report(AgentReport {
            task_id: task_id.clone(),
            status: TaskStatus::Succeeded,
            summary: "may need more iterations".to_string(),
            artifacts: vec![],
            followups: vec![],
            satisfied_criteria: vec![],
            usage: None,
            verdict: None,
        });

        // Resume from the persisted state, as `orchestrate-resume` would.
        let resumed = Orchestrator::from_state(orch.state().clone());
        let task = resumed
            .state()
            .tasks
            .iter()
            .find(|t| t.id == task_id)
            .expect("task exists");
        assert_eq!(
            task.status,
            TaskStatus::Pending,
            "an incomplete task must be re-queued so it can finish"
        );
    }

    /// The guard must not fire on a legitimate success that simply has no artifacts.
    #[test]
    fn report_with_summary_but_no_artifacts_is_still_success() {
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
            followups: vec![],
            satisfied_criteria: vec![],
            usage: None,
            verdict: None,
        });

        let report = orch
            .state()
            .reports
            .iter()
            .find(|r| r.task_id == task_id)
            .expect("report recorded");
        assert_eq!(report.status, TaskStatus::Succeeded);
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
                usage: None,
                verdict: if i == 3 {
                    Some(passing_verdict_for_directive(&directive()))
                } else {
                    None
                },
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
            usage: None,
            verdict: None,
        });

        assert!(!matches!(
            orch.next(),
            OrchestratorDecision::Succeeded { .. }
        ));
    }

    /// Regression (dogfood run inspector-dogfood-002): the planner's summary
    /// quoted all three criteria verbatim ("builder should report 'code
    /// written'") and the summary-substring fallback marked the run complete
    /// before the builder and verifier ever ran. With an inspector in the role
    /// set, ONLY a validated pass verdict completes the goal — worker claims,
    /// structured or not, never do.
    #[test]
    fn worker_claims_do_not_complete_goal_when_inspector_present() {
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
            usage: None,
            verdict: None,
        });

        assert!(
            !matches!(orch.next(), OrchestratorDecision::Succeeded { .. }),
            "worker claims must not complete the goal; only the inspector's pass verdict does"
        );
    }

    /// Regression (dogfood run inspector-dogfood-003): agents reliably WRITE
    /// verdict.json to the workspace even when they forget to embed the verdict
    /// in their report JSON. When the workspace is known, the file is a valid
    /// fallback verdict channel.
    #[test]
    fn verdict_json_file_is_fallback_when_report_omits_verdict() {
        let dir = std::env::temp_dir().join(format!("exoclaw-verdict-fallback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let verdict = passing_verdict_for_directive(&directive());
        std::fs::write(
            dir.join("verdict.json"),
            serde_json::to_string_pretty(&verdict).unwrap(),
        )
        .unwrap();

        let mut orch = Orchestrator::new(directive(), default_agent_roles())
            .with_workspace(dir.to_string_lossy().to_string());
        let task_id = prompt_verifier(&mut orch);

        // Report claims success, lists verdict.json as an artifact, but omits
        // the verdict field — exactly what the Kimi worker did.
        orch.record_report(verifier_report(&task_id, None));

        let report = orch
            .state()
            .reports
            .iter()
            .find(|r| r.task_id == task_id)
            .expect("report recorded");
        assert_eq!(
            report.status,
            TaskStatus::Succeeded,
            "the verdict.json file must satisfy the verdict requirement: {}",
            report.summary
        );
        assert_eq!(
            orch.state().verdict.as_ref().map(|v| v.verdict),
            Some(Verdict::Pass)
        );
        assert!(matches!(
            orch.next(),
            OrchestratorDecision::Succeeded { .. }
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The full inspector flow: workers produce, the inspector verifies, and
    /// its pass verdict is what completes the run.
    #[test]
    fn inspector_pass_verdict_completes_goal() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = prompt_verifier(&mut orch);

        // Even with the inspector present, the run must not complete before
        // the verifier has reported.
        orch.record_report(verifier_report(
            &task_id,
            Some(passing_verdict_for_directive(&directive())),
        ));

        assert!(matches!(
            orch.next(),
            OrchestratorDecision::Succeeded { .. }
        ));
    }

    /// A fail-fixable verdict means the run must NOT report success.
    #[test]
    fn fail_verdict_does_not_complete_goal() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = prompt_verifier(&mut orch);

        let verdict = InspectionVerdict {
            verdict: Verdict::FailFixable,
            criteria: vec![],
            diagnosis: Some("builder skipped error handling".to_string()),
            route_hint: Some(VerdictRouteHint {
                retry_task: Some("add error handling".to_string()),
                respec: None,
            }),
            confidence: Some(VerdictConfidence::High),
        };
        orch.record_report(verifier_report(&task_id, Some(verdict)));

        assert!(
            !matches!(orch.next(), OrchestratorDecision::Succeeded { .. }),
            "a fail verdict must never complete the goal"
        );
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
            usage: None,
            verdict: None,
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
            usage: None,
            verdict: None,
        });
        assert!(orch.state().tasks.iter().any(|task| {
            task.depends_on == vec![task_id.clone()] && task.prompt.contains("Follow-up requested")
        }));
    }

    fn passing_verdict_for_directive(directive: &PrimeDirective) -> InspectionVerdict {
        InspectionVerdict {
            verdict: Verdict::Pass,
            criteria: directive
                .success_criteria
                .iter()
                .map(|c| CriterionAssessment {
                    criterion: c.clone(),
                    met: true,
                    evidence: "ran the tests; all pass".to_string(),
                })
                .collect(),
            diagnosis: None,
            route_hint: None,
            confidence: Some(VerdictConfidence::High),
        }
    }

    /// Drive the coordinator to the verifier task and return its id.
    fn prompt_verifier(orch: &mut Orchestrator) -> String {
        // Complete planner and builder so the verifier task is next.
        for summary in ["planning complete", "implementation complete"] {
            let task_id = match orch.next() {
                OrchestratorDecision::PromptAgent { task } => task.id,
                other => panic!("expected prompt, got {:?}", other),
            };
            orch.record_report(AgentReport {
                task_id,
                status: TaskStatus::Succeeded,
                summary: summary.to_string(),
                artifacts: vec![],
                followups: vec![],
                satisfied_criteria: vec![],
                usage: None,
                verdict: None,
            });
        }
        match orch.next() {
            OrchestratorDecision::PromptAgent { task } => {
                assert_eq!(task.agent_id, "verifier");
                task.id
            }
            other => panic!("expected verifier prompt, got {:?}", other),
        }
    }

    fn verifier_report(task_id: &str, verdict: Option<InspectionVerdict>) -> AgentReport {
        AgentReport {
            task_id: task_id.to_string(),
            status: TaskStatus::Succeeded,
            summary: "verification complete".to_string(),
            artifacts: vec!["verdict.json".to_string()],
            followups: vec![],
            satisfied_criteria: vec![],
            usage: None,
            verdict,
        }
    }

    /// Regression: the soft-review failure mode. A verification that claims
    /// success without a structured verdict rubber-stamps the work and teaches
    /// the conductor to trust it; record it as the failure it is.
    #[test]
    fn verifier_success_without_verdict_is_failure() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = prompt_verifier(&mut orch);

        orch.record_report(verifier_report(&task_id, None));

        let report = orch
            .state()
            .reports
            .iter()
            .find(|r| r.task_id == task_id)
            .expect("report recorded");
        assert_eq!(report.status, TaskStatus::Failed);
        assert!(
            report.summary.contains("no verdict"),
            "the summary must say why: {}",
            report.summary
        );
        assert!(orch.state().verdict.is_none());
    }

    /// A pass verdict that doesn't cover every success criterion is invalid.
    #[test]
    fn pass_verdict_missing_a_criterion_is_failure() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = prompt_verifier(&mut orch);

        let mut verdict = passing_verdict_for_directive(&directive());
        verdict.criteria.pop(); // drop the last criterion's assessment

        orch.record_report(verifier_report(&task_id, Some(verdict)));

        let report = orch
            .state()
            .reports
            .iter()
            .find(|r| r.task_id == task_id)
            .expect("report recorded");
        assert_eq!(report.status, TaskStatus::Failed);
        assert!(
            report.summary.contains("does not assess criterion"),
            "the summary must say why: {}",
            report.summary
        );
    }

    /// Evidence-gated grading: a met criterion with empty evidence counts as unmet.
    #[test]
    fn pass_verdict_with_empty_evidence_is_failure() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = prompt_verifier(&mut orch);

        let mut verdict = passing_verdict_for_directive(&directive());
        verdict.criteria[0].evidence = "  ".to_string();

        orch.record_report(verifier_report(&task_id, Some(verdict)));

        let report = orch
            .state()
            .reports
            .iter()
            .find(|r| r.task_id == task_id)
            .expect("report recorded");
        assert_eq!(report.status, TaskStatus::Failed);
        assert!(
            report.summary.contains("without evidence"),
            "the summary must say why: {}",
            report.summary
        );
    }

    /// A valid pass verdict is recorded on the run state and the task succeeds.
    #[test]
    fn valid_pass_verdict_is_recorded_on_state() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = prompt_verifier(&mut orch);

        orch.record_report(verifier_report(
            &task_id,
            Some(passing_verdict_for_directive(&directive())),
        ));

        let report = orch
            .state()
            .reports
            .iter()
            .find(|r| r.task_id == task_id)
            .expect("report recorded");
        assert_eq!(report.status, TaskStatus::Succeeded);
        assert_eq!(
            orch.state().verdict.as_ref().map(|v| v.verdict),
            Some(Verdict::Pass)
        );
    }

    /// fail-fixable means the inspection happened and the work fell short; the
    /// verification task itself succeeded. It must carry a retry_task.
    #[test]
    fn fail_fixable_verdict_succeeds_and_is_recorded() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = prompt_verifier(&mut orch);

        let verdict = InspectionVerdict {
            verdict: Verdict::FailFixable,
            criteria: vec![],
            diagnosis: Some("builder skipped error handling in parser".to_string()),
            route_hint: Some(VerdictRouteHint {
                retry_task: Some("add error handling to parser::parse".to_string()),
                respec: None,
            }),
            confidence: Some(VerdictConfidence::High),
        };
        orch.record_report(verifier_report(&task_id, Some(verdict)));

        let report = orch
            .state()
            .reports
            .iter()
            .find(|r| r.task_id == task_id)
            .expect("report recorded");
        assert_eq!(report.status, TaskStatus::Succeeded);
        assert_eq!(
            orch.state().verdict.as_ref().map(|v| v.verdict),
            Some(Verdict::FailFixable)
        );
    }

    #[test]
    fn fail_fixable_without_retry_task_is_failure() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = prompt_verifier(&mut orch);

        let verdict = InspectionVerdict {
            verdict: Verdict::FailFixable,
            criteria: vec![],
            diagnosis: Some("something is wrong".to_string()),
            route_hint: None,
            confidence: None,
        };
        orch.record_report(verifier_report(&task_id, Some(verdict)));

        let report = orch
            .state()
            .reports
            .iter()
            .find(|r| r.task_id == task_id)
            .expect("report recorded");
        assert_eq!(report.status, TaskStatus::Failed);
        assert!(
            report.summary.contains("retry_task"),
            "the summary must say why: {}",
            report.summary
        );
    }

    #[test]
    fn fail_escalate_without_respec_is_failure() {
        let mut orch = Orchestrator::new(directive(), default_agent_roles());
        let task_id = prompt_verifier(&mut orch);

        let verdict = InspectionVerdict {
            verdict: Verdict::FailEscalate,
            criteria: vec![],
            diagnosis: Some("task was impossible as specced".to_string()),
            route_hint: None,
            confidence: None,
        };
        orch.record_report(verifier_report(&task_id, Some(verdict)));

        let report = orch
            .state()
            .reports
            .iter()
            .find(|r| r.task_id == task_id)
            .expect("report recorded");
        assert_eq!(report.status, TaskStatus::Failed);
        assert!(
            report.summary.contains("respec"),
            "the summary must say why: {}",
            report.summary
        );
    }

    /// The verifier must get the inspector template (cold context, verbatim
    /// intent, verdict contract); other roles keep the generic template.
    #[test]
    fn verifier_gets_inspector_prompt() {
        let orch = Orchestrator::new(directive(), default_agent_roles());
        let verifier_prompt = &orch.state().tasks[2].prompt;
        assert!(verifier_prompt.contains("You are the Inspector"));
        assert!(verifier_prompt.contains("verdict.json"));
        assert!(verifier_prompt.contains("fail-escalate"));
        // Intent injected verbatim.
        assert!(verifier_prompt.contains("Ship a lightweight Exo agent workflow"));
        assert!(verifier_prompt.contains("planning complete"));
        assert!(verifier_prompt.contains("keep daemon light"));

        let planner_prompt = &orch.state().tasks[0].prompt;
        assert!(!planner_prompt.contains("You are the Inspector"));
        assert!(planner_prompt.contains("Prime directive"));
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
            usage: None,
            verdict: None,
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
