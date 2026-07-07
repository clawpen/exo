//! Host diagnostics for Exo.

use serde::Serialize;
use std::path::Path;
use std::process::Command;

pub struct DoctorArgs {
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    platform: String,
    checks: Vec<DoctorCheck>,
    ok: bool,
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    name: &'static str,
    status: CheckStatus,
    detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

pub async fn execute(args: DoctorArgs) -> anyhow::Result<()> {
    let checks = run_checks();
    let ok = checks.iter().all(|c| c.status != CheckStatus::Fail);
    let report = DoctorReport {
        platform: std::env::consts::OS.to_string(),
        checks,
        ok,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("Exo doctor ({})", report.platform);
    for check in &report.checks {
        let marker = match check.status {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
        };
        println!("  [{:<4}] {:<24} {}", marker, check.name, check.detail);
    }
    if report.ok {
        println!("Result: usable");
    } else {
        println!("Result: action required");
    }

    Ok(())
}

fn run_checks() -> Vec<DoctorCheck> {
    let mut checks = vec![
        check_state_dir(),
        check_secrets_dir(),
        check_rust_binary(),
        check_backend(),
    ];
    checks.extend(platform_checks());
    checks
}

fn check_state_dir() -> DoctorCheck {
    match exo_runtime::ContainerManager::new() {
        Ok(manager) => DoctorCheck {
            name: "state-dir",
            status: CheckStatus::Ok,
            detail: manager.state_dir().display().to_string(),
        },
        Err(e) => DoctorCheck {
            name: "state-dir",
            status: CheckStatus::Fail,
            detail: e.to_string(),
        },
    }
}

fn check_secrets_dir() -> DoctorCheck {
    match exo_runtime::SecretStore::new() {
        Ok(store) => DoctorCheck {
            name: "secrets-dir",
            status: CheckStatus::Ok,
            detail: store.dir().display().to_string(),
        },
        Err(e) => DoctorCheck {
            name: "secrets-dir",
            status: CheckStatus::Warn,
            detail: e.to_string(),
        },
    }
}

fn check_rust_binary() -> DoctorCheck {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|e| format!("unknown: {}", e));
    DoctorCheck {
        name: "exo-binary",
        status: CheckStatus::Ok,
        detail: exe,
    }
}

fn check_backend() -> DoctorCheck {
    let detail = if cfg!(target_os = "macos") {
        "native-macos available; linux microVM backend is experimental"
    } else if cfg!(windows) {
        "windows-wsl2 backend"
    } else {
        "linux-runtime backend"
    };
    DoctorCheck {
        name: "backend",
        status: CheckStatus::Ok,
        detail: detail.to_string(),
    }
}

#[cfg(target_os = "macos")]
fn platform_checks() -> Vec<DoctorCheck> {
    vec![
        check_command(
            "system_profiler",
            &["SPDisplaysDataType", "-json"],
            "gpu-detection",
        ),
        check_sandbox(),
        check_virtualization_framework(),
    ]
}

#[cfg(not(target_os = "macos"))]
fn platform_checks() -> Vec<DoctorCheck> {
    vec![]
}

#[cfg(target_os = "macos")]
fn check_command(bin: &str, args: &[&str], name: &'static str) -> DoctorCheck {
    match Command::new(bin).args(args).output() {
        Ok(out) if out.status.success() => DoctorCheck {
            name,
            status: CheckStatus::Ok,
            detail: format!("{} available", bin),
        },
        Ok(out) => DoctorCheck {
            name,
            status: CheckStatus::Warn,
            detail: format!("{} exited {:?}", bin, out.status.code()),
        },
        Err(e) => DoctorCheck {
            name,
            status: CheckStatus::Warn,
            detail: format!("{} unavailable: {}", bin, e),
        },
    }
}

#[cfg(target_os = "macos")]
fn check_sandbox() -> DoctorCheck {
    if !Path::new("/usr/bin/sandbox-exec").exists() {
        return DoctorCheck {
            name: "sandbox-exec",
            status: CheckStatus::Warn,
            detail: "not present; native mode will use env isolation only".to_string(),
        };
    }
    match Command::new("/usr/bin/sandbox-exec")
        .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
        .status()
    {
        Ok(status) if status.success() => DoctorCheck {
            name: "sandbox-exec",
            status: CheckStatus::Ok,
            detail: "available".to_string(),
        },
        Ok(status) => DoctorCheck {
            name: "sandbox-exec",
            status: CheckStatus::Warn,
            detail: format!(
                "preflight exited {:?}; use --sandbox required to fail closed",
                status.code()
            ),
        },
        Err(e) => DoctorCheck {
            name: "sandbox-exec",
            status: CheckStatus::Warn,
            detail: format!("preflight failed: {}", e),
        },
    }
}

#[cfg(target_os = "macos")]
fn check_virtualization_framework() -> DoctorCheck {
    let path = Path::new("/System/Library/Frameworks/Virtualization.framework");
    DoctorCheck {
        name: "virtualization-fw",
        status: if path.exists() {
            CheckStatus::Ok
        } else {
            CheckStatus::Warn
        },
        detail: if path.exists() {
            "present".to_string()
        } else {
            "not present; macOS Linux microVM backend unavailable".to_string()
        },
    }
}
