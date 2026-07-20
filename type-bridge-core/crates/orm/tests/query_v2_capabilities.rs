//! Offline capability-negotiation coverage: authority construction under
//! declared required capabilities, client-side advertisement preflight,
//! and executor admission of multi-row transport requirements.

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, QueryOutput, QueryPattern, QueryPlan, ReadStage,
};
use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteLimits};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan, SourcedSchemaFact,
    TypeFact, ValueFact, ValueFactId, encode_declared_schema,
};
use type_bridge_contract::value::{ValueTypeTag, ValueTypeTag as Tag};
use type_bridge_contract::{query_given_rows_capability, query_plan_capability_vocabulary};
use type_bridge_orm::query_v2_prepared::{
    QueryAuthority, decode_remote_capabilities, encode_prepared_remote_request,
};
use type_bridge_orm::query_v2_remote::preflight_remote_request;
use type_bridge_orm::session::backend::BoundedAnswerLimits;
use type_bridge_query::MigrationAssertionValidationContext;
use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};

const SCOPE: &str = "query-v2-capabilities";
const PROFILE: &str = "typedb-3.12.1/v1";
const NONCE: &str = "capability-nonce-0123456789ab";

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("fixture type")
}

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding id"),
        QueryVariable::new(variable).expect("variable"),
    )
}

fn binding_id(id: u16) -> BindingId {
    BindingId::new(id).expect("binding id")
}

fn declared_schema(required: CapabilitySet) -> DeclaredSchema {
    let person = type_id(TypeKind::Entity, "person");
    let name = AttributeId::new("name").expect("attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("type fact")),
        SchemaFact::Value(ValueFact::new(ValueFactId::new(name.clone()), Tag::String)),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person, name).expect("owns id"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-capabilities-fixture").expect("document"),
                byte,
                byte + 1,
                line,
                1,
                line,
                2,
            )
            .expect("span"),
        )
    });
    DeclaredSchema::from_facts(FormatVersion::V1, required, sourced).expect("declared schema")
}

fn plan_with_input(managed: &type_bridge_contract::schema_delta::ManagedSchemaState) -> QueryPlan {
    QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_name").expect("input name"),
            ValueTypeTag::String,
            false,
        )],
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: true,
                    type_id: type_id(TypeKind::Entity, "person"),
                },
                QueryPattern::Has {
                    attribute: binding_id(1),
                    attribute_id: AttributeId::new("name").expect("attribute"),
                    owner: binding_id(0),
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("plan")
}

fn invocation_json(rows: &[&str]) -> String {
    let rows: Vec<_> = rows
        .iter()
        .map(|value| vec![serde_json::json!({"kind": "string", "value": value})])
        .collect();
    serde_json::json!({"operation": "rows", "rows": rows}).to_string()
}

fn advertisement(with_given_rows: bool) -> Vec<u8> {
    let mut capabilities = query_plan_capability_vocabulary();
    if with_given_rows {
        capabilities.insert(query_given_rows_capability());
    }
    RemoteCapabilities::new(capabilities)
        .encode()
        .expect("advertisement bytes")
}

fn limits() -> RemoteLimits {
    RemoteLimits {
        deadline_ms: None,
        max_bytes: 1 << 20,
        max_items: 100,
    }
}

struct Fixture {
    authority: QueryAuthority,
    plan_bytes: Vec<u8>,
}

fn fixture(required: CapabilitySet) -> Fixture {
    let declared = declared_schema(required);
    let bytes = encode_declared_schema(&declared).expect("declared bytes");
    let authority =
        QueryAuthority::from_declared_bytes(&bytes, SCOPE, PROFILE).expect("authority builds");
    let profile = SemanticProfileId::new(PROFILE).expect("profile");
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new(SCOPE).expect("scope"),
            profile,
            declared.required_capabilities().clone(),
        ),
    )
    .expect("managed state");
    let plan_bytes = plan_with_input(&managed)
        .canonical_bytes()
        .expect("plan bytes");
    Fixture {
        authority,
        plan_bytes,
    }
}

