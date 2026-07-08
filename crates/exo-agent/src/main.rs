//! Exo Agent - Lightweight agent runtime
//!
//! A minimal, fast agent runtime designed to run inside exo containers.
//! Provides LLM integration, memory, tools, and stdio communication.

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::io::Read;
use tracing::info;

mod agent;
mod channel;
mod config;
mod llm;
mod memory;
mod tools;

use agent::ExoAgent;
use config::AgentConfig;

#[derive(Parser)]
#[command(name = "exo-agent", author, version, about = "Exo Agent")]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,

    /// Agent name
    #[arg(short, long)]
    name: Option<String>,

    /// LLM model
    #[arg(short, long)]
    model: Option<String>,

    /// System prompt
    #[arg(short = 'S', long)]
    system: Option<String>,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Run one orchestration prompt: AgentPrompt JSON stdin -> AgentReport JSON stdout
    RunOnce {
        /// Accept AgentPrompt JSON on stdin (default; kept explicit for scripts)
        #[arg(long)]
        stdin_json: bool,

        /// Emit AgentReport JSON on stdout (default; kept explicit for scripts)
        #[arg(long)]
        stdout_json: bool,

        /// Do not call an LLM; emit a deterministic succeeded report
        #[arg(long)]
        mock: bool,
    },
}

/// Orchestration prompt from exoclaw.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentPrompt {
    pub task_id: String,
    pub agent_id: String,
    pub prompt: String,
}

/// Orchestration report expected by exoclaw.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentReport {
    pub task_id: String,
    pub status: String,
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub followups: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    info!("Exo Agent v{} starting", env!("CARGO_PKG_VERSION"));

    // Parse arguments
    let args = Args::parse();

    if let Some(Commands::RunOnce {
        stdin_json: _,
        stdout_json: _,
        mock,
    }) = args.command
    {
        return run_once(args, mock).await;
    }

    // Load configuration
    let mut config = if let Some(path) = &args.config {
        AgentConfig::from_file(std::path::Path::new(path))?
    } else {
        AgentConfig::from_env()?
    };

    // Override with CLI args
    if let Some(name) = args.name {
        config.name = name;
    }
    if let Some(model) = args.model {
        config.llm.model = model;
    }
    if let Some(system) = args.system {
        config.system_prompt = Some(system);
    }

    // Create and run agent
    let mut agent = ExoAgent::new(config).await?;

    // Handle shutdown gracefully
    tokio::select! {
        result = agent.run() => {
            result?;
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Shutdown signal received");
            agent.stop();
        }
    }

    Ok(())
}

async fn run_once(args: Args, mock: bool) -> Result<()> {
    let prompt = read_agent_prompt()?;
    let mock = mock || std::env::var("EXO_AGENT_MOCK").ok().as_deref() == Some("1");

    let report = if mock {
        AgentReport {
            task_id: prompt.task_id,
            status: "succeeded".to_string(),
            summary: format!("{} completed", prompt.agent_id),
            artifacts: vec![],
            followups: vec![],
        }
    } else {
        run_once_real(args, prompt).await?
    };

    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

async fn run_once_real(args: Args, prompt: AgentPrompt) -> Result<AgentReport> {
    let mut config = if let Some(path) = &args.config {
        AgentConfig::from_file(std::path::Path::new(path))?
    } else {
        AgentConfig::from_env()?
    };

    if let Some(name) = args.name {
        config.name = name;
    }
    if let Some(model) = args.model {
        config.llm.model = model;
    }
    if let Some(system) = args.system {
        config.system_prompt = Some(system);
    }

    let task_id = prompt.task_id.clone();
    let agent_id = prompt.agent_id.clone();
    let mut agent = ExoAgent::new(config).await?;

    match agent
        .run_once(&build_orchestration_user_prompt(&prompt))
        .await
    {
        Ok(output) => Ok(
            report_from_agent_output(&task_id, &output).unwrap_or_else(|| AgentReport {
                task_id,
                status: "succeeded".to_string(),
                summary: output.trim().to_string(),
                artifacts: vec![],
                followups: vec![],
            }),
        ),
        Err(e) => Ok(AgentReport {
            task_id,
            status: "failed".to_string(),
            summary: format!("{} failed: {}", agent_id, e),
            artifacts: vec![],
            followups: vec![],
        }),
    }
}

fn read_agent_prompt() -> Result<AgentPrompt> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    Ok(serde_json::from_str(input.trim())?)
}

