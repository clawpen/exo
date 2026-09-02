//! S1 executable conformance suite.
//!
//! Ports `test.sh` (smoke/isolation/features/edge/integration) to Rust,
//! driving the real `exo` binary and asserting on **JSON payloads + exit
//! codes**, never stdout prose. Adds a sixth category — **containment** —
//! which probes the sandbox boundary the way a hostile in-container workload
//! (e.g. LLM-generated code) would. All probes are non-destructive.
//!
//! Backend selection: `EXO_CONFORMANCE_BACKEND` (e.g. "linux", "native").
//! Default: `linux` on macOS (the vm-mac microVM, our lead backend), the
//! direct runtime on Linux. Tests SKIP (with a printed reason) when the
//! backend is unavailable, and known isolation gaps skip naming the B-track
//! item that closes them — flip the caps table when the fix lands.
//!
//! Run: `cargo test -p exo --test conformance` (tests self-serialize — one
//! microVM means one serial guest-RPC channel).

use std::process::Command;
use std::sync::OnceLock;

const EXO: &str = env!("CARGO_BIN_EXE_exo");

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Backend name passed as `--backend <name>`, or empty for the platform
/// default (direct runtime on Linux).
fn backend() -> &'static str {
    static B: OnceLock<String> = OnceLock::new();
    B.get_or_init(|| {
        std::env::var("EXO_CONFORMANCE_BACKEND").unwrap_or_else(|_| {
            if cfg!(target_os = "macos") {
                "linux".to_string() // vm-mac microVM
            } else {
                String::new() // direct rootless runtime
            }
        })
    })
}

fn backend_args() -> Vec<&'static str> {
    match backend() {
        "" => vec![],
        b => vec!["--backend", Box::leak(b.to_string().into_boxed_str())],
    }
}

fn exo(args: &[&str]) -> (i32, String, String) {
    let mut full: Vec<&str> = Vec::new();
    full.extend_from_slice(args);
    let out = Command::new(EXO)
        .args(&full)
        .output()
        .expect("spawn exo binary");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Run with backend args inserted right after the subcommand token. They
/// must NOT go at the end: `run`/`exec` use `trailing_var_arg`, so a
/// trailing `--backend linux` is swallowed into the container's argv
/// (`sleep 60 --backend linux` dies instantly — that was a real harness bug).
fn exob(args: &[&str]) -> (i32, String, String) {
    let bargs = backend_args();
    if bargs.is_empty() {
        return exo(args);
    }
    let mut full: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        full.push(args[i]); // leading globals like --json
        i += 1;
    }
    if i < args.len() {
        full.push(args[i]); // the subcommand
        i += 1;
    }
    full.extend(bargs);
    full.extend_from_slice(&args[i..]);
    exo(&full)
}

/// Last JSON object/array line on stdout (container output may precede it).
fn payload(stdout: &str) -> serde_json::Value {
    let line = stdout
        .lines()
        .rev()
        .find(|l| {
            let t = l.trim_start();
            t.starts_with('{') || t.starts_with('[')
        })
        .unwrap_or_else(|| panic!("no JSON payload on stdout:\n{stdout}"));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("payload not JSON: {e}\n{line}"))
}

fn envelope(stderr: &str) -> serde_json::Value {
    let line = stderr
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON envelope on stderr:\n{stderr}"));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("envelope not JSON: {e}\n{line}"))
}

/// Unique container name per test + process.
fn cname(tag: &str) -> String {
    format!("conf-{tag}-{}", std::process::id())
}

/// Best-effort `rm -f` on drop.
struct Guard(String);
impl Guard {
    fn new(name: &str) -> Self {
        Guard(name.to_string())
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = exob(&["rm", "-f", &self.0]);
    }
}

/// Probe: can we run a trivial container at all? Cached for the suite run.
/// This also covers image presence (run auto-prepares the image).
fn backend_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        let (code, _, err) = exob(&["run", "--rm", "alpine", "echo", "conformance-probe"]);
        if code != 0 {
            eprintln!("conformance: backend probe failed (exit {code}): {err}");
        }
        code == 0
    })
}

macro_rules! require_backend {
    () => {
        if !backend_available() {
            eprintln!("SKIP: backend unavailable");
            return;
        }
    };
}

/// Tests share one backend (one microVM, one serial guest-RPC channel), so
/// they must not run concurrently: parallel exec polls starve container
/// starts. Hold this lock for the whole test body.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static M: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(())).lock().unwrap()
}

