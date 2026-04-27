//! `exo events` command — query the lifecycle event log.
//!
//! Reads the sqlite ring-buffer log written by the daemon + reconciler.
//! Linux-only (the log lives at /var/lib/exo/events.db on the daemon host).

use anyhow::Result;

pub struct EventsArgs {
    pub container: Option<String>,
    pub limit: usize,
    pub json: bool,
}

pub async fn execute(args: EventsArgs) -> Result<()> {
    #[cfg(windows)]
    {
        return execute_windows(args).await;
    }

    #[cfg(not(windows))]
    {
        return execute_linux(args).await;
    }
}

#[cfg(windows)]
async fn execute_windows(_args: EventsArgs) -> Result<()> {
    // The event log lives in WSL with the daemon; surfacing it from Windows
    // would mean another wsl-exec hop. Out of scope for M2 — print a hint.
    println!("`exo events` reads the daemon's event log inside WSL2.");
    println!("Run `wsl -d Ubuntu -- exo events [--container NAME] [--limit N] [--json]` to query it directly.");
    Ok(())
}

#[cfg(not(windows))]
async fn execute_linux(args: EventsArgs) -> Result<()> {
    use exo_runtime::EventLog;

    let log = EventLog::open_default()?;

    let events = match &args.container {
        Some(c) => log.for_container(c, args.limit)?,
        None => log.recent(args.limit)?,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&events)?);
        return Ok(());
    }

    if events.is_empty() {
        if let Some(c) = &args.container {
            println!("No events recorded for {}.", c);
        } else {
            println!("No events recorded.");
        }
        return Ok(());
    }

    // Newest-first table.
    println!(
        "{:<24} {:<20} {:<18} {}",
        "TIMESTAMP", "CONTAINER", "EVENT", "DETAIL"
    );
    println!("{}", "-".repeat(90));
    for e in events {
        let ts = format_ts(e.ts_millis);
        let name = truncate(&e.container_name, 20);
        let event_type = format!("{:?}", e.event_type).to_lowercase();
        let event_type = truncate(&event_type, 18);
        let detail = e.detail.as_deref().unwrap_or("-");
        println!("{:<24} {:<20} {:<18} {}", ts, name, event_type, detail);
    }

    Ok(())
}

#[cfg(not(windows))]
fn format_ts(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Utc
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ms.to_string())
}

#[cfg(not(windows))]
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
