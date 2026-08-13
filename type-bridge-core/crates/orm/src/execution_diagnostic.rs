//! Redacted lowering into the binding-neutral SDK execution diagnostic.

use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticDetailValue, DiagnosticPathSegment,
};
use type_bridge_contract::sdk_diagnostic::{
    SdkCommitFailureOutcome, SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticMessage,
    SdkDiagnosticName, SdkDiagnosticPathSegment, SdkExecutionDiagnostic, SdkProviderOperation,
    SdkQueryDiagnosticCategory, SdkQueryDiagnosticIdentity, SdkQueryDiagnosticPathKind,
};
use type_bridge_core_lib::version::VersionError;

use crate::_schema::SchemaError;
use crate::error::{ClassifiedCommitError, CommitFailureCertainty, OrmError};
use crate::hooks::HookError;
use crate::match_request::result_validation::exactly_one_cardinality_error;
use crate::match_request::selected_result_executor::released_model_execution_error;
use crate::match_request::{
    MatchError, MatchErrorCategory, MatchErrorDetailValue, MatchErrorPathSegment,
};

pub(crate) fn lower_orm_error(
    error: &OrmError,
    provider_operation: SdkProviderOperation,
) -> SdkExecutionDiagnostic {
    match error {
        OrmError::Match(error) => lower_match_error(error),
        OrmError::UnsupportedVersion(error) => match error {
            VersionError::Probe(_) | VersionError::Parse(_) => {
                SdkExecutionDiagnostic::provider_failure(provider_operation)
            }
            VersionError::Unsupported { .. }
            | VersionError::BandMismatch { .. }
            | VersionError::EmbeddedUnavailable { .. }
            | VersionError::FeatureUnsupported { .. } => unsupported(
                "provider_version_unsupported",
                "The provider version is outside the supported execution window",
            ),
        },
        OrmError::Connection(_) | OrmError::QueryExecution(_) => {
            SdkExecutionDiagnostic::provider_failure(provider_operation)
        }
        OrmError::Transaction(_) => transaction(
            "transaction_state_invalid",
            "The transaction state does not permit the requested operation",
        ),
        OrmError::Hydration { .. } => integrity(
            "provider_hydration_failed",
            "Provider evidence could not be hydrated as the projected model",
        ),
        OrmError::NotFound(_) => invalid_input(
            "thing_not_found",
            "No exact projected thing exists for the supplied identity",
        ),
        OrmError::InvalidFilter(_) => invalid_input(
            "invalid_operation_input",
            "The operation input does not satisfy the projected model contract",
        ),
        OrmError::DescriptorValidation { .. }
        | OrmError::DescriptorConflict { .. }
        | OrmError::DescriptorNotFound(_) => integrity(
            "runtime_projection_mismatch",
            "Runtime descriptor state does not match the installed projection",
        ),
        OrmError::Compilation(_) | OrmError::Serialization(_) => {
            SdkExecutionDiagnostic::internal_failure()
        }
        OrmError::Schema(error) => match error {
            SchemaError::Validation { .. } | SchemaError::Conflict { .. } => integrity(
                "schema_authority_mismatch",
                "Schema authority does not match the installed runtime projection",
            ),
            SchemaError::Sync(_) => {
                SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Schema)
            }
        },
        OrmError::Hook(error) => match error {
            HookError::Rejected { .. } => invalid_input(
                "operation_rejected",
                "The operation was rejected before provider completion",
            ),
            HookError::Internal { .. } => SdkExecutionDiagnostic::internal_failure(),
        },
    }
}

/// Construct the stable cross-binding diagnostic for an explicitly closed
/// generated-query resource.
///
/// The helper is binding-neutral so Rust, Node, Python, and native adapters do
/// not invent different categories, codes, or messages for the same lifecycle
/// fence.
#[doc(hidden)]
#[must_use]
pub fn query_resource_closed_diagnostic() -> SdkExecutionDiagnostic {
    invalid_input(
        "query_resource_closed",
        "The generated query resource is closed",
    )
}

pub(crate) fn lower_commit_error(error: &ClassifiedCommitError) -> SdkExecutionDiagnostic {
    let outcome = match error {
        ClassifiedCommitError::Driver {
            certainty: CommitFailureCertainty::DefinitelyAborted,
            ..
        } => SdkCommitFailureOutcome::DefinitelyAborted,
        ClassifiedCommitError::Driver {
            certainty: CommitFailureCertainty::Unknown,
            ..
        }
        | ClassifiedCommitError::Orm(_) => SdkCommitFailureOutcome::Unknown,
    };
    SdkExecutionDiagnostic::commit_failure(outcome)
}

