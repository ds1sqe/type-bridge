//! Runtime-owned classification and value-free rendering of provider failures.

pub use type_bridge_contract::failure::{
    AnnotationKind, Code, Failure, LockKind, Phase, Reason, TransportKind,
};

const STC2_LOCK_SENTENCES: &[(&str, LockKind)] = &[
    (
        "isolation conflict: Transaction uses a lock held by a concurrent commit.",
        LockKind::Exclusive,
    ),
    (
        "isolation conflict: Transaction uses data a concurrent commit deletes.",
        LockKind::RequireDeletedKey,
    ),
    (
        "isolation conflict: Transaction deletes data a concurrent commit requires.",
        LockKind::DeletingRequiredKey,
    ),
];

/// Lock kind from the fixed tail of the root `[STC2]` line only; `Caused:` lines
/// and echoed values elsewhere cannot select a kind.
fn stc2_lock(message: &str) -> LockKind {
    let Some(root) = message
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("[STC2]"))
    else {
        return LockKind::Unknown;
    };
    STC2_LOCK_SENTENCES
        .iter()
        .find_map(|(sentence, lock)| root.ends_with(sentence).then_some(*lock))
        .unwrap_or(LockKind::Unknown)
}

fn reason(code: Code, message: &str) -> Reason {
    match code.as_str() {
        "STC2" => Reason::Conflict {
            lock: stc2_lock(message),
        },
        "CNT5" => Reason::Cardinality,
        "CNT9" => Reason::Unique,
        "CNT4" | "CNT6" | "CNT7" | "CNT8" | "CNT10" => Reason::Constraint {
            annotation: match code.as_str() {
                "CNT4" => AnnotationKind::Abstract,
                "CNT6" => AnnotationKind::Regex,
                "CNT7" => AnnotationKind::Range,
                "CNT8" => AnnotationKind::Values,
                "CNT10" => AnnotationKind::Distinct,
                _ => unreachable!(),
            },
        },
        _ => Reason::Other { code },
    }
}
/// Classify a query failure using the structural driver code and error kind.
/// Transport failures do not become known rejections merely because of prose.
#[must_use]
pub fn classify_query(code: &str, message: &str, server_error: bool) -> Option<Failure> {
    server_error
        .then(|| Code::new(code))
        .flatten()
        .map(|code| Failure::Rejected {
            phase: Phase::Query,
            reason: reason(code, message),
            op_index: None,
        })
}
/// Classify a commit; only the pinned structural root-code table proves rejection.
#[must_use]
pub fn classify_commit(code: &str, message: &str, server_error: bool) -> Failure {
    let code = code
        .strip_prefix('[')
        .and_then(|code| code.strip_suffix(']'))
        .unwrap_or(code);
    if server_error
        && crate::server_commit_certainty(code) == crate::CommitFailureCertainty::DefinitelyAborted
        && let Some(code) = Code::new(code)
    {
        return Failure::Rejected {
            phase: Phase::Commit,
            reason: reason(code, message),
            op_index: None,
        };
    }
    Failure::Indeterminate {
        code: Code::new(code),
    }
}
/// Render a controlled code and fixed description, discarding all provider prose.
#[must_use]
pub fn redacted_failure(failure: &Failure) -> String {
    let description = match failure {
        Failure::Indeterminate { .. } => "TypeDB commit outcome unknown".to_owned(),
        Failure::Rejected { phase, reason, .. } => {
            let phase = match phase {
                Phase::Query => "query",
                Phase::Commit => "commit",
                _ => "operation",
            };
            let suffix = match reason {
                Reason::Conflict {
                    lock: LockKind::Exclusive,
                } => ": exclusive lock conflict",
                Reason::Conflict {
                    lock: LockKind::RequireDeletedKey,
                } => ": required data deleted",
                Reason::Conflict {
                    lock: LockKind::DeletingRequiredKey,
                } => ": deleting required data",
                Reason::Conflict { .. } => ": isolation conflict",
                Reason::Cardinality => ": cardinality constraint",
                Reason::Unique => ": unique constraint",
                Reason::Constraint { .. } => ": annotation constraint",
                _ => "",
            };
            format!("TypeDB {phase} rejected{suffix}")
        }
        _ => "TypeDB operation failed".to_owned(),
    };
    format!("[{}] {description}", failure.code().unwrap_or("UNKNOWN"))
}
/// Prefixes the runtime and its released carriers put before a redacted rendering.
const REDACTED_MESSAGE_PREFIXES: &[&str] = &[
    "",
    "Commit failed: ",
    "Query failed: ",
    "Row stream failed: ",
    "Document stream failed: ",
];
/// Recover only our own exact redacted description on released string surfaces.
/// Staged operation indexes and schema context require structural error carriers.
#[must_use]
pub fn failure_from_message(message: &str) -> Option<Failure> {
    // Only runtime-owned prefixes may precede the rendering; anything else may echo caller text.
    let text = REDACTED_MESSAGE_PREFIXES.iter().find_map(|prefix| {
        message
            .strip_prefix(prefix)
            .filter(|text| text.starts_with('['))
    })?;
    let (code, description) = text.strip_prefix('[')?.split_once("] ")?;
    let (description, transport) =
        if let Some(kind) = description.strip_prefix("TypeDB commit outcome unknown: ") {
            (
                "TypeDB commit outcome unknown",
                Some(TransportKind::from_description(kind)?),
            )
        } else {
            (description, None)
        };
    let candidate = if description == "TypeDB commit outcome unknown" {
        Failure::Indeterminate {
            code: Code::new(code),
        }
    } else {
        let phase = if description.starts_with("TypeDB commit rejected") {
            Phase::Commit
        } else if description.starts_with("TypeDB query rejected") {
            Phase::Query
        } else {
            return None;
        };
        if phase == Phase::Commit
            && crate::server_commit_certainty(code)
                != crate::CommitFailureCertainty::DefinitelyAborted
        {
            return None;
        }
        let lock = match description.split_once(": ").map(|(_, tail)| tail) {
            Some("exclusive lock conflict") => LockKind::Exclusive,
            Some("required data deleted") => LockKind::RequireDeletedKey,
            Some("deleting required data") => LockKind::DeletingRequiredKey,
            _ => LockKind::Unknown,
        };
        let lock_message = STC2_LOCK_SENTENCES
            .iter()
            .find_map(|(sentence, kind)| (*kind == lock).then(|| format!("[STC2] {sentence}")))
            .unwrap_or_default();
        Failure::Rejected {
            phase,
            reason: reason(Code::new(code)?, &lock_message),
            op_index: None,
        }
    };
    let mut rendered = redacted_failure(&candidate);
    if let Some(kind) = transport {
        rendered.push_str(": ");
        rendered.push_str(kind.description());
    }
    (rendered == text).then_some(candidate)
}

