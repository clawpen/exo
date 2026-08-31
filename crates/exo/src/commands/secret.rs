//! Secret management commands.

use exo_runtime::SecretStore;
use std::io::{IsTerminal, Read};

pub struct SecretSetArgs {
    pub name: String,
    pub value: Option<String>,
}

pub struct SecretRemoveArgs {
    pub name: String,
}

pub struct SecretListArgs {
    pub json: bool,
}

pub async fn set(args: SecretSetArgs) -> anyhow::Result<()> {
    let value = resolve_secret_value(&args.name, args.value)?;
    SecretStore::new()?.set(&args.name, value.trim_end_matches('\n'))?;
    println!("Secret {} stored", args.name);
    Ok(())
}

pub async fn remove(args: SecretRemoveArgs) -> anyhow::Result<()> {
    let removed = SecretStore::new()?.remove(&args.name)?;
    if removed {
        println!("Secret {} removed", args.name);
        Ok(())
    } else {
        // Existence is validated on removal (agent contract A5): a typo'd
        // name is a failure, not a silent no-op.
        Err(exo_runtime::ExoError::SecretNotFound(args.name).into())
    }
}

pub async fn list(args: SecretListArgs) -> anyhow::Result<()> {
    let names = SecretStore::new()?.list()?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "secrets": names }))?
        );
        return Ok(());
    }
    if names.is_empty() {
        println!("No secrets stored.");
    } else {
        for name in names {
            println!("{}", name);
        }
    }
    Ok(())
}

fn resolve_secret_value(name: &str, explicit: Option<String>) -> anyhow::Result<String> {
    if let Some(value) = explicit {
        return Ok(value);
    }
    if let Ok(value) = std::env::var(name) {
        return Ok(value);
    }
    if std::io::stdin().is_terminal() {
        return Err(exo_runtime::ExoError::InvalidInput(format!(
            "no value provided for secret '{name}'; pass --value, set ${name}, or pipe the value on stdin"
        ))
        .into());
    }

    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    if value.is_empty() {
        return Err(exo_runtime::ExoError::InvalidInput(format!(
            "stdin did not contain a value for secret '{name}'"
        ))
        .into());
    }
    Ok(value)
}
