//! Typed error taxonomy — the error half of the exo agent contract.
//!
//! Every user-facing failure should be an [`ExoError`], so that the CLI (and
//! any agent driving it) can branch on a stable machine-readable code and a
//! documented exit code instead of parsing prose. The contract is documented
//! in `docs/EXIT_CODES.md`; codes and exit codes are additive-only within a
//! schema version.
//!
//! Typed errors raised inside `anyhow::Result` code paths survive as
//! downcastable sources: use [`exit_code_for`] at the process boundary to
//! recover the taxonomy from an `anyhow::Error` chain.

use serde::Serialize;

/// Successful execution.
pub const EXIT_OK: i32 = 0;
/// The requested object (container, image, volume, secret) does not exist.
pub const EXIT_NOT_FOUND: i32 = 2;
/// The request conflicts with current state (already exists, running, stopped).
pub const EXIT_CONFLICT: i32 = 3;
/// The backend is unavailable, or does not support the requested feature.
pub const EXIT_BACKEND: i32 = 4;
/// The request itself is malformed (bad flags, bad names, bad references).
pub const EXIT_INVALID_INPUT: i32 = 5;
/// Anything else: bugs, I/O failures, unconverted legacy errors.
pub const EXIT_INTERNAL: i32 = 6;

/// Result type for operations that fail with the typed taxonomy.
pub type ExoResult<T> = Result<T, ExoError>;

/// Typed errors for all user-facing exo failures.
///
/// Variant names map 1:1 to the machine-readable `code()` strings and to the
/// exit-code classes in `docs/EXIT_CODES.md`. When adding a variant, update
/// the doc and keep mappings stable — agents depend on them.
#[derive(thiserror::Error, Debug)]
pub enum ExoError {
    // --- not found (exit 2) ---
    #[error("container not found: {0}")]
    ContainerNotFound(String),

    #[error("image not found: {0}")]
    ImageNotFound(String),

    #[error("volume not found: {0}")]
    VolumeNotFound(String),

    #[error("secret not found: {0}")]
    SecretNotFound(String),

    // --- conflict / state (exit 3) ---
    #[error("container already exists: {0}")]
    ContainerAlreadyExists(String),

    #[error("container is running: {0}")]
    ContainerRunning(String),

    #[error("container is not running: {0}")]
    ContainerNotRunning(String),

    // --- backend unavailable / unsupported (exit 4) ---
    #[error("daemon unreachable: {0}")]
    DaemonUnreachable(String),

    #[error("backend unavailable: {0}")]
    BackendUnavailable(String),

    #[error("feature '{feature}' is not supported by the '{backend}' backend")]
    BackendUnsupported { feature: String, backend: String },

    #[error("registry authentication failed: {0}")]
    RegistryAuth(String),

    #[error("registry unavailable: {0}")]
    RegistryUnavailable(String),

    // --- invalid input (exit 5) ---
    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("invalid name: {0}")]
    InvalidName(String),

    // --- internal (exit 6) ---
    #[error("internal error: {0}")]
    Internal(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    // --- workload result (exit code passes through) ---
    /// The workload inside the container exited non-zero (attach-mode `run`,
    /// `exec`, `start --attach`). Unlike every other variant, `exit_code()`
    /// returns the *container's own* exit code so `exo run` behaves like
    /// `docker run`; the `CONTAINER_EXITED` envelope code is what tells the
    /// agent the number came from the workload, not from exo.
    #[error("container {name} exited with code {code}")]
    ContainerExited { name: String, code: i32 },
}

impl ExoError {
    /// Process exit code for this error. Part of the agent contract.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::ContainerNotFound(_)
            | Self::ImageNotFound(_)
            | Self::VolumeNotFound(_)
            | Self::SecretNotFound(_) => EXIT_NOT_FOUND,

            Self::ContainerAlreadyExists(_)
            | Self::ContainerRunning(_)
            | Self::ContainerNotRunning(_) => EXIT_CONFLICT,

            Self::DaemonUnreachable(_)
            | Self::BackendUnavailable(_)
            | Self::BackendUnsupported { .. }
            | Self::RegistryAuth(_)
            | Self::RegistryUnavailable(_) => EXIT_BACKEND,

            Self::InvalidInput(_) | Self::InvalidName(_) => EXIT_INVALID_INPUT,

            Self::Internal(_) | Self::Io(_) => EXIT_INTERNAL,