// ---------------------------------------------------------------------------
// Per-backend capability/expectation table.
//
// Gap(reason) = documented deviation, skipped with the B-track item that
// closes it. When the fix lands, flip to Enforced and the probe starts
// enforcing the boundary.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Support {
    Enforced,
    Gap(&'static str),
}

struct Caps {
    /// PID/user namespaces: container can't see other processes, mount escapes denied.
    namespaces: Support,
    /// --memory/--cpu enforced via cgroups.
    resource_limits: Support,
    /// -v host bind mounts.
    bind_mounts: Support,
    /// -p host:guest port mapping.
    ports: Support,
    /// stdin streamed into attach-mode runs (-i).
    stdio: Support,
}

fn caps() -> Caps {
    let vm_mac = cfg!(target_os = "macos") && backend() == "linux";
    if vm_mac {
        Caps {
            namespaces: Support::Gap("B-track: guest PID/user namespaces (overlayfs+mount/UTS/IPC only)"),
            resource_limits: Support::Gap("B1: vm-mac resource limits (BACKEND_UNSUPPORTED today)"),
            bind_mounts: Support::Gap("B2: vm-mac host bind mounts (BACKEND_UNSUPPORTED today)"),
            ports: Support::Enforced, // host loopback tunnels (2e8fe6a)
            stdio: Support::Gap("B-track: stdin streaming to guest (-i attaches with EOF)"),
        }
    } else {
        Caps {
            namespaces: Support::Enforced,
            resource_limits: Support::Enforced,
            bind_mounts: Support::Enforced,
            ports: Support::Enforced,
            stdio: Support::Enforced,
        }
    }
}

macro_rules! require_support {
    ($s:expr) => {
        match $s {
            Support::Enforced => {}
            Support::Gap(reason) => {
                eprintln!("SKIP (known gap): {reason}");
                return;
            }
        }
    };
}

// ===========================================================================
// SMOKE (port of test.sh category 1)
// ===========================================================================

mod smoke {
    use super::*;

    #[test]
    fn run_produces_output() {
        require_backend!();
        let _s = serial();
        let (code, stdout, _) = exob(&["run", "--rm", "alpine", "echo", "Hello from Exo"]);
        assert_eq!(code, 0);
        assert!(stdout.contains("Hello from Exo"), "stdout: {stdout}");
    }

    #[test]
    fn detached_stop_remove_lifecycle() {
        require_backend!();
        let _s = serial();
        let n = cname("lifecycle");
        let _g = Guard::new(&n);

        let (code, stdout, _) = exob(&["--json", "run", "-d", "--name", &n, "alpine", "sleep", "60"]);
        assert_eq!(code, 0);
        let p = payload(&stdout);
        assert_eq!(p["schema"], 1);
        assert_eq!(p["detached"], true);
        assert_eq!(p["name"], n);

        let (code, stdout, _) = exob(&["--json", "stop", &n]);
        assert_eq!(code, 0);
        assert_eq!(payload(&stdout)["status"], "stopped");

        let (code, stdout, _) = exob(&["--json", "rm", &n]);
        assert_eq!(code, 0);
        assert_eq!(payload(&stdout)["status"], "removed");

        // Absent after removal: typed not-found.
        let (code, _, stderr) = exob(&["--json", "stop", &n]);
        assert_eq!(code, 2, "stderr: {stderr}");
        assert_eq!(envelope(&stderr)["error"]["code"], "CONTAINER_NOT_FOUND");
    }

    #[test]
    fn auto_remove_leaves_nothing() {
        require_backend!();
        let _s = serial();
        let n = cname("autorm");
        let (code, _, _) = exob(&["run", "--rm", "--name", &n, "alpine", "true"]);
        assert_eq!(code, 0);
        let (_, stdout, _) = exob(&["--json", "list", "-a"]);
        assert!(
            !stdout.contains(&n),
            "--rm container still listed: {stdout}"
        );
    }

    #[test]
    fn list_json_shows_container() {
        require_backend!();
        let _s = serial();
        let n = cname("list");
        let _g = Guard::new(&n);
        let (code, _, _) = exob(&["run", "-d", "--name", &n, "alpine", "sleep", "60"]);
        assert_eq!(code, 0);
        let (code, stdout, _) = exob(&["--json", "list", "-a"]);
        assert_eq!(code, 0);
        assert!(stdout.contains(&n), "container not in list: {stdout}");
    }

    #[test]
    fn logs_return_container_output() {
        require_backend!();
        let _s = serial();
        let n = cname("logs");
        let _g = Guard::new(&n);
        let marker = format!("marker-{}", std::process::id());
        let (code, _, _) = exob(&["run", "--name", &n, "alpine", "echo", &marker]);
        assert_eq!(code, 0);
        let (code, stdout, _) = exob(&["--json", "logs", &n]);
        assert_eq!(code, 0);
        let p = payload(&stdout);
        assert!(
            p["content"].as_str().unwrap_or("").contains(&marker),
            "logs missing marker: {p}"
        );
    }

