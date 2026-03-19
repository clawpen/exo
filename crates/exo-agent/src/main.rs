use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing::{info, error};

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
