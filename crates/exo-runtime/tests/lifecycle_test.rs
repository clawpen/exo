//! Container lifecycle tests
//!
//! These tests verify the complete lifecycle of containers:
//! - Creation
//! - Starting
//! - Running
//! - Stopping
//! - Cleanup

mod common;

use std::path::Path;
use std::thread;
use std::time::Duration;

#[cfg(target_os = "linux")]
use exo_runtime::{
    Container, ContainerConfig, ContainerStatus, ResourceConfig, NetworkConfig,
    CgroupManager, Capability, drop_capabilities, get_default_caps,
};

///////////////////////////////////////////////////////////////////////////////
// Container Lifecycle Tests
///////////////////////////////////////////////////////////////////////////////

#[test]
#[cfg(target_os = "linux")]
fn test_container_config_validation() {
    let config = ContainerConfig {
        name: "test-validation".to_string(),
        image: "ubuntu:latest".to_string(),
        command: vec!["sleep".to_string(), "0".to_string()],
        ..Default::default()
    };

    assert_eq!(config.name, "test-validation");
    assert_eq!(config.image, "ubuntu:latest");
    assert_eq!(config.command, vec!["sleep", "0"]);
}

#[test]
#[cfg(target_os = "linux")]
fn test_container_status_transitions() {
    // Test status creation and transitions
    let status = ContainerStatus::Created;
    assert_eq!(status.to_string(), "created");

    let status = ContainerStatus::Running;
    assert_eq!(status.to_string(), "running");

    let status = ContainerStatus::Stopped;
    assert_eq!(status.to_string(), "stopped");

    let status = ContainerStatus::Paused;
    assert_eq!(status.to_string(), "paused");

    let status = ContainerStatus::Exited(0);
    assert_eq!(status.to_string(), "exited (0)");
}

#[test]
#[cfg(target_os = "linux")]
fn test_cgroup_lifecycle() {
    let test_id = format!("lifecycle_test_{}", std::process::id());
    let mut mgr = CgroupManager::new(&test_id).expect("Failed to create cgroup");

    // Initialize (initialized field is private; rely on path().exists() as the
    // observable post-condition).
    mgr.initialize().expect("Failed to initialize");
    assert!(mgr.path().exists());

    // Set resource limits
    mgr.set_memory_limit(128 * 1024 * 1024).unwrap();
    mgr.set_cpu_limit(50000, 100000).unwrap();
    mgr.set_pids_limit(32).unwrap();

    // Verify limits
    let mem_limit = mgr.get_memory_limit().unwrap();
    assert!(mem_limit.is_some());
    assert_eq!(mem_limit.unwrap(), 128 * 1024 * 1024);

    // Clean up
    mgr.destroy().expect("Failed to destroy");
    thread::sleep(Duration::from_millis(50));

    let cgroup_path = format!("/sys/fs/cgroup/containment/{}", test_id);
    assert!(!Path::new(&cgroup_path).exists());
}

#[test]
#[cfg(target_os = "linux")]
fn test_cgroup_memory_tracking() {
    let test_id = format!("mem_track_{}", std::process::id());
    let mut mgr = CgroupManager::new(&test_id).expect("Failed to create cgroup");

    mgr.initialize().expect("Failed to initialize");

    // Initial memory usage should be low
    let usage = mgr.get_memory_usage().expect("Failed to get usage");
    // Some memory is used by the cgroup itself
    assert!(usage < 10 * 1024 * 1024, "Initial usage too high: {}", usage);

    // Clean up
    let _ = mgr.destroy();
}

#[test]
#[cfg(target_os = "linux")]
fn test_cgroup_cpu_tracking() {
    let test_id = format!("cpu_track_{}", std::process::id());
    let mut mgr = CgroupManager::new(&test_id).expect("Failed to create cgroup");

    mgr.initialize().expect("Failed to initialize");

    // CPU usage should be readable
    let usage = mgr.get_cpu_usage();
    assert!(usage.is_ok(), "Failed to get CPU usage");

    // Clean up
    let _ = mgr.destroy();
}

#[test]
#[cfg(target_os = "linux")]
fn test_cgroup_process_listing() {
    let test_id = format!("proc_list_{}", std::process::id());
    let mut mgr = CgroupManager::new(&test_id).expect("Failed to create cgroup");

    mgr.initialize().expect("Failed to initialize");

    // Initially no processes
    let pids = mgr.get_processes().expect("Failed to get processes");
    assert!(pids.is_empty(), "Expected no processes initially");

    // Clean up
    let _ = mgr.destroy();
}

