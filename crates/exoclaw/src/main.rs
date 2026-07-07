//! # exoClaw CLI - Secure Local Agent Harness

use anyhow::Context;
use clap::{Parser, Subcommand};
use exoclaw::{
    default_agent_roles, into_provider, new_run_id, run_to_completion_with_observer, Agent,
    AgentReport, AgentTask, BuiltinExecutor, CommandAgentExecutor, DateTimeTool, ExoAgentExecutor,
    LlmConfig, MailboxEvent, OpenAiCompatibleProvider, OrchestrationState, Orchestrator,
    OrchestratorDecision, PrimeDirective, RunObserver, RunOutcome, RunRecord, RunStore,
    ToolRegistry,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "exoclaw")]
#[command(about = "Secure local AI agent harness", long_about = None)]
#[command(version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show status
    Status,

    /// Show available tools
    Tools {
        /// Show detailed tool information
        #[arg(long)]
        verbose: bool,
    },

    /// Run a task
    Run {
        /// The task to run
        task: String,
    },

    /// Start the web gateway
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value_t = 3847)]
        port: u16,
    },

    /// Create a lightweight multi-agent orchestration plan
    Orchestrate {
        /// Prime directive/objective
        directive: String,

        /// Success criterion (repeatable)
        #[arg(short = 's', long = "success")]
        success: Vec<String>,

        /// Constraint (repeatable)
        #[arg(short, long = "constraint")]
        constraint: Vec<String>,

        /// Print JSON state
        #[arg(long)]
        json: bool,
    },

    /// Run a prime directive through the lightweight multi-agent coordinator
    OrchestrateRun {
        /// Prime directive/objective
        directive: Option<String>,

        /// Success criterion (repeatable)
        #[arg(short = 's', long = "success")]
        success: Vec<String>,

        /// Constraint (repeatable)
        #[arg(short, long = "constraint")]
        constraint: Vec<String>,

        /// Command used for every agent. Receives AgentPrompt JSON on stdin and
        /// should print AgentReport JSON on stdout.
        #[arg(long)]
        agent_cmd: Option<String>,

        /// Use `exo run` as the agent executor
        #[arg(long)]
        use_exo: bool,

        /// Exo binary path for --use-exo
        #[arg(long, default_value = "exo")]
        exo_bin: String,

        /// Exo backend for --use-exo
        #[arg(long, default_value = "native")]
        exo_backend: String,

        /// Exo image for --use-exo
        #[arg(long, default_value = "host")]
        exo_image: String,

        /// Agent command for --use-exo
        #[arg(long, default_value = "cat")]
        exo_agent_cmd: String,

        /// Volume mount for --use-exo (SRC:DEST), repeatable
        #[arg(long = "volume")]
        volumes: Vec<String>,

        /// Secret to pass to exo run for --use-exo, repeatable
        #[arg(long = "secret")]
        secrets: Vec<String>,

        /// Sandbox mode for --use-exo (auto/off/required)
        #[arg(long)]
        sandbox: Option<String>,

        /// Output final orchestration state as JSON
        #[arg(long)]
        json: bool,

        /// Read stable JSON input from a file
        #[arg(long)]
        json_input: Option<std::path::PathBuf>,

        /// Persist orchestration state under this directory
        #[arg(long)]
        state_dir: Option<std::path::PathBuf>,

        /// Explicit run id; generated if omitted
        #[arg(long)]
        run_id: Option<String>,

        /// Maximum coordinator rounds/prompts before blocking
        #[arg(long)]
        max_rounds: Option<u32>,
    },

    /// Resume a previously persisted orchestration run
    OrchestrateResume {
        /// Run id
        run_id: String,

        /// State directory containing run directories
        #[arg(long)]
        state_dir: Option<std::path::PathBuf>,

        /// Command used for every agent. Receives AgentPrompt JSON on stdin and
        /// should print AgentReport JSON on stdout.
        #[arg(long)]
        agent_cmd: Option<String>,

        /// Use `exo run` as the agent executor
        #[arg(long)]
        use_exo: bool,

        /// Exo binary path for --use-exo
        #[arg(long, default_value = "exo")]
        exo_bin: String,

        /// Exo backend for --use-exo
        #[arg(long, default_value = "native")]
        exo_backend: String,

        /// Exo image for --use-exo
        #[arg(long, default_value = "host")]
        exo_image: String,

        /// Agent command for --use-exo
        #[arg(long, default_value = "cat")]
        exo_agent_cmd: String,

        /// Volume mount for --use-exo (SRC:DEST), repeatable
        #[arg(long = "volume")]
        volumes: Vec<String>,

        /// Secret to pass to exo run for --use-exo, repeatable
        #[arg(long = "secret")]
        secrets: Vec<String>,

        /// Sandbox mode for --use-exo (auto/off/required)
        #[arg(long)]
        sandbox: Option<String>,

        /// Output final orchestration state as JSON
        #[arg(long)]
        json: bool,

        /// Maximum coordinator rounds/prompts before blocking
        #[arg(long)]
        max_rounds: Option<u32>,
    },

    /// List persisted orchestration runs
    OrchestrateList {
        /// State directory containing run directories
        #[arg(long)]
        state_dir: Option<std::path::PathBuf>,

        /// Print run summaries as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show one persisted orchestration run
    OrchestrateStatus {
        /// Run id
        run_id: String,

        /// State directory containing run directories
        #[arg(long)]
        state_dir: Option<std::path::PathBuf>,

        /// Include mailbox events in JSON output
        #[arg(long)]
        include_mailbox: bool,

        /// Include lifecycle events in JSON output
        #[arg(long)]
        include_events: bool,

        /// Print status as JSON
        #[arg(long)]
        json: bool,
    },

    /// Append/read the durable run mailbox event log
    EventLog {
        #[command(subcommand)]
        command: EventLogCommands,
    },
}

