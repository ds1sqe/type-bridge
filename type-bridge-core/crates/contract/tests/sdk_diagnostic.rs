use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::diagnostic::DiagnosticCategory;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::sdk_diagnostic::{
    MAX_SDK_DIAGNOSTIC_CODE_BYTES, MAX_SDK_DIAGNOSTIC_DETAILS, MAX_SDK_DIAGNOSTIC_MESSAGE_BYTES,
    MAX_SDK_DIAGNOSTIC_NAME_BYTES, MAX_SDK_DIAGNOSTIC_PATH_SEGMENTS, SdkCommitFailureOutcome,
    SdkDiagnosticBuildError, SdkDiagnosticCategory, SdkDiagnosticCode, SdkDiagnosticDetailValue,
    SdkDiagnosticMessage, SdkDiagnosticName, SdkDiagnosticPathSegment, SdkDiagnosticVersion,
    SdkExecutionDiagnostic, SdkProviderOperation, SdkQueryDiagnosticCategory,
    SdkQueryDiagnosticIdentity, SdkQueryDiagnosticPathKind,
};

fn code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("test code")
}

fn message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("test message")
}

fn name(value: &'static str) -> SdkDiagnosticName {
    SdkDiagnosticName::new(value).expect("test name")
}

fn leaked_text(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

#[test]
fn category_vocabulary_is_exact_without_changing_the_wire_category() {
    let categories = [
        SdkDiagnosticCategory::InvalidInput,
        SdkDiagnosticCategory::UnsupportedCapability,
        SdkDiagnosticCategory::ResourceLimit,
        SdkDiagnosticCategory::Integrity,
        SdkDiagnosticCategory::Provider,
        SdkDiagnosticCategory::Transaction,
        SdkDiagnosticCategory::Cancelled,
        SdkDiagnosticCategory::Internal,
    ];
    assert_eq!(
        categories.map(SdkDiagnosticCategory::as_str),
        [
            "invalid_input",
            "unsupported_capability",
            "resource_limit",
            "integrity",
            "provider",
            "transaction",
            "cancelled",
            "internal",
        ]
    );
    assert_eq!(SdkDiagnosticVersion::CURRENT.get(), 1);

    assert_eq!(
        DiagnosticCategory::InvalidContract.as_str(),
        "invalid_contract"
    );
    assert_eq!(
        DiagnosticCategory::UnsupportedCapability.as_str(),
        "unsupported_capability"
    );
}

#[test]
fn stable_components_reject_noncanonical_or_sensitive_shapes() {
    assert_eq!(
        SdkDiagnosticCode::new("").unwrap_err(),
        SdkDiagnosticBuildError::InvalidCode
    );
    for malformed in [
        "Invalid_input",
        "invalid-input",
        "invalid__input",
        "invalid_input_",
        "1invalid_input",
    ] {
        assert_eq!(
            SdkDiagnosticCode::new(malformed).unwrap_err(),
            SdkDiagnosticBuildError::InvalidCode
        );
    }
    assert!(SdkDiagnosticCode::new(leaked_text("a".repeat(MAX_SDK_DIAGNOSTIC_CODE_BYTES))).is_ok());
    assert_eq!(
        SdkDiagnosticCode::new(leaked_text("a".repeat(MAX_SDK_DIAGNOSTIC_CODE_BYTES + 1)))
            .unwrap_err(),
        SdkDiagnosticBuildError::InvalidCode
    );

    for unsafe_message in [
        "",
        " leading space",
        "trailing space ",
        "provider\nbacktrace",
        "/home/example/custom-root",
        "C:\\secret\\credentials",
    ] {
        assert_eq!(
            SdkDiagnosticMessage::new(unsafe_message).unwrap_err(),
            SdkDiagnosticBuildError::InvalidMessage
        );
    }
    assert!(
        SdkDiagnosticMessage::new(leaked_text("a".repeat(MAX_SDK_DIAGNOSTIC_MESSAGE_BYTES)))
            .is_ok()
    );
    assert_eq!(
        SdkDiagnosticMessage::new(leaked_text(
            "a".repeat(MAX_SDK_DIAGNOSTIC_MESSAGE_BYTES + 1)
        ))
        .unwrap_err(),
        SdkDiagnosticBuildError::InvalidMessage
    );

    assert!(SdkDiagnosticName::new(leaked_text("a".repeat(MAX_SDK_DIAGNOSTIC_NAME_BYTES))).is_ok());
    assert_eq!(
        SdkDiagnosticName::new("field.name").unwrap_err(),
        SdkDiagnosticBuildError::InvalidName
    );
}

#[test]
fn path_and_detail_context_is_typed_ordered_and_bounded() {
    let person = TypeId::new(TypeKind::Entity, "person").expect("person type");
    let owns_name = OwnsFactId::new(
        person.clone(),
        AttributeId::new("person-name").expect("attribute"),
    )
    .expect("owns identity");
    let role = RoleId::new("membership", "member").expect("role");
    let capability = CapabilityId::new("crud.entity-single").expect("capability");

    let diagnostic = SdkExecutionDiagnostic::invalid_input(
        code("invalid_projected_field"),
        message("The projected field does not belong to the selected model"),
    )
    .try_at(SdkDiagnosticPathSegment::Argument(name("input")))
    .expect("argument path")
    .try_at(SdkDiagnosticPathSegment::Index(2))
    .expect("index path")
    .try_at(SdkDiagnosticPathSegment::Type(person.clone()))
    .expect("type path")
    .try_at(SdkDiagnosticPathSegment::Field(owns_name.clone()))
    .expect("field path")
    .try_at(SdkDiagnosticPathSegment::Role(role.clone()))
    .expect("role path")
    .try_with_detail(
        name("required_capability"),
        SdkDiagnosticDetailValue::Capability(capability),
    )
    .expect("capability detail")
    .try_with_detail(name("actual_type"), SdkDiagnosticDetailValue::Type(person))
    .expect("type detail")
    .try_with_detail(
        name("actual_field"),
        SdkDiagnosticDetailValue::Field(owns_name),
    )
    .expect("field detail")
    .try_with_detail(name("actual_role"), SdkDiagnosticDetailValue::Role(role))
    .expect("role detail");

    assert_eq!(diagnostic.path().len(), 5);
    assert_eq!(
        diagnostic
            .details()
            .keys()
            .map(|key| key.as_str())
            .collect::<Vec<_>>(),
        [
            "actual_field",
            "actual_role",
            "actual_type",
            "required_capability"
        ]
    );

    let duplicate = diagnostic
        .clone()
        .try_with_detail(name("actual_type"), SdkDiagnosticDetailValue::Count(1));
    assert_eq!(
        duplicate.unwrap_err(),
        SdkDiagnosticBuildError::DuplicateDetail
    );

    let mut full_path =
        SdkExecutionDiagnostic::invalid_input(code("invalid_input"), message("Invalid input"));
    for index in 0..MAX_SDK_DIAGNOSTIC_PATH_SEGMENTS {
        full_path = full_path
            .try_at(SdkDiagnosticPathSegment::Index(index as u64))
            .expect("path within limit");
    }
    assert_eq!(
        full_path
            .try_at(SdkDiagnosticPathSegment::Index(0))
            .unwrap_err(),
        SdkDiagnosticBuildError::PathLimitExceeded
    );

    let mut full_details =
        SdkExecutionDiagnostic::invalid_input(code("invalid_input"), message("Invalid input"));
    for index in 0..MAX_SDK_DIAGNOSTIC_DETAILS {
        let key = leaked_text(format!("detail_{index}"));
        full_details = full_details
            .try_with_detail(name(key), SdkDiagnosticDetailValue::Count(index as u64))
            .expect("detail within limit");
    }
    assert_eq!(
        full_details
            .try_with_detail(name("one_more_detail"), SdkDiagnosticDetailValue::Count(0))
            .unwrap_err(),
        SdkDiagnosticBuildError::DetailLimitExceeded
    );
}

#[test]
fn provider_failure_is_redacted_and_commit_certainty_is_not_collapsed() {
    let provider = SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Write);
    assert_eq!(provider.version(), SdkDiagnosticVersion::V1);
    assert_eq!(provider.category(), SdkDiagnosticCategory::Provider);
    assert_eq!(provider.code().as_str(), "provider_operation_failed");
    assert_eq!(
        provider.message().as_str(),
        "The database provider could not complete the requested operation"
    );
    assert_eq!(
        provider.details().get(&name("operation")),
        Some(&SdkDiagnosticDetailValue::ProviderOperation(
            SdkProviderOperation::Write
        ))
    );

    let definitely_aborted =
        SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::DefinitelyAborted);
    let unknown = SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown);
    assert_eq!(
        definitely_aborted.code().as_str(),
        "commit_definitely_aborted"
    );
    assert_eq!(unknown.code().as_str(), "commit_outcome_unknown");
    assert_eq!(
        definitely_aborted.details().get(&name("commit_outcome")),
        Some(&SdkDiagnosticDetailValue::CommitOutcome(
            SdkCommitFailureOutcome::DefinitelyAborted
        ))
    );
    assert_eq!(
        unknown.details().get(&name("commit_outcome")),
        Some(&SdkDiagnosticDetailValue::CommitOutcome(
            SdkCommitFailureOutcome::Unknown
        ))
    );
    assert_ne!(definitely_aborted, unknown);
}

