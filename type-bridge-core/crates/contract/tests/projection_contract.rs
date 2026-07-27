use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{
    BindingProjectionFingerprint, BindingTarget, CodeResourceDigest, ProjectedAnnotation,
    ProjectionConfig, ProjectionHandler, ProjectionHandlerVersion, RustCreatePolicy,
    canonical_binding_projection_bytes,
};
use type_bridge_contract::schema::{
    AnnotationFactId, AnnotationKindId, AnnotationSubjectId, SchemaAnnotationValue,
};
use type_bridge_contract::schema_fingerprint::SemanticSchemaFingerprint;

fn semantic(bytes: &[u8]) -> SemanticSchemaFingerprint {
    SemanticSchemaFingerprint::compute(SemanticProfileId::new("typedb-3.12.1/v1").unwrap(), bytes)
        .unwrap()
}

#[test]
fn projected_annotations_reuse_authoritative_subject_and_payload_validation() {
    use type_bridge_contract::id::{TypeId, TypeKind};

    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let invalid_subject = ProjectedAnnotation::new(
        AnnotationFactId::new(
            AnnotationSubjectId::Type(person.clone()),
            AnnotationKindId::Key,
        ),
        SchemaAnnotationValue::Presence,
    )
    .unwrap_err();
    assert_eq!(
        invalid_subject.code().as_str(),
        "invalid_annotation_subject"
    );

    let mismatched_payload = ProjectedAnnotation::new(
        AnnotationFactId::new(AnnotationSubjectId::Type(person), AnnotationKindId::Doc),
        SchemaAnnotationValue::Presence,
    )
    .unwrap_err();
    assert_eq!(
        mismatched_payload.code().as_str(),
        "invalid_annotation_payload"
    );
}

