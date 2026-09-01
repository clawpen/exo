//! Agent-facing CLI reference generator (A7).
//!
//! `docs/AGENT_CLI.md` is *generated* from the clap definitions in this
//! binary — the derive tree is the single source of truth for commands and
//! flags, so the doc cannot silently drift from the parser. Regenerate after
//! any CLI change with:
//!
//! ```sh
//! exo agent-docs > docs/AGENT_CLI.md
//! ```
//!
//! A CLI contract test (`agent_docs_match_committed_reference`) fails when
//! the committed doc is stale, which is the CI drift check.
//!
//! The JSON success-payload shapes are not derivable from clap; they live in
//! `JSON_SHAPES` below as their own single source of truth (mirrored in the
//! prose contract, `docs/EXIT_CODES.md`).

use clap::{Arg, ArgAction, Command, CommandFactory};

use crate::Cli;

/// Render the full markdown reference.
pub fn render() -> String {
    let mut out = String::new();
    out.push_str(
        "# Exo CLI Agent Reference\n\
         \n\
         > **GENERATED — do not edit by hand.** Regenerate with \
         `exo agent-docs > docs/AGENT_CLI.md`. A contract test fails the \
         build when this file is stale.\n\
         >\n\
         > Source of truth: the clap definitions in `crates/exo/src/main.rs`. \
         The error contract (exit codes, error envelope) lives in \
         `docs/EXIT_CODES.md`.\n\
         \n\
         The primary consumer of this CLI is an AI agent. Everything below is \
         stable within schema 1: commands, flags, JSON payload shapes, exit \
         codes. Additive changes only.\n",
    );

    let root = Cli::command();
    render_global_flags(&mut out, &root);

    out.push_str("\n## Commands\n");
    for sub in visible_subcommands(&root) {
        render_command(&mut out, sub, &format!("exo {}", sub.get_name()), 3);
    }

    render_json_shapes(&mut out);
    render_error_summary(&mut out);
    out
}

/// Global flags are rendered once, from the root command, and filtered out
/// of every subcommand section.
fn render_global_flags(out: &mut String, root: &Command) {
    out.push_str("\n## Global flags\n\n");
    out.push_str("Available on every command (place before the container's argv in `run`/`exec`):\n\n");
    out.push_str("| Flag | Description |\n|---|---|\n");
    for arg in root.get_arguments().filter(|a| a.is_global_set()) {
        out.push_str(&format!(
            "| `{}` | {} |\n",
            flag_display(arg),
            help_text(arg)
        ));
    }
}