#[test]
fn fixed_cancellation_and_internal_failures_expose_no_runtime_context() {
    let cancelled = SdkExecutionDiagnostic::cancelled_before_dispatch();
    assert_eq!(cancelled.category(), SdkDiagnosticCategory::Cancelled);
    assert_eq!(cancelled.code().as_str(), "cancelled_before_dispatch");
    assert!(cancelled.path().is_empty());
    assert!(cancelled.details().is_empty());

    let internal = SdkExecutionDiagnostic::internal_failure();
    assert_eq!(internal.category(), SdkDiagnosticCategory::Internal);
    assert_eq!(internal.code().as_str(), "internal_failure");
    assert!(internal.path().is_empty());
    assert!(internal.details().is_empty());
    assert_eq!(
        internal.to_string(),
        "internal [internal_failure]: The operation failed inside the TypeBridge runtime"
    );
}

#[test]
fn query_failure_retains_bounded_owned_code_category_path_and_details() {
    let source = String::from("request_token_mismatch");
    let code = SdkDiagnosticCode::new(source.clone()).unwrap();
    drop(source);
    let identity = SdkQueryDiagnosticIdentity::new("entity:person").unwrap();
    let diagnostic =
        SdkExecutionDiagnostic::query_failure(SdkQueryDiagnosticCategory::ResultDecode, code)
            .try_at(SdkDiagnosticPathSegment::Query(
                SdkQueryDiagnosticPathKind::ProviderEvidence,
            ))
            .unwrap()
            .try_at(SdkDiagnosticPathSegment::QueryBinding(7))
            .unwrap()
            .try_at(SdkDiagnosticPathSegment::QueryField {
                owner: identity.clone(),
                name: SdkQueryDiagnosticIdentity::new("display-name").unwrap(),
            })
            .unwrap()
            .try_at(SdkDiagnosticPathSegment::QueryRole {
                owner: SdkQueryDiagnosticIdentity::new("relation:membership").unwrap(),
                name: SdkQueryDiagnosticIdentity::new("member").unwrap(),
            })
            .unwrap()
            .try_at(SdkDiagnosticPathSegment::QueryRoleEdge(3))
            .unwrap()
            .try_at(SdkDiagnosticPathSegment::QueryOutputSlot(1))
            .unwrap()
            .try_at(SdkDiagnosticPathSegment::QueryOutputName(
                SdkQueryDiagnosticIdentity::new("people").unwrap(),
            ))
            .unwrap()
            .try_with_detail(
                SdkDiagnosticName::new(String::from("actual")).unwrap(),
                SdkDiagnosticDetailValue::QueryIdentity(identity),
            )
            .unwrap()
            .try_with_detail(name("delta"), SdkDiagnosticDetailValue::Signed(-9))
            .unwrap()
            .try_with_detail(
                name("missing"),
                SdkDiagnosticDetailValue::QueryIdentityList(vec![
                    SdkQueryDiagnosticIdentity::new("name").unwrap(),
                    SdkQueryDiagnosticIdentity::new("email").unwrap(),
                ]),
            )
            .unwrap();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
    assert_eq!(diagnostic.code().as_str(), "request_token_mismatch");
    assert_eq!(
        diagnostic.details().get(&name("query_category")),
        Some(&SdkDiagnosticDetailValue::QueryCategory(
            SdkQueryDiagnosticCategory::ResultDecode
        ))
    );
    assert_eq!(diagnostic.path().len(), 7);
}

#[test]
fn query_identity_text_is_bounded_and_rejects_free_text_channels() {
    assert!(SdkQueryDiagnosticIdentity::new("entity:person").is_ok());
    for value in [
        "",
        "provider secret",
        "https://provider.invalid",
        "user:password@provider",
        "line\nbreak",
    ] {
        assert_eq!(
            SdkQueryDiagnosticIdentity::new(value).unwrap_err(),
            SdkDiagnosticBuildError::InvalidName
        );
    }
}