pub(crate) fn redacted_commit_failure(failure: &Failure, error: &impl ProviderError) -> String {
    let mut message = redacted_failure(failure);
    if matches!(failure, Failure::Indeterminate { .. })
        && let Some(kind) = error.transport_kind()
    {
        message.push_str(": ");
        message.push_str(kind.description());
    }
    message
}
// Driver 3.14.0 (band 9) stringifies tonic transport errors into Other; only entire pinned
// templates are trusted. Band 8 has no pinned templates, so its Other is always opaque.
// Reviewed for 3.14.0: the driver changed only its concept-document and JSON
// value rendering since 3.13.6; its transport stack (tonic, h2, rustls) is unchanged.
#[cfg(feature = "band9")]
const DRIVER_3_14_0_TRANSPORT_TEMPLATES: &[(&str, TransportKind)] = &[
    (
        "http2 error: transport error: http2 error: keep-alive timed out: operation timed out",
        TransportKind::Timeout,
    ),
    (
        "http2 error: keep-alive timed out: operation timed out",
        TransportKind::Timeout,
    ),
    (
        "transport error: http2 error: keep-alive timed out: operation timed out",
        TransportKind::Timeout,
    ),
    (
        "invalid peer certificate: UnknownIssuer: transport error: invalid peer certificate: UnknownIssuer: invalid peer certificate: UnknownIssuer",
        TransportKind::Tls,
    ),
];

