//! CLI contract tests (agent contract, docs/EXIT_CODES.md).
//!
//! These drive the real `exo` binary and assert the machine-readable
//! surface: exit-code classes, the JSON error envelope on stderr, and
//! schema-1 success payloads. They deliberately use only operations that
//! work without containers or a running backend (absent-container lookups,
//! input validation, empty stores) so they run in any CI lane.

use std::process::Command;

const EXO: &str = env!("CARGO_BIN_EXE_exo");

fn run_exo(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(EXO)
        .args(args)
        .output()
        .expect("spawn exo binary");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Parse the single JSON envelope line from stderr.
fn envelope(stderr: &str) -> serde_json::Value {
    let line = stderr
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON envelope on stderr:\n{stderr}"));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("envelope not JSON: {e}\n{line}"))
}

fn ghost() -> String {
    format!("a5-ghost-{}", std::process::id())
}

#[test]
fn exec_without_command_is_invalid_input() {
    let (code, _, stderr) = run_exo(&["--json", "exec", "somecontainer"]);
    assert_eq!(code, 5, "exit code");
    let env = envelope(&stderr);
    assert_eq!(env["schema"], 1);
    assert_eq!(env["error"]["code"], "INVALID_INPUT");
    assert_eq!(env["error"]["retryable"], false);
}

#[test]
fn unknown_backend_is_invalid_input() {
    let (code, _, _) = run_exo(&["--json", "stop", "x", "--backend", "bogus"]);
    #[cfg(target_os = "macos")]
    assert_eq!(code, 5);
    #[cfg(not(target_os = "macos"))]
    let _ = code; // backend selection differs per platform; covered by macOS lane
}

#[cfg(not(windows))]
mod lifecycle {
    use super::*;

    /// Backend flag that resolves locally without a VM/daemon: native on
    /// macOS, the direct manager path on Linux.
    #[cfg(target_os = "macos")]
    const LOCAL_BACKEND: &[&str] = &["--backend", "native"];
    #[cfg(target_os = "linux")]
    const LOCAL_BACKEND: &[&str] = &[];

    #[test]
    fn stop_absent_is_typed_not_found() {
        let ghost = ghost();
        let mut args = vec!["--json", "stop", &ghost];
        args.extend_from_slice(LOCAL_BACKEND);
        let (code, _, stderr) = run_exo(&args);
        assert_eq!(code, 2, "stderr: {stderr}");
        assert_eq!(envelope(&stderr)["error"]["code"], "CONTAINER_NOT_FOUND");
    }

    #[test]
    fn start_absent_is_typed_not_found() {
        let ghost = ghost();
        let mut args = vec!["--json", "start", &ghost];
        args.extend_from_slice(LOCAL_BACKEND);
        let (code, _, stderr) = run_exo(&args);
        assert_eq!(code, 2, "stderr: {stderr}");
        assert_eq!(envelope(&stderr)["error"]["code"], "CONTAINER_NOT_FOUND");
    }

    #[test]
    fn remove_absent_is_typed_not_found() {
        let ghost = ghost();
        let mut args = vec!["--json", "rm", &ghost];
        args.extend_from_slice(LOCAL_BACKEND);
        let (code, _, stderr) = run_exo(&args);
        assert_eq!(code, 2, "stderr: {stderr}");
        assert_eq!(envelope(&stderr)["error"]["code"], "CONTAINER_NOT_FOUND");
    }

    #[test]
    fn absent_failures_are_not_retryable() {
        let ghost = ghost();
        let mut args = vec!["--json", "stop", &ghost];
        args.extend_from_slice(LOCAL_BACKEND);
        let (_, _, stderr) = run_exo(&args);
        assert_eq!(envelope(&stderr)["error"]["retryable"], false);
    }
}

#[test]
fn images_json_payload_carries_schema() {
    let (code, stdout, _) = run_exo(&["images", "--json"]);
    assert_eq!(code, 0);
    let payload: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("images --json emits JSON");
    assert_eq!(payload["schema"], 1);
    assert!(payload["images"].is_array());
}

#[test]
fn secret_remove_absent_is_typed_not_found() {
    let ghost = ghost();
    let (code, _, stderr) = run_exo(&["--json", "secret", "remove", &ghost]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert_eq!(envelope(&stderr)["error"]["code"], "SECRET_NOT_FOUND");
}

#[test]
fn volume_remove_absent_is_typed_not_found() {
    let ghost = ghost();
    let (code, _, stderr) = run_exo(&["--json", "volume", "rm", &ghost]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert_eq!(envelope(&stderr)["error"]["code"], "VOLUME_NOT_FOUND");
}

#[test]
fn volume_inspect_absent_is_typed_not_found() {
    let ghost = ghost();
    let (code, _, stderr) = run_exo(&["--json", "volume", "inspect", &ghost]);
    assert_eq!(code, 2, "stderr: {stderr}");
    assert_eq!(envelope(&stderr)["error"]["code"], "VOLUME_NOT_FOUND");
}

#[test]
fn pull_invalid_reference_is_invalid_input() {
    let (code, _, stderr) = run_exo(&["--json", "pull", "INVALID@@REF/::"]);
    assert_eq!(code, 5, "stderr: {stderr}");
    assert_eq!(envelope(&stderr)["error"]["code"], "INVALID_INPUT");
}

#[test]
fn human_mode_has_no_json_on_stderr() {
    let (code, _, stderr) = run_exo(&["exec", "somecontainer"]);
    assert_eq!(code, 5);
    assert!(
        !stderr.trim_start().starts_with('{'),
        "human mode must not emit the envelope: {stderr}"
    );
    assert!(stderr.contains("Error:"));
}
