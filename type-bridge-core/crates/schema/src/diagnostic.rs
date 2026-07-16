use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::schema::{
    DiagnosticLabel, SchemaDiagnostic, SchemaDiagnostics, SourceSpan,
};

pub(crate) fn diagnostic(
    category: DiagnosticCategory,
    code: &'static str,
    message: impl Into<String>,
    primary: Option<SourceSpan>,
) -> SchemaDiagnostics {
    let diagnostic = Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static schema diagnostic code is valid"),
        message,
    );
    SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, primary))
}

pub(crate) fn diagnostic_with_related(
    category: DiagnosticCategory,
    code: &'static str,
    message: impl Into<String>,
    primary: SourceSpan,
    related: SourceSpan,
    related_message: impl Into<String>,
) -> SchemaDiagnostics {
    let diagnostic = Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static schema diagnostic code is valid"),
        message,
    );
    SchemaDiagnostics::one(
        SchemaDiagnostic::new(diagnostic, Some(primary))
            .with_related(DiagnosticLabel::new(related, related_message)),
    )
}
