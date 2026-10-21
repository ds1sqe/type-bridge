use super::*;
use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::id::AttributeId;
use type_bridge_contract::sdk_diagnostic::*;
use type_bridge_contract::value::ValueTypeTag;

fn fixture() -> SdkExecutionDiagnostic {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let field = OwnsFactId::new(person.clone(), AttributeId::new("name").unwrap()).unwrap();
    let role = RoleId::new("membership", "member").unwrap();
    let identity = |value: &str| SdkQueryDiagnosticIdentity::new(value).unwrap();
    let fingerprint: Fingerprint = serde_json::from_value(serde_json::json!({
        "domain": "fixture.diagnostic", "algorithm": "sha256",
        "canonicalization": "typebridge.canonical-json/v1",
        "semantic_profile": "typedb-3.12.1/v1", "digest": "a".repeat(64),
    }))
    .unwrap();
    let unprofiled: Fingerprint = serde_json::from_value(serde_json::json!({
        "domain": "fixture.diagnostic", "algorithm": "sha256",
        "canonicalization": "typebridge.canonical-json/v1", "digest": "b".repeat(64),
    }))
    .unwrap();
    let mut value = SdkExecutionDiagnostic::invalid_input(
        SdkDiagnosticCode::new("fixture_error").unwrap(),
        SdkDiagnosticMessage::new("Fixture diagnostic").unwrap(),
    );
    use SdkDiagnosticPathSegment as P;
    for item in [
        P::Argument(SdkDiagnosticName::new("input").unwrap()),
        P::Index(u64::MAX),
        P::Type(person.clone()),
        P::Field(field.clone()),
        P::Role(role.clone()),
        P::Query(SdkQueryDiagnosticPathKind::Request),
        P::QueryBinding(u16::MAX),
        P::QueryField {
            owner: identity("entity:person"),
            name: identity("displayName"),
        },
        P::QueryRole {
            owner: identity("relation:membership"),
            name: identity("member"),
        },
        P::QueryRoleEdge(u16::MAX),
        P::QueryOutputSlot(u64::MAX),
        P::QueryOutputName(identity("result")),
        P::ContractField(identity("schema")),
        P::ContractIdentity(identity("entity:person")),
    ] {
        value = value.try_at(item).unwrap();
    }
    use SdkDiagnosticDetailValue as D;
    for (key, item) in [
        ("boolean", D::Boolean(true)),
        ("count", D::Count(u64::MAX)),
        ("bytes", D::ByteCount(u64::MAX)),
        ("signed", D::Signed(i64::MIN)),
        (
            "capability",
            D::Capability(CapabilityId::new("schema.annotations").unwrap()),
        ),
        ("value_type", D::ValueType(ValueTypeTag::String)),
        ("type", D::Type(person)),
        ("field", D::Field(field)),
        ("role", D::Role(role)),
        ("fingerprint", D::Fingerprint(fingerprint)),
        ("unprofiled", D::Fingerprint(unprofiled)),
        (
            "provider",
            D::ProviderOperation(SdkProviderOperation::Write),
        ),
        ("commit", D::CommitOutcome(SdkCommitFailureOutcome::Unknown)),
        (
            "query_category",
            D::QueryCategory(SdkQueryDiagnosticCategory::InvalidPlan),
        ),
        ("identity", D::QueryIdentity(identity("entity:person"))),
        (
            "identities",
            D::QueryIdentityList(
                (0..32)
                    .map(|index| identity(&format!("entity:person{index}")))
                    .collect(),
            ),
        ),
    ] {
        value = value
            .try_with_detail(SdkDiagnosticName::new(key).unwrap(), item)
            .unwrap();
    }
    value
}

#[test]
#[ignore = "requires KOTLINC and a local JDK for actual JVM diagnostic projection"]
fn all_execution_diagnostic_variants_cross_the_jvm_without_loss() {
    use std::path::Path;
    use std::process::Command;
    let compiler = std::env::var_os("KOTLINC").expect("KOTLINC is required");
    let stage = crate::test_support::Stage::new();
    let jar = stage.path().join("diagnostics.jar");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let compiled = Command::new(compiler)
        .args(["-Werror", "-jvm-target", "21"])
        .arg(root.join("kotlin/src/main/kotlin"))
        .arg(root.join("kotlin/src/test/kotlin/DiagnosticAssertions.kt"))
        .arg("-include-runtime")
        .arg("-d")
        .arg(&jar)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let classpath = format!("-Djava.class.path={}", jar.display());
    let args = jni::InitArgsBuilder::new()
        .version(jni::JNIVersion::V1_8)
        .option(&classpath)
        .option("-Xcheck:jni")
        .build()
        .unwrap();
    let vm = jni::JavaVM::new(args).unwrap();
    vm.attach_current_thread(|env| -> Result<(), JvmError> {
        for _ in 0..20 {
            assert!(matches!(
                throw(env, &fixture()),
                Err(JvmError::Jni(jni::errors::Error::JavaException))
            ));
            let exception = env.exception_occurred().expect("SDK exception is pending");
            env.exception_clear();
            let checked = env.call_static_method(
                jni_str!("DiagnosticAssertions"),
                jni_str!("verify"),
                jni_sig!("(Lio/github/ds1sqe/typebridge/SdkExecutionException;)V"),
                &[JValue::Object(exception.as_ref())],
            );
            if checked.is_err() {
                env.exception_describe();
            }
            checked?;
        }
        let code = || SdkDiagnosticCode::new("fixture_error").unwrap();
        let message = || SdkDiagnosticMessage::new("Fixture diagnostic").unwrap();
        for diagnostic in [
            SdkExecutionDiagnostic::invalid_input(code(), message()),
            SdkExecutionDiagnostic::unsupported_capability(code(), message()),
            SdkExecutionDiagnostic::resource_limit(code(), message()),
            SdkExecutionDiagnostic::integrity(code(), message()),
            SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Read),
            SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::DefinitelyAborted),
            SdkExecutionDiagnostic::cancelled_before_dispatch(),
            SdkExecutionDiagnostic::internal_failure(),
        ] {
            assert!(matches!(
                throw(env, &diagnostic),
                Err(JvmError::Jni(jni::errors::Error::JavaException))
            ));
            let exception = env.exception_occurred().expect("SDK exception is pending");
            env.exception_clear();
            let expected = text(env, diagnostic.category().as_str())?;
            let checked = env.call_static_method(
                jni_str!("DiagnosticAssertions"),
                jni_str!("verifyCategory"),
                jni_sig!(
                    "(Lio/github/ds1sqe/typebridge/SdkExecutionException;Ljava/lang/String;)V"
                ),
                &[
                    JValue::Object(exception.as_ref()),
                    JValue::Object(&expected),
                ],
            );
            if checked.is_err() {
                env.exception_describe();
            }
            checked?;
        }
        Ok(())
    })
    .unwrap();
}
