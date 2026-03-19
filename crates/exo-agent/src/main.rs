use clap::{Parser, Subcommand};
use colored::Colorize;
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::info;

use exo_gateway::{Gateway, GatewayConfig};

#[derive(Parser)]
#[command(name = "exo-agent")]
#[command(about = "Exo Agent - Containerized AI agent runtime")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the agent gateway server
    Gateway {
        /// Address to bind to
        #[arg(short, long, default_value = "127.0.0.1:8080")]
        bind: String,

        /// Directory containing skills
        #[arg(short, long)]
        skills_dir: Option<PathBuf>,

        /// Session timeout in seconds
        #[arg(long, default_value = "300")]
        session_timeout: u64,

        /// Disable cron scheduler
        #[arg(long)]
        no_cron: bool,
    },

    /// List available skills
    ListSkills {
        /// Skills directory
        #[arg(short, long)]
        skills_dir: Option<PathBuf>,
    },

    /// Invoke a tool directly
    Invoke {
        /// Skill name
        skill: String,

        /// Tool name
        tool: String,

        /// Arguments as JSON
        #[arg(short, long, default_value = "{}")]
        args: String,
    },

    /// Create a new skill template
    NewSkill {
        /// Skill name
        name: String,

        /// Output directory
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
    },

    /// Interactive shell/REPL mode
    Shell {
        /// Gateway WebSocket URL
        #[arg(short, long, default_value = "ws://127.0.0.1:8080/ws")]
        url: String,

        /// Agent ID
        #[arg(short, long)]
        agent_id: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("exo_agent=info".parse()?)
                .add_directive("exo_gateway=info".parse()?),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Gateway { bind, skills_dir, session_timeout, no_cron } => {
            info!("Starting Exo Agent Gateway");

            let bind_addr: SocketAddr = bind.parse()
                .map_err(|e| anyhow::anyhow!("Invalid bind address: {}", e))?;

            let config = GatewayConfig {
                bind_addr,
                skills_dir,
                session_timeout_secs: session_timeout,
                enable_cron: !no_cron,
            };

            let gateway = Gateway::new(config).await?;
            gateway.run().await?;
        }

        Commands::ListSkills { skills_dir } => {
            let registry = if let Some(dir) = skills_dir {
                let reg = exo_gateway::SkillRegistry::with_skills_dir(&dir);
                reg.load_from_dir(&dir).await?;
                reg
            } else {
                exo_gateway::SkillRegistry::new()
            };

            let skills = registry.list_skills().await;

            if skills.is_empty() {
                println!("No skills registered.");
            } else {
                println!("Available Skills:");
                for skill in skills {
                    println!("  {} (v{}) - {}", skill.name, skill.version, skill.description);
                    println!("    Tools: {}", skill.tool_count);
                }
            }
        }

        Commands::Invoke { skill, tool, args } => {
            let args: serde_json::Value = serde_json::from_str(&args)
                .map_err(|e| anyhow::anyhow!("Invalid JSON args: {}", e))?;

            info!(skill = %skill, tool = %tool, "Invoking tool");

            // For now, just print what would be invoked
            // TODO: Actually execute through exo-runtime
            println!("Would invoke {}:{} with args: {}", skill, tool, args);
        }

        Commands::NewSkill { name, output } => {
            create_skill_template(&name, &output).await?;
        }

        Commands::Shell { url, agent_id } => {
            run_shell(&url, agent_id).await?;
        }
    }

    Ok(())
}

async fn create_skill_template(name: &str, output: &std::path::Path) -> anyhow::Result<()> {
    let skill_dir = output.join(name);
    tokio::fs::create_dir_all(&skill_dir).await?;

    let manifest = format!(r#"name: {}
version: "0.1.0"
description: "A brief description of your skill"
author: "Your Name"

runtime:
  type: container
  image: alpine:latest
  resources:
    memory: "256M"
    cpu: 0.25
    gpu: false

tools:
  - name: hello
    description: "Say hello"
    parameters:
      type: object
      properties:
        name:
          type: string
          description: "Name to greet"
      required: ["name"]
    returns:
      type: object
      properties:
        message:
          type: string
    timeout_ms: 30000
"#, name);

    let manifest_path = skill_dir.join("skill.yaml");
    tokio::fs::write(&manifest_path, manifest).await?;

    // Create a simple Dockerfile
    let dockerfile = r#"FROM alpine:latest

# Install any dependencies
RUN apk add --no-cache bash

# Copy tool scripts
COPY tools/ /tools/

# Default entrypoint
ENTRYPOINT ["/bin/bash"]
"#;

    let dockerfile_path = skill_dir.join("Dockerfile");
    tokio::fs::write(&dockerfile_path, dockerfile).await?;

    // Create tools directory with example
    let tools_dir = skill_dir.join("tools");
    tokio::fs::create_dir_all(&tools_dir).await?;

    let example_tool = r#"#!/bin/bash
# Example tool script
# Input comes via stdin as JSON

# Read and parse input
read -r input
echo "Hello, $(echo "$input" | jq -r '.name')!"
"#;

    let tool_path = tools_dir.join("hello.sh");
    tokio::fs::write(&tool_path, example_tool).await?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(&tool_path).await?.permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&tool_path, perms).await?;
    }

    println!("Created skill template at: {}", skill_dir.display());
    println!("  - skill.yaml: Skill manifest");
    println!("  - Dockerfile: Container definition");
    println!("  - tools/: Tool scripts");

    Ok(())
}