#[cfg(feature = "band9")]
fn other_transport_kind(message: &str) -> TransportKind {
    DRIVER_3_14_0_TRANSPORT_TEMPLATES
        .iter()
        .find_map(|(template, kind)| (*template == message).then_some(*kind))
        .unwrap_or(TransportKind::Other)
}

pub(crate) trait ProviderError {
    fn provider_code(&self) -> String;
    fn transport_kind(&self) -> Option<TransportKind>;
}

macro_rules! driver_error_adapter {
    ($driver:ident, $other_kind:expr) => {
        impl ProviderError for crate::$driver::Error {
            fn provider_code(&self) -> String {
                self.code()
            }
            fn transport_kind(&self) -> Option<TransportKind> {
                use crate::$driver::{Error, error::ConnectionError};
                Some(match self {
                    Error::Server(_) => return None,
                    Error::Connection(error) => match error {
                        ConnectionError::RequestTimeout { .. } => TransportKind::Timeout,
                        ConnectionError::RPCMethodUnavailable { .. }
                        | ConnectionError::ConnectionRefusedNetworking => {
                            TransportKind::Unavailable
                        }
                        ConnectionError::ServerConnectionFailed { details, .. } => {
                            // The driver stringifies child errors here; trust only complete value-free unit templates.
                            let templates = [
                                (
                                    ConnectionError::TokenCredentialInvalid.to_string(),
                                    TransportKind::Authentication,
                                ),
                                (
                                    ConnectionError::EncryptionSettingsMismatch.to_string(),
                                    TransportKind::Tls,
                                ),
                                (
                                    ConnectionError::SslCertificateNotValidated.to_string(),
                                    TransportKind::Tls,
                                ),
                                (
                                    ConnectionError::AbsentTlsConfigForTlsConnection.to_string(),
                                    TransportKind::Tls,
                                ),
                                (
                                    ConnectionError::ServerConnectionIsClosed.to_string(),
                                    TransportKind::Closed,
                                ),
                                (
                                    ConnectionError::ServerConnectionIsClosedUnexpectedly
                                        .to_string(),
                                    TransportKind::Closed,
                                ),
                                (
                                    ConnectionError::TransactionIsClosed.to_string(),
                                    TransportKind::Closed,
                                ),
                                (
                                    ConnectionError::UnexpectedConnectionClose.to_string(),
                                    TransportKind::Closed,
                                ),
                                (
                                    ConnectionError::BrokenPipe.to_string(),
                                    TransportKind::Closed,
                                ),
                                (
                                    ConnectionError::DatabaseImportChannelIsClosed.to_string(),
                                    TransportKind::Closed,
                                ),
                                (
                                    ConnectionError::DatabaseExportChannelIsClosed.to_string(),
                                    TransportKind::Closed,
                                ),
                            ];
                            let classify = |part: &str| {
                                templates.iter().find_map(|(template, kind)| {
                                    (part == template.as_str()).then_some(*kind)
                                })
                            };
                            let mut parts = details.split(";\n");
                            match parts.next().and_then(classify) {
                                Some(kind) if parts.all(|part| classify(part) == Some(kind)) => {
                                    kind
                                }
                                _ => TransportKind::Unavailable,
                            }
                        }
                        ConnectionError::TokenCredentialInvalid => TransportKind::Authentication,
                        ConnectionError::EncryptionSettingsMismatch
                        | ConnectionError::SslCertificateNotValidated
                        | ConnectionError::AbsentTlsConfigForTlsConnection
                        | ConnectionError::SchemeTlsSettingsMismatch { .. } => TransportKind::Tls,
                        ConnectionError::ServerConnectionIsClosed
                        | ConnectionError::ServerConnectionIsClosedUnexpectedly
                        | ConnectionError::TransactionIsClosed
                        | ConnectionError::TransactionIsClosedWithErrors { .. }
                        | ConnectionError::UnexpectedConnectionClose
                        | ConnectionError::DatabaseImportChannelIsClosed
                        | ConnectionError::DatabaseExportChannelIsClosed
                        | ConnectionError::BrokenPipe => TransportKind::Closed,
                        _ => TransportKind::Other,
                    },
                    Error::Other(message) => $other_kind(message.as_str()),
                    _ => TransportKind::Other,
                })
            }
        }
    };
}
#[cfg(feature = "band8")]
driver_error_adapter!(driver_b8, |_: &str| TransportKind::Other);
#[cfg(feature = "band9")]
driver_error_adapter!(driver_b9, other_transport_kind);

