use std::fs::File;
use std::io::Write;
use std::path::Path;
use tracing::info;

/// Download `url` to `dest` if `dest` does not already exist.
pub async fn download_file_if_missing(url: &str, dest: &Path) -> anyhow::Result<()> {
    if dest.exists() {
        let meta = dest.metadata()?;
        info!("Using cached {} ({} bytes)", dest.display(), meta.len());
        return Ok(());
    }
    info!("Downloading {} to {}", url, dest.display());
    let response = reqwest::get(url).await?;
    response.error_for_status_ref()?;
    let bytes = response.bytes().await?;
    let mut file = File::create(dest)?;
    file.write_all(&bytes)?;
    info!("Downloaded {} ({} bytes)", dest.display(), bytes.len());
    Ok(())
}

/// Force-download `url` to `dest`, overwriting any existing file.
pub async fn download_file(url: &str, dest: &Path) -> anyhow::Result<()> {
    info!("Downloading {} to {}", url, dest.display());
    let response = reqwest::get(url).await?;
    response.error_for_status_ref()?;
    let bytes = response.bytes().await?;
    let mut file = File::create(dest)?;
    file.write_all(&bytes)?;
    info!("Downloaded {} ({} bytes)", dest.display(), bytes.len());
    Ok(())
}