/// Lower one ORM execution failure into the stable redacted SDK diagnostic.
///
/// This hidden cross-binding seam intentionally consumes and discards the
/// provider cause so ABI implementations cannot accidentally expose it.
#[doc(hidden)]
#[must_use]
pub fn lower_execution_error(
    error: OrmError,
    provider_operation: SdkProviderOperation,
) -> SdkExecutionDiagnostic {
    lower_orm_error(&error, provider_operation)
}

/// Lower one classified commit failure into the stable redacted SDK diagnostic.
///
/// The returned value retains commit certainty but never provider text.
#[doc(hidden)]
#[must_use]
pub fn lower_classified_commit_error(error: ClassifiedCommitError) -> SdkExecutionDiagnostic {
    lower_commit_error(&error)
}

/// Lower one canonical typed-query failure without collapsing its stable
/// category, code, typed path, or admissible deterministic details.
///
/// Provider/category messages are discarded. Detail keys that may contain
/// parser causes, credentials, query values, endpoints, or other free text are
/// omitted rather than flattened into the SDK diagnostic.
#[doc(hidden)]
#[must_use]
pub fn lower_match_error(error: &MatchError) -> SdkExecutionDiagnostic {
    let category = match error.category() {
        MatchErrorCategory::InvalidPlan => SdkQueryDiagnosticCategory::InvalidPlan,
        MatchErrorCategory::Cardinality => SdkQueryDiagnosticCategory::Cardinality,
        MatchErrorCategory::UnsupportedCapability => {
            SdkQueryDiagnosticCategory::UnsupportedCapability
        }
        MatchErrorCategory::StaleSchema => SdkQueryDiagnosticCategory::StaleSchema,
        MatchErrorCategory::ResourceLimit => SdkQueryDiagnosticCategory::ResourceLimit,
        MatchErrorCategory::Cancelled => SdkQueryDiagnosticCategory::Cancelled,
        MatchErrorCategory::Provider => SdkQueryDiagnosticCategory::Provider,
        MatchErrorCategory::ResultDecode => SdkQueryDiagnosticCategory::ResultDecode,
    };
    let code = SdkDiagnosticCode::new(error.code().as_str().to_owned()).unwrap_or_else(|_| {
        SdkDiagnosticCode::new("invalid_query_diagnostic_code")
            .expect("fallback typed query code is canonical")
    });
    let mut diagnostic = SdkExecutionDiagnostic::query_failure(category, code);
    for segment in error.path().segments() {
        let Some(segment) = lower_match_path(segment) else {
            return SdkExecutionDiagnostic::internal_failure();
        };
        diagnostic = diagnostic
            .try_at(segment)
            .expect("canonical match paths fit the SDK path ceiling");
    }
    for (key, value) in error.details() {
        let Ok(key) = SdkDiagnosticName::new(key.clone()) else {
            continue;
        };
        let Some(value) = lower_match_detail(key.as_str(), value) else {
            continue;
        };
        diagnostic = diagnostic
            .try_with_detail(key, value)
            .expect("canonical match details fit the SDK detail ceiling");
    }
    diagnostic
}