#[derive(Subcommand)]
enum EventLogCommands {
    /// Append one mailbox event to a run
    Append {
        /// Run id
        #[arg(long)]
        run_id: String,

        /// State directory containing run directories
        #[arg(long)]
        state_dir: Option<std::path::PathBuf>,

        /// Event kind, e.g. message, checkpoint, sleep, wake, handoff
        #[arg(long, default_value = "message")]
        kind: String,

        /// Sender agent/id
        #[arg(long)]
        from_agent: Option<String>,

        /// Recipient agent/id
        #[arg(long)]
        to_agent: Option<String>,

        /// Related task id
        #[arg(long)]
        task_id: Option<String>,

        /// Human-readable event message
        message: String,

        /// Optional JSON payload object/string
        #[arg(long)]
        payload_json: Option<String>,

        /// Print appended event as JSON
        #[arg(long)]
        json: bool,
    },

    /// List mailbox events for a run
    List {
        /// Run id
        #[arg(long)]
        run_id: String,

        /// State directory containing run directories
        #[arg(long)]
        state_dir: Option<std::path::PathBuf>,

        /// Only events with sequence greater than this value
        #[arg(long, default_value_t = 0)]
        since: u64,

        /// Filter events sent from or to this agent/id
        #[arg(long)]
        agent: Option<String>,

        /// Print events as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OrchestrateRunInput {
    pub objective: String,
    #[serde(default)]
    pub success_criteria: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub executor: ExecutorConfig,
    pub run_id: Option<String>,
    #[serde(default = "default_orchestrate_run_max_rounds")]
    pub max_rounds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ExecutorConfig {
    Builtin,
    Command {
        command: String,
    },
    Exo {
        #[serde(default = "default_exo_bin")]
        exo_bin: String,
        #[serde(default = "default_exo_backend")]
        backend: String,
        #[serde(default = "default_exo_image")]
        image: String,
        #[serde(default = "default_exo_agent_cmd")]
        agent_command: String,
        #[serde(default)]
        volumes: Vec<String>,
        #[serde(default)]
        secrets: Vec<String>,
        sandbox: Option<String>,
    },
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        ExecutorConfig::Builtin
    }
}

fn default_exo_bin() -> String {
    "exo".to_string()
}
fn default_exo_backend() -> String {
    "native".to_string()
}
fn default_exo_image() -> String {
    "host".to_string()
}
fn default_exo_agent_cmd() -> String {
    "cat".to_string()
}

fn default_orchestrate_run_max_rounds() -> u32 {
    24
}

#[derive(Debug, Serialize)]
struct OrchestrateRunOutput<'a> {
    pub run_id: &'a str,
    pub outcome: &'a exoclaw::RunOutcome,
    pub state_path: String,
    pub events_path: String,
    pub mailbox_path: String,
    pub state: &'a exoclaw::OrchestrationState,
}