async fn run_shell(url: &str, agent_id: Option<String>) -> anyhow::Result<()> {
    use colored::Colorize;
    use rustyline::error::ReadlineError;
    use rustyline::DefaultEditor;
    use tokio::sync::mpsc;
    use futures::{SinkExt, StreamExt};

    println!("{}", "╔═══════════════════════════════════════╗".bright_blue());
    println!("{}", "║      Exo Agent Interactive Shell      ║".bright_blue());
    println!("{}", "╚═══════════════════════════════════════╝".bright_blue());
    println!();
    println!("Connecting to {}...", url.cyan());

    // Connect to WebSocket
    let (ws_stream, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect: {}", e))?;

    println!("{}", "Connected!".green());
    println!();
    println!("Type {} for available commands", "/help".yellow());
    println!();

    let (mut write, mut read) = ws_stream.split();

    // Send Hello message
    let hello = serde_json::json!({
        "type": "hello",
        "version": "1.0.0",
        "agent_id": agent_id,
        "capabilities": ["shell", "interactive"]
    });
    write
        .send(tokio_tungstenite::tungstenite::Message::Text(
            hello.to_string(),
        ))
        .await?;

    // Channel for passing messages from WebSocket reader to main thread
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    // Spawn WebSocket reader task
    tokio::spawn(async move {
        while let Some(msg) = read.next().await {
            if let Ok(tokio_tungstenite::tungstenite::Message::Text(text)) = msg {
                let _ = tx.send(text);
            }
        }
    });

    // Setup rustyline
    let mut rl = DefaultEditor::new()?;
    let history_path = dirs::cache_dir().map(|d| d.join("exo-agent").join("history"));

    if let Some(ref path) = history_path {
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = rl.load_history(path);
    }

    let prompt = format!("{} ", "exo>".bright_green().bold());

    loop {
        // Check for incoming WebSocket messages
        while let Ok(msg) = rx.try_recv() {
            match serde_json::from_str::<serde_json::Value>(&msg) {
                Ok(json) => {
                    print_incoming_message(&json);
                }
                Err(_) => {
                    println!("{} {}", "◄─".bright_cyan(), msg);
                }
            }
        }

        // Read user input (non-blocking would be better, but this works for now)
        match rl.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                rl.add_history_entry(line)?;

                // Handle shell commands
                if line.starts_with('/') {
                    match handle_shell_command(line, &mut write).await {
                        ShellResult::Continue => continue,
                        ShellResult::Exit => break,
                        ShellResult::Error(e) => println!("{} {}", "Error:".red(), e),
                    }
                } else {
                    // Send as raw message
                    let msg = tokio_tungstenite::tungstenite::Message::Text(line.to_string());
                    if let Err(e) = write.send(msg).await {
                        println!("{} {}", "Failed to send:".red(), e);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("{}", "Interrupted".yellow());
                break;
            }
            Err(ReadlineError::Eof) => {
                println!("{}", "EOF".yellow());
                break;
            }
            Err(err) => {
                println!("{} {:?}", "Error:".red(), err);
                break;
            }
        }
    }

    // Save history
    if let Some(ref path) = history_path {
        let _ = rl.save_history(path);
    }

    println!("{}", "Goodbye!".bright_blue());
    Ok(())
}

enum ShellResult {
    Continue,
    Exit,
    Error(String),
}

async fn handle_shell_command<W>(cmd: &str, write: &mut W) -> ShellResult
where
    W: futures::SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
    W::Error: std::fmt::Display,
{
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return ShellResult::Continue;
    }

    match parts[0] {
        "/help" | "/h" => {
            println!("{}", "Available Commands:".bright_yellow().underline());
            println!("  {} - Show this help", "/help, /h".cyan());
            println!("  {} - List available skills", "/skills, /s".cyan());
            println!("  {} - List available tools", "/tools, /t".cyan());
            println!(
                "  {} <skill> <tool> [args] - Call a tool",
                "/call, /c".cyan()
            );
            println!("  {} - Send ping to server", "/ping".cyan());
            println!("  {} - Exit the shell", "/exit, /quit, /q".cyan());
            println!();
            println!("{}", "Raw Messages:".bright_yellow().underline());
            println!("  Type any JSON to send directly to the gateway");
            ShellResult::Continue
        }

        "/exit" | "/quit" | "/q" => {
            println!("{}", "Disconnecting...".yellow());
            ShellResult::Exit
        }

        "/ping" => {
            let ping = serde_json::json!({ "type": "ping" });
            if let Err(e) = write
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    ping.to_string(),
                ))
                .await
            {
                ShellResult::Error(e.to_string())
            } else {
                println!("{} Ping sent", "►─".bright_green());
                ShellResult::Continue
            }
        }

        "/skills" | "/s" => {
            // Request skills list via REST would be better, but for now use a tool call pattern
            println!(
                "{}",
                "Available skills will be shown in session info...".dimmed()
            );
            let msg = serde_json::json!({ "type": "get_skills" });
            if let Err(e) = write
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    msg.to_string(),
                ))
                .await
            {
                ShellResult::Error(e.to_string())
            } else {
                ShellResult::Continue
            }
        }

        "/tools" | "/t" => {
            println!(
                "{}",
                "Available tools will be shown in session info...".dimmed()
            );
            let msg = serde_json::json!({ "type": "get_tools" });
            if let Err(e) = write
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    msg.to_string(),
                ))
                .await
            {
                ShellResult::Error(e.to_string())
            } else {
                ShellResult::Continue
            }
        }

        "/call" | "/c" => {
            if parts.len() < 3 {
                println!(
                    "{} Usage: /call <skill> <tool> [json_args]",
                    "Error:".red()
                );
                return ShellResult::Continue;
            }

            let skill = parts[1];
            let tool = parts[2];
            let args: serde_json::Value = if parts.len() > 3 {
                parts[3..]
                    .join(" ")
                    .parse()
                    .unwrap_or(serde_json::json!({}))
            } else {
                serde_json::json!({})
            };

            let request_id = uuid::Uuid::new_v4().to_string();
            let msg = serde_json::json!({
                "type": "tool_request",
                "request_id": request_id,
                "skill": skill,
                "tool": tool,
                "args": args,
                "timeout_ms": 30000
            });

            println!(
                "{} Calling {}:{}...",
                "►─".bright_green(),
                skill.cyan(),
                tool.cyan()
            );

            if let Err(e) = write
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    msg.to_string(),
                ))
                .await
            {
                ShellResult::Error(e.to_string())
            } else {
                ShellResult::Continue
            }
        }

        _ => ShellResult::Error(format!(
            "Unknown command: {}. Type /help for available commands.",
            parts[0]
        )),
    }
}