fn render_command(out: &mut String, cmd: &Command, path: &str, level: usize) {
    let heading = "#".repeat(level);
    out.push_str(&format!("\n{heading} `{path}`\n\n"));

    if let Some(about) = cmd.get_about() {
        out.push_str(&about.to_string());
        out.push('\n');
    }

    let aliases: Vec<_> = cmd.get_all_aliases().collect();
    if !aliases.is_empty() {
        out.push_str(&format!(
            "\nAliases: {}\n",
            aliases
                .iter()
                .map(|a| format!("`{a}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    out.push_str(&format!("\n**Usage:** `{}{}`\n", path, usage_suffix(cmd)));

    let own_args: Vec<&Arg> = cmd.get_arguments().filter(|a| !a.is_global_set()).collect();
    let positionals: Vec<&&Arg> = own_args.iter().filter(|a| a.is_positional()).collect();
    let options: Vec<&&Arg> = own_args.iter().filter(|a| !a.is_positional()).collect();

    if !positionals.is_empty() {
        out.push_str("\n| Argument | Required | Description |\n|---|---|---|\n");
        for arg in positionals {
            out.push_str(&format!(
                "| `{}` | {} | {} |\n",
                positional_display(arg),
                if arg.is_required_set() { "yes" } else { "no" },
                help_text(arg)
            ));
        }
    }
    if !options.is_empty() {
        out.push_str("\n| Flag | Value | Default | Description |\n|---|---|---|---|\n");
        for arg in options {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                flag_display(arg),
                value_display(arg),
                default_display(arg),
                help_text(arg)
            ));
        }
    }

    for sub in visible_subcommands(cmd) {
        render_command(
            out,
            sub,
            &format!("{path} {}", sub.get_name()),
            level + 1,
        );
    }
}

/// Success payloads per command (schema 1). Single source of truth for the
/// JSON shapes; the prose contract in `docs/EXIT_CODES.md` mirrors this
/// table. Additive-only within a schema version.
const JSON_SHAPES: &[(&str, &str)] = &[
    ("run -d", "{\"schema\":1,\"id\",\"name\",\"detached\":true}"),
    (
        "run (attach)",
        "container output streams raw; exit code carries the result (`CONTAINER_EXITED`)",
    ),
    ("stop", "{\"schema\":1,\"container\",\"status\":\"stopped\"|\"not_running\"}"),
    (
        "start",
        "{\"schema\":1,\"container\",\"status\":\"started\"|\"already_running\"}",
    ),
    ("rm", "{\"schema\":1,\"container\",\"status\":\"removed\"}"),
    ("exec", "{\"schema\":1,\"container\",\"exit_code\"}"),
    ("logs", "{\"schema\":1,\"container\",\"content\"}"),
    ("pull", "{\"schema\":1,\"image\",\"cached\",\"config_digest\",\"layers\"}"),
    ("images", "{\"schema\":1,\"images\":[{\"repository\",\"tag\",\"registry\"}…]}"),
    ("list / ps", "JSON array of container objects"),
    (
        "doctor / events / backend info / gpu list / vm status / secret list / volume ls / volume inspect",
        "per-command objects",
    ),
];

fn render_json_shapes(out: &mut String) {
    out.push_str(
        "\n## JSON success payloads (schema 1)\n\n\
         With `--json`, every success payload carries `\"schema\": 1` on stdout. \
         Shapes per command:\n\n\
         | Command | Payload |\n|---|---|\n",
    );
    for (cmd, shape) in JSON_SHAPES {
        out.push_str(&format!("| `{cmd}` | `{}` |\n", shape.replace('|', "\\|")));
    }
    out.push_str(
        "\nLifecycle `status` strings are additive-only: `stopped`, `not_running`, \
         `started`, `already_running`, `removed`.\n",
    );
}

fn render_error_summary(out: &mut String) {
    out.push_str(
        "\n## Errors\n\n\
         Failures emit a structured envelope on **stderr** and exit with a \
         documented class code (never 1):\n\n\
         ```json\n\
         {\"schema\":1,\"error\":{\"code\":\"CONTAINER_NOT_FOUND\",\"message\":\"container not found: web\",\"retryable\":false}}\n\
         ```\n\n\
         | Exit | Class |\n|---|---|\n\
         | 0 | success |\n\
         | 2 | not found |\n\
         | 3 | conflict / state |\n\
         | 4 | backend / registry |\n\
         | 5 | invalid input |\n\
         | 6 | internal |\n\n\
         Exception: attach-mode `run`/`exec`/`start --attach` exit with the \
         **container's own** exit code; the envelope's `CONTAINER_EXITED` code \
         disambiguates a workload exit from an exo failure.\n\n\
         The full taxonomy, `code` strings, `retryable` semantics, and the \
         idempotency matrix are in `docs/EXIT_CODES.md`.\n",
    );
}

fn visible_subcommands(cmd: &Command) -> impl Iterator<Item = &Command> {
    cmd.get_subcommands().filter(|s| !s.is_hide_set())
}

/// `run` usage: `[OPTIONS] <image> [command]...` — positional args in order,
/// `[OPTIONS]` when the command has any, `<SUBCOMMAND>` when it has any.
fn usage_suffix(cmd: &Command) -> String {
    let mut parts = String::new();
    let has_options = cmd
        .get_arguments()
        .any(|a| !a.is_positional() && !a.is_global_set());
    if has_options {
        parts.push_str(" [OPTIONS]");
    }
    for arg in cmd.get_arguments().filter(|a| a.is_positional()) {
        parts.push(' ');
        parts.push_str(&positional_display(arg));
    }
    if visible_subcommands(cmd).next().is_some() {
        parts.push_str(" <SUBCOMMAND>");
    }
    parts
}

fn positional_display(arg: &Arg) -> String {
    let name = arg.get_id().as_str();
    let variadic = is_variadic(arg);
    let inner = if variadic {
        format!("{name}...")
    } else {
        name.to_string()
    };
    if arg.is_required_set() {
        format!("<{inner}>")
    } else {
        format!("[{inner}]")
    }
}

fn flag_display(arg: &Arg) -> String {
    match (arg.get_short(), arg.get_long()) {
        (Some(s), Some(l)) => format!("-{s}, --{l}"),
        (Some(s), None) => format!("-{s}"),
        (None, Some(l)) => format!("--{l}"),
        (None, None) => arg.get_id().to_string(),
    }
}

fn value_display(arg: &Arg) -> String {
    if is_flag(arg) {
        return "—".to_string();
    }
    let name = arg
        .get_value_names()
        .and_then(|n| n.first().map(|s| s.to_string()))
        .unwrap_or_else(|| "VALUE".to_string());
    if is_variadic(arg) {
        format!("{name} (repeatable)")
    } else {
        name
    }
}

fn default_display(arg: &Arg) -> String {
    let defaults = arg.get_default_values();
    if defaults.is_empty() {
        return "—".to_string();
    }
    defaults
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

fn is_flag(arg: &Arg) -> bool {
    matches!(
        arg.get_action(),
        ArgAction::SetTrue | ArgAction::SetFalse | ArgAction::Count
    )
}

fn is_variadic(arg: &Arg) -> bool {
    arg.get_num_args().is_some_and(|r| r.max_values() > 1)
        || matches!(arg.get_action(), ArgAction::Append)
}

fn help_text(arg: &Arg) -> String {
    arg.get_help()
        .map(|h| h.to_string().replace('|', "\\|"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_top_level_commands() {
        let doc = render();
        for name in [
            "exo run",
            "exo list",
            "exo start",
            "exo stop",
            "exo remove",
            "exo logs",
            "exo exec",
            "exo pull",
            "exo images",
            "exo import",
            "exo secret",
            "exo doctor",
            "exo events",
            "exo daemon",
            "exo backend",
            "exo gpu",
            "exo vm",
            "exo volume",
        ] {
            assert!(doc.contains(&format!("`{name}`")), "missing {name}");
        }
        // Hidden commands stay out of the agent reference.
        assert!(!doc.contains("exo vm serve"));
        // Global flags rendered in their own section, not per-command.
        assert_eq!(doc.matches("| `--json` |").count(), 1);
        // Aliases surface.
        assert!(doc.contains("`ps`"));
        assert!(doc.contains("`rm`"));
        // JSON shapes and error summary present.
        assert!(doc.contains("JSON success payloads"));
        assert!(doc.contains("CONTAINER_EXITED"));
    }
}