#[test]
fn authorities_build_for_schemas_with_nonempty_required_capabilities() {
    let mut required = CapabilitySet::new();
    required.insert(CapabilityId::new("schema.roles").expect("capability"));
    // The declared artifact is the caller's authority: its own required
    // capabilities are available during resolution, exactly as on the
    // server.
    fixture(required);
}

#[test]
fn encode_refuses_executors_that_do_not_advertise_the_plan() {
    let fixture = fixture(CapabilitySet::new());
    let starved = RemoteCapabilities::new(CapabilitySet::new())
        .encode()
        .expect("starved advertisement");
    let error = encode_prepared_remote_request(
        &fixture.authority,
        &fixture.plan_bytes,
        &invocation_json(&["ada"]),
        &starved,
        limits(),
        NONCE,
    )
    .expect_err("plan must be refused before any bytes exist");
    assert_eq!(error.code().as_str(), "query_remote_capability_unsupported");
}

#[test]
fn encode_refuses_multi_row_batches_without_the_given_transport() {
    let fixture = fixture(CapabilitySet::new());
    let error = encode_prepared_remote_request(
        &fixture.authority,
        &fixture.plan_bytes,
        &invocation_json(&["ada", "bob"]),
        &advertisement(false),
        limits(),
        NONCE,
    )
    .expect_err("multi-row batch must be refused without given transport");
    assert_eq!(error.code().as_str(), "query_remote_capability_unsupported");

    encode_prepared_remote_request(
        &fixture.authority,
        &fixture.plan_bytes,
        &invocation_json(&["ada", "bob"]),
        &advertisement(true),
        limits(),
        NONCE,
    )
    .expect("advertised given transport admits the batch");
}

#[test]
fn preflight_rejects_multi_row_batches_the_executor_cannot_transport() {
    let declared = declared_schema(CapabilitySet::new());
    let bytes = encode_declared_schema(&declared).expect("declared bytes");
    let authority =
        QueryAuthority::from_declared_bytes(&bytes, SCOPE, PROFILE).expect("authority builds");
    let profile = SemanticProfileId::new(PROFILE).expect("profile");
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new(SCOPE).expect("scope"),
            profile.clone(),
            CapabilitySet::new(),
        ),
    )
    .expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let plan_bytes = plan_with_input(&managed)
        .canonical_bytes()
        .expect("plan bytes");

    let request = encode_prepared_remote_request(
        &authority,
        &plan_bytes,
        &invocation_json(&["ada", "bob"]),
        &advertisement(true),
        limits(),
        NONCE,
    )
    .expect("client with a truthful advertisement encodes");

    // An executor that does not advertise the transport must reject the
    // same request at admission, before any provider resource exists.
    let context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let rejection = preflight_remote_request(
        &request,
        &context,
        &query_plan_capability_vocabulary(),
        BoundedAnswerLimits::default(),
    )
    .map(|_| ())
    .expect_err("admission must fail without the transport capability");
    let envelope = rejection.into_failure_envelope();
    let body = String::from_utf8(envelope).expect("failure envelope is JSON");
    assert!(
        body.contains("query_remote_capability_unsupported"),
        "{body}"
    );

    let mut advertised = query_plan_capability_vocabulary();
    advertised.insert(query_given_rows_capability());
    preflight_remote_request(
        &request,
        &context,
        &advertised,
        BoundedAnswerLimits::default(),
    )
    .map(|_| ())
    .expect("advertised transport admits the request");
}

#[test]
fn advertisements_decode_into_sorted_capability_ids() {
    let decoded = decode_remote_capabilities(&advertisement(true)).expect("advertisement decodes");
    assert!(decoded.contains(&"query.input.given-rows".to_owned()));
    let mut sorted = decoded.clone();
    sorted.sort();
    assert_eq!(decoded, sorted);
    assert!(decode_remote_capabilities(b"not an advertisement").is_err());
}