fn print_incoming_message(json: &serde_json::Value) {
    let msg_type = json
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    match msg_type {
        "welcome" => {
            println!("\n{} {}", "✓".bright_green().bold(), "Connected to gateway".green());
            if let Some(session_id) = json.get("session_id").and_then(|v| v.as_str()) {
                println!("  Session: {}", session_id.dimmed());
            }
        }
        "session_info" => {
            if let Some(tools) = json.get("active_tools").and_then(|v| v.as_array()) {
                if !tools.is_empty() {
                    println!(
                        "\n{} {}",
                        "Tools:".bright_yellow(),
                        tools
                            .iter()
                            .filter_map(|t| t.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                            .cyan()
                    );
                }
            }
        }
        "tool_response" => {
            if let Some(request_id) = json.get("request_id").and_then(|v| v.as_str()) {
                print!("\n{} ", format!("[{}]", &request_id[..8]).dimmed());
            }

            if let Some(result) = json.get("result") {
                if result.get("type").and_then(|v| v.as_str()) == Some("success") {
                    println!("{}", "✓ Success".green());
                    if let Some(output) = result.get("output") {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(output)
                                .unwrap_or_default()
                                .cyan()
                        );
                    }
                } else if result.get("error").is_some() {
                    println!("{}", "✗ Error".red());
                    println!("{}", result["error"].to_string().red());
                } else {
                    println!("{}", result.to_string().cyan());
                }
            }
        }
        "pong" => {
            println!("{} {}", "◄─".bright_cyan(), "Pong!".green());
        }
        "error" => {
            println!(
                "\n{} {}",
                "✗ Error:".red().bold(),
                json.to_string().red()
            );
        }
        _ => {
            println!(
                "{} {}",
                "◄─".bright_cyan(),
                serde_json::to_string_pretty(json).unwrap_or_default()
            );
        }
    }
}
