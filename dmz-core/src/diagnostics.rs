
// Structured error wrapper that intercepts raw errors and
// classifies them before printing.

#[derive(Debug)]
pub enum ErrorCategory {
    /// Faults originating from the DMZ engine, kernel primitives, or namespace allocation.
    PlatformFault,
    /// Faults originating from the developer's code, compile errors, or third-party package conflicts.
    UserCodeFault,
}

#[derive(Debug)]
pub struct DiagnosticError {
    pub category: ErrorCategory,
    pub message: String,
    pub remediation: String,
}

impl DiagnosticError {
    pub fn classify(err: &dyn std::error::Error) -> Self {
        let err_str = err.to_string();
        
        if err_str.contains("unshare") || err_str.contains("seatbelt") || err_str.contains("Job Object") || err_str.contains("zstd stream") {
            Self {
                category: ErrorCategory::PlatformFault,
                message: format!("Engine/Kernel execution error: {}", err_str),
                remediation: String::from("Ensure your host OS supports native user namespaces/sandbox profiles, or check DMZ system permissions."),
            }
        } else {
            Self {
                category: ErrorCategory::UserCodeFault,
                message: format!("Workspace execution or compile error: {}", err_str),
                remediation: String::from("Check your project dependency manifests (Cargo.toml, package.json) or compiler syntax errors."),
            }
        }
    }
}