    #[test]
    fn exec_reports_exit_code() {
        require_backend!();
        let _s = serial();
        let n = cname("exec");
        let _g = Guard::new(&n);
        let (code, _, _) = exob(&["run", "-d", "--name", &n, "alpine", "sleep", "60"]);
        assert_eq!(code, 0);

        // Success payload carries the exit code as data.
        let (code, stdout, _) = exob(&["--json", "exec", &n, "true"]);
        assert_eq!(code, 0);
        assert_eq!(payload(&stdout)["exit_code"], 0);

        // Non-zero propagates as the process exit code (A2 passthrough).
        let (code, _, stderr) = exob(&["--json", "exec", &n, "sh", "-c", "exit 7"]);
        assert_eq!(code, 7, "stderr: {stderr}");
        assert_eq!(envelope(&stderr)["error"]["code"], "CONTAINER_EXITED");
    }
}

// ===========================================================================
// EDGE (port of test.sh category 4)
// ===========================================================================

mod edge {
    use super::*;

    #[test]
    fn name_conflict_is_typed_conflict() {
        require_backend!();
        let _s = serial();
        let n = cname("conflict");
        let _g = Guard::new(&n);
        let (code, _, _) = exob(&["run", "-d", "--name", &n, "alpine", "sleep", "60"]);
        assert_eq!(code, 0);
        let (code, _, stderr) = exob(&["--json", "run", "-d", "--name", &n, "alpine", "sleep", "60"]);
        assert_eq!(code, 3, "stderr: {stderr}");
        assert_eq!(envelope(&stderr)["error"]["code"], "CONTAINER_ALREADY_EXISTS");
    }

    #[test]
    fn invalid_image_is_typed_not_found() {
        require_backend!();
        let _s = serial();
        let (code, _, stderr) = exob(&[
            "--json", "run", "--rm", "nonexistent/image:xyzzy", "echo", "x",
        ]);
        assert!(code == 2 || code == 4, "expected 2 or 4, got {code}: {stderr}");
        let env = envelope(&stderr);
        let c = env["error"]["code"].as_str().unwrap();
        assert!(
            c == "IMAGE_NOT_FOUND" || c.starts_with("REGISTRY") || c == "BACKEND_UNAVAILABLE",
            "unexpected code {c}"
        );
    }

    #[test]
    fn concurrent_creates_all_succeed() {
        require_backend!();
        let _s = serial();
        let handles: Vec<_> = (0..5)
            .map(|i| {
                let n = cname(&format!("conc-{i}"));
                std::thread::spawn(move || {
                    let (code, _, err) =
                        exob(&["run", "-d", "--name", &n, "alpine", "sleep", "30"]);
                    (n, code, err)
                })
            })
            .collect();
        let mut guards = Vec::new();
        for h in handles {
            let (n, code, err) = h.join().unwrap();
            assert_eq!(code, 0, "concurrent create failed: {err}");
            guards.push(Guard::new(&n));
        }
        let (_, stdout, _) = exob(&["--json", "list", "-a"]);
        for g in &guards {
            assert!(stdout.contains(&g.0), "missing {} in list", g.0);
        }
    }

    #[test]
    fn crashed_container_is_tracked_and_code_passes_through() {
        require_backend!();
        let _s = serial();
        let n = cname("crash");
        let _g = Guard::new(&n);
        let (code, _, stderr) = exob(&["--json", "run", "--name", &n, "alpine", "sh", "-c", "exit 137"]);
        assert_eq!(code, 137, "stderr: {stderr}");
        assert_eq!(envelope(&stderr)["error"]["code"], "CONTAINER_EXITED");
        let (_, stdout, _) = exob(&["--json", "list", "-a"]);
        assert!(stdout.contains(&n), "crashed container not tracked: {stdout}");
    }
}

// ===========================================================================
// CONTAINMENT — escape probes against the sandbox boundary.
//
// Threat model: the workload inside the container is hostile (LLM-generated
// code, a curious student). Probes are strictly non-destructive: reads and
// expected-to-be-denied attempts only. Known gaps skip with the B-track item
// that closes them.
// ===========================================================================

mod containment {
    use super::*;