#[derive(Debug, Serialize)]
struct RunSummary {
    pub run_id: String,
    pub status: exoclaw::OrchestrationStatus,
    pub outcome: Option<RunOutcome>,
    pub objective: String,
    pub round: u32,
    pub max_rounds: u32,
    pub task_count: usize,
    pub report_count: usize,
    pub state_path: String,
    pub events_path: String,
    pub mailbox_path: String,
}

#[derive(Debug, Serialize)]
struct RunStatusOutput {
    pub summary: RunSummary,
    pub record: RunRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<exoclaw::RunEvent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mailbox: Option<Vec<MailboxEvent>>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Status => {
            show_status();
            Ok(())
        }
        Commands::Tools { verbose } => {
            list_tools(verbose);
            Ok(())
        }
        Commands::Run { task } => run_task(task),
        Commands::Serve { port } => start_gateway(port),
        Commands::Orchestrate {
            directive,
            success,
            constraint,
            json,
        } => orchestrate(directive, success, constraint, json),
        Commands::OrchestrateRun {
            directive,
            success,
            constraint,
            agent_cmd,
            use_exo,
            exo_bin,
            exo_backend,
            exo_image,
            exo_agent_cmd,
            volumes,
            secrets,
            sandbox,
            json,
            json_input,
            state_dir,
            run_id,
            max_rounds,
        } => orchestrate_run(OrchestrateRunArgs {
            objective: directive,
            success_criteria: success,
            constraints: constraint,
            agent_cmd,
            use_exo,
            exo_bin,
            exo_backend,
            exo_image,
            exo_agent_cmd,
            volumes,
            secrets,
            sandbox,
            json,
            json_input,
            state_dir,
            run_id,
            max_rounds,
        }),
        Commands::OrchestrateResume {
            run_id,
            state_dir,
            agent_cmd,
            use_exo,
            exo_bin,
            exo_backend,
            exo_image,
            exo_agent_cmd,
            volumes,
            secrets,
            sandbox,
            json,
            max_rounds,
        } => orchestrate_resume(OrchestrateResumeArgs {
            run_id,
            state_dir,
            agent_cmd,
            use_exo,
            exo_bin,
            exo_backend,
            exo_image,
            exo_agent_cmd,
            volumes,
            secrets,
            sandbox,
            json,
            max_rounds,
        }),
        Commands::OrchestrateList { state_dir, json } => orchestrate_list(state_dir, json),
        Commands::OrchestrateStatus {
            run_id,
            state_dir,
            include_mailbox,
            include_events,
            json,
        } => orchestrate_status(run_id, state_dir, include_mailbox, include_events, json),
        Commands::EventLog { command } => event_log(command),
    }
}

fn show_status() {
    println!("exoClaw v{}", exoclaw::VERSION);
    println!("Architecture: Local-first");
    println!();
    println!("Components:");
    println!("  [OK] Core harness");
    println!("  [OK] Tool registry");
    println!("  [OK] LLM integration (LM Studio, Ollama, Claude)");
    println!("  [OK] Agent runtime");
    println!("  [OK] Web UI");
    println!();
    println!("Security:");
    println!("  - Local-only execution (no network exposure)");
    println!("  - Permission dialogs for sensitive operations");
    println!("  - Audit logging");
}