/// Discard free text while retaining a structural code, fixed kind and owned operation.
pub(crate) fn redact_provider_error(error: &impl ProviderError, operation: &'static str) -> String {
    let code = controlled_code(error);
    let code = code.as_ref().map_or("UNKNOWN", Code::as_str);
    let description = match error.transport_kind() {
        Some(kind) => format!("TypeDB transport failed: {}", kind.description()),
        None => "TypeDB operation failed".to_owned(),
    };
    format!("{operation}: [{code}] {description}")
}

fn controlled_code(error: &impl ProviderError) -> Option<Code> {
    let code = error.provider_code();
    let code = code
        .strip_prefix('[')
        .and_then(|code| code.strip_suffix(']'))
        .unwrap_or(&code);
    Code::new(code)
}

const INVALID_ADDRESS_OPERATION: &str = "Invalid TypeDB address";
const INVALID_ADDRESS_DESCRIPTION: &str = "expected a TypeDB server address such as host:port";

/// Local configuration error: fixed TypeBridge guidance plus the driver code, never the
/// configured address or the driver's parse prose.
pub(crate) fn redact_address_error(error: &impl ProviderError) -> String {
    let code = controlled_code(error);
    let code = code.as_ref().map_or("UNKNOWN", Code::as_str);
    format!("{INVALID_ADDRESS_OPERATION}: [{code}] {INVALID_ADDRESS_DESCRIPTION}")
}