fn build_orchestration_user_prompt(prompt: &AgentPrompt) -> String {
    format!(
        "{}\n\nYou are agent `{}` working on task `{}`.\nReturn either plain text summary or an AgentReport JSON object with fields: task_id, status, summary, artifacts, followups.",
        prompt.prompt, prompt.agent_id, prompt.task_id
    )
}

fn report_from_agent_output(task_id: &str, output: &str) -> Option<AgentReport> {
    if let Some(report) = parse_report_candidate(task_id, output.trim()) {
        return Some(report);
    }

    for line in output
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
    {
        if let Some(report) = parse_report_candidate(task_id, line) {
            return Some(report);
        }
    }

    for block in fenced_code_blocks(output) {
        if let Some(report) = parse_report_candidate(task_id, block.trim()) {
            return Some(report);
        }
    }

    for object in json_object_candidates(output).into_iter().rev() {
        if let Some(report) = parse_report_candidate(task_id, object.trim()) {
            return Some(report);
        }
    }
    None
}

fn parse_report_candidate(task_id: &str, candidate: &str) -> Option<AgentReport> {
    let mut report = serde_json::from_str::<AgentReport>(candidate).ok()?;
    report.task_id = task_id.to_string();
    Some(report)
}

fn fenced_code_blocks(output: &str) -> Vec<&str> {
    let mut blocks = vec![];
    let mut rest = output;
    while let Some(start) = rest.find("```") {
        let after_start = &rest[start + 3..];
        let content_start = after_start.find('\n').map(|idx| idx + 1).unwrap_or(0);
        let content = &after_start[content_start..];
        let Some(end) = content.find("```") else {
            break;
        };
        blocks.push(&content[..end]);
        rest = &content[end + 3..];
    }
    blocks
}

fn json_object_candidates(output: &str) -> Vec<&str> {
    let mut candidates = vec![];
    let mut start: Option<usize> = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;

    for (idx, ch) in output.char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start_idx) = start.take() {
                        candidates.push(&output[start_idx..idx + ch.len_utf8()]);
                    }
                }
            }
            _ => {}
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_last_agent_report_json_line() {
        let output = r#"
thinking...
{"task_id":"","status":"succeeded","summary":"builder completed","artifacts":["a"],"followups":[]}
"#;
        let report = report_from_agent_output("task-9", output).unwrap();
        assert_eq!(report.task_id, "task-9");
        assert_eq!(report.status, "succeeded");
        assert_eq!(report.summary, "builder completed");
        assert_eq!(report.artifacts, vec!["a"]);
    }

    #[test]
    fn parses_kimi_style_fenced_json_report() {
        let output = r#"
```json
{
  "task_id": "task-live-1",
  "status": "succeeded",
  "summary": "planner completed live Kimi integration check",
  "artifacts": [],
  "followups": []
}
```
"#;
        let report = report_from_agent_output("task-live-1", output).unwrap();
        assert_eq!(report.task_id, "task-live-1");
        assert_eq!(report.status, "succeeded");
        assert_eq!(
            report.summary,
            "planner completed live Kimi integration check"
        );
    }

    #[test]
    fn parses_multiline_json_embedded_in_text() {
        let output = r#"
Here is the report:
{
  "task_id": "",
  "status": "succeeded",
  "summary": "verifier completed with braces {ok}",
  "artifacts": ["report.json"],
  "followups": ["builder should review"]
}
Done.
"#;
        let report = report_from_agent_output("task-3", output).unwrap();
        assert_eq!(report.task_id, "task-3");
        assert_eq!(report.summary, "verifier completed with braces {ok}");
        assert_eq!(report.followups, vec!["builder should review"]);
    }

    #[test]
    fn builds_orchestration_prompt_with_ids() {
        let prompt = AgentPrompt {
            task_id: "task-1".to_string(),
            agent_id: "planner".to_string(),
            prompt: "Prime directive: ship".to_string(),
        };
        let text = build_orchestration_user_prompt(&prompt);
        assert!(text.contains("Prime directive: ship"));
        assert!(text.contains("planner"));
        assert!(text.contains("task-1"));
        assert!(text.contains("AgentReport JSON"));
    }
}
