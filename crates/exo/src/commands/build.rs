//! `exo build` — build an image from an agent manifest (`exo.toml`).
//!
//! Implemented now (testable offline): parse/validate the manifest, pull the
//! base, execute COPY steps into a new content-addressed layer, register the
//! built image, and compose its rootfs. RUN-step execution needs the runtime's
//! container-exec path and is the next E2 slice (tracked in ROADMAP_ENTERPRISE.md).

use exo_image::{AgentManifest, ExoIgnore, ImageReference, ImageStore, LayerStore, RegistryClient,
    DEFAULT_IMAGE_ROOT};
use std::path::{Path, PathBuf};

pub struct BuildArgs {
    /// Path to the manifest/Dockerfile, or a directory containing `exo.toml`.
    pub file: Option<String>,
    /// Image name for Dockerfile builds (e.g. `-t my-agent`).
    pub tag: Option<String>,
    /// Skip vulnerability scan after build.
    pub skip_scan: bool,
    /// Generate and save an SBOM after build.
    pub sbom: bool,
}

pub async fn execute(args: BuildArgs) -> anyhow::Result<()> {
    // Resolve the input path: explicit file, dir/exo.toml, or ./exo.toml.
    let path = match args.file {
        Some(f) => {
            let p = PathBuf::from(f);
            if p.is_dir() { p.join("exo.toml") } else { p }
        }
        None => PathBuf::from("exo.toml"),
    };
    if !path.exists() {
        anyhow::bail!("no build file at {:?} (expected exo.toml or a Dockerfile)", path);
    }
    // COPY sources are resolved relative to the build file's directory.
    let ctx_dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));

    // exo.toml -> agent manifest; anything else is treated as a Dockerfile.
    let is_toml = path.extension().map(|e| e == "toml").unwrap_or(false);
    let manifest = if is_toml {
        AgentManifest::load(&path)?
    } else {
        let name = args.tag.clone().unwrap_or_else(|| {
            ctx_dir.file_name().map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "image".to_string())
        });
        let text = std::fs::read_to_string(&path)?;
        AgentManifest::from_dockerfile(&text, &name)?
    };
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

    // Load .exoignore from the build context (exclude node_modules, .git, secrets).
    let ignore = match std::fs::read_to_string(ctx_dir.join(".exoignore")) {
        Ok(text) => ExoIgnore::parse(&text),
        Err(_) => ExoIgnore::default(),
    };

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
            copy_path_filtered(&from, &to, &ctx_dir, &ignore)
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
    // Vulnerability scan hook on the built image rootfs.
    if !args.skip_scan && !is_scan_disabled_by_env() {
        let rootfs = PathBuf::from(DEFAULT_IMAGE_ROOT)
            .join("rootfs")
            .join(built_ref.replace([':', '/'], "_"));
        if rootfs.exists() {
            match exo_runtime::scan_image_rootfs(&built_ref, &rootfs) {
                Ok(report) => {
                    save_scan_report(&built_ref, &report)?;
                    print_scan_summary(&report);
                    if should_fail_on_scan() && (report.critical_count() + report.high_count()) > 0 {
                        anyhow::bail!(
                            "Build blocked by vulnerability scan policy: {} critical, {} high",
                            report.critical_count(),
                            report.high_count()
                        );
                    }
                }
                Err(e) => {
                    if should_fail_on_scan() {
                        anyhow::bail!("Vulnerability scan failed: {}", e);
                    } else {
                        tracing::warn!("Vulnerability scan failed (non-fatal): {}", e);
                    }
                }
            }
        }
    }

    // SBOM generation hook on the built image rootfs.
    if args.sbom || should_generate_sbom_by_env() {
        let rootfs = PathBuf::from(DEFAULT_IMAGE_ROOT)
            .join("rootfs")
            .join(built_ref.replace([':', '/'], "_"));
        if rootfs.exists() {
            let format = exo_runtime::SbomFormat::default();
            match exo_runtime::generate_sbom(&built_ref, &rootfs, format) {
                Ok(sbom_json) => {
                    match exo_runtime::save_sbom(Path::new(DEFAULT_IMAGE_ROOT), &built_ref, format, &sbom_json) {
                        Ok(path) => println!("  SBOM saved to {}", path.display()),
                        Err(e) => tracing::warn!("Failed to save SBOM: {}", e),
                    }
                }
                Err(e) => tracing::warn!("SBOM generation failed (non-fatal): {}", e),
            }
        }
    }

    println!("\nBuilt {} ({} layers) ✓", built_ref, layers.len());
    Ok(())
}

fn is_scan_disabled_by_env() -> bool {
    std::env::var("EXO_SKIP_SCAN").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn should_generate_sbom_by_env() -> bool {
    std::env::var("EXO_SBOM").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn should_fail_on_scan() -> bool {
    std::env::var("EXO_SCAN_FATAL").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn save_scan_report(
    image_ref: &str,
    report: &exo_runtime::VulnerabilityReport,
) -> anyhow::Result<()> {
    let scan_dir = PathBuf::from(DEFAULT_IMAGE_ROOT).join("scans");
    std::fs::create_dir_all(&scan_dir)?;
    let filename = format!("{}.json", image_ref.replace([':', '/'], "_"));
    let path = scan_dir.join(filename);
    let json = serde_json::to_string_pretty(report)?;
    std::fs::write(&path, json)?;
    Ok(())
}

fn print_scan_summary(report: &exo_runtime::VulnerabilityReport) {
    if report.vulnerabilities.is_empty() {
        println!("  Scan: no vulnerabilities detected ({})", report.scanner);
        return;
    }
    println!(
        "  Scan: {} vulnerabilities found ({} critical, {} high, {} medium, {} low) via {}",
        report.vulnerabilities.len(),
        report.critical_count(),
        report.high_count(),
        report.medium_count(),
        report.low_count(),
        report.scanner
    );
    for vuln in report.vulnerabilities.iter().take(5) {
        println!(
            "    - {} ({}) in {} {}",
            vuln.id, vuln.severity, vuln.package, vuln.version
        );
    }
    if report.vulnerabilities.len() > 5 {
        println!("    ... and {} more", report.vulnerabilities.len() - 5);
    }
}

/// Recursively copy a file/dir tree, skipping anything matched by `.exoignore`
/// (matched on the path relative to the build context root).
fn copy_path_filtered(from: &Path, to: &Path, ctx_root: &Path, ignore: &ExoIgnore)
    -> std::io::Result<()>
{
    if !ignore.is_empty() {
        if let Ok(rel) = from.strip_prefix(ctx_root) {
            if ignore.is_ignored(&rel.to_string_lossy()) {
                return Ok(());
            }
        }
    }
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_path_filtered(&entry.path(), &to.join(entry.file_name()), ctx_root, ignore)?;
        }
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to)?;
    }
    Ok(())
}