            // The container's own exit code passes through (docker run
            // semantics). Clamped into the valid process-exit range; 0 is
            // impossible by construction but maps to 1 defensively.
            Self::ContainerExited { code, .. } => match code {
                1..=255 => *code,
                _ => 1,
            },
        }
    }

    /// Stable machine-readable code for JSON consumers. Part of the agent
    /// contract; never rename without a schema-version bump.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ContainerNotFound(_) => "CONTAINER_NOT_FOUND",
            Self::ImageNotFound(_) => "IMAGE_NOT_FOUND",
            Self::VolumeNotFound(_) => "VOLUME_NOT_FOUND",
            Self::SecretNotFound(_) => "SECRET_NOT_FOUND",
            Self::ContainerAlreadyExists(_) => "CONTAINER_ALREADY_EXISTS",
            Self::ContainerRunning(_) => "CONTAINER_RUNNING",
            Self::ContainerNotRunning(_) => "CONTAINER_NOT_RUNNING",
            Self::DaemonUnreachable(_) => "DAEMON_UNREACHABLE",
            Self::BackendUnavailable(_) => "BACKEND_UNAVAILABLE",
            Self::BackendUnsupported { .. } => "BACKEND_UNSUPPORTED",
            Self::RegistryAuth(_) => "REGISTRY_AUTH",
            Self::RegistryUnavailable(_) => "REGISTRY_UNAVAILABLE",
            Self::InvalidInput(_) => "INVALID_INPUT",
            Self::InvalidName(_) => "INVALID_NAME",
            Self::Internal(_) => "INTERNAL",
            Self::Io(_) => "IO",
            Self::ContainerExited { .. } => "CONTAINER_EXITED",
        }
    }

    /// Whether retrying the same request as-is may succeed. Lets agents
    /// distinguish "try again" from "change the request".
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::DaemonUnreachable(_)
                | Self::BackendUnavailable(_)
                | Self::RegistryUnavailable(_)
                | Self::Io(_)
        )
    }

    /// JSON envelope for `--json` error output (schema 1).
    pub fn envelope(&self) -> ErrorEnvelope {
        ErrorEnvelope {
            schema: 1,
            error: ErrorBody {
                code: self.code(),
                message: self.to_string(),
                retryable: self.retryable(),
            },
        }
    }
}

/// Serializable error envelope emitted on `--json` failures.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub schema: u32,
    pub error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl From<crate::ContainerError> for ExoError {
    fn from(err: crate::ContainerError) -> Self {
        match err {
            crate::ContainerError::Io(io) => Self::Io(io),
            other => Self::Internal(other.to_string()),
        }
    }
}

/// Recover the contract exit code from an `anyhow::Error` chain.
///
/// Typed [`ExoError`]s raised through `anyhow::Result` paths (via `?` or
/// `anyhow::Error::from`) are downcast back to their taxonomy. Untyped legacy
/// errors map to [`EXIT_INTERNAL`].
pub fn exit_code_for(err: &anyhow::Error) -> i32 {
    err.downcast_ref::<ExoError>()
        .map(ExoError::exit_code)
        .unwrap_or(EXIT_INTERNAL)
}