pub(crate) fn is_redacted_provider_message(message: &str) -> bool {
    let Some((operation, rest)) = message.split_once(": [") else {
        return false;
    };
    if !matches!(
        operation,
        "Query failed"
            | "Row stream failed"
            | "Document stream failed"
            | "force close failed"
            | "Invalid TypeDB address"
            | "Failed to connect"
            | "gRPC server version failed"
            | "Failed to open transaction"
            | "Database lookup failed"
            | "Database create failed"
            | "Database delete failed"
            | "Schema export failed"
            | "Rollback failed"
            | "Close failed"
    ) {
        return false;
    }
    let Some((code, description)) = rest.split_once("] ") else {
        return false;
    };
    if code != "UNKNOWN" && Code::new(code).is_none() {
        return false;
    }
    if operation == INVALID_ADDRESS_OPERATION && description == INVALID_ADDRESS_DESCRIPTION {
        return true;
    }
    description == "TypeDB operation failed"
        || description
            .strip_prefix("TypeDB transport failed: ")
            .and_then(TransportKind::from_description)
            .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    macro_rules! driver_transport_tests {
        ($name:literal, $driver:ident) => {
            #[cfg(feature = $name)]
            #[test]
            fn $driver() {
                use crate::$driver::{Addresses, Error, error::ConnectionError};
                let addresses = Addresses::try_from_address_str("localhost:1729").unwrap();
                let cases = [
                    (
                        Error::Connection(ConnectionError::RequestTimeout {
                            timeout: "SECRET_SENTINEL".to_owned(),
                        }),
                        TransportKind::Timeout,
                    ),
                    (
                        Error::Connection(ConnectionError::ServerConnectionFailed {
                            configured_addresses: addresses.clone(),
                            accessed_addresses: addresses.clone(),
                            details: "SECRET_SENTINEL".to_owned(),
                        }),
                        TransportKind::Unavailable,
                    ),
                    // TLS and authentication unit variants have no free-text fields.
                    (
                        Error::Connection(ConnectionError::SslCertificateNotValidated),
                        TransportKind::Tls,
                    ),
                    (
                        Error::Connection(ConnectionError::TokenCredentialInvalid),
                        TransportKind::Authentication,
                    ),
                    (
                        Error::Connection(ConnectionError::TransactionIsClosedWithErrors {
                            errors: "SECRET_SENTINEL".to_owned(),
                        }),
                        TransportKind::Closed,
                    ),
                    (
                        Error::Connection(ConnectionError::ServerConnectionFailedNetworking {
                            error: "SECRET_SENTINEL timeout certificate".to_owned(),
                        }),
                        TransportKind::Other,
                    ),
                ];
                for (error, kind) in cases {
                    let message = redact_provider_error(&error, "Database create failed");
                    assert_eq!(error.transport_kind(), Some(kind));
                    assert!(
                        message.starts_with("Database create failed: ["),
                        "{message}"
                    );
                    assert!(message.contains(&error.code()), "{message}");
                    assert!(message.ends_with(kind.description()), "{message}");
                    assert!(!message.contains("SECRET_SENTINEL"), "{message}");
                    let failure = classify_commit(&error.code(), "", false);
                    let commit_message = redacted_commit_failure(&failure, &error);
                    assert!(
                        commit_message.ends_with(kind.description()),
                        "{commit_message}"
                    );
                    assert_eq!(failure_from_message(&commit_message), Some(failure));
                    assert!(!commit_message.contains("SECRET_SENTINEL"));
                    let retained = crate::RuntimeError::Connection(message);
                    assert!(!format!("{retained:?}").contains("SECRET_SENTINEL"));
                    assert!(std::error::Error::source(&retained).is_none());
                    assert!(is_redacted_provider_message(&match retained {
                        crate::RuntimeError::Connection(message) => message,
                        _ => unreachable!(),
                    }));
                }
                for (child, kind) in [
                    (
                        ConnectionError::TokenCredentialInvalid,
                        TransportKind::Authentication,
                    ),
                    (
                        ConnectionError::SslCertificateNotValidated,
                        TransportKind::Tls,
                    ),
                    (
                        ConnectionError::ServerConnectionIsClosedUnexpectedly,
                        TransportKind::Closed,
                    ),
                ] {
                    let template = child.to_string();
                    for details in [template.clone(), format!("{template};\n{template}")] {
                        let wrapped = Error::Connection(ConnectionError::ServerConnectionFailed {
                            configured_addresses: addresses.clone(),
                            accessed_addresses: addresses.clone(),
                            details,
                        });
                        assert_eq!(wrapped.transport_kind(), Some(kind));
                        let message = redact_provider_error(&wrapped, "Failed to connect");
                        assert!(message.contains(&wrapped.code()));
                        assert!(message.ends_with(kind.description()));
                    }
                    let spoofed = Error::Connection(ConnectionError::ServerConnectionFailed {
                        configured_addresses: addresses.clone(),
                        accessed_addresses: addresses.clone(),
                        details: format!("{template} SECRET_SENTINEL"),
                    });
                    assert_eq!(spoofed.transport_kind(), Some(TransportKind::Unavailable));
                    assert!(
                        !redact_provider_error(&spoofed, "Failed to connect")
                            .contains("SECRET_SENTINEL")
                    );
                }
                use tonic_types::{ErrorDetails, StatusExt};
                let mut details = ErrorDetails::new();
                details.set_error_info("CNT9", "SECRET_SENTINEL", std::collections::HashMap::new());
                details.set_debug_info(
                    vec!["SECRET_SENTINEL timeout TLS".to_owned()],
                    "SECRET_SENTINEL",
                );
                let error = Error::from(tonic::Status::with_error_details(
                    tonic::Code::InvalidArgument,
                    "SECRET_SENTINEL",
                    details,
                ));
                let message = redact_provider_error(&error, "Database create failed");
                assert_eq!(
                    message,
                    "Database create failed: [CNT9] TypeDB operation failed"
                );
                assert_eq!(error.transport_kind(), None);
            }
        };
    }
    driver_transport_tests!("band8", driver_b8);
    driver_transport_tests!("band9", driver_b9);

    #[cfg(feature = "band9")]
    macro_rules! pinned_transport_test {
        ($name:ident, $index:expr) => {
            #[test]
            fn $name() {
                let (template, kind) = DRIVER_3_14_0_TRANSPORT_TEMPLATES[$index];
                assert_eq!(other_transport_kind(template), kind);
                assert_eq!(
                    other_transport_kind(&format!("{template} SECRET_SENTINEL")),
                    TransportKind::Other
                );
                assert_eq!(
                    other_transport_kind(&format!("SECRET_SENTINEL {template}")),
                    TransportKind::Other
                );
            }
        };
    }
    #[cfg(feature = "band9")]
    pinned_transport_test!(pinned_driver_wrapped_h2_keepalive, 0);
    #[cfg(feature = "band9")]
    pinned_transport_test!(pinned_driver_h2_keepalive, 1);
    #[cfg(feature = "band9")]
    pinned_transport_test!(pinned_driver_tonic_h2_keepalive, 2);

    #[cfg(feature = "band9")]
    pinned_transport_test!(pinned_driver_unknown_certificate_issuer, 3);

    #[cfg(feature = "band8")]
    #[test]
    fn band8_other_never_matches_band9_templates() {
        use crate::driver_b8::Error;
        for template in [
            "http2 error: keep-alive timed out: operation timed out",
            "invalid peer certificate: UnknownIssuer: transport error: invalid peer certificate: UnknownIssuer: invalid peer certificate: UnknownIssuer",
        ] {
            assert_eq!(
                Error::Other(template.to_owned()).transport_kind(),
                Some(TransportKind::Other)
            );
        }
    }

    #[test]
    fn address_errors_use_fixed_guidance_without_the_address() {
        let messages = [
            #[cfg(feature = "band8")]
            crate::driver_b8::Addresses::try_from_address_str("SECRET_SENTINEL:not-a-port")
                .map(|_| ())
                .map_err(|error| redact_address_error(&error)),
            #[cfg(feature = "band9")]
            crate::driver_b9::Addresses::try_from_address_str("SECRET_SENTINEL:not-a-port")
                .map(|_| ())
                .map_err(|error| redact_address_error(&error)),
        ];
        for message in messages {
            let message = message.unwrap_err();
            assert!(
                message.starts_with("Invalid TypeDB address: ["),
                "{message}"
            );
            assert!(
                message.ends_with("] expected a TypeDB server address such as host:port"),
                "{message}"
            );
            assert!(!message.contains("SECRET_SENTINEL"), "{message}");
            assert!(!message.contains("transport"), "{message}");
            assert!(is_redacted_provider_message(&message), "{message}");
        }
        assert!(!is_redacted_provider_message(
            "Failed to connect: [UNKNOWN] expected a TypeDB server address such as host:port"
        ));
    }

    #[test]
    fn transport_templates_require_review_on_driver_upgrade() {
        assert!(include_str!("../Cargo.toml").contains("typedb-driver = { version = \"=3.14.0\""));
    }

    #[test]
    fn closed_transport_messages_do_not_accept_free_text_or_operations() {
        for message in [
            "SECRET_SENTINEL: [CXN39] TypeDB transport failed: timeout",
            "Database create failed: [SECRET_SENTINEL] TypeDB transport failed: timeout",
            "Database create failed: [CXN39] TypeDB transport failed: timeout SECRET_SENTINEL",
        ] {
            assert!(!is_redacted_provider_message(message));
        }
    }

    #[test]
    fn classifier_uses_structural_codes_and_fixed_lock_wording() {
        for (text, lock) in [
            (
                "[STC2] Commit in database 'db' failed with isolation conflict: Transaction uses a lock held by a concurrent commit.",
                LockKind::Exclusive,
            ),
            (
                "[STC2] Commit in database 'db' failed with isolation conflict: Transaction uses data a concurrent commit deletes.",
                LockKind::RequireDeletedKey,
            ),
            (
                "[STC2] Commit in database 'db' failed with isolation conflict: Transaction deletes data a concurrent commit requires.",
                LockKind::DeletingRequiredKey,
            ),
            ("unrecognized SECRET_SENTINEL", LockKind::Unknown),
        ] {
            let failure = classify_commit("STC2", text, true);
            assert_eq!(
                failure,
                Failure::Rejected {
                    phase: Phase::Commit,
                    reason: Reason::Conflict { lock },
                    op_index: None
                }
            );
            let message = redacted_failure(&failure);
            assert_eq!(failure_from_message(&message), Some(failure));
            assert!(!message.contains("SECRET_SENTINEL"));
        }
        // Lock sentences echoed in a quoted name or a `Caused:` line select nothing.
        for text in [
            "[STC2] Commit in database 'isolation conflict: Transaction uses a lock held by a concurrent commit.' failed with isolation conflict: SECRET_SENTINEL.",
            "[STC2] Commit in database 'db' failed with isolation conflict: SECRET_SENTINEL.\nCaused: [SST1] isolation conflict: Transaction uses data a concurrent commit deletes.",
            "Caused: [SST1] isolation conflict: Transaction uses a lock held by a concurrent commit.",
        ] {
            assert_eq!(
                classify_commit("STC2", text, true),
                Failure::Rejected {
                    phase: Phase::Commit,
                    reason: Reason::Conflict {
                        lock: LockKind::Unknown
                    },
                    op_index: None
                },
                "{text}"
            );
        }
        assert!(!classify_commit("STC1", "[STC2] SECRET_SENTINEL", true).retry_safe());
        assert!(!classify_commit("STC2", "SECRET_SENTINEL", false).retry_safe());
        assert!(classify_query("CNT9", "SECRET_SENTINEL", false).is_none());
    }
    #[test]
    fn constraints_and_indeterminate_round_trip_only_controlled_descriptions() {
        for code in [
            "CNT4", "CNT5", "CNT6", "CNT7", "CNT8", "CNT9", "CNT10", "DVL1",
        ] {
            for phase in [Phase::Query, Phase::Commit] {
                let failure = if phase == Phase::Query {
                    classify_query(code, "SECRET_SENTINEL", true).unwrap()
                } else {
                    classify_commit(code, "SECRET_SENTINEL", true)
                };
                assert_eq!(failure.code(), Some(code));
                assert!(failure.retry_safe());
                assert_eq!(
                    failure_from_message(&redacted_failure(&failure)),
                    Some(failure)
                );
            }
        }
        for code in ["STC1", "STC5", "STC6", "DCT5", "SCT3", "CXN06", "NEW999"] {
            let failure = classify_commit(code, "SECRET_SENTINEL", true);
            assert!(!failure.retry_safe());
            assert_eq!(
                failure_from_message(&redacted_failure(&failure)),
                Some(failure)
            );
        }
        assert!(failure_from_message("[CNT9] SECRET_SENTINEL").is_none());
        assert!(failure_from_message("[STC1] TypeDB commit rejected").is_none());
        assert!(failure_from_message("[NEW999] TypeDB commit rejected").is_none());
        assert!(
            failure_from_message("[CNT9] TypeDB query rejected: unique constraint SECRET_SENTINEL")
                .is_none()
        );
    }
    #[test]
    fn caller_prefixes_cannot_forge_a_typed_failure() {
        let rendered = redacted_failure(&classify_commit("STC2", "", true));
        for prefix in [
            "",
            "Commit failed: ",
            "Query failed: ",
            "Row stream failed: ",
            "Document stream failed: ",
        ] {
            assert!(
                failure_from_message(&format!("{prefix}{rendered}")).is_some(),
                "{prefix}"
            );
        }
        for message in [
            format!("SECRET_SENTINEL {rendered}"),
            format!("Unknown transaction type: {rendered}"),
            format!("Dynamic put for person references unknown attribute {rendered}"),
            format!("Commit failed: SECRET_SENTINEL {rendered}"),
            format!("Commit failed: Commit failed: {rendered}"),
        ] {
            assert!(failure_from_message(&message).is_none(), "{message}");
        }
    }
    #[test]
    fn code_validation_rejects_values_and_overlong_or_invalid_codes() {
        for code in [
            "",
            "CNT",
            "CNT-9",
            "CNT9/value",
            "CNT９",
            "CXNN06",
            "SECRET_SENTINEL",
        ] {
            assert!(Code::new(code).is_none());
        }
        assert!(Code::new("CNT9").is_some());
        assert_eq!(
            classify_commit(
                "[CXN06]",
                include_str!("../tests/fixtures/errors-3.13.6/paused.txt"),
                false
            )
            .code(),
            Some("CXN06")
        );
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;
    #[test]
    fn captured_3136_traces_have_value_free_typed_outcomes() {
        for (code, text, phase, expected) in [
            (
                "STC2",
                include_str!("../tests/fixtures/errors-3.13.6/stc2.txt"),
                Phase::Commit,
                "conflict",
            ),
            (
                "STC2",
                include_str!("../tests/fixtures/errors-3.13.6/require_deleted_key.txt"),
                Phase::Commit,
                "conflict",
            ),
            (
                "STC2",
                include_str!("../tests/fixtures/errors-3.13.6/deleting_required_key.txt"),
                Phase::Commit,
                "conflict",
            ),
            (
                "CNT5",
                include_str!("../tests/fixtures/errors-3.13.6/cnt5.txt"),
                Phase::Commit,
                "cardinality",
            ),
            (
                "CNT5",
                include_str!("../tests/fixtures/errors-3.13.6/cnt5_plays.txt"),
                Phase::Commit,
                "cardinality",
            ),
            (
                "CNT9",
                include_str!("../tests/fixtures/errors-3.13.6/cnt9.txt"),
                Phase::Query,
                "unique",
            ),
            (
                "CNT6",
                include_str!("../tests/fixtures/errors-3.13.6/cnt6_regex.txt"),
                Phase::Query,
                "constraint",
            ),
            (
                "CNT7",
                include_str!("../tests/fixtures/errors-3.13.6/cnt7_range.txt"),
                Phase::Query,
                "constraint",
            ),
        ] {
            let failure = if phase == Phase::Query {
                classify_query(code, text, true).unwrap()
            } else {
                classify_commit(code, text, true)
            };
            let details = failure.host_json();
            assert_eq!(details["reason"], expected);
            assert_eq!(details["code"], code);
            assert_eq!(details["retry_safe"], true);
            assert!(!format!("{failure:?}").contains("redacted"));
            assert_eq!(
                failure_from_message(&redacted_failure(&failure)),
                Some(failure)
            );
        }
        assert_eq!(
            classify_commit(
                "STC2",
                include_str!("../tests/fixtures/errors-3.13.6/require_deleted_key.txt"),
                true
            )
            .host_json()["lock_kind"],
            "require_deleted_key"
        );
        assert_eq!(
            classify_commit(
                "STC2",
                include_str!("../tests/fixtures/errors-3.13.6/deleting_required_key.txt"),
                true
            )
            .host_json()["lock_kind"],
            "deleting_required_key"
        );
    }
}
