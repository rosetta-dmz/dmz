// Diagnostic error classification and user-facing remediation

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

/// High-level categorization of execution and compilation failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCategory {
    PlatformFault,
    PermissionDenied,
    ToolchainFault,
    IoFault,
    UserCodeFault,
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlatformFault => write!(f, "Platform Fault"),
            Self::PermissionDenied => write!(f, "Permission Denied"),
            Self::ToolchainFault => write!(f, "Toolchain Fault"),
            Self::IoFault => write!(f, "I/O Failure"),
            Self::UserCodeFault => write!(f, "User Code Fault"),
        }
    }
}

/// Structured diagnostic outcome containing classification, detail, and actionable remediation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticError {
    pub category: ErrorCategory,
    pub message: String,
    pub remediation: String,
}

impl fmt::Display for DiagnosticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {}\nRemediation: {}",
            self.category, self.message, self.remediation
        )
    }
}

impl Error for DiagnosticError {}

impl DiagnosticError {
    /// Classifies an error by inspecting concrete error types via downcasting,
    /// traversing the causal chain, and matching known diagnostic failure signatures.
    pub fn classify(err: &(dyn Error + 'static)) -> Self {
        // Inspect causal chain for typed std::io::Error or core error downcast
        let mut curr: Option<&(dyn Error + 'static)> = Some(err);
        while let Some(e) = curr {
            if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                return Self::from_io_error(io_err, err);
            }
            if let Some(toolchain_err) = e.downcast_ref::<crate::toolchain::ToolchainError>() {
                return Self {
                    category: ErrorCategory::ToolchainFault,
                    message: format!("Toolchain execution failure: {}", toolchain_err),
                    remediation: String::from(
                        "Verify that required compilers, build tools, or packages are installed in the workspace.",
                    ),
                };
            }
            if let Some(import_err) = e.downcast_ref::<crate::archive::importer::ImportError>() {
                return match import_err {
                    crate::archive::importer::ImportError::VerificationFailed(msg) => Self {
                        category: ErrorCategory::PlatformFault,
                        message: format!("Archive integrity verification failed: {}", msg),
                        remediation: String::from(
                            "Ensure the air-gap archive has not been corrupted or tampered with.",
                        ),
                    },
                    crate::archive::importer::ImportError::ExtractionError(msg) => Self {
                        category: ErrorCategory::IoFault,
                        message: format!("Archive extraction failure: {}", msg),
                        remediation: String::from(
                            "Check available disk space, path lengths, and archive extraction limits.",
                        ),
                    },
                    crate::archive::importer::ImportError::IoError(msg) => Self {
                        category: ErrorCategory::IoFault,
                        message: format!("Archive I/O error: {}", msg),
                        remediation: String::from(
                            "Ensure read/write access to destination workspace directories.",
                        ),
                    },
                };
            }
            curr = e.source();
        }

        Self::classify_from_chain(err)
    }

    /// Fallback classifier for non-static dynamic errors.
    pub fn classify_dyn(err: &dyn Error) -> Self {
        Self::classify_from_chain(err)
    }

    fn from_io_error(io_err: &std::io::Error, root: &(dyn Error + 'static)) -> Self {
        match io_err.kind() {
            std::io::ErrorKind::PermissionDenied => Self {
                category: ErrorCategory::PermissionDenied,
                message: format!("Access denied: {}", root),
                remediation: String::from(
                    "Check workspace file permissions or ensure the user has rights to invoke containerless isolation.",
                ),
            },
            std::io::ErrorKind::NotFound => Self {
                category: ErrorCategory::ToolchainFault,
                message: format!("Target or toolchain executable not found: {}", root),
                remediation: String::from(
                    "Ensure the required toolchain or target path exists within the hermetic workspace.",
                ),
            },
            std::io::ErrorKind::OutOfMemory => Self {
                category: ErrorCategory::PlatformFault,
                message: format!("Memory quota or system limits exhausted: {}", root),
                remediation: String::from(
                    "Adjust sandbox memory limits or reduce workspace memory footprint.",
                ),
            },
            _ => {
                #[cfg(unix)]
                if let Some(raw_os_err) = io_err.raw_os_error() {
                    if raw_os_err == libc::EPERM || raw_os_err == libc::EACCES {
                        return Self {
                            category: ErrorCategory::PermissionDenied,
                            message: format!("OS permission denied (errno {}): {}", raw_os_err, root),
                            remediation: String::from(
                                "Verify process isolation capabilities and host permission policies.",
                            ),
                        };
                    }
                    if raw_os_err == libc::ENOSPC {
                        return Self {
                            category: ErrorCategory::IoFault,
                            message: format!("Disk space exhausted (errno {}): {}", raw_os_err, root),
                            remediation: String::from(
                                "Free disk space on the host volume hosting the workspace.",
                            ),
                        };
                    }
                }
                Self {
                    category: ErrorCategory::IoFault,
                    message: format!("Filesystem I/O error: {}", root),
                    remediation: String::from(
                        "Check available disk space, socket path lengths, and filesystem integrity.",
                    ),
                }
            }
        }
    }

    fn classify_from_chain(err: &dyn Error) -> Self {
        let mut full_chain = Vec::new();
        let mut curr_opt: Option<&dyn Error> = Some(err);
        while let Some(e) = curr_opt {
            full_chain.push(e.to_string());
            curr_opt = e.source();
        }
        let combined_msg = full_chain.join(" -> ");
        let normalized = combined_msg.to_lowercase();

        // 1. Evaluate platform & kernel isolation signatures
        if normalized.contains("unshare")
            || normalized.contains("seatbelt")
            || normalized.contains("job object")
            || normalized.contains("zstd stream")
            || normalized.contains("namespace")
            || normalized.contains("pivot_root")
            || normalized.contains("cgroup")
            || normalized.contains("clone_new")
            || normalized.contains("sandbox-exec")
            || normalized.contains("platform fault")
            || normalized.contains("platform error")
            || normalized.contains("unsupported platform")
        {
            return Self {
                category: ErrorCategory::PlatformFault,
                message: format!("Engine/Kernel execution error: {}", combined_msg),
                remediation: String::from(
                    "Ensure your host OS supports native user namespaces/sandbox profiles, or check DMZ system permissions.",
                ),
            };
        }

        // Evaluate permission denied signatures
        if normalized.contains("permission denied")
            || normalized.contains("operation not permitted")
            || normalized.contains("access is denied")
            || normalized.contains("eacces")
            || normalized.contains("eperm")
        {
            return Self {
                category: ErrorCategory::PermissionDenied,
                message: format!("Permission denial error: {}", combined_msg),
                remediation: String::from(
                    "Verify filesystem permissions and user security privileges for containerless isolation.",
                ),
            };
        }

        // Evaluate toolchain and missing executable signatures
        if normalized.contains("command not found")
            || normalized.contains("executable not found")
            || normalized.contains("no such file or directory")
            || normalized.contains("cannot find binary")
        {
            return Self {
                category: ErrorCategory::ToolchainFault,
                message: format!("Toolchain or binary resolution error: {}", combined_msg),
                remediation: String::from(
                    "Verify the requested compiler or toolchain is bundled in .dmz or declared in the workspace configuration.",
                ),
            };
        }

        // Evaluate storage exhaustion signatures
        if normalized.contains("no space left on device")
            || normalized.contains("disk full")
            || normalized.contains("enospc")
        {
            return Self {
                category: ErrorCategory::IoFault,
                message: format!("Storage exhaustion error: {}", combined_msg),
                remediation: String::from(
                    "Free up disk space on the volume hosting the workspace.",
                ),
            };
        }

        // Default fallback to user code / compile error
        Self {
            category: ErrorCategory::UserCodeFault,
            message: format!("Workspace execution or compile error: {}", combined_msg),
            remediation: String::from(
                "Check your project dependency manifests (Cargo.toml, package.json) or compiler syntax errors.",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[derive(Debug)]
    struct CustomPlatformError(String);

    impl fmt::Display for CustomPlatformError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Sandbox isolation failed: {}", self.0)
        }
    }

    impl Error for CustomPlatformError {}

    #[derive(Debug)]
    struct ChainedWrapperError {
        msg: String,
        source: io::Error,
    }

    impl fmt::Display for ChainedWrapperError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Execution wrapper: {}", self.msg)
        }
    }

    impl Error for ChainedWrapperError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.source)
        }
    }

    #[test]
    fn test_classify_platform_fault_signatures() {
        let signatures = [
            "unshare(CLONE_NEWUSER) failed: EPERM",
            "seatbelt sandbox execution profile error",
            "Job Object limit assignment failure",
            "corrupted zstd stream buffer",
            "pivot_root into ephemeral mount failed",
            "cgroup v2 memory.max allocation error",
        ];

        for sig in signatures {
            let err = CustomPlatformError(sig.into());
            let diag = DiagnosticError::classify(&err);
            assert_eq!(
                diag.category,
                ErrorCategory::PlatformFault,
                "Failed to classify platform signature: {}",
                sig
            );
            assert!(diag.message.contains(sig));
            assert!(diag.remediation.contains("user namespaces"));
        }
    }

    #[test]
    fn test_classify_io_permission_denied() {
        let err = io::Error::new(io::ErrorKind::PermissionDenied, "operation not permitted");
        let diag = DiagnosticError::classify(&err);
        assert_eq!(diag.category, ErrorCategory::PermissionDenied);
        assert!(diag.remediation.contains("permissions"));
    }

    #[test]
    fn test_classify_io_not_found_as_toolchain_fault() {
        let err = io::Error::new(io::ErrorKind::NotFound, "cargo binary not found");
        let diag = DiagnosticError::classify(&err);
        assert_eq!(diag.category, ErrorCategory::ToolchainFault);
        assert!(diag.remediation.contains("toolchain"));
    }

    #[test]
    fn test_classify_io_out_of_memory() {
        let err = io::Error::new(io::ErrorKind::OutOfMemory, "host cgroup limit hit");
        let diag = DiagnosticError::classify(&err);
        assert_eq!(diag.category, ErrorCategory::PlatformFault);
        assert!(diag.remediation.contains("memory limits"));
    }

    #[test]
    fn test_classify_user_code_fault() {
        let err = CustomPlatformError("error[E0425]: cannot find value `foo` in this scope".into());
        let diag = DiagnosticError::classify(&err);
        assert_eq!(diag.category, ErrorCategory::UserCodeFault);
        assert!(diag.remediation.contains("Cargo.toml"));
    }

    #[test]
    fn test_classify_chained_error_sources_downcasting() {
        let inner = io::Error::new(io::ErrorKind::PermissionDenied, "chroot denied");
        let outer = ChainedWrapperError {
            msg: "Failed to initialize containerless jail".into(),
            source: inner,
        };

        let diag = DiagnosticError::classify(&outer);
        assert_eq!(diag.category, ErrorCategory::PermissionDenied);
        assert!(diag.message.contains("Access denied"));
    }

    #[test]
    fn test_classify_toolchain_error_downcast() {
        let err = crate::toolchain::ToolchainError::DiscoveryFailed("rustc not found".into());
        let diag = DiagnosticError::classify(&err);
        assert_eq!(diag.category, ErrorCategory::ToolchainFault);
        assert!(diag.message.contains("Toolchain Discovery Error"));
    }

    #[test]
    fn test_classify_import_error_downcast() {
        let err =
            crate::archive::importer::ImportError::VerificationFailed("SHA-256 mismatch".into());
        let diag = DiagnosticError::classify(&err);
        assert_eq!(diag.category, ErrorCategory::PlatformFault);
        assert!(diag.message.contains("SHA-256 mismatch"));
    }

    #[test]
    fn test_classify_dmz_sandbox_platform_error() {
        let err = dmz_sandbox::platform::PlatformError::SeatbeltError(
            "Failed to apply Seatbelt profile".into(),
        );
        let diag = DiagnosticError::classify(&err);
        assert_eq!(diag.category, ErrorCategory::PlatformFault);
        assert!(diag.message.contains("Seatbelt error"));
    }

    #[test]
    fn test_classify_dyn_non_static() {
        let err_str = "error: package `foo` not found in registry";
        let custom = CustomPlatformError(err_str.into());
        let dyn_ref: &dyn Error = &custom;
        let diag = DiagnosticError::classify_dyn(dyn_ref);
        assert_eq!(diag.category, ErrorCategory::UserCodeFault);
    }

    #[test]
    fn test_display_and_trait_conformance() {
        let diag = DiagnosticError {
            category: ErrorCategory::PlatformFault,
            message: "Kernel allocation fault".into(),
            remediation: "Check permissions".into(),
        };

        // Display
        let formatted = format!("{}", diag);
        assert!(formatted.contains("[Platform Fault] Kernel allocation fault"));
        assert!(formatted.contains("Remediation: Check permissions"));
        assert_eq!(diag.category.to_string(), "Platform Fault");

        // Clone & PartialEq
        let cloned = diag.clone();
        assert_eq!(diag, cloned);

        // Serialize & Deserialize roundtrip
        let serialized = serde_json::to_string(&diag).expect("serialization failed");
        let deserialized: DiagnosticError =
            serde_json::from_str(&serialized).expect("deserialization failed");
        assert_eq!(diag, deserialized);
    }
}
