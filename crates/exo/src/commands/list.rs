//! List containers command

use exo_runtime::ContainerManager;
use anyhow::Result;

pub struct ListArgs {
    pub all: bool,
    pub json: bool,
}

pub async fn execute(args: ListArgs) -> Result<()> {
    // For JSON output, suppress tracing warnings to stdout by redirecting to stderr
    // This prevents corruption of JSON output
    if args.json {
        // Redirect tracing to stderr for JSON output
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            .with_writer(std::io::stderr)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    }

    let manager = ContainerManager::new()?;

    // Refresh status for all containers
    let mut containers = manager.list()?;
    for container in &mut containers {
        let _ = manager.refresh_status(container);
    }

    // Re-load after refresh
    containers = if args.all {
        manager.list()?
    } else {
        manager.list_running()?
    };

    if args.json {
        // JSON output format
        let json_containers: Vec<exo_runtime::ContainerJson> = containers
            .into_iter()
            .map(exo_runtime::ContainerJson::from)
            .collect();
        
        let output = exo_runtime::ContainerListJson {
            containers: json_containers,
        };
        
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    
    // Table output format
    if containers.is_empty() {
        if args.all {
            println!("No containers found.");
        } else {
            println!("No running containers. Use --all to show all containers.");
        }
        return Ok(());
    }
    
    // Print header
    println!(
        "{:<12} {:<20} {:<16} {:<12} {:<8} {:<20}",
        "CONTAINER ID", "NAME", "IMAGE", "STATUS", "PID", "CREATED"
    );
    println!("{}", "-".repeat(90));
    
    // Print containers
    for container in containers {
        let id_short = &container.id[..8.min(container.id.len())];
        let name = truncate(&container.name, 20);
        let image = truncate(&container.image, 16);
        let status = truncate(&container.status, 12);
        let pid = container.pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());
        let created = format_created(container.created_at);
        
        println!(
            "{:<12} {:<20} {:<16} {:<12} {:<8} {:<20}",
            id_short, name, image, status, pid, created
        );
    }
    
    Ok(())
}

/// Truncate a string to max length, adding "..." if needed.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Format creation timestamp as relative time.
fn format_created(created: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Utc;
    
    let now = Utc::now();
    let duration = now.signed_duration_since(created);
    
    if duration.num_minutes() < 1 {
        "Just now".to_string()
    } else if duration.num_minutes() < 60 {
        format!("{} minutes ago", duration.num_minutes())
    } else if duration.num_hours() < 24 {
        format!("{} hours ago", duration.num_hours())
    } else {
        format!("{} days ago", duration.num_days())
    }
}