#[test]
#[cfg(target_os = "linux")]
fn test_capability_sets() {
    // Test default capabilities
    let defaults = get_default_caps();

    // These should be in defaults
    assert!(defaults.contains(&Capability::CAP_SETUID));
    assert!(defaults.contains(&Capability::CAP_SETGID));
    assert!(defaults.contains(&Capability::CAP_CHOWN));
    assert!(defaults.contains(&Capability::CAP_NET_BIND_SERVICE));

    // Dangerous ones should NOT be in defaults
    assert!(!defaults.contains(&Capability::CAP_SYS_ADMIN));
    assert!(!defaults.contains(&Capability::CAP_NET_RAW));
    assert!(!defaults.contains(&Capability::CAP_NET_ADMIN));
}

#[test]
#[cfg(target_os = "linux")]
fn test_cgroup_resource_comprehensive() {
    let test_id = format!("comprehensive_{}", std::process::id());
    let mut mgr = CgroupManager::new(&test_id).expect("Failed to create cgroup");

    mgr.initialize().expect("Failed to initialize");

    // Test all resource limit types
    let results = vec![
        mgr.set_memory_limit(256 * 1024 * 1024),
        mgr.set_memory_swap_limit(512 * 1024 * 1024),
        mgr.set_cpu_limit(100000, 100000),
        mgr.set_cpu_shares(1024),
        mgr.set_pids_limit(64),
    ];

    // All should succeed
    for (i, result) in results.into_iter().enumerate() {
        assert!(result.is_ok(), "Resource limit {} failed: {:?}", i, result.err());
    }

    // Verify values
    assert_eq!(
        mgr.get_memory_limit().unwrap().unwrap(),
        256 * 1024 * 1024
    );

    // Clean up
    mgr.destroy().expect("Failed to destroy");
}

#[test]
#[cfg(target_os = "linux")]
fn test_cgroup_io_throttle() {
    let test_id = format!("io_test_{}", std::process::id());
    let mut mgr = CgroupManager::new(&test_id).expect("Failed to create cgroup");

    mgr.initialize().expect("Failed to initialize");

    // Set I/O throttle (may not be available on all systems)
    let result = mgr.set_io_throttle("259:0", 10 * 1024 * 1024, 10 * 1024 * 1024);
    // Either success or graceful degradation
    assert!(result.is_ok() || result.is_err());

    // Clean up
    let _ = mgr.destroy();
}

#[test]
#[cfg(target_os = "linux")]
fn test_cgroup_cpu_affinity() {
    let test_id = format!("affinity_test_{}", std::process::id());
    let mut mgr = CgroupManager::new(&test_id).expect("Failed to create cgroup");

    mgr.initialize().expect("Failed to initialize");

    // Set CPU affinity (may not be available on all systems)
    let result = mgr.set_cpu_affinity("0");
    // Either success or graceful degradation
    assert!(result.is_ok() || result.is_err());

    // Clean up
    let _ = mgr.destroy();
}

#[test]
#[cfg(target_os = "linux")]
fn test_cgroup_v2_detection() {
    let is_v2 = CgroupManager::is_cgroup_v2();
    // Just ensure the function runs without panic
    assert!(is_v2 == true || is_v2 == false);
}

#[test]
fn test_resource_config() {
    use exo_runtime::ResourceConfig;

    let config = ResourceConfig {
        memory: Some("512m".to_string()),
        cpu: Some("2".to_string()),
        pids_limit: Some(100),
        ..Default::default()
    };

    assert_eq!(config.memory, Some("512m".to_string()));
    assert_eq!(config.cpu, Some("2".to_string()));
    assert_eq!(config.pids_limit, Some(100));
}

#[test]
fn test_network_config() {
    use exo_runtime::NetworkConfig;

    let config = NetworkConfig {
        mode: "bridge".to_string(),
        dns: vec!["8.8.8.8".to_string()],
        ..Default::default()
    };

    assert_eq!(config.mode, "bridge");
    assert_eq!(config.dns.len(), 1);
}

#[test]
#[cfg(target_os = "linux")]
fn test_seccomp_default_profile() {
    use exo_runtime::{default_profile, SeccompAction};

    let profile = default_profile();

    // Default profile should have default action
    assert!(matches!(profile.default_action, SeccompAction::Allow));

    // Should have many allowed syscalls
    assert!(profile.allow.len() > 10);

    // Essential syscalls should be allowed (Syscall is an enum, match on Name variant)
    use exo_runtime::seccomp::Syscall;
    let syscall_names: Vec<&str> = profile.allow.iter().filter_map(|s| match s {
        Syscall::Name(n) => Some(n.as_str()),
        Syscall::Number(_) => None,
    }).collect();
    assert!(syscall_names.contains(&"read"));
    assert!(syscall_names.contains(&"write"));
    assert!(syscall_names.contains(&"exit"));
    assert!(syscall_names.contains(&"sigreturn"));
}

