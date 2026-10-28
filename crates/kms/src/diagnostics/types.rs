#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "ERROR",
            Severity::Warning => "WARN",
            Severity::Information => "INFO",
            Severity::Hint => "HINT",
        }
    }
}

pub struct CodeDescription {
    pub href: String,
}

/// Diagnostic information produced by the structural rules.
pub struct Diagnostic {
    pub code: String,
    pub code_description: Option<CodeDescription>,
    pub location: String,
    pub severity: Severity,
    pub message: String,
    pub suggested_actions: Vec<String>,
}
