use serde_json::Value;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, QueryInvocation, QueryOperand, QueryOperation,
    QueryOutput, QueryPattern, QueryPlan, ReadStage,
};
use type_bridge_contract::query_remote::{
    RemoteLimits, RemoteQueryRequest, checked_remote_deadline, checked_remote_limit,
};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
use type_bridge_contract::{query_given_rows_capability, query_plan_capability_vocabulary};

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding id"),
        QueryVariable::new(variable).expect("query variable"),
    )
}

fn binding_id(id: u16) -> BindingId {
    BindingId::new(id).expect("binding id")
}

fn managed_semantics() -> ManagedSemanticSchemaFingerprint {
    ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile"),
        b"query-remote-managed-fixture",
    )
    .expect("managed semantic fingerprint")
}

fn person_name_patterns() -> Vec<QueryPattern> {
    vec![
        QueryPattern::Isa {
            binding: binding_id(0),
            include_subtypes: true,
            type_id: TypeId::new(TypeKind::Entity, "person").expect("type id"),
        },
        QueryPattern::Has {
            attribute: binding_id(1),
            attribute_id: AttributeId::new("name").expect("attribute id"),
            owner: binding_id(0),
        },
    ]
}

fn limits() -> RemoteLimits {
    RemoteLimits {
        deadline_ms: None,
        max_bytes: 1 << 20,
        max_items: 100,
    }
}

const NONCE: &str = "remote-nonce-0123456789abcdef";

/// A plan whose canonical bytes exceed the 1 MiB per-string ceiling.
///
/// Each literal stays far below the per-string limit; only the whole
/// document is large, exactly the shape the plan contract's 16 MiB
/// artifact limit admits.
fn oversized_plan() -> QueryPlan {
    let mut patterns = person_name_patterns();
    for label in ["x", "y", "z"] {
        patterns.push(QueryPattern::Value {
            comparator: ValueComparator::NotEqual,
            left: QueryOperand::Binding {
                binding: binding_id(1),
            },
            right: QueryOperand::Literal {
                value: CanonicalValue::String(
                    CanonicalString::new(label.repeat(600 * 1024)).expect("canonical literal"),
                ),
            },
        });
    }
    QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        Vec::new(),
        vec![ReadStage::Match { patterns }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed_semantics(),
    )
    .expect("oversized plan")
}

fn input_plan() -> QueryPlan {
    let mut patterns = person_name_patterns();
    patterns.push(QueryPattern::Value {
        comparator: ValueComparator::Equal,
        left: QueryOperand::Binding {
            binding: binding_id(1),
        },
        right: QueryOperand::Input {
            column: InputColumnId::new(0),
        },
    });
    QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_name").expect("input name"),
            ValueTypeTag::String,
            false,
        )],
        vec![ReadStage::Match { patterns }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed_semantics(),
    )
    .expect("input plan")
}

fn string_row(value: &str) -> InputRow {
    InputRow::new(vec![Some(CanonicalValue::String(
        CanonicalString::new(value).expect("canonical string"),
    ))])
}

#[test]
fn plans_beyond_the_string_ceiling_ride_the_request_envelope() {
    let plan = oversized_plan();
    assert!(
        plan.canonical_bytes().expect("plan bytes").len() > 1024 * 1024,
        "fixture must exceed the per-string ceiling"
    );
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");

    let request =
        RemoteQueryRequest::new(&plan, &invocation, limits(), NONCE).expect("request envelope");
    let bytes = request.encode().expect("request bytes");
    let decoded = RemoteQueryRequest::decode(&bytes).expect("request decodes");
    assert_eq!(decoded.plan().expect("embedded plan decodes"), plan);
}

#[test]
fn string_embedded_plans_are_rejected() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let bytes = RemoteQueryRequest::new(&plan, &invocation, limits(), NONCE)
        .expect("request envelope")
        .encode()
        .expect("request bytes");

    // Re-embed the plan in the retired string form and re-encode
    // canonically; the envelope decodes but the plan accessor must
    // fail closed instead of accepting the old wire shape.
    let mut envelope: Value = serde_json::from_slice(&bytes).expect("envelope JSON");
    let plan_text = serde_json::to_string(&envelope["plan"]).expect("plan text");
    envelope["plan"] = Value::String(plan_text);
    let stringified = serde_json::to_vec(&envelope).expect("stringified envelope");

    let decoded = RemoteQueryRequest::decode(&stringified).expect("envelope still decodes");
    assert!(decoded.plan().is_err(), "string plan must not decode");
}

#[test]
fn multi_row_invocations_require_the_given_transport_capability() {
    let plan = input_plan();
    let empty = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("single-row invocation");
    assert!(empty.transport_capabilities().is_empty());

    let multi = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![string_row("ada"), string_row("bob")],
    )
    .expect("multi-row invocation");
    let capabilities = multi.transport_capabilities();
    assert_eq!(capabilities.len(), 1);
    assert!(capabilities.contains(&query_given_rows_capability()));
    // Plans never require the transport capability themselves, so it is
    // advertised separately from the plan vocabulary.
    assert!(!query_plan_capability_vocabulary().contains(&query_given_rows_capability()));
}

#[test]
fn limit_conversion_rejects_the_signed_and_oversized_ranges() {
    assert_eq!(checked_remote_limit(0).expect("zero"), 0);
    assert_eq!(
        checked_remote_limit(i128::from(u64::MAX)).expect("max"),
        u64::MAX
    );
    for invalid in [-1, i128::from(u64::MAX) + 1] {
        assert_eq!(
            checked_remote_limit(invalid)
                .expect_err("out of range")
                .code()
                .as_str(),
            "query_remote_limit_invalid"
        );
    }
    assert_eq!(checked_remote_deadline(None).expect("absent"), None);
    assert_eq!(
        checked_remote_deadline(Some(30_000)).expect("present"),
        Some(30_000)
    );
    assert_eq!(
        checked_remote_deadline(Some(-1))
            .expect_err("negative deadline")
            .code()
            .as_str(),
        "query_remote_limit_invalid"
    );
}