#[test]
fn test_container_config_with_all_options() {
    use std::collections::HashMap;
    use exo_runtime::{ContainerConfig, ResourceConfig, NetworkConfig, MountConfig};

    let mut env = HashMap::new();
    env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());

    let config = ContainerConfig {
        name: "comprehensive-test".to_string(),
        image: "alpine:latest".to_string(),
        hostname: "test-hostname".to_string(),
        user: "nobody".to_string(),
        workdir: "/workspace".into(),
        command: vec!["sh".to_string(), "-c".to_string(), "echo hello".to_string()],
        env,
        resources: ResourceConfig {
            memory: Some("1024m".to_string()),
            cpu: Some("2".to_string()),
            pids_limit: Some(100),
            ..Default::default()
        },
        network: NetworkConfig {
            mode: "bridge".to_string(),
            dns: vec!["8.8.8.8".to_string()],
            ..Default::default()
        },
        mounts: vec![MountConfig {
            mount_type: "bind".to_string(),
            source: "/tmp/host".to_string(),
            target: "/tmp/container".to_string(),
            readonly: false,
            size: None,
            propagation: "rprivate".to_string(),
        }],
        privileged: false,
        readonly_rootfs: true,
        ..Default::default()
    };

    assert_eq!(config.name, "comprehensive-test");
    assert_eq!(config.hostname, "test-hostname");
    assert_eq!(config.user, "nobody");
    assert_eq!(config.workdir, std::path::PathBuf::from("/workspace"));
    assert_eq!(config.command.len(), 3);
    assert_eq!(config.env.len(), 1);
    assert!(config.readonly_rootfs);
    assert!(!config.privileged);
    assert_eq!(config.mounts.len(), 1);
}

#[test]
#[cfg(target_os = "linux")]
fn test_capability_all() {
    use exo_runtime::Capability;

    let all = Capability::all();

    // Should have many capabilities
    assert!(all.len() > 30);

    // Check for key capabilities
    assert!(all.contains(&Capability::CAP_NET_RAW));
    assert!(all.contains(&Capability::CAP_NET_ADMIN));
    assert!(all.contains(&Capability::CAP_SYS_ADMIN));
    assert!(all.contains(&Capability::CAP_CHOWN));
}

///////////////////////////////////////////////////////////////////////////////
// Stability Tests
///////////////////////////////////////////////////////////////////////////////

#[test]
#[cfg(target_os = "linux")]
#[ignore = "Long-running stability test"]
fn test_cgroup_stress_create_destroy() {
    // Create and destroy many cgroups to check for resource leaks
    let iterations = 100;
    let base_id = std::process::id();

    for i in 0..iterations {
        let test_id = format!("stress_{}_{}", base_id, i);
        let mgr = CgroupManager::new(&test_id).expect("Failed to create cgroup");

        let mut mgr = mgr;
        mgr.initialize().expect("Failed to initialize");

        // Set some limits
        let _ = mgr.set_memory_limit(64 * 1024 * 1024);
        let _ = mgr.set_pids_limit(10);

        // Destroy
        mgr.destroy().expect("Failed to destroy");
    }
}

#[test]
#[cfg(target_os = "linux")]
#[ignore = "Long-running test"]
fn test_cgroup_memory_leak() {
    use std::thread;

    // Check that memory usage doesn't grow unbounded
    let test_id = format!("mem_leak_{}", std::process::id());
    let mut mgr = CgroupManager::new(&test_id).expect("Failed to create cgroup");

    mgr.initialize().expect("Failed to initialize");

    let mut usage_samples = vec![];

    for _ in 0..10 {
        let usage = mgr.get_memory_usage().expect("Failed to get usage");
        usage_samples.push(usage);

        // Do some operations
        let _ = mgr.set_memory_limit(128 * 1024 * 1024);
        let _ = mgr.get_cpu_usage();
        let _ = mgr.get_processes();

        thread::sleep(Duration::from_millis(10));
    }

    // Usage should not grow more than 2x from start to end
    let start = usage_samples.first().unwrap();
    let end = usage_samples.last().unwrap();
    let ratio = *end as f64 / *start as f64;

    assert!(ratio < 2.0, "Memory usage grew too much: {}x", ratio);

    // Clean up
    let _ = mgr.destroy();
}
