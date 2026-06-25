//! Push image command — publish a locally-stored image to its registry.

use exo_image::{ImageReference, ImageStore, RegistryClient};

pub struct PushArgs {
    pub image: String,
    pub sign: bool,
    pub cosign_key: Option<String>,
}

pub async fn execute(args: PushArgs) -> anyhow::Result<()> {
    let image_ref = ImageReference::parse(&args.image)?;
    println!("Pushing {} to {}", image_ref, image_ref.registry);

    let store = ImageStore::default();
    if !store.has_image(&image_ref) {
        anyhow::bail!("{} not found locally; pull or build it first", image_ref);
    }

    let mut client = RegistryClient::new(store)?;
    client.push(&image_ref).await?;

    println!("Successfully pushed {}", args.image);

    // Optional cosign signing after push.
    if args.sign || should_sign_by_env() {
        let key_path = exo_runtime::resolve_key_path(args.cosign_key.as_deref());
        match exo_runtime::sign_image(&args.image,
            key_path.as_deref(),
        ) {
            Ok(()) => println!("  Signed {} with cosign", args.image),
            Err(e) => {
                anyhow::bail!("Failed to sign image: {}", e);
            }
        }
    }

    Ok(())
}

fn should_sign_by_env() -> bool {
    std::env::var("EXO_SIGN").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}