/// Lower one authenticated remote-query diagnostic into the stable SDK
/// diagnostic without retaining its dynamic message or arbitrary text.
///
/// Internal model-executor codes first regain their released compatibility
/// surface. Other canonical codes, category-equivalent query classification,
/// typed paths, numeric/Boolean details, and closed identity details survive.
/// Invalid path identities or bounded SDK collection overflow fail closed as
/// an internal diagnostic instead of publishing a structurally incomplete
/// error.
#[doc(hidden)]
#[must_use]
pub fn lower_remote_query_diagnostic(error: Diagnostic) -> SdkExecutionDiagnostic {
    if error.code().as_str() == "query_v2_model_exactly_one" {
        let Some(actual) = error.details().get("actual").and_then(|value| match value {
            DiagnosticDetailValue::Long(value) => usize::try_from(*value).ok(),
            DiagnosticDetailValue::Text(_)
            | DiagnosticDetailValue::Boolean(_)
            | DiagnosticDetailValue::TextList(_) => None,
        }) else {
            return SdkExecutionDiagnostic::internal_failure();
        };
        let Some(error) = exactly_one_cardinality_error(actual) else {
            return SdkExecutionDiagnostic::internal_failure();
        };
        return lower_match_error(&error);
    }

    if error.code().as_str().starts_with("query_v2_model_") {
        return lower_match_error(&released_model_execution_error(&error));
    }

    let category = match error.category() {
        DiagnosticCategory::InvalidContract => SdkQueryDiagnosticCategory::InvalidPlan,
        DiagnosticCategory::UnsupportedCapability => {
            SdkQueryDiagnosticCategory::UnsupportedCapability
        }
        DiagnosticCategory::ResourceLimit => SdkQueryDiagnosticCategory::ResourceLimit,
        DiagnosticCategory::Cancelled => SdkQueryDiagnosticCategory::Cancelled,
        DiagnosticCategory::Integrity => SdkQueryDiagnosticCategory::ResultDecode,
    };
    let Ok(code) = SdkDiagnosticCode::new(error.code().as_str().to_owned()) else {
        return SdkExecutionDiagnostic::internal_failure();
    };
    let mut diagnostic = SdkExecutionDiagnostic::query_failure(category, code);
    for segment in error.path().segments() {
        let segment = match segment {
            DiagnosticPathSegment::Field(value) => {
                let Ok(value) = SdkQueryDiagnosticIdentity::new(value.clone()) else {
                    return SdkExecutionDiagnostic::internal_failure();
                };
                SdkDiagnosticPathSegment::ContractField(value)
            }
            DiagnosticPathSegment::Index(index) => SdkDiagnosticPathSegment::Index(*index),
            DiagnosticPathSegment::Identifier(value) => {
                let Ok(value) = SdkQueryDiagnosticIdentity::new(value.clone()) else {
                    return SdkExecutionDiagnostic::internal_failure();
                };
                SdkDiagnosticPathSegment::ContractIdentity(value)
            }
        };
        let Ok(next) = diagnostic.try_at(segment) else {
            return SdkExecutionDiagnostic::internal_failure();
        };
        diagnostic = next;
    }
    for (key, value) in error.details() {
        if key == "query_category" {
            continue;
        }
        let Ok(name) = SdkDiagnosticName::new(key.clone()) else {
            continue;
        };
        let Some(value) = lower_remote_detail(name.as_str(), value) else {
            continue;
        };
        let Ok(next) = diagnostic.try_with_detail(name, value) else {
            return SdkExecutionDiagnostic::internal_failure();
        };
        diagnostic = next;
    }
    diagnostic
}

fn lower_remote_detail(
    key: &str,
    value: &DiagnosticDetailValue,
) -> Option<SdkDiagnosticDetailValue> {
    match value {
        DiagnosticDetailValue::Long(value) => Some(SdkDiagnosticDetailValue::Signed(*value)),
        DiagnosticDetailValue::Boolean(value) => {
            Some(SdkDiagnosticDetailValue::Boolean(*value))
        }
        DiagnosticDetailValue::Text(value) if is_admissible_remote_identity_detail(key) => {
            SdkQueryDiagnosticIdentity::new(value.clone())
                .ok()
                .map(SdkDiagnosticDetailValue::QueryIdentity)
        }
        DiagnosticDetailValue::TextList(values)
            if is_admissible_remote_identity_list_detail(key)
                && values.len()
                    <= type_bridge_contract::sdk_diagnostic::MAX_SDK_QUERY_DIAGNOSTIC_IDENTITY_LIST =>
        {
            values
                .iter()
                .cloned()
                .map(SdkQueryDiagnosticIdentity::new)
                .collect::<Result<Vec<_>, _>>()
                .ok()
                .map(SdkDiagnosticDetailValue::QueryIdentityList)
        }
        DiagnosticDetailValue::Text(_) | DiagnosticDetailValue::TextList(_) => None,
    }
}

fn is_admissible_remote_identity_detail(key: &str) -> bool {
    matches!(
        key,
        "actual"
            | "actual_category"
            | "actual_type_label"
            | "actual_value_type"
            | "binding"
            | "capability"
            | "concrete"
            | "declared"
            | "descriptor"
            | "expected"
            | "field_name"
            | "field_owner"
            | "format"
            | "identity_kind"
            | "operation"
            | "result_kind"
            | "scope"
            | "subject"
    )
}