/// JSON error envelope for an `anyhow::Error` chain, for `--json` failures.
///
/// Typed errors keep their taxonomy; untyped legacy errors become `INTERNAL`.
/// Emit this on **stderr** (with the process exit code from
/// [`exit_code_for`]) so stdout stays pure data for the consumer.
pub fn envelope_for(err: &anyhow::Error) -> ErrorEnvelope {
    match err.downcast_ref::<ExoError>() {
        Some(typed) => typed.envelope(),
        None => ErrorEnvelope {
            schema: 1,
            error: ErrorBody {
                code: "INTERNAL",
                message: format!("{err:#}"),
                retryable: false,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_documented_classes() {
        assert_eq!(ExoError::ContainerNotFound("c".into()).exit_code(), 2);
        assert_eq!(ExoError::ImageNotFound("i".into()).exit_code(), 2);
        assert_eq!(ExoError::VolumeNotFound("v".into()).exit_code(), 2);
        assert_eq!(ExoError::SecretNotFound("s".into()).exit_code(), 2);
        assert_eq!(ExoError::ContainerAlreadyExists("c".into()).exit_code(), 3);
        assert_eq!(ExoError::ContainerRunning("c".into()).exit_code(), 3);
        assert_eq!(ExoError::ContainerNotRunning("c".into()).exit_code(), 3);
        assert_eq!(ExoError::DaemonUnreachable("d".into()).exit_code(), 4);
        assert_eq!(ExoError::BackendUnavailable("b".into()).exit_code(), 4);
        assert_eq!(
            ExoError::BackendUnsupported {
                feature: "exec".into(),
                backend: "linux".into()
            }
            .exit_code(),
            4
        );
        assert_eq!(ExoError::InvalidInput("x".into()).exit_code(), 5);
        assert_eq!(ExoError::InvalidName("x".into()).exit_code(), 5);
        assert_eq!(ExoError::Internal("x".into()).exit_code(), 6);
        assert_eq!(
            ExoError::Io(std::io::Error::new(std::io::ErrorKind::Other, "x")).exit_code(),
            6
        );
    }

    #[test]
    fn codes_are_stable_screaming_snake() {
        assert_eq!(ExoError::ContainerNotFound("c".into()).code(), "CONTAINER_NOT_FOUND");
        assert_eq!(
            ExoError::BackendUnsupported {
                feature: "f".into(),
                backend: "b".into()
            }
            .code(),
            "BACKEND_UNSUPPORTED"
        );
        assert_eq!(ExoError::DaemonUnreachable("d".into()).code(), "DAEMON_UNREACHABLE");
    }

    #[test]
    fn retryable_only_for_transient_classes() {
        assert!(ExoError::DaemonUnreachable("d".into()).retryable());
        assert!(ExoError::BackendUnavailable("b".into()).retryable());
        assert!(!ExoError::ContainerNotFound("c".into()).retryable());
        assert!(!ExoError::InvalidInput("x".into()).retryable());
        assert!(!ExoError::BackendUnsupported {
            feature: "f".into(),
            backend: "b".into()
        }
        .retryable());
    }

    #[test]
    fn envelope_serializes_to_contract_shape() {
        let err = ExoError::ImageNotFound("alpine:3.20".into());
        let json = serde_json::to_value(err.envelope()).unwrap();
        assert_eq!(json["schema"], 1);
        assert_eq!(json["error"]["code"], "IMAGE_NOT_FOUND");
        assert!(json["error"]["message"].as_str().unwrap().contains("alpine:3.20"));
        assert_eq!(json["error"]["retryable"], false);
    }

    #[test]
    fn typed_errors_survive_anyhow_chains() {
        let anyhow_err: anyhow::Error = ExoError::ContainerNotFound("web".into()).into();
        assert_eq!(exit_code_for(&anyhow_err), 2);

        let wrapped = anyhow_err.context("while stopping container");
        assert_eq!(exit_code_for(&wrapped), 2);

        let legacy = anyhow::anyhow!("some stringly failure");
        assert_eq!(exit_code_for(&legacy), 6);
    }

    #[test]
    fn container_error_converts() {
        let typed: ExoError = crate::ContainerError::Mount("bad mount".into()).into();
        assert_eq!(typed.exit_code(), 6);
        assert!(typed.to_string().contains("bad mount"));
    }

    #[test]
    fn container_exited_passes_through_container_code() {
        let err = ExoError::ContainerExited {
            name: "web".into(),
            code: 42,
        };
        assert_eq!(err.exit_code(), 42);
        assert_eq!(err.code(), "CONTAINER_EXITED");
        assert!(!err.retryable());
        assert_eq!(err.to_string(), "container web exited with code 42");

        // Clamping: out-of-range codes map to 1 (0 is success, never emit it).
        assert_eq!(
            ExoError::ContainerExited {
                name: "x".into(),
                code: 0
            }
            .exit_code(),
            1
        );
        assert_eq!(
            ExoError::ContainerExited {
                name: "x".into(),
                code: 300
            }
            .exit_code(),
            1
        );

        // Survives anyhow chains with the container's code intact.
        let anyhow_err: anyhow::Error = ExoError::ContainerExited {
            name: "web".into(),
            code: 137,
        }
        .into();
        assert_eq!(exit_code_for(&anyhow_err), 137);
        let json = serde_json::to_value(envelope_for(&anyhow_err)).unwrap();
        assert_eq!(json["error"]["code"], "CONTAINER_EXITED");
    }

    #[test]
    fn registry_variants_follow_backend_class() {
        assert_eq!(ExoError::RegistryAuth("docker.io".into()).exit_code(), 4);
        assert_eq!(ExoError::RegistryAuth("r".into()).code(), "REGISTRY_AUTH");
        assert!(!ExoError::RegistryAuth("r".into()).retryable());
        assert_eq!(
            ExoError::RegistryUnavailable("docker.io".into()).exit_code(),
            4
        );
        assert_eq!(
            ExoError::RegistryUnavailable("r".into()).code(),
            "REGISTRY_UNAVAILABLE"
        );
        assert!(ExoError::RegistryUnavailable("r".into()).retryable());
    }

    #[test]
    fn envelope_for_recovers_typed_and_wraps_legacy() {
        let typed: anyhow::Error = ExoError::VolumeNotFound("data".into()).into();
        let json = serde_json::to_value(envelope_for(&typed)).unwrap();
        assert_eq!(json["error"]["code"], "VOLUME_NOT_FOUND");

        let legacy = anyhow::anyhow!("stringly boom").context("while pulling");
        let json = serde_json::to_value(envelope_for(&legacy)).unwrap();
        assert_eq!(json["error"]["code"], "INTERNAL");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("while pulling"));
        assert_eq!(json["error"]["retryable"], false);
    }
}
