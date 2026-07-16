//! Deterministic emitters over validated binding-neutral runtime projections.

mod package;
mod python;
mod rust;
mod typescript;

pub use package::GeneratedPackage;
pub use python::PythonEmitter;
pub use rust::RustEmitter;
pub use typescript::TypeScriptEmitter;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};

fn invalid(code: &'static str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("schema-codegen diagnostic code is valid"),
        message,
    )
}
