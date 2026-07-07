//! Image pulling from OCI registries.
//!
//! This module provides a high-level interface for pulling images.

use anyhow::Result;
use tracing::info;

use crate::{ImageReference, ImageStore, RegistryClient};

/// Image puller - downloads images from registries.
pub struct ImagePuller {
    client: RegistryClient,
}

impl ImagePuller {
    /// Create a new image puller.
    pub fn new(store: ImageStore) -> Result<Self> {
        let client = RegistryClient::new(store)?;
        Ok(Self { client })
    }

    /// Pull an image from a registry.
    pub async fn pull(&mut self, reference: &ImageReference) -> Result<()> {
        info!("Pulling image {}", reference);
        self.client.pull(reference).await?;
        info!("Successfully pulled {}", reference);
        Ok(())
    }
}
