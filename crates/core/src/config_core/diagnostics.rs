use crate::config_core::AppConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDiagnosticKind {
    UnknownSection,
    UnknownRootKey,
    UnknownColorKey,
    InvalidSyntax,
    InvalidValue,
    DuplicateRootKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiagnostic {
    pub line_number: usize,
    pub kind: ConfigDiagnosticKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigParseReport {
    pub config: AppConfig,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

impl ConfigParseReport {
    pub(crate) fn new(config: AppConfig, diagnostics: Vec<ConfigDiagnostic>) -> Self {
        Self {
            config,
            diagnostics,
        }
    }
}
