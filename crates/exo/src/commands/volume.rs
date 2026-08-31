//! Named volume management commands.

use exo_runtime::VolumeStore;
use serde::Serialize;

pub struct VolumeCreateArgs {
    pub name: String,
}

pub struct VolumeRemoveArgs {
    pub name: String,
}

pub struct VolumeListArgs {
    pub json: bool,
}

pub struct VolumeInspectArgs {
    pub name: String,
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct VolumeInfo {
    name: String,
    path: String,
}

pub async fn create(args: VolumeCreateArgs) -> anyhow::Result<()> {
    let store = VolumeStore::new()?;
    let path = store.create(&args.name)?;
    println!("{}", path.display());
    Ok(())
}

pub async fn remove(args: VolumeRemoveArgs) -> anyhow::Result<()> {
    let store = VolumeStore::new()?;
    if store.remove(&args.name)? {
        println!("Volume {} removed", args.name);
        Ok(())
    } else {
        // Existence is validated on removal (agent contract A5).
        Err(exo_runtime::ExoError::VolumeNotFound(args.name).into())
    }
}

pub async fn list(args: VolumeListArgs) -> anyhow::Result<()> {
    let store = VolumeStore::new()?;
    let volumes: Vec<VolumeInfo> = store
        .list()?
        .into_iter()
        .map(|name| {
            let path = store.path(&name)?;
            Ok(VolumeInfo {
                name,
                path: path.display().to_string(),
            })
        })
        .collect::<anyhow::Result<_>>()?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "volumes": volumes }))?
        );
        return Ok(());
    }

    if volumes.is_empty() {
        println!("No volumes found.");
        return Ok(());
    }

    println!("{:<24} PATH", "NAME");
    for volume in volumes {
        println!("{:<24} {}", volume.name, volume.path);
    }
    Ok(())
}

pub async fn inspect(args: VolumeInspectArgs) -> anyhow::Result<()> {
    let store = VolumeStore::new()?;
    let path = store.path(&args.name)?;
    if !path.exists() {
        return Err(exo_runtime::ExoError::VolumeNotFound(args.name.clone()).into());
    }
    let info = VolumeInfo {
        name: args.name,
        path: path.display().to_string(),
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        println!("Name: {}", info.name);
        println!("Path: {}", info.path);
    }
    Ok(())
}