fn list_tools(verbose: bool) {
    println!("Available Tools:");
    println!();

    let tools = vec![(
        "get_datetime",
        "Get current date and time",
        "None - Always available",
    )];

    for (name, description, permission) in tools {
        if verbose {
            println!("  {} - {}", name, description);
            println!("    Permission: {}", permission);
        } else {
            println!("  {} - {}", name, description);
        }
    }
}

fn run_task(task: String) -> anyhow::Result<()> {
    println!("Running task: {}", task);
    println!();

    // Create tool registry
    let registry = ToolRegistry::new();
    registry.register(Box::new(DateTimeTool));

    // Check if LM Studio is available
    let llm_config = LlmConfig::lm_studio("llama-3.2-3b-instruct".to_string());
    let llm = OpenAiCompatibleProvider::new(llm_config);
    let llm: Arc<dyn exoclaw::LlmProvider> = into_provider(llm);

    let agent = Agent::new("exoClaw".to_string(), llm, Arc::new(registry));

    match agent.run_task_sync(task) {
        Ok(result) => {
            println!();
            println!("Result:");
            println!("{}", result.output);
            println!();
            println!("Completed in {}ms", result.duration_ms);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!();
            eprintln!("Make sure LM Studio is running with:");
            eprintln!("  1. Open LM Studio");
            eprintln!("  2. Load a model (e.g., Llama 3.2 3B)");
            eprintln!("  3. Enable the server");
            eprintln!("  4. The server should be on localhost:1234");
        }
    }

    Ok(())
}

fn start_gateway(port: u16) -> anyhow::Result<()> {
    println!("Starting web gateway on port {}...", port);
    println!("Press Ctrl+C to stop");

    // For now, just print status
    println!();
    println!(
        "exoClaw web UI would be available at http://localhost:{}",
        port
    );
    println!();

    // TODO: Start actual web server when async runtime is fully set up

    Ok(())
}

fn orchestrate(
    objective: String,
    success_criteria: Vec<String>,
    constraints: Vec<String>,
    json: bool,
) -> anyhow::Result<()> {
    let directive = PrimeDirective {
        objective,
        success_criteria,
        constraints,
        max_rounds: 12,
    };
    let mut orchestrator = Orchestrator::new(directive, default_agent_roles());

    if json {
        println!("{}", serde_json::to_string_pretty(orchestrator.state())?);
        return Ok(());
    }

    println!("Prime directive:");
    println!("{}", orchestrator.state().directive.objective);
    println!();
    println!("Initial agent prompts:");
    loop {
        match orchestrator.next() {
            OrchestratorDecision::PromptAgent { task } => {
                println!("--- {} -> {} ---", task.id, task.agent_id);
                println!("{}", task.prompt);
                println!();
            }
            OrchestratorDecision::Succeeded { summary } => {
                println!("Succeeded: {}", summary);
                break;
            }
            OrchestratorDecision::Blocked { reason } => {
                // Dry-run stops once all initial prompts are emitted.
                println!("Coordinator waiting for agent reports: {}", reason);
                break;
            }
        }
    }
    Ok(())
}

struct OrchestrateRunArgs {
    objective: Option<String>,
    success_criteria: Vec<String>,
    constraints: Vec<String>,
    agent_cmd: Option<String>,
    use_exo: bool,
    exo_bin: String,
    exo_backend: String,
    exo_image: String,
    exo_agent_cmd: String,
    volumes: Vec<String>,
    secrets: Vec<String>,
    sandbox: Option<String>,
    json: bool,
    json_input: Option<std::path::PathBuf>,
    state_dir: Option<std::path::PathBuf>,
    run_id: Option<String>,
    max_rounds: Option<u32>,
}

struct OrchestrateResumeArgs {
    run_id: String,
    state_dir: Option<std::path::PathBuf>,
    agent_cmd: Option<String>,
    use_exo: bool,
    exo_bin: String,
    exo_backend: String,
    exo_image: String,
    exo_agent_cmd: String,
    volumes: Vec<String>,
    secrets: Vec<String>,
    sandbox: Option<String>,
    json: bool,
    max_rounds: Option<u32>,
}

