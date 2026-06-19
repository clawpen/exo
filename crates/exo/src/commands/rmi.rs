//! `exo rmi` — remove an image and reclaim any layers it alone held.
//!
//! Unlike a flat per-image rootfs, removal here is refcount-aware: the image is
//! unregistered from the layer index, its manifest/rootfs are dropped, and only
//! layers no *other* image still references are pruned from the shared store.

use exo_image::{ImageReference, ImageStore, LayerStore, DEFAULT_IMAGE_ROOT};
use std::path::PathBuf;

pub struct RmiArgs {
    pub image: String,
}

pub async fn execute(args: RmiArgs) -> anyhow::Result<()> {
    let reference = ImageReference::parse(&args.image)?;
    let root = PathBuf::from(DEFAULT_IMAGE_ROOT);
    let store = ImageStore::new(root.clone());
    let cas = LayerStore::new(root);

    // Drop manifest + composed rootfs.
    store.remove_image(&reference)?;

    // Unregister from the layer index, then prune layers nothing references.
    let was_registered = cas.unregister_image(&reference.to_string())?;
    let (pruned, reclaimed) = cas.prune()?;

    if !was_registered && pruned == 0 {
        println!("Removed {} (no shared layers reclaimed)", reference);
    } else {
        println!(
            "Removed {} — pruned {} orphaned layer(s), reclaimed {} bytes",
            reference, pruned, reclaimed
        );
    }
    Ok(())
}