    /// A file written on the host must not be visible inside a container.
    #[test]
    fn host_filesystem_invisible() {
        require_backend!();
        let _s = serial();
        let secret = format!("HOST_SECRET_{}", std::process::id());
        let path = std::env::temp_dir().join(format!("exo-escape-probe-{}", std::process::id()));
        std::fs::write(&path, &secret).unwrap();
        let probe = path.to_string_lossy().to_string();

        let n = cname("fs");
        let _g = Guard::new(&n);
        let (code, _, _) = exob(&["run", "-d", "--name", &n, "alpine", "sleep", "60"]);
        assert_eq!(code, 0);

        let (_, stdout, _) = exob(&["exec", &n, "sh", "-c", &format!("cat {probe} 2>/dev/null || echo DENIED")]);
        let _ = std::fs::remove_file(&path);
        assert!(
            !stdout.contains(&secret),
            "ESCAPE: container read a host file: {stdout}"
        );
    }

    /// Container must not see processes outside its namespace.
    #[test]
    fn pid_namespace_isolated() {
        require_backend!();
        let _s = serial();
        require_support!(caps().namespaces);

        let n = cname("pid");
        let _g = Guard::new(&n);
        let (code, _, _) = exob(&["run", "-d", "--name", &n, "alpine", "sleep", "60"]);
        assert_eq!(code, 0);

        let (code, stdout, _) = exob(&["exec", &n, "ps", "aux"]);
        assert_eq!(code, 0);
        let lines = stdout.lines().count();
        assert!(
            lines <= 10,
            "container sees {lines} processes — namespace leak?\n{stdout}"
        );
    }

    /// /dev must be curated: no host device nodes, no /dev/mem.
    #[test]
    fn device_access_curated() {
        require_backend!();
        let _s = serial();
        let n = cname("dev");
        let _g = Guard::new(&n);
        let (code, _, _) = exob(&["run", "-d", "--name", &n, "alpine", "sleep", "60"]);
        assert_eq!(code, 0);

        let (_, stdout, _) = exob(&["exec", &n, "ls", "/dev"]);
        assert!(
            !stdout.contains("mem"),
            "ESCAPE: /dev/mem visible in container: {stdout}"
        );
        for node in ["sda", "vda", "nvme", "kvm", "dri"] {
            assert!(
                !stdout.lines().any(|l| l.trim() == node),
                "ESCAPE: /dev/{node} visible in container: {stdout}"
            );
        }
    }

    /// Capability escape: mounting inside the container must be denied.
    /// (A successful mount as container-root = classic chroot escape vector.)
    #[test]
    fn mount_escape_denied() {
        require_backend!();
        let _s = serial();
        require_support!(caps().namespaces);

        let n = cname("mount");
        let _g = Guard::new(&n);
        let (code, _, _) = exob(&["run", "-d", "--name", &n, "alpine", "sleep", "60"]);
        assert_eq!(code, 0);

        let (code, stdout, _) = exob(&[
            "exec", &n, "sh", "-c",
            "mkdir -p /tmp/m && mount -t tmpfs none /tmp/m && echo MOUNTED || echo DENIED",
        ]);
        assert!(
            !stdout.contains("MOUNTED"),
            "ESCAPE: in-container mount succeeded (exit {code}): {stdout}"
        );
    }

    /// Memory limit must actually bound the workload.
    #[test]
    fn memory_limit_enforced() {
        require_backend!();
        let _s = serial();
        match caps().resource_limits {
            Support::Gap(_) => {
                // Limits must at least be *rejected* typed, not silently ignored (A6).
                let (code, _, stderr) = exob(&[
                    "--json", "run", "--rm", "--memory", "64M", "alpine", "true",
                ]);
                assert_eq!(code, 4, "stderr: {stderr}");
                assert_eq!(envelope(&stderr)["error"]["code"], "BACKEND_UNSUPPORTED");
                return;
            }
            Support::Enforced => {}
        }

        let n = cname("oom");
        let _g = Guard::new(&n);
        // Try to allocate 256M under a 64M limit; the workload must die.
        let (code, _, _) = exob(&[
            "run", "--name", &n, "--memory", "64M",
            "alpine", "sh", "-c",
            "head -c 268435456 /dev/zero | tail -c 268435456 > /dev/null; echo SURVIVED",
        ]);
        assert_ne!(code, 0, "workload survived a 4x-over-limit allocation");
    }
}

// ===========================================================================
// FEATURES (port of test.sh category 3)
// ===========================================================================

mod features {
    use super::*;

