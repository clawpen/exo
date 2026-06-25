//! Stats command implementation.

use anyhow::Result;

pub struct StatsArgs {
    pub container: String,
    pub json: bool,
}

pub async fn execute(args: StatsArgs) -> Result<()> {
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
async fn execute_windows(_args: StatsArgs) -> Result<()> {
    println!("Container stats are not yet supported on Windows; run inside WSL2.");
    Ok(())
}

#[cfg(not(windows))]
async fn execute_linux(args: StatsArgs) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let req = serde_json::json!({
        "type": "stats",
        "content": {
            "container_id": args.container
        }
    });

    let mut stream = UnixStream::connect("/tmp/exo-daemon.sock")?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.write_all(req.to_string().as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let resp: serde_json::Value = serde_json::from_str(line.trim())?;
    let resp_type = resp.get("type").and_then(|v| v.as_str()).unwrap_or("");

    if resp_type == "error" {
        let msg = resp
            .get("content")
            .and_then(|c| c.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("daemon returned error");
        anyhow::bail!("{}", msg);
    }

    if resp_type != "stats" {
        anyhow::bail!("unexpected daemon response type: {}", resp_type);
    }

    let stats_json = resp
        .get("content")
        .and_then(|c| c.get("stats"))
        .and_then(|s| s.as_str())
        .unwrap_or("{}");
    let payload: serde_json::Value = serde_json::from_str(stats_json)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let container = payload
        .get("container")
        .and_then(|v| v.as_str())
        .unwrap_or(&args.container);
    let status = payload
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    println!("Container: {}", container);
    println!("Status:    {}", status);

    if let Some(stats) = payload.get("stats").and_then(|v| v.as_object()) {
        println!("");
        println!("| Metric          | Value               |");
        println!("| ---             | ---:                |");
        if let Some(mem) = stats.get("memory_usage").and_then(|v| v.as_u64()) {
            println!("| Memory usage    | {} MiB |", mem / 1024 / 1024);
        }
        if let Some(limit) = stats.get("memory_limit").and_then(|v| v.as_u64()) {
            println!("| Memory limit    | {} MiB |", limit / 1024 / 1024);
        }
        if let Some(cpu) = stats.get("cpu_usage_ns").and_then(|v| v.as_u64()) {
            println!("| CPU usage       | {} ns |", cpu);
        }
        if let Some(pids) = stats.get("pids").and_then(|v| v.as_u64()) {
            println!("| PIDs            | {} |", pids);
        }
        if let Some(r) = stats.get("io_rbytes").and_then(|v| v.as_u64()) {
            println!("| I/O read        | {} B |", r);
        }
        if let Some(w) = stats.get("io_wbytes").and_then(|v| v.as_u64()) {
            println!("| I/O written     | {} B |", w);
        }
        if let Some(t) = stats.get("cpu_throttled").and_then(|v| v.as_u64()) {
            println!("| CPU throttled   | {} periods |", t);
        }
    } else {
        println!("No live stats available (container not running or no cgroup).");
    }

    Ok(())
}
