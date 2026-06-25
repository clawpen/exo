use exo_image::{ImageReference, RegistryClient, ImageStore};
use std::path::PathBuf;

pub struct PullArgs {
    pub image: String,
    pub skip_scan: bool,
    pub verify: bool,
    pub cosign_key: Option<String>,
    pub sbom: bool,
}

pub async fn execute(args: PullArgs) -> anyhow::Result<()> {
    println!("Pulling image: {}", args.image);

    // Optional signature verification before any network access.
    if args.verify || should_verify_by_env() {
        let key_path = exo_runtime::resolve_key_path(args.cosign_key.as_deref());
        match exo_runtime::verify_image(&args.image,
            key_path.as_deref(),
        ) {
            Ok(()) => println!("  Signature verified"),
            Err(e) => {
                anyhow::bail!("Signature verification failed: {}", e);
            }
        }
    }

    // Parse the image reference
    let image_ref = ImageReference::parse(&args.image)?;
    println!("  Registry: {}", image_ref.registry);
    println!("  Repository: {}", image_ref.repository);
    println!("  Tag: {}", image_ref.tag);
    if let Some(ref digest) = image_ref.digest {
        println!("  Digest: {}", digest);
    }

    // Create image store
    let store = ImageStore::default();

    // Check if already pulled
    if store.has_image(&image_ref) {
        println!("  Image already exists locally");
    } else {
        // Create registry client and pull
        let mut client = RegistryClient::new(store.clone())?;
        let pulled = client.pull(&image_ref).await?;

        println!("  Config: {}", pulled.config_digest);
        println!("  Layers: {}", pulled.layer_digests.len());
    }

    let rootfs = store.rootfs_path(&image_ref);

    // SBOM generation hook
    if args.sbom || should_generate_sbom_by_env() {
        if rootfs.exists() {
            let format = exo_runtime::SbomFormat::default();
            match exo_runtime::generate_sbom(&args.image, &rootfs, format) {
                Ok(sbom_json) => {
                    match exo_runtime::save_sbom(store.root(), &args.image, format, &sbom_json) {
                        Ok(path) => println!("  SBOM saved to {}", path.display()),
                        Err(e) => tracing::warn!("Failed to save SBOM: {}", e),
                    }
                }
                Err(e) => tracing::warn!("SBOM generation failed (non-fatal): {}", e),
            }
        }
    }

    // Vulnerability scan hook
    if !args.skip_scan && !is_scan_disabled_by_env() {
        if rootfs.exists() {
            match exo_runtime::scan_image_rootfs(&args.image, &rootfs) {
                Ok(report) => {
                    save_scan_report(&image_ref, &store, &report)?;
                    print_scan_summary(&report);
                    if should_fail_on_scan() && (report.critical_count() + report.high_count()) > 0 {
                        anyhow::bail!(
                            "Pull blocked by vulnerability scan policy: {} critical, {} high",
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

    println!("\nSuccessfully pulled {}", args.image);

    Ok(())
}

fn should_verify_by_env() -> bool {
    std::env::var("EXO_VERIFY").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn should_generate_sbom_by_env() -> bool {
    std::env::var("EXO_SBOM").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn is_scan_disabled_by_env() -> bool {
    std::env::var("EXO_SKIP_SCAN").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn should_fail_on_scan() -> bool {
    std::env::var("EXO_SCAN_FATAL").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn save_scan_report(
    image_ref: &ImageReference,
    store: &ImageStore,
    report: &exo_runtime::VulnerabilityReport,
) -> anyhow::Result<()> {
    let scan_dir = store.root().join("scans");
    std::fs::create_dir_all(&scan_dir)?;
    let filename = format!("{}_{}.json", image_ref.repository.replace('/', "_"), image_ref.tag);
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
