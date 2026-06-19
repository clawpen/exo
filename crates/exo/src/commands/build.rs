//! `exo build` — build an image from an agent manifest (`exo.toml`).
//!
//! This first slice resolves and validates the manifest, pulls the base image,
//! and prints the build plan. Executing RUN steps and committing COPY/env into a
//! new layer requires the runtime's container-exec path and is the next milestone
//! (tracked in ROADMAP_ENTERPRISE.md, E2).

use exo_image::{AgentManifest, ImageReference, ImageStore, RegistryClient};
use std::path::PathBuf;

pub struct BuildArgs {
    /// Path to the manifest, or a directory containing `exo.toml`.
    pub file: Option<String>,
}

pub async fn execute(args: BuildArgs) -> anyhow::Result<()> {
    // Resolve the manifest path: explicit file, dir/exo.toml, or ./exo.toml.
    let path = match args.file {
        Some(f) => {
            let p = PathBuf::from(f);
            if p.is_dir() { p.join("exo.toml") } else { p }
        }
        None => PathBuf::from("exo.toml"),
    };
    if !path.exists() {
        anyhow::bail!("no manifest at {:?} (expected exo.toml)", path);
    }

    let manifest = AgentManifest::load(&path)?;
    println!("{}", manifest.plan());

    // Pull the base image so the build has something to layer on.
    let base = ImageReference::parse(&manifest.agent.from)?;
    let store = ImageStore::default();
    if store.has_image(&base) {
        println!("Base {} already present", base);
    } else {
        println!("Pulling base {}...", base);
        let mut client = RegistryClient::new(store)?;
        client.pull(&base).await?;
    }

    if !manifest.build.run.is_empty() || !manifest.build.copy.is_empty() {
        println!(
            "\nNote: {} run step(s) and {} copy step(s) parsed but not yet executed \
             (layer commit lands in the next E2 slice).",
            manifest.build.run.len(),
            manifest.build.copy.len()
        );
    }
    println!("\nResolved build plan for {} ✓", manifest.image_reference());
    Ok(())
}
