//! CLI command implementations

pub mod backend;
pub mod daemon;
pub mod doctor;
pub mod events;
pub mod exec;
pub mod gpu;
pub mod images;
pub mod import;
pub mod list;
pub mod logs;
pub mod pull;
pub mod remove;
pub mod run;
pub mod secret;
pub mod start;
pub mod stop;
pub mod vm;
pub mod volume;

#[cfg(target_os = "macos")]
pub mod mac;

/// Emit a lifecycle outcome: one JSON status line in `--json` mode, a human
/// message otherwise. Status strings are part of the agent contract —
/// additive-only: "stopped", "not_running", "started", "already_running",
/// "removed".
pub fn emit_lifecycle_status(container: &str, status: &str, json: bool) {
    if json {
        let mut fields = serde_json::Map::new();
        fields.insert("container".to_string(), container.into());
        fields.insert("status".to_string(), status.into());
        print_json(fields);
    } else {
        match status {
            "stopped" => println!("Container {} stopped", container),
            "not_running" => println!("Container {} is not running", container),
            "started" => println!("Container {} started", container),
            "already_running" => println!("Container {} is already running", container),
            "removed" => println!("Container {} removed", container),
            other => println!("Container {} {}", container, other),
        }
    }
}

/// Print one JSON line to stdout for `--json` success output.
///
/// Every payload carries `"schema": 1` per the agent contract
/// (docs/EXIT_CODES.md); builders here add it so call sites can't forget.
pub fn print_json(mut fields: serde_json::Map<String, serde_json::Value>) {
    fields.insert("schema".to_string(), serde_json::Value::from(1));
    println!(
        "{}",
        serde_json::to_string(&serde_json::Value::Object(fields))
            .expect("JSON serialization of CLI output")
    );
}
