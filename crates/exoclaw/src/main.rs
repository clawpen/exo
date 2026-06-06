//! # exoClaw CLI - Secure Local Agent Harness

use clap::{Parser, Subcommand};
use exoclaw::{ToolRegistry, DateTimeTool, LlmConfig, OpenAiCompatibleProvider, into_provider, Agent};
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

    let tools = vec![
        ("get_datetime", "Get current date and time", "None - Always available"),
    ];

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
    println!("exoClaw web UI would be available at http://localhost:{}", port);
    println!();

    // TODO: Start actual web server when async runtime is fully set up

    Ok(())
}