struct StoreRunObserver {
    store: RunStore,
    run_id: String,
}

impl StoreRunObserver {
    fn new(store: RunStore, run_id: String) -> Self {
        Self { store, run_id }
    }
}

impl RunObserver for StoreRunObserver {
    fn on_task_prompted(&mut self, task: &AgentTask) -> anyhow::Result<()> {
        self.store.append_mailbox_event(
            MailboxEvent::new(
                &self.run_id,
                "task_prompted",
                format!("coordinator prompted {}", task.agent_id),
            )
            .from("coordinator")
            .to(&task.agent_id)
            .task_id(&task.id)
            .payload(serde_json::json!({
                "prompt": &task.prompt,
                "attempts": task.attempts,
                "depends_on": &task.depends_on,
            })),
        )?;
        Ok(())
    }

    fn on_agent_report(
        &mut self,
        task: &AgentTask,
        report: &AgentReport,
        state: &OrchestrationState,
    ) -> anyhow::Result<()> {
        self.store.append_mailbox_event(
            MailboxEvent::new(
                &self.run_id,
                "agent_report",
                format!("{} reported {:?}", task.agent_id, report.status),
            )
            .from(&task.agent_id)
            .to("coordinator")
            .task_id(&task.id)
            .payload(serde_json::json!({
                "status": report.status,
                "summary": &report.summary,
                "artifacts": &report.artifacts,
                "followups": &report.followups,
            })),
        )?;

        for followup in &report.followups {
            self.store.append_mailbox_event(
                MailboxEvent::new(&self.run_id, "handoff_requested", followup)
                    .from(&task.agent_id)
                    .to("coordinator")
                    .task_id(&task.id)
                    .payload(serde_json::json!({ "followup": followup })),
            )?;
        }

        self.store.save(&RunRecord {
            run_id: self.run_id.clone(),
            state: state.clone(),
            outcome: None,
        })?;
        Ok(())
    }

    fn on_run_finished(&mut self, outcome: &RunOutcome) -> anyhow::Result<()> {
        self.store.append_mailbox_event(
            MailboxEvent::new(
                &self.run_id,
                "run_finished",
                format!("{:?}: {}", outcome.status, outcome.message),
            )
            .from("coordinator")
            .payload(serde_json::json!({
                "status": outcome.status,
                "rounds": outcome.rounds,
                "message": &outcome.message,
            })),
        )?;
        Ok(())
    }
}