fn is_admissible_remote_identity_list_detail(key: &str) -> bool {
    matches!(
        key,
        "allowed"
            | "allowed_type_domains"
            | "expected"
            | "missing"
            | "required"
            | "unexpected_fields"
            | "unreachable_bindings"
    )
}

fn lower_match_path(segment: &MatchErrorPathSegment) -> Option<SdkDiagnosticPathSegment> {
    Some(match segment {
        MatchErrorPathSegment::Request => {
            SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Request)
        }
        MatchErrorPathSegment::Plan => {
            SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Plan)
        }
        MatchErrorPathSegment::Operation => {
            SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Operation)
        }
        MatchErrorPathSegment::Predicate => {
            SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Predicate)
        }
        MatchErrorPathSegment::Output => {
            SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Output)
        }
        MatchErrorPathSegment::ProviderEvidence => {
            SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::ProviderEvidence)
        }
        MatchErrorPathSegment::Result => {
            SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Result)
        }
        MatchErrorPathSegment::Binding(binding) => {
            SdkDiagnosticPathSegment::QueryBinding(binding.get())
        }
        MatchErrorPathSegment::Field(field) => SdkDiagnosticPathSegment::QueryField {
            owner: SdkQueryDiagnosticIdentity::new(field.owner.as_str()).ok()?,
            name: SdkQueryDiagnosticIdentity::new(field.name.clone()).ok()?,
        },
        MatchErrorPathSegment::Role(role) => SdkDiagnosticPathSegment::QueryRole {
            owner: SdkQueryDiagnosticIdentity::new(role.owner.as_str()).ok()?,
            name: SdkQueryDiagnosticIdentity::new(role.name.clone()).ok()?,
        },
        MatchErrorPathSegment::RoleEdge(edge) => {
            SdkDiagnosticPathSegment::QueryRoleEdge(edge.get())
        }
        MatchErrorPathSegment::OutputSlot(slot) => {
            SdkDiagnosticPathSegment::QueryOutputSlot(u64::try_from(*slot).ok()?)
        }
        MatchErrorPathSegment::OutputName(name) => SdkDiagnosticPathSegment::QueryOutputName(
            SdkQueryDiagnosticIdentity::new(name.clone()).ok()?,
        ),
        MatchErrorPathSegment::Index(index) => {
            SdkDiagnosticPathSegment::Index(u64::try_from(*index).ok()?)
        }
    })
}

fn lower_match_detail(
    key: &str,
    value: &MatchErrorDetailValue,
) -> Option<SdkDiagnosticDetailValue> {
    match value {
        MatchErrorDetailValue::Unsigned(value) => {
            if key.ends_with("_bytes") {
                Some(SdkDiagnosticDetailValue::ByteCount(*value))
            } else {
                Some(SdkDiagnosticDetailValue::Count(*value))
            }
        }
        MatchErrorDetailValue::Signed(value) => Some(SdkDiagnosticDetailValue::Signed(*value)),
        MatchErrorDetailValue::Boolean(value) => Some(SdkDiagnosticDetailValue::Boolean(*value)),
        MatchErrorDetailValue::Text(value) if is_admissible_query_identity_detail(key) => {
            SdkQueryDiagnosticIdentity::new(value.clone())
                .ok()
                .map(SdkDiagnosticDetailValue::QueryIdentity)
        }
        MatchErrorDetailValue::TextList(values)
            if is_admissible_query_identity_list_detail(key) =>
        {
            if values.len()
                > type_bridge_contract::sdk_diagnostic::MAX_SDK_QUERY_DIAGNOSTIC_IDENTITY_LIST
            {
                return None;
            }
            values
                .iter()
                .cloned()
                .map(SdkQueryDiagnosticIdentity::new)
                .collect::<Result<Vec<_>, _>>()
                .ok()
                .map(SdkDiagnosticDetailValue::QueryIdentityList)
        }
        MatchErrorDetailValue::Text(_) | MatchErrorDetailValue::TextList(_) => None,
    }
}

fn is_admissible_query_identity_detail(key: &str) -> bool {
    matches!(
        key,
        "actual"
            | "actual_category"
            | "actual_type_label"
            | "actual_value_type"
            | "binding"
            | "capability"
            | "concrete"
            | "declared"
            | "descriptor"
            | "expected"
            | "field_name"
            | "field_owner"
            | "identity_kind"
            | "limit"
            | "scope"
    )
}

