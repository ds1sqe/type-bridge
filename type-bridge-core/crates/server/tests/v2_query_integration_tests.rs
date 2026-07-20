//! V2 envelope endpoints beside the retained V1 surface.
//!
//! The V2 route test executes against a live TypeDB (TYPEDB_ADDRESS /
//! TYPEDB_HTTP_PORT); run it explicitly with `-- --ignored`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::{
    OrderDirection, OrderTerm, QueryInvocation, QueryOperation, QueryOutput, QueryPattern,
    QueryPlan, ReadStage,
};
use type_bridge_contract::query_plan_capability_vocabulary;
use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteLimits};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan, SourcedSchemaFact,
    TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
use type_bridge_orm::TxType;
use type_bridge_orm::query_v2::{QueryRowValue, QueryV2Outcome};
use type_bridge_orm::query_v2_remote::{decode_remote_outcome, encode_remote_request};
use type_bridge_orm::session::backend::BoundedAnswerLimits;
use type_bridge_orm::session::database::Database;
use type_bridge_orm::session::real_driver::{ConnectOptions, ensure_database_exists};
use type_bridge_query::{MigrationAssertionValidationContext, validate_query_plan};
use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};
use type_bridge_server::test_helpers::{MockExecutor, make_pipeline};
use type_bridge_server::transport::v2::{V2QueryState, create_router_with_v2};

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding id"),
        QueryVariable::new(variable).expect("variable"),
    )
}

fn binding_id(id: u16) -> BindingId {
    BindingId::new(id).expect("binding id")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn v2_envelope_endpoints_serve_beside_v1() {
    let address = std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into());
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    let database_name = format!("tb_server_v2_query_{}", std::process::id());
    ensure_database_exists(
        &address,
        &database_name,
        &username,
        &password,
        ConnectOptions::default(),
    )
    .await
    .expect("database exists");
    let database = Database::connect(&address, &database_name, &username, &password)
        .await
        .expect("connected database");

    let person = TypeId::new(TypeKind::Entity, "server-v2-person").unwrap();
    let name = AttributeId::new("server-v2-name").unwrap();
    database
        .execute_raw(
            &format!(
                "define\n\
                 attribute {name}, value string;\n\
                 entity {person}, owns {name};",
                name = name.label(),
                person = person.label(),
            ),
            TxType::Schema,
        )
        .await
        .expect("schema definition");
    database
        .execute_raw(
            &format!(
                "insert $a isa {person}, has {name} \"ada\"; \
                 $b isa {person}, has {name} \"bob\";",
                person = person.label(),
                name = name.label(),
            ),
            TxType::Write,
        )
        .await
        .expect("data insertion");

    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap())
                .unwrap(),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).unwrap(),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).unwrap();
        let line = u32::try_from(index + 1).unwrap();
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("server-v2-query").unwrap(),
                byte,
                byte + 1,
                line,
                1,
                line,
                2,
            )
            .unwrap(),
        )
    });
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new("server-v2-query").unwrap(),
            profile,
            CapabilitySet::new(),
        ),
    )
    .unwrap();

    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: true,
                        type_id: person.clone(),
                    },
                    QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: name.clone(),
                        owner: binding_id(0),
                    },
                ],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated =
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL).unwrap();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).unwrap();

    let state = Arc::new(V2QueryState {
        advertised: query_plan_capability_vocabulary(),
        ceilings: BoundedAnswerLimits::default(),
        database,
        managed,
        resolved,
    });
    let router = create_router_with_v2(Arc::new(make_pipeline(MockExecutor::new(), false)), state);

    // The retained V1 surface still answers.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Negotiation: the executor advertises the first vocabulary.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v2/capabilities")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let advertised = RemoteCapabilities::decode(&bytes).expect("advertisement");
    for capability in plan.required_capabilities().iter() {
        assert!(advertised.capabilities().contains(capability));
    }

    // The envelope executes through the versioned endpoint.
    let nonce = "server-v2-nonce-0123456789";
    let limits = RemoteLimits {
        deadline_ms: Some(30_000),
        max_bytes: 1 << 20,
        max_items: 100,
    };
    let request =
        encode_remote_request(&validated, &invocation, limits, nonce).expect("request envelope");
    let expected_request =
        type_bridge_contract::query_remote::RemoteRequestFingerprint::compute(&request)
            .expect("request fingerprint");
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/query")
                .header("content-type", "application/json")
                .body(Body::from(request))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let outcome = decode_remote_outcome(
        &bytes,
        &validated,
        QueryOperation::Rows,
        nonce,
        &expected_request,
        limits,
    )
    .expect("typed outcome");
    let QueryV2Outcome::Rows(rows) = &outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    let names = rows
        .iter()
        .map(|row| match &row.values()[1] {
            QueryRowValue::Attribute {
                value: CanonicalValue::String(value),
                ..
            } => value.as_str().to_owned(),
            other => panic!("expected string names: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["ada".to_owned(), "bob".to_owned()]);
}