fn orchestrate_run(args: OrchestrateRunArgs) -> anyhow::Result<()> {
    let input = if let Some(path) = args.json_input {
        let bytes = std::fs::read(&path)?;
        serde_json::from_slice::<OrchestrateRunInput>(&bytes)?
    } else {
        OrchestrateRunInput {
            objective: args.objective.ok_or_else(|| {
                anyhow::anyhow!("directive is required unless --json-input is used")
            })?,
            success_criteria: args.success_criteria,
            constraints: args.constraints,
            executor: if args.use_exo {
                ExecutorConfig::Exo {
                    exo_bin: args.exo_bin,
                    backend: args.exo_backend,
                    image: args.exo_image,
                    agent_command: args.exo_agent_cmd,
                    volumes: args.volumes,
                    secrets: args.secrets,
                    sandbox: args.sandbox,
                }
            } else if let Some(cmd) = args.agent_cmd {
                ExecutorConfig::Command { command: cmd }
            } else {
                ExecutorConfig::Builtin
            },
            run_id: args.run_id,
            max_rounds: args
                .max_rounds
                .unwrap_or_else(default_orchestrate_run_max_rounds),
        }
    };

    let run_id = input.run_id.clone().unwrap_or_else(new_run_id);
    let max_rounds = args.max_rounds.unwrap_or(input.max_rounds);
    let resolved_input = OrchestrateRunInput {
        run_id: Some(run_id.clone()),
        max_rounds,
        ..input.clone()
    };
    let directive = PrimeDirective {
        objective: input.objective.clone(),
        success_criteria: input.success_criteria.clone(),
        constraints: input.constraints.clone(),
        max_rounds,
    };
    let mut orchestrator = Orchestrator::new(directive, default_agent_roles());
    let store = if let Some(dir) = args.state_dir {
        RunStore::new(dir)?
    } else {
        RunStore::new_default()?
    };

    store.save_input(&run_id, &resolved_input)?;
    store.save(&RunRecord {
        run_id: run_id.clone(),
        state: orchestrator.state().clone(),
        outcome: None,
    })?;
    store.append_event(&run_id, "started", "orchestration run started")?;
    store.append_mailbox_event(
        MailboxEvent::new(&run_id, "run_started", "orchestration run started")
            .from("coordinator")
            .payload(serde_json::json!({
                "objective": &orchestrator.state().directive.objective,
                "success_criteria": &orchestrator.state().directive.success_criteria,
                "constraints": &orchestrator.state().directive.constraints,
            })),
    )?;
    let mut observer = StoreRunObserver::new(store.clone(), run_id.clone());

    let outcome = run_with_executor_config_value(&mut orchestrator, input.executor, &mut observer)?;

    store.save(&RunRecord {
        run_id: run_id.clone(),
        state: orchestrator.state().clone(),
        outcome: Some(outcome.clone()),
    })?;
    store.append_event(&run_id, "finished", &format!("{:?}", outcome.status))?;

    if args.json {
        let output = OrchestrateRunOutput {
            run_id: &run_id,
            outcome: &outcome,
            state_path: store.state_path(&run_id).display().to_string(),
            events_path: store.events_path(&run_id).display().to_string(),
            mailbox_path: store.mailbox_path(&run_id).display().to_string(),
            state: orchestrator.state(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("Run ID: {}", run_id);
    println!("State: {}", store.state_path(&run_id).display());
    println!("Events: {}", store.events_path(&run_id).display());
    println!("Mailbox: {}", store.mailbox_path(&run_id).display());
    println!("Outcome: {:?}", outcome.status);
    println!("Rounds: {}", outcome.rounds);
    println!("{}", outcome.message);
    println!();
    println!("Reports:");
    for report in &orchestrator.state().reports {
        println!(
            "- {} [{:?}]: {}",
            report.task_id, report.status, report.summary
        );
    }
    Ok(())
}

fn orchestrate_resume(args: OrchestrateResumeArgs) -> anyhow::Result<()> {
    let store = open_run_store(args.state_dir.clone())?;
    let record = store.load(&args.run_id)?;
    let previous_status = record.state.status;
    let mut state = record.state;
    if let Some(max_rounds) = args.max_rounds {
        state.directive.max_rounds = max_rounds;
    }
    let mut orchestrator = Orchestrator::from_state(state);
    let executor_config = resume_executor_config(&args, &store)?;

    store.append_event(&args.run_id, "resumed", "orchestration run resumed")?;
    store.append_mailbox_event(
        MailboxEvent::new(&args.run_id, "run_resumed", "orchestration run resumed")
            .from("coordinator")
            .payload(serde_json::json!({
                "previous_status": previous_status,
                "round": orchestrator.state().round,
            })),
    )?;

    store.save(&RunRecord {
        run_id: args.run_id.clone(),
        state: orchestrator.state().clone(),
        outcome: None,
    })?;

    let mut observer = StoreRunObserver::new(store.clone(), args.run_id.clone());
    let outcome =
        run_with_executor_config_value(&mut orchestrator, executor_config, &mut observer)?;

    store.save(&RunRecord {
        run_id: args.run_id.clone(),
        state: orchestrator.state().clone(),
        outcome: Some(outcome.clone()),
    })?;
    store.append_event(&args.run_id, "finished", &format!("{:?}", outcome.status))?;

    if args.json {
        let output = OrchestrateRunOutput {
            run_id: &args.run_id,
            outcome: &outcome,
            state_path: store.state_path(&args.run_id).display().to_string(),
            events_path: store.events_path(&args.run_id).display().to_string(),
            mailbox_path: store.mailbox_path(&args.run_id).display().to_string(),
            state: orchestrator.state(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("Run ID: {}", args.run_id);
    println!("State: {}", store.state_path(&args.run_id).display());
    println!("Events: {}", store.events_path(&args.run_id).display());
    println!("Mailbox: {}", store.mailbox_path(&args.run_id).display());
    println!("Outcome: {:?}", outcome.status);
    println!("Rounds: {}", outcome.rounds);
    println!("{}", outcome.message);
    Ok(())
}

fn orchestrate_list(state_dir: Option<std::path::PathBuf>, json: bool) -> anyhow::Result<()> {
    let store = open_run_store(state_dir)?;
    let mut summaries = vec![];
    for run_id in store.list_run_ids()? {
        match store.load(&run_id) {
            Ok(record) => summaries.push(run_summary(&store, &record)),
            Err(e) => eprintln!("warning: failed to load run {}: {}", run_id, e),
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&summaries)?);
    } else {
        for summary in summaries {
            println!(
                "{} [{:?}] round {}/{} tasks={} reports={} - {}",
                summary.run_id,
                summary.status,
                summary.round,
                summary.max_rounds,
                summary.task_count,
                summary.report_count,
                summary.objective
            );
        }
    }
    Ok(())
}

fn orchestrate_status(
    run_id: String,
    state_dir: Option<std::path::PathBuf>,
    include_mailbox: bool,
    include_events: bool,
    json: bool,
) -> anyhow::Result<()> {
    let store = open_run_store(state_dir)?;
    let record = store.load(&run_id)?;
    let output = RunStatusOutput {
        summary: run_summary(&store, &record),
        record,
        events: if include_events {
            Some(store.read_events(&run_id)?)
        } else {
            None
        },
        mailbox: if include_mailbox {
            Some(store.read_mailbox(&run_id)?)
        } else {
            None
        },
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Run ID: {}", output.summary.run_id);
        println!("Status: {:?}", output.summary.status);
        println!("Outcome: {:?}", output.summary.outcome);
        println!(
            "Rounds: {}/{}",
            output.summary.round, output.summary.max_rounds
        );
        println!("Tasks: {}", output.summary.task_count);
        println!("Reports: {}", output.summary.report_count);
        println!("Objective: {}", output.summary.objective);
        println!("State: {}", output.summary.state_path);
        println!("Events: {}", output.summary.events_path);
        println!("Mailbox: {}", output.summary.mailbox_path);
    }
    Ok(())
}

fn run_summary(store: &RunStore, record: &RunRecord) -> RunSummary {
    RunSummary {
        run_id: record.run_id.clone(),
        status: record.state.status,
        outcome: record.outcome.clone(),
        objective: record.state.directive.objective.clone(),
        round: record.state.round,
        max_rounds: record.state.directive.max_rounds,
        task_count: record.state.tasks.len(),
        report_count: record.state.reports.len(),
        state_path: store.state_path(&record.run_id).display().to_string(),
        events_path: store.events_path(&record.run_id).display().to_string(),
        mailbox_path: store.mailbox_path(&record.run_id).display().to_string(),
    }
}

fn resume_executor_config(
    args: &OrchestrateResumeArgs,
    store: &RunStore,
) -> anyhow::Result<ExecutorConfig> {
    if args.use_exo {
        return Ok(ExecutorConfig::Exo {
            exo_bin: args.exo_bin.clone(),
            backend: args.exo_backend.clone(),
            image: args.exo_image.clone(),
            agent_command: args.exo_agent_cmd.clone(),
            volumes: args.volumes.clone(),
            secrets: args.secrets.clone(),
            sandbox: args.sandbox.clone(),
        });
    }
    if let Some(command) = &args.agent_cmd {
        return Ok(ExecutorConfig::Command {
            command: command.clone(),
        });
    }

    match store.load_input::<OrchestrateRunInput>(&args.run_id) {
        Ok(input) => Ok(input.executor),
        Err(_) => Ok(ExecutorConfig::Builtin),
    }
}

fn run_with_executor_config_value(
    orchestrator: &mut Orchestrator,
    config: ExecutorConfig,
    observer: &mut dyn RunObserver,
) -> anyhow::Result<RunOutcome> {
    match config {
        ExecutorConfig::Exo {
            exo_bin,
            backend,
            image,
            agent_command,
            volumes,
            secrets,
            sandbox,
        } => {
            let mut executor =
                ExoAgentExecutor::new(vec!["sh".to_string(), "-c".to_string(), agent_command]);
            executor.exo_bin = exo_bin;
            executor.backend = backend;
            executor.image = image;
            executor.sandbox = sandbox;
            executor.secrets = secrets;
            executor.volumes = parse_volume_pairs(volumes)?;
            run_to_completion_with_observer(orchestrator, &mut executor, 100, observer)
        }
        ExecutorConfig::Command { command } => {
            let mut executor = CommandAgentExecutor::new(command);
            run_to_completion_with_observer(orchestrator, &mut executor, 100, observer)
        }
        ExecutorConfig::Builtin => {
            let mut executor = BuiltinExecutor::new();
            run_to_completion_with_observer(orchestrator, &mut executor, 100, observer)
        }
    }
}

fn parse_volume_pairs(values: Vec<String>) -> anyhow::Result<Vec<(String, String)>> {
    values
        .into_iter()
        .map(|value| {
            let Some((source, target)) = value.split_once(':') else {
                anyhow::bail!("invalid volume '{}'; expected SRC:DEST", value);
            };
            Ok((source.to_string(), target.to_string()))
        })
        .collect()
}

fn event_log(command: EventLogCommands) -> anyhow::Result<()> {
    match command {
        EventLogCommands::Append {
            run_id,
            state_dir,
            kind,
            from_agent,
            to_agent,
            task_id,
            message,
            payload_json,
            json,
        } => {
            let store = open_run_store(state_dir)?;
            let mut event = MailboxEvent::new(&run_id, kind, message);
            if let Some(from) = from_agent {
                event = event.from(from);
            }
            if let Some(to) = to_agent {
                event = event.to(to);
            }
            if let Some(task_id) = task_id {
                event = event.task_id(task_id);
            }
            if let Some(payload_json) = payload_json {
                event = event.payload(parse_payload_json(&payload_json)?);
            }
            let event = store.append_mailbox_event(event)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&event)?);
            } else {
                println!(
                    "#{} {} {} -> {}: {}",
                    event.sequence,
                    event.kind,
                    event.from.as_deref().unwrap_or("-"),
                    event.to.as_deref().unwrap_or("-"),
                    event.message
                );
            }
            Ok(())
        }
        EventLogCommands::List {
            run_id,
            state_dir,
            since,
            agent,
            json,
        } => {
            let store = open_run_store(state_dir)?;
            let mut events = store.read_mailbox_since(&run_id, since)?;
            if let Some(agent) = agent {
                events.retain(|event| {
                    event.from.as_deref() == Some(agent.as_str())
                        || event.to.as_deref() == Some(agent.as_str())
                });
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&events)?);
            } else {
                for event in events {
                    let task = event
                        .task_id
                        .as_deref()
                        .map(|task_id| format!(" ({})", task_id))
                        .unwrap_or_default();
                    println!(
                        "#{} [{}] {} -> {}{}: {}",
                        event.sequence,
                        event.kind,
                        event.from.as_deref().unwrap_or("-"),
                        event.to.as_deref().unwrap_or("-"),
                        task,
                        event.message
                    );
                }
            }
            Ok(())
        }
    }
}

fn open_run_store(state_dir: Option<std::path::PathBuf>) -> anyhow::Result<RunStore> {
    if let Some(dir) = state_dir {
        RunStore::new(dir)
    } else {
        RunStore::new_default()
    }
}

fn parse_payload_json(input: &str) -> anyhow::Result<Value> {
    serde_json::from_str(input).with_context(|| format!("invalid --payload-json: {}", input))
}
