//! GPU inspection commands.

use serde::Serialize;

pub struct GpuListArgs {
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct GpuList {
    gpus: Vec<GpuEntry>,
}

#[derive(Debug, Serialize)]
struct GpuEntry {
    id: String,
    name: String,
    vendor: String,
    memory_mb: Option<u64>,
    metal: bool,
    builtin: bool,
}

pub async fn list(args: GpuListArgs) -> anyhow::Result<()> {
    let gpus = detect_gpu_entries()?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&GpuList { gpus })?);
        return Ok(());
    }

    if gpus.is_empty() {
        println!("No GPUs detected");
        return Ok(());
    }

    println!(
        "{:<4} {:<10} {:<40} {:<10} {}",
        "ID", "VENDOR", "NAME", "MEM(MB)", "METAL"
    );
    for gpu in gpus {
        println!(
            "{:<4} {:<10} {:<40} {:<10} {}",
            gpu.id,
            gpu.vendor,
            truncate(&gpu.name, 40),
            gpu.memory_mb
                .map(|m| m.to_string())
                .unwrap_or_else(|| "-".to_string()),
            if gpu.metal { "yes" } else { "no" }
        );
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn detect_gpu_entries() -> anyhow::Result<Vec<GpuEntry>> {
    Ok(exo_mac::detect_gpus()?
        .into_iter()
        .enumerate()
        .map(|(idx, gpu)| GpuEntry {
            id: idx.to_string(),
            name: gpu.name,
            vendor: gpu.vendor.as_str().to_string(),
            memory_mb: gpu.vram_mb,
            metal: gpu.metal_supported,
            builtin: gpu.builtin,
        })
        .collect())
}

#[cfg(not(target_os = "macos"))]
fn detect_gpu_entries() -> anyhow::Result<Vec<GpuEntry>> {
    Ok(exo_gpu::detect_gpus()?
        .into_iter()
        .map(|gpu| GpuEntry {
            id: gpu.id,
            name: gpu.name,
            vendor: gpu.gpu_type.to_string(),
            memory_mb: gpu.memory_mb,
            metal: false,
            builtin: false,
        })
        .collect())
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!(
            "{}…",
            truncated
                .chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        value.to_string()
    }
}
