//! `exo image inspect` — per-layer detail and shared-vs-exclusive disk usage.

use exo_image::{ImageReference, LayerStore, DEFAULT_IMAGE_ROOT};
use std::path::PathBuf;

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

pub struct InspectArgs {
    pub image: String,
    pub json: bool,
}

pub async fn inspect(args: InspectArgs) -> anyhow::Result<()> {
    let reference = ImageReference::parse(&args.image)?;
    let cas = LayerStore::new(PathBuf::from(DEFAULT_IMAGE_ROOT));

    let info = cas
        .inspect(&reference.to_string())?
        .ok_or_else(|| anyhow::anyhow!("{} not found locally", reference))?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }

    println!("Image:   {}", info.reference);
    println!("Config:  {}", info.config_digest);
    println!("Total:   {}", human(info.total_size));
    println!(
        "Exclusive: {} (rest shared with other images)",
        human(info.exclusive_size)
    );
    println!("\n{:<20} {:>12}  {}", "LAYER", "SIZE", "SHARED BY");
    println!("{}", "-".repeat(48));
    for l in &info.layers {
        let short: String = l.digest.chars().take(19).collect();
        let shared = if l.refcount > 1 {
            format!("{} images", l.refcount)
        } else {
            "exclusive".to_string()
        };
        println!("{:<20} {:>12}  {}", short, human(l.size), shared);
    }
    Ok(())
}