#[test]
fn python_projection_has_byte_exact_config_preimage_and_fingerprint_goldens() {
    let target = BindingTarget::Python;
    let config = ProjectionConfig::python();
    let semantic = semantic(b"schema-golden");
    let handlers = [ProjectionHandler::python_v1()];
    let resources =
        [
            CodeResourceDigest::from_bytes("typebridge.python.runtime-support", b"runtime-v1")
                .unwrap(),
        ];

    assert_eq!(to_canonical_json(&target).unwrap(), br#""python""#);
    assert_eq!(
        to_canonical_json(&config).unwrap(),
        br#"{"binding":"python","naming_policy":"typebridge.python/v1"}"#,
    );

    let canonical =
        canonical_binding_projection_bytes(target, &semantic, &config, &handlers, &resources)
            .unwrap();
    assert_eq!(
        String::from_utf8(canonical).unwrap(),
        r#"{"config":{"binding":"python","naming_policy":"typebridge.python/v1"},"format_version":1,"generator_handlers":[{"id":"typebridge.generator.python","version":1}],"referenced_code_resources":[{"content_fingerprint":{"algorithm":"sha256","canonicalization":"typebridge.raw-bytes/v1","digest":"03a67069f51f5ea767249959c1fd48f0500290671d251c09a8bca3a974cc9215","domain":"typebridge.binding.code-resource"},"id":"typebridge.python.runtime-support"}],"semantic_schema_fingerprint":{"algorithm":"sha256","canonicalization":"typebridge.schema-canonical-json/v1","digest":"ac1cccede374d406fc8e5aa6d20adbd3a13f0454906d888784f9a2be50bb08cc","domain":"typebridge.schema.semantic","semantic_profile":"typedb-3.12.1/v1"},"target":"python"}"#,
    );

    let fingerprint =
        BindingProjectionFingerprint::compute(target, &semantic, &config, &handlers, &resources)
            .unwrap();
    assert_eq!(
        fingerprint.as_fingerprint().digest().to_hex(),
        "e4ed154eff79c451cac8164a9afe5b8fe46a5c2d350cbc7cedc1d266cacd958a",
    );
    assert_eq!(
        to_canonical_json(&fingerprint).unwrap(),
        br#"{"algorithm":"sha256","canonicalization":"typebridge.binding-projection/v1","digest":"e4ed154eff79c451cac8164a9afe5b8fe46a5c2d350cbc7cedc1d266cacd958a","domain":"typebridge.binding.projection","semantic_profile":"typedb-3.12.1/v1"}"#,
    );
}

#[test]
fn evidence_order_is_nonsemantic_and_empty_resources_are_valid() {
    let target = BindingTarget::Python;
    let config = ProjectionConfig::python();
    let semantic_schema = semantic(b"schema");
    let python = ProjectionHandler::python_v1();
    let extension = ProjectionHandler::new("typebridge.generator.docs", 1).unwrap();
    let first_resource = CodeResourceDigest::from_bytes("typebridge.python.a", b"a").unwrap();
    let second_resource = CodeResourceDigest::from_bytes("typebridge.python.z", b"z").unwrap();

    let first = BindingProjectionFingerprint::compute(
        target,
        &semantic_schema,
        &config,
        &[python.clone(), extension.clone()],
        &[first_resource.clone(), second_resource.clone()],
    )
    .unwrap();
    let reversed = BindingProjectionFingerprint::compute(
        target,
        &semantic_schema,
        &config,
        &[extension, python.clone()],
        &[second_resource, first_resource],
    )
    .unwrap();
    assert_eq!(first, reversed);
    BindingProjectionFingerprint::compute(target, &semantic_schema, &config, &[python], &[])
        .expect("an all-code emitter has no fabricated resource entry");
}

#[test]
fn malformed_or_ambiguous_reproducibility_evidence_fails_closed() {
    assert_eq!(
        ProjectionHandler::new("python", 1)
            .unwrap_err()
            .code()
            .as_str(),
        "malformed_projection_component_id",
    );
    assert_eq!(
        ProjectionHandlerVersion::new(0)
            .unwrap_err()
            .code()
            .as_str(),
        "invalid_projection_handler_version",
    );
    assert_eq!(
        CodeResourceDigest::from_bytes("Python.resource", b"x")
            .unwrap_err()
            .code()
            .as_str(),
        "malformed_projection_component_id",
    );

    let target = BindingTarget::Python;
    let config = ProjectionConfig::python();
    let semantic = semantic(b"schema");
    let error =
        BindingProjectionFingerprint::compute(target, &semantic, &config, &[], &[]).unwrap_err();
    assert_eq!(error.code().as_str(), "missing_target_projection_handler");

    let unrelated = ProjectionHandler::new("typebridge.generator.docs", 1).unwrap();
    let error =
        BindingProjectionFingerprint::compute(target, &semantic, &config, &[unrelated], &[])
            .unwrap_err();
    assert_eq!(error.code().as_str(), "missing_target_projection_handler");

    let duplicate_handlers = [
        ProjectionHandler::python_v1(),
        ProjectionHandler::new("typebridge.generator.python", 2).unwrap(),
    ];
    let error =
        BindingProjectionFingerprint::compute(target, &semantic, &config, &duplicate_handlers, &[])
            .unwrap_err();
    assert_eq!(error.code().as_str(), "duplicate_projection_handler_id");

    let duplicate_resources = [
        CodeResourceDigest::from_bytes("typebridge.python.template", b"first").unwrap(),
        CodeResourceDigest::from_bytes("typebridge.python.template", b"second").unwrap(),
    ];
    let error = BindingProjectionFingerprint::compute(
        target,
        &semantic,
        &config,
        &[ProjectionHandler::python_v1()],
        &duplicate_resources,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "duplicate_projection_resource_id");
}

#[test]
fn every_projection_affecting_input_changes_the_fingerprint() {
    let target = BindingTarget::Python;
    let config = ProjectionConfig::python();
    let semantic_schema = semantic(b"schema");
    let handler_v1 = [ProjectionHandler::python_v1()];
    let handler_v2 = [ProjectionHandler::new("typebridge.generator.python", 2).unwrap()];
    let resource_v1 =
        [CodeResourceDigest::from_bytes("typebridge.python.runtime-support", b"v1").unwrap()];
    let resource_v2 =
        [CodeResourceDigest::from_bytes("typebridge.python.runtime-support", b"v2").unwrap()];

    let baseline = BindingProjectionFingerprint::compute(
        target,
        &semantic_schema,
        &config,
        &handler_v1,
        &resource_v1,
    )
    .unwrap();
    let changed_schema = BindingProjectionFingerprint::compute(
        target,
        &semantic(b"other-schema"),
        &config,
        &handler_v1,
        &resource_v1,
    )
    .unwrap();
    let changed_handler = BindingProjectionFingerprint::compute(
        target,
        &semantic_schema,
        &config,
        &handler_v2,
        &resource_v1,
    )
    .unwrap();
    let changed_resource = BindingProjectionFingerprint::compute(
        target,
        &semantic_schema,
        &config,
        &handler_v1,
        &resource_v2,
    )
    .unwrap();

    assert_ne!(baseline, changed_schema);
    assert_ne!(baseline, changed_handler);
    assert_ne!(baseline, changed_resource);
}

#[test]
fn canonical_projection_content_changes_the_runtime_fingerprint() {
    let target = BindingTarget::Python;
    let config = ProjectionConfig::python();
    let semantic_schema = semantic(b"schema");
    let handlers = [ProjectionHandler::python_v1()];
    let first = BindingProjectionFingerprint::compute_with_projection(
        target,
        &semantic_schema,
        &config,
        &handlers,
        &[],
        br#"{"models":[]}"#,
    )
    .unwrap();
    let second = BindingProjectionFingerprint::compute_with_projection(
        target,
        &semantic_schema,
        &config,
        &handlers,
        &[],
        br#"{"models":[{"id":"changed"}]}"#,
    )
    .unwrap();
    assert_ne!(first, second);
}

#[test]
fn python_target_identifiers_reject_keywords_and_malformed_names() {
    use type_bridge_contract::projection::TargetIdentifier;

    assert_eq!(
        TargetIdentifier::python("valid_name").unwrap().as_str(),
        "valid_name"
    );
    for value in ["class", "has-hyphen", "9starts_with_digit"] {
        assert_eq!(
            TargetIdentifier::python(value).unwrap_err().code().as_str(),
            "invalid_python_projection_identifier"
        );
    }
}

#[test]
fn typescript_projection_has_versioned_config_handler_and_identifiers() {
    use type_bridge_contract::projection::TargetIdentifier;

    let target = BindingTarget::TypeScript;
    let config = ProjectionConfig::typescript();
    assert_eq!(to_canonical_json(&target).unwrap(), br#""typescript""#);
    assert_eq!(
        to_canonical_json(&config).unwrap(),
        br#"{"binding":"typescript","naming_policy":"typebridge.typescript/v1"}"#,
    );
    BindingProjectionFingerprint::compute(
        target,
        &semantic(b"typescript-schema"),
        &config,
        &[ProjectionHandler::typescript_v1()],
        &[],
    )
    .expect("TypeScript handler satisfies target evidence");
    assert_eq!(
        TargetIdentifier::typescript("validName").unwrap().as_str(),
        "validName"
    );
    for value in ["class", "has-hyphen", "9startsWithDigit"] {
        assert_eq!(
            TargetIdentifier::typescript(value)
                .unwrap_err()
                .code()
                .as_str(),
            "invalid_typescript_projection_identifier",
        );
    }
}

#[test]
fn rust_projection_has_versioned_config_create_policy_handler_and_identifiers() {
    use type_bridge_contract::projection::TargetIdentifier;

    let target = BindingTarget::Rust;
    let config = ProjectionConfig::rust();
    assert_eq!(to_canonical_json(&target).unwrap(), br#""rust""#);
    assert_eq!(
        to_canonical_json(&config).unwrap(),
        br#"{"binding":"rust","create_policy":"typebridge.rust.validated-create-input/v1","naming_policy":"typebridge.rust/v1"}"#,
    );
    assert_eq!(
        config.rust_create_policy(),
        Some(RustCreatePolicy::ValidatedInputV1)
    );
    let rust = BindingProjectionFingerprint::compute(
        target,
        &semantic(b"rust-schema"),
        &config,
        &[ProjectionHandler::rust_v1()],
        &[],
    )
    .expect("Rust handler and create policy satisfy target evidence");
    let python = BindingProjectionFingerprint::compute(
        BindingTarget::Python,
        &semantic(b"rust-schema"),
        &ProjectionConfig::python(),
        &[ProjectionHandler::python_v1()],
        &[],
    )
    .unwrap();
    assert_ne!(rust, python);
    assert_eq!(
        TargetIdentifier::rust("PersonCreate").unwrap().as_str(),
        "PersonCreate"
    );
    for value in ["type", "self", "has-hyphen", "9starts_with_digit", "_"] {
        assert_eq!(
            TargetIdentifier::rust(value).unwrap_err().code().as_str(),
            "invalid_rust_projection_identifier",
        );
    }
}