fn is_admissible_query_identity_list_detail(key: &str) -> bool {
    matches!(
        key,
        "allowed_type_domains" | "missing" | "unexpected_fields" | "unreachable_bindings"
    )
}

fn invalid_input(code: &'static str, message: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(sdk_code(code), sdk_message(message))
}

fn unsupported(code: &'static str, message: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::unsupported_capability(sdk_code(code), sdk_message(message))
}

fn integrity(code: &'static str, message: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::integrity(sdk_code(code), sdk_message(message))
}

fn transaction(code: &'static str, message: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::transaction_failure(sdk_code(code), sdk_message(message))
}

fn sdk_code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static ORM SDK diagnostic code is canonical")
}

fn sdk_message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static ORM SDK diagnostic message is valid")
}

#[cfg(test)]
mod tests {
    use type_bridge_contract::sdk_diagnostic::{
        SdkDiagnosticCategory, SdkDiagnosticDetailValue, SdkQueryDiagnosticCategory,
        SdkQueryDiagnosticPathKind,
    };
    use type_bridge_core_lib::version::Version;

    use super::*;

    #[test]
    fn transaction_lifecycle_errors_keep_the_frozen_category_and_redaction() {
        const SECRET: &str = "transaction-provider-secret";
        let diagnostic = lower_execution_error(
            OrmError::Transaction(SECRET.to_owned()),
            SdkProviderOperation::Read,
        );
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Transaction);
        assert_eq!(diagnostic.code().as_str(), "transaction_state_invalid");
        assert!(!format!("{diagnostic:?} {diagnostic}").contains(SECRET));
    }

    #[test]
    fn version_probe_and_parse_failures_are_redacted_provider_failures() {
        const SECRET: &str = "https://user:password@provider.invalid/v1/version";
        for error in [
            VersionError::Probe(SECRET.to_owned()),
            VersionError::Parse(SECRET.to_owned()),
        ] {
            let diagnostic = lower_execution_error(
                OrmError::UnsupportedVersion(error),
                SdkProviderOperation::Schema,
            );
            assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Provider);
            assert_eq!(diagnostic.code().as_str(), "provider_operation_failed");
            assert_eq!(
                diagnostic.details().values().next(),
                Some(&SdkDiagnosticDetailValue::ProviderOperation(
                    SdkProviderOperation::Schema
                ))
            );
            assert!(!format!("{diagnostic:?} {diagnostic}").contains(SECRET));
        }
    }

    #[test]
    fn true_version_incompatibility_remains_unsupported() {
        let diagnostic = lower_execution_error(
            OrmError::UnsupportedVersion(VersionError::Unsupported {
                component: "server",
                found: Version::new(4, 0, 0),
            }),
            SdkProviderOperation::Connect,
        );
        assert_eq!(
            diagnostic.category(),
            SdkDiagnosticCategory::UnsupportedCapability
        );
        assert_eq!(diagnostic.code().as_str(), "provider_version_unsupported");
    }

    #[test]
    fn match_categories_retain_the_exact_query_category_and_stable_code() {
        let cases = [
            (
                MatchErrorCategory::InvalidPlan,
                SdkDiagnosticCategory::InvalidInput,
                SdkQueryDiagnosticCategory::InvalidPlan,
            ),
            (
                MatchErrorCategory::Cardinality,
                SdkDiagnosticCategory::InvalidInput,
                SdkQueryDiagnosticCategory::Cardinality,
            ),
            (
                MatchErrorCategory::UnsupportedCapability,
                SdkDiagnosticCategory::UnsupportedCapability,
                SdkQueryDiagnosticCategory::UnsupportedCapability,
            ),
            (
                MatchErrorCategory::StaleSchema,
                SdkDiagnosticCategory::Integrity,
                SdkQueryDiagnosticCategory::StaleSchema,
            ),
            (
                MatchErrorCategory::ResourceLimit,
                SdkDiagnosticCategory::ResourceLimit,
                SdkQueryDiagnosticCategory::ResourceLimit,
            ),
            (
                MatchErrorCategory::Cancelled,
                SdkDiagnosticCategory::Cancelled,
                SdkQueryDiagnosticCategory::Cancelled,
            ),
            (
                MatchErrorCategory::Provider,
                SdkDiagnosticCategory::Provider,
                SdkQueryDiagnosticCategory::Provider,
            ),
            (
                MatchErrorCategory::ResultDecode,
                SdkDiagnosticCategory::Integrity,
                SdkQueryDiagnosticCategory::ResultDecode,
            ),
        ];

        for (match_category, sdk_category, query_category) in cases {
            let diagnostic = lower_match_error(&MatchError::new(
                match_category,
                "stable_query_code",
                "dynamic message is discarded",
            ));
            assert_eq!(diagnostic.category(), sdk_category);
            assert_eq!(diagnostic.code().as_str(), "stable_query_code");
            assert_eq!(
                diagnostic
                    .details()
                    .get(&SdkDiagnosticName::new("query_category").unwrap()),
                Some(&SdkDiagnosticDetailValue::QueryCategory(query_category))
            );
        }
    }

    #[test]
    fn schema_function_given_capability_retains_direct_sdk_structure() {
        let diagnostic = lower_match_error(
            &MatchError::new(
                MatchErrorCategory::UnsupportedCapability,
                "schema_function_given_rows_unsupported",
                "dynamic provider context is discarded",
            )
            .at(MatchErrorPathSegment::Operation)
            .with_detail("capability", "query.input.given-rows"),
        );

        assert_eq!(
            diagnostic.category(),
            SdkDiagnosticCategory::UnsupportedCapability
        );
        assert_eq!(
            diagnostic.code().as_str(),
            "schema_function_given_rows_unsupported"
        );
        assert_eq!(
            diagnostic.path(),
            &[SdkDiagnosticPathSegment::Query(
                SdkQueryDiagnosticPathKind::Operation
            )]
        );
        assert_eq!(
            diagnostic
                .details()
                .get(&SdkDiagnosticName::new("capability").unwrap()),
            Some(&SdkDiagnosticDetailValue::QueryIdentity(
                SdkQueryDiagnosticIdentity::new("query.input.given-rows").unwrap()
            ))
        );
    }

    #[test]
    fn in_flight_cancellation_is_cancelled_while_timeout_remains_resource_limit() {
        for (category, code, sdk_category, query_category) in [
            (
                MatchErrorCategory::Cancelled,
                "provider_cancelled",
                SdkDiagnosticCategory::Cancelled,
                SdkQueryDiagnosticCategory::Cancelled,
            ),
            (
                MatchErrorCategory::ResourceLimit,
                "transaction_deadline_exceeded",
                SdkDiagnosticCategory::ResourceLimit,
                SdkQueryDiagnosticCategory::ResourceLimit,
            ),
        ] {
            let diagnostic = lower_match_error(
                &MatchError::new(category, code, "dynamic provider context is discarded")
                    .at(MatchErrorPathSegment::ProviderEvidence),
            );
            assert_eq!(diagnostic.category(), sdk_category);
            assert_eq!(diagnostic.code().as_str(), code);
            assert_eq!(
                diagnostic.path(),
                &[SdkDiagnosticPathSegment::Query(
                    SdkQueryDiagnosticPathKind::ProviderEvidence
                )]
            );
            assert_eq!(
                diagnostic
                    .details()
                    .get(&SdkDiagnosticName::new("query_category").unwrap()),
                Some(&SdkDiagnosticDetailValue::QueryCategory(query_category))
            );
        }
    }

    #[test]
    fn authenticated_remote_exactly_one_uses_the_released_cardinality_surface() {
        for (actual, code) in [(0_i64, "no_result"), (2_i64, "not_unique")] {
            let error = Diagnostic::new(
                DiagnosticCategory::InvalidContract,
                type_bridge_contract::diagnostic::DiagnosticCode::new("query_v2_model_exactly_one")
                    .unwrap(),
                "internal model execution detail",
            )
            .with_detail("actual", actual);

            let diagnostic = lower_remote_query_diagnostic(error);
            assert_eq!(diagnostic.category(), SdkDiagnosticCategory::InvalidInput);
            assert_eq!(diagnostic.code().as_str(), code);
            assert_eq!(
                diagnostic.message().as_str(),
                "The typed query result does not satisfy the requested cardinality"
            );
            assert_eq!(
                diagnostic.path(),
                &[SdkDiagnosticPathSegment::Query(
                    SdkQueryDiagnosticPathKind::Result
                )]
            );
            assert_eq!(
                diagnostic
                    .details()
                    .get(&SdkDiagnosticName::new("actual").unwrap()),
                Some(&SdkDiagnosticDetailValue::Count(actual as u64))
            );
            assert_eq!(
                diagnostic
                    .details()
                    .get(&SdkDiagnosticName::new("query_category").unwrap()),
                Some(&SdkDiagnosticDetailValue::QueryCategory(
                    SdkQueryDiagnosticCategory::Cardinality
                ))
            );
        }
    }

    #[test]
    fn authenticated_remote_model_limit_uses_the_released_resource_surface() {
        let error = Diagnostic::new(
            DiagnosticCategory::ResourceLimit,
            type_bridge_contract::diagnostic::DiagnosticCode::new(
                "query_v2_model_role_player_limit",
            )
            .unwrap(),
            "internal model execution detail",
        );

        let diagnostic = lower_remote_query_diagnostic(error);
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::ResourceLimit);
        assert_eq!(diagnostic.code().as_str(), "hydrated_role_player_limit");
        assert_eq!(
            diagnostic.path(),
            &[SdkDiagnosticPathSegment::Query(
                SdkQueryDiagnosticPathKind::ProviderEvidence
            )]
        );
        assert_eq!(
            diagnostic
                .details()
                .get(&SdkDiagnosticName::new("query_category").unwrap()),
            Some(&SdkDiagnosticDetailValue::QueryCategory(
                SdkQueryDiagnosticCategory::ResourceLimit
            ))
        );
    }

    #[test]
    fn malformed_remote_exactly_one_proof_fails_closed() {
        for error in [
            Diagnostic::new(
                DiagnosticCategory::InvalidContract,
                type_bridge_contract::diagnostic::DiagnosticCode::new("query_v2_model_exactly_one")
                    .unwrap(),
                "missing cardinality detail",
            ),
            Diagnostic::new(
                DiagnosticCategory::InvalidContract,
                type_bridge_contract::diagnostic::DiagnosticCode::new("query_v2_model_exactly_one")
                    .unwrap(),
                "impossible exactly-one success",
            )
            .with_detail("actual", 1_i64),
        ] {
            let diagnostic = lower_remote_query_diagnostic(error);
            assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Internal);
            assert_eq!(diagnostic.code().as_str(), "internal_failure");
        }
    }

    #[test]
    fn authenticated_remote_diagnostic_preserves_structure_and_redacts_free_text() {
        const SECRET: &str = "https://user:password@remote.invalid/private";
        let error = Diagnostic::new(
            DiagnosticCategory::Integrity,
            type_bridge_contract::diagnostic::DiagnosticCode::new("query_remote_evidence_mismatch")
                .unwrap(),
            SECRET,
        )
        .with_path(
            type_bridge_contract::diagnostic::DiagnosticPath::from_segments([
                DiagnosticPathSegment::Field("outcome".to_owned()),
                DiagnosticPathSegment::Index(2),
                DiagnosticPathSegment::Identifier("person".to_owned()),
            ]),
        )
        .with_detail("attempt", 7_i64)
        .with_detail("expected", vec!["person".to_owned(), "employee".to_owned()])
        .with_detail("retryable", false)
        .with_detail("subject", "person")
        .with_detail("cause", SECRET);

        let diagnostic = lower_remote_query_diagnostic(error);
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
        assert_eq!(diagnostic.code().as_str(), "query_remote_evidence_mismatch");
        assert_eq!(
            diagnostic.path(),
            &[
                SdkDiagnosticPathSegment::ContractField(
                    SdkQueryDiagnosticIdentity::new("outcome").unwrap()
                ),
                SdkDiagnosticPathSegment::Index(2),
                SdkDiagnosticPathSegment::ContractIdentity(
                    SdkQueryDiagnosticIdentity::new("person").unwrap()
                ),
            ]
        );
        assert_eq!(
            diagnostic
                .details()
                .get(&SdkDiagnosticName::new("attempt").unwrap()),
            Some(&SdkDiagnosticDetailValue::Signed(7))
        );
        assert_eq!(
            diagnostic
                .details()
                .get(&SdkDiagnosticName::new("subject").unwrap()),
            Some(&SdkDiagnosticDetailValue::QueryIdentity(
                SdkQueryDiagnosticIdentity::new("person").unwrap()
            ))
        );
        assert_eq!(
            diagnostic
                .details()
                .get(&SdkDiagnosticName::new("expected").unwrap()),
            Some(&SdkDiagnosticDetailValue::QueryIdentityList(vec![
                SdkQueryDiagnosticIdentity::new("person").unwrap(),
                SdkQueryDiagnosticIdentity::new("employee").unwrap(),
            ]))
        );
        assert!(
            !diagnostic
                .details()
                .contains_key(&SdkDiagnosticName::new("cause").unwrap())
        );
        assert!(!format!("{diagnostic:?} {diagnostic}").contains(SECRET));
    }

    #[test]
    fn match_lowering_preserves_structural_path_and_admissible_details_only() {
        const SECRET: &str = "https://user:password@provider.invalid/private";
        let field = crate::match_request::FieldId::new(
            crate::match_request::DescriptorId::new("entity:person"),
            "display-name",
        );
        let role = crate::match_request::RoleId::new(
            crate::match_request::DescriptorId::new("relation:membership"),
            "member",
        );
        let error = MatchError::new(
            MatchErrorCategory::ResultDecode,
            "hydrated_result_mismatch",
            SECRET,
        )
        .with_path(crate::match_request::MatchErrorPath::from_segments([
            MatchErrorPathSegment::Request,
            MatchErrorPathSegment::Plan,
            MatchErrorPathSegment::Operation,
            MatchErrorPathSegment::Predicate,
            MatchErrorPathSegment::Output,
            MatchErrorPathSegment::ProviderEvidence,
            MatchErrorPathSegment::Result,
            MatchErrorPathSegment::Binding(crate::match_request::BindingId::new(2)),
            MatchErrorPathSegment::Field(field),
            MatchErrorPathSegment::Role(role),
            MatchErrorPathSegment::RoleEdge(crate::match_request::RoleEdgeId::new(4)),
            MatchErrorPathSegment::OutputSlot(3),
            MatchErrorPathSegment::OutputName("people".to_owned()),
            MatchErrorPathSegment::Index(9),
        ]))
        .with_detail("actual", "entity:employee")
        .with_detail("answer_bytes", 17_u64)
        .with_detail("delta", MatchErrorDetailValue::Signed(-3))
        .with_detail("complete", true)
        .with_detail("missing", vec!["email".to_owned(), "identifier".to_owned()])
        .with_detail("cause", SECRET)
        .with_detail("endpoint", SECRET)
        .with_detail("credential", SECRET)
        .with_detail("arbitrary_text", "must-not-cross");

        let diagnostic = lower_match_error(&error);
        assert_eq!(diagnostic.code().as_str(), "hydrated_result_mismatch");
        assert_eq!(
            diagnostic.path(),
            &[
                SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Request),
                SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Plan),
                SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Operation),
                SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Predicate),
                SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Output),
                SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::ProviderEvidence),
                SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::Result),
                SdkDiagnosticPathSegment::QueryBinding(2),
                SdkDiagnosticPathSegment::QueryField {
                    owner: SdkQueryDiagnosticIdentity::new("entity:person").unwrap(),
                    name: SdkQueryDiagnosticIdentity::new("display-name").unwrap(),
                },
                SdkDiagnosticPathSegment::QueryRole {
                    owner: SdkQueryDiagnosticIdentity::new("relation:membership").unwrap(),
                    name: SdkQueryDiagnosticIdentity::new("member").unwrap(),
                },
                SdkDiagnosticPathSegment::QueryRoleEdge(4),
                SdkDiagnosticPathSegment::QueryOutputSlot(3),
                SdkDiagnosticPathSegment::QueryOutputName(
                    SdkQueryDiagnosticIdentity::new("people").unwrap(),
                ),
                SdkDiagnosticPathSegment::Index(9),
            ]
        );
        assert_eq!(
            diagnostic
                .details()
                .get(&SdkDiagnosticName::new("answer_bytes").unwrap()),
            Some(&SdkDiagnosticDetailValue::ByteCount(17))
        );
        assert_eq!(
            diagnostic
                .details()
                .get(&SdkDiagnosticName::new("delta").unwrap()),
            Some(&SdkDiagnosticDetailValue::Signed(-3))
        );
        for excluded in ["cause", "endpoint", "credential", "arbitrary_text"] {
            assert!(
                !diagnostic
                    .details()
                    .contains_key(&SdkDiagnosticName::new(excluded).unwrap())
            );
        }
        assert!(!format!("{diagnostic:?} {diagnostic}").contains(SECRET));
        assert!(!format!("{diagnostic:?}").contains("must-not-cross"));
    }
}
