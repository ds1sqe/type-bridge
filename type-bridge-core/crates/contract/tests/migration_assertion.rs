use serde_json::{Value, json};
use type_bridge_contract::migration_assertion_capability_vocabulary;
use type_bridge_contract::migration_assertion::{
    AssertionBinding, AssertionExpectation, AssertionPattern, BindingId,
    MigrationAssertionPlan, QueryVariable, decode_migration_assertion_plan,
};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding id"),
        QueryVariable::new(variable).expect("query variable"),
    )
}

fn managed_semantics(seed: &[u8]) -> ManagedSemanticSchemaFingerprint {
    ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile"),
        seed,
    )
    .expect("managed semantic fingerprint")
}

fn exact_plan() -> MigrationAssertionPlan {
    MigrationAssertionPlan::new(
        vec![binding(0, "person")],
        vec![AssertionPattern::Isa {
            binding: BindingId::new(0).expect("binding id"),
            include_subtypes: false,
            type_id: TypeId::new(TypeKind::Entity, "person").expect("type id"),
        }],
        vec![BindingId::new(0).expect("binding id")],
        Vec::new(),
        managed_semantics(b"migration-assertion-managed-fixture"),
        AssertionExpectation::NoRows,
    )
    .expect("exact plan")
}

#[test]
fn migration_assertion_capability_vocabulary_is_exact_and_deterministic() {
    let vocabulary = migration_assertion_capability_vocabulary();
    assert_eq!(vocabulary, migration_assertion_capability_vocabulary());
    assert_eq!(
        vocabulary
            .iter()
            .map(|capability| capability.as_str())
            .collect::<Vec<_>>(),
        vec![
            "query.migration-assertion",
            "query.pattern.has",
            "query.pattern.isa",
            "query.pattern.isa-subtypes",
            "query.pattern.links",
            "query.pattern.negation",
            "query.pattern.value",
        ]
    );
}

#[test]
fn canonical_bytes_and_fingerprint_are_exact_and_stable() {
    let plan = exact_plan();
    let bytes = plan.canonical_bytes().expect("canonical bytes");
    let expected = json!({
        "bindings": [{"id": 0, "variable": "person"}],
        "expectation": "no_rows",
        "format": 1,
        "managed_semantics": plan.managed_semantics(),
        "outputs": [0],
        "patterns": [{
            "binding": 0,
            "include_subtypes": false,
            "kind": "isa",
            "type_id": {"kind": "entity", "label": "person"}
        }],
        "required_capabilities": [
            "query.migration-assertion",
            "query.pattern.isa"
        ],
        "witnesses": []
    });
    assert_eq!(bytes, serde_json::to_vec(&expected).expect("expected JSON"));
    assert_eq!(decode_migration_assertion_plan(&bytes).expect("decode"), plan);
    assert_eq!(plan.fingerprint().expect("fingerprint"), plan.fingerprint().expect("repeat"));
}

#[test]
fn malformed_sparse_unknown_and_forged_capability_bytes_fail_closed() {
    let bytes = exact_plan().canonical_bytes().expect("canonical bytes");
    let mut sparse: Value = serde_json::from_slice(&bytes).expect("JSON");
    sparse["bindings"][0]["id"] = json!(1);
    assert!(decode_migration_assertion_plan(&serde_json::to_vec(&sparse).expect("JSON")).is_err());

    let mut forged: Value = serde_json::from_slice(&bytes).expect("JSON");
    forged["required_capabilities"] = json!([
        "query.future",
        "query.migration-assertion",
        "query.pattern.isa"
    ]);
    assert_eq!(
        decode_migration_assertion_plan(&serde_json::to_vec(&forged).expect("JSON"))
            .expect_err("forged capability")
            .code()
            .as_str(),
        "migration_assertion_capability_mismatch"
    );

    let mut unknown: Value = serde_json::from_slice(&bytes).expect("JSON");
    unknown["patterns"][0]["future"] = json!(true);
    assert!(decode_migration_assertion_plan(&serde_json::to_vec(&unknown).expect("JSON")).is_err());

    let mut wrong_domain: Value = serde_json::from_slice(&bytes).expect("JSON");
    wrong_domain["managed_semantics"]["domain"] = json!("typebridge.schema.semantic");
    assert!(
        decode_migration_assertion_plan(
            &serde_json::to_vec(&wrong_domain).expect("JSON")
        )
        .is_err()
    );
}

#[test]
fn variable_binding_and_pattern_limits_are_enforced() {
    assert!(QueryVariable::new("$person").is_err());
    assert!(BindingId::new(256).is_err());

    let id = BindingId::new(0).expect("binding id");
    let mut nested = AssertionPattern::Isa {
        binding: id,
        include_subtypes: false,
        type_id: TypeId::new(TypeKind::Entity, "person").expect("type id"),
    };
    for _ in 0..64 {
        nested = AssertionPattern::Not { patterns: vec![nested] };
    }
    assert_eq!(
        MigrationAssertionPlan::new(
            vec![binding(0, "person")],
            vec![nested],
            vec![id],
            Vec::new(),
            managed_semantics(b"migration-assertion-limit-fixture"),
            AssertionExpectation::NoRows,
        )
        .expect_err("depth limit")
        .code()
        .as_str(),
        "migration_assertion_pattern_depth_limit"
    );
}
