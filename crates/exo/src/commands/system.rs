//! `exo system` — disk usage (df) and layer GC (prune).
//!
//! Surfaces the content-addressed layer store's dedup accounting — the numbers
//! behind Exo's "space savings vs Docker" claim — and reclaims unreferenced
//! layers.

use exo_image::{LayerStore, DEFAULT_IMAGE_ROOT};
use std::path::PathBuf;

fn store() -> LayerStore {
    LayerStore::new(PathBuf::from(DEFAULT_IMAGE_ROOT))
}

fn human(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1} {}", v, U[i])
}

pub struct DfArgs {
    pub json: bool,
}

pub async fn df(args: DfArgs) -> anyhow::Result<()> {
    let usage = store().disk_usage()?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&usage)?);
        return Ok(());
    }

    println!("{:<22} {}", "Images:", usage.images);
    println!("{:<22} {}", "Unique layers:", usage.unique_layers);
    println!("{:<22} {}", "Physical (on disk):", human(usage.physical_bytes));
    println!("{:<22} {}", "Logical (no dedup):", human(usage.logical_bytes));
    println!(
        "{:<22} {} ({}% saved)",
        "Reclaimed by dedup:",
        human(usage.reclaimable_via_dedup()),
        usage.savings_pct()
    );
    Ok(())
}

pub async fn prune() -> anyhow::Result<()> {
    let (removed, reclaimed) = store().prune()?;
    println!("Pruned {} unreferenced layer(s), reclaimed {}", removed, human(reclaimed));
    Ok(())
}

pub struct CheckArgs {
    pub repair: bool,
    pub json: bool,
}

pub async fn check(args: CheckArgs) -> anyhow::Result<()> {
    let s = store();
    let report = s.check()?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if report.is_healthy() {
        println!("Store healthy: {} image(s), no issues", report.images);
    } else {
        println!("Store issues found ({} image(s)):", report.images);
        for (reference, missing) in &report.dangling_images {
            println!("  dangling: {} is missing {} layer(s)", reference, missing.len());
        }
        if !report.orphan_layers.is_empty() {
            println!("  {} orphan layer(s) reclaimable", report.orphan_layers.len());
        }
    }

    if args.repair && !report.is_healthy() {
        let (imgs, layers) = s.repair()?;
        println!("Repaired: removed {} dangling image(s), pruned {} layer(s)", imgs, layers);
    } else if args.repair {
        println!("Nothing to repair.");
    }
    Ok(())
}