    #[test]
    fn env_vars_passed() {
        require_backend!();
        let _s = serial();
        let (code, stdout, _) = exob(&[
            "run", "--rm", "-e", "MY_TEST_VAR=hello", "-e", "ANOTHER_VAR=world",
            "alpine", "sh", "-c", "echo \"MY_TEST_VAR=$MY_TEST_VAR $ANOTHER_VAR\"",
        ]);
        assert_eq!(code, 0);
        assert!(
            stdout.contains("MY_TEST_VAR=hello world"),
            "env not passed: {stdout}"
        );
    }

    #[test]
    fn volume_mount_read_write() {
        require_backend!();
        let _s = serial();
        match caps().bind_mounts {
            Support::Gap(_) => {
                // Must be a typed rejection, never silent success (A6).
                let (code, _, stderr) = exob(&[
                    "--json", "run", "--rm", "-v", "/tmp:/data", "alpine", "true",
                ]);
                assert_eq!(code, 4, "stderr: {stderr}");
                assert_eq!(envelope(&stderr)["error"]["code"], "BACKEND_UNSUPPORTED");
                return;
            }
            Support::Enforced => {}
        }

        let dir = std::env::temp_dir().join(format!("exo-vol-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("host-file.txt"), "host-data").unwrap();
        let mount = format!("{}:/data", dir.to_string_lossy());

        let n = cname("vol");
        let _g = Guard::new(&n);
        let (code, _, err) = exob(&["run", "-d", "--name", &n, "-v", &mount, "alpine", "sleep", "60"]);
        assert_eq!(code, 0, "run -v failed: {err}");

        let (_, stdout, _) = exob(&["exec", &n, "cat", "/data/host-file.txt"]);
        assert!(stdout.contains("host-data"), "volume read failed: {stdout}");

        let (code, _, _) = exob(&["exec", &n, "sh", "-c", "echo container-data > /data/container-file.txt"]);
        assert_eq!(code, 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("container-file.txt")).unwrap_or_default(),
            "container-data\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn port_mapping_binds_host_port() {
        require_backend!();
        let _s = serial();
        require_support!(caps().ports);

        let port = 18080 + (std::process::id() % 500);
        let mapping = format!("{port}:80");
        let n = cname("port");
        let _g = Guard::new(&n);
        let (code, _, err) = exob(&["run", "-d", "--name", &n, "-p", &mapping, "alpine", "sleep", "60"]);
        assert_eq!(code, 0, "run -p failed: {err}");

        // Host side must at least bind/accept (guest end has no listener).
        let mut bound = false;
        for _ in 0..20 {
            if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
                bound = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        assert!(bound, "host port {port} never accepted a connection");
    }

    #[test]
    fn gpu_passthrough_when_present() {
        require_backend!();
        let _s = serial();
        let has_gpu = std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v nvidia-smi || command -v rocm-smi")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !has_gpu {
            eprintln!("SKIP: no GPU on this host");
            return;
        }
        let (code, stdout, _) = exob(&[
            "run", "--rm", "--gpu", "--gpu-type", "auto", "alpine", "true",
        ]);
        assert_eq!(code, 0, "gpu run failed: {stdout}");
    }
}

// ===========================================================================
// INTEGRATION (port of test.sh category 5)
// ===========================================================================

mod integration {
    use super::*;

    #[test]
    fn stdio_roundtrip() {
        require_backend!();
        let _s = serial();
        require_support!(caps().stdio);
        let mut full = vec!["run"];
        full.extend(backend_args());
        full.extend(["--rm", "-i", "alpine", "sh", "-c", "read m; echo \"PONG: $m\""]);
        let mut child = Command::new(EXO)
            .args(&full)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn exo");
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"PING\n")
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("PONG: PING"),
            "stdio round-trip failed: {stdout}"
        );
    }

    #[test]
    fn json_channel_passthrough() {
        require_backend!();
        let _s = serial();
        require_support!(caps().stdio);
        let mut full = vec!["run"];
        full.extend(backend_args());
        full.extend(["--rm", "-i", "alpine", "sh", "-c", "head -1"]);
        let mut child = Command::new(EXO)
            .args(&full)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn exo");
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"{\"type\":\"test\",\"data\":\"hello\"}\n")
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("\"type\":\"test\""),
            "JSON channel failed: {stdout}"
        );
    }

    #[test]
    fn process_spawn_in_container() {
        require_backend!();
        let _s = serial();
        let n = cname("spawn");
        let _g = Guard::new(&n);
        let (code, _, _) = exob(&["run", "-d", "--name", &n, "alpine", "sleep", "60"]);
        assert_eq!(code, 0);
        let (code, _, err) = exob(&["exec", &n, "sh", "-c", "sleep 5 &"]);
        assert_eq!(code, 0, "process spawn failed: {err}");
    }
}
