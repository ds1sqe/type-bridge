use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ValidatedSourcePattern {
    pub(crate) original: String,
    pub(crate) segments: Vec<PatternSegment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PatternSegment {
    Recursive,
    Component(String),
}

fn violation(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
    pattern: String,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static source-pattern diagnostic code is valid"),
        message,
    )
    .with_detail("pattern", pattern)
}

pub(crate) fn validate_source_pattern(
    value: String,
    maximum_bytes: usize,
) -> Result<ValidatedSourcePattern, Diagnostic> {
    if value.is_empty() || value.len() > maximum_bytes {
        return Err(violation(
            DiagnosticCategory::ResourceLimit,
            "schema_source_pattern_size_limit",
            "schema source pattern size is outside its configured limit",
            value,
        )
        .with_detail(
            "maximum_bytes",
            i64::try_from(maximum_bytes).unwrap_or(i64::MAX),
        ));
    }
    if value.starts_with('/')
        || is_windows_absolute(&value)
        || value.contains('\\')
        || value.contains('\0')
    {
        return Err(violation(
            DiagnosticCategory::InvalidContract,
            "invalid_schema_source_path",
            "schema source patterns must be portable relative paths",
            value,
        ));
    }
    if value.nfc().ne(value.chars()) {
        return Err(violation(
            DiagnosticCategory::InvalidContract,
            "schema_source_pattern_not_nfc",
            "schema source patterns must use NFC path spelling",
            value,
        ));
    }

    let mut segments = Vec::new();
    for segment in value.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(violation(
                DiagnosticCategory::InvalidContract,
                "invalid_schema_source_path",
                "schema source patterns cannot contain empty, dot, or parent segments",
                value,
            ));
        }
        if segment == "**" {
            segments.push(PatternSegment::Recursive);
            continue;
        }
        if segment.contains("**")
            || segment.contains('[')
            || segment.contains(']')
            || segment.contains('{')
            || segment.contains('}')
        {
            return Err(violation(
                DiagnosticCategory::InvalidContract,
                "unsupported_schema_glob_syntax",
                "schema source pattern uses syntax outside the portable glob grammar",
                value,
            ));
        }
        segments.push(PatternSegment::Component(segment.to_owned()));
    }
    Ok(ValidatedSourcePattern {
        original: value,
        segments,
    })
}

fn is_windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}
