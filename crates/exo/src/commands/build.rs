//! `exo build` — build an image from an agent manifest (`exo.toml`).
//!
//! Implemented now (testable offline): parse/validate the manifest, pull the
//! base, execute COPY steps into a new content-addressed layer, register the
//! built image, and compose its rootfs. RUN-step execution needs the runtime's
//! container-exec path and is the next E2 slice (tracked in ROADMAP_ENTERPRISE.md).

use exo_image::{AgentManifest, ImageReference, ImageStore, LayerStore, RegistryClient,
    DEFAULT_IMAGE_ROOT};
use std::path::{Path, PathBuf};

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
    // COPY sources are resolved relative to the manifest's directory.
    let ctx_dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));

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

    let cas = LayerStore::new(PathBuf::from(DEFAULT_IMAGE_ROOT));

    // Start the built image's layer set from the base image's layers.
    let index = cas.load_index()?;
    let mut layers = index
        .images
        .get(&base.to_string())
        .map(|r| r.layers.clone())
        .ok_or_else(|| anyhow::anyhow!("base {} not in layer index after pull", base))?;

    // Execute COPY steps: stage files at their destination paths, then commit
    // the staging tree as a single new layer (the COPY diff).
    if !manifest.build.copy.is_empty() {
        let stage = std::env::temp_dir().join(format!("exo-build-{}", std::process::id()));
        if stage.exists() {
            std::fs::remove_dir_all(&stage).ok();
        }
        std::fs::create_dir_all(&stage)?;

        for [src, dst] in &manifest.build.copy {
            let from = ctx_dir.join(src);
            // Destination path inside the rootfs (strip leading '/').
            let rel = dst.trim_start_matches('/');
            let to = stage.join(rel);
            copy_path(&from, &to)
                .map_err(|e| anyhow::anyhow!("COPY {} -> {}: {}", src, dst, e))?;
            println!("  copied {} -> {}", src, dst);
        }

        let digest = cas.commit_layer(&stage)?;
        layers.push(digest);
        std::fs::remove_dir_all(&stage).ok();
    }

    // Register and compose the built image. Normalize the reference the same way
    // pulls do, so `exo image inspect <name>` resolves to the same index key.
    let built = ImageReference::parse(&manifest.image_reference())?;
    let built_ref = built.to_string();

    // Generate an OCI config + manifest (ENV/CMD/workdir + layers) so the built
    // image is pushable to any registry, then register it in the layer index.
    let store = ImageStore::default();
    let config_digest = exo_image::build_and_store(
        &store,
        &built,
        &layers,
        &manifest.build.env,
        &manifest.build.cmd,
        manifest.build.workdir.as_deref(),
    )?;
    cas.register_image(&built_ref, layers.clone(), config_digest)?;
    let rootfs = PathBuf::from(DEFAULT_IMAGE_ROOT)
        .join("rootfs")
        .join(built_ref.replace([':', '/'], "_"));
    cas.compose_rootfs(&rootfs, &layers)?;

    if !manifest.build.run.is_empty() {
        println!(
            "\nNote: {} run step(s) parsed but NOT executed yet \
             (needs runtime exec; next E2 slice).",
            manifest.build.run.len()
        );
    }
    println!("\nBuilt {} ({} layers) ✓", built_ref, layers.len());
    Ok(())
}

/// Recursively copy a file or directory tree from `from` to `to`.
fn copy_path(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_path(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to)?;
    }
    Ok(())
}
