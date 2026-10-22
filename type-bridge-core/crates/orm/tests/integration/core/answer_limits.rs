//! Per-transport answer-limit defaults against a live TypeDB.
//!
//! A direct session and a proxy connection start from different defaults: a
//! direct session from the 2.2 executor values, a proxy connection from what
//! one remote envelope carries. An answer between the two passes direct and
//! is refused through the proxy as a wire limit.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use crate::common::dynamic_crud::{attr, setup_dynamic_typeql, unique_schema_suffix};
use crate::internal::*;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::DiagnosticDetailValue;
use type_bridge_contract::query_plan::query_plan_v2_capability_vocabulary;
use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
use type_bridge_contract::query_remote_v2::query_remote_v2_required_capabilities;
use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
use type_bridge_orm::query_v2_prepared::QueryAuthority;
use type_bridge_orm::query_v2_remote::{
    RemoteReplySigningKey, execute_admitted_remote_request_v2, preflight_remote_request_v2,
};
use type_bridge_orm::session::backend::{AnswerLimits, BoundedAnswerLimits};
use type_bridge_orm::*;
use type_bridge_schema_compat::released_typeql_to_declared_projection;

/// Past the proxy item default (65,536), well under the direct one (100,000).
const ENTITIES: u64 = 66_000;
const INSERT_BATCH: u64 = 1_000;
const TEST_TIMEOUT_MILLISECONDS: u64 = 120_000;

fn count_request(registry: &DescriptorRegistry, type_name: &str) -> ValidatedMatchRequest {
    let root = BindingId::new(0);
    validate_match_request(
        registry,
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: root,
                    descriptor: registry.descriptor_id(type_name).expect("registered item"),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                }],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::CountBy { root },
        ),
    )
    .expect("count request passes validation")
}

/// The query authority a server holds for the defined schema.
fn authority(define: &str) -> QueryAuthority {
    let declared = released_typeql_to_declared_projection(
        DocumentId::new("answer-limits.tql").expect("document"),
        define,
    )
    .expect("released descriptor projection");
    QueryAuthority::from_declared_bytes(
        &encode_declared_schema(&declared).expect("declared schema bytes"),
        "typebridge-v1-descriptor-registry",
        "typedb-3.12.1/v1",
    )
    .expect("matching query authority")
}

fn signer() -> RemoteReplySigningKey {
    RemoteReplySigningKey::from_secret_bytes([0x5c; 32])
}

/// A default-configured server's advertisement: no answer-limit overrides.
fn advertisement() -> RemoteCapabilities {
    let mut capabilities = query_plan_v2_capability_vocabulary()
        .iter()
        .cloned()
        .collect::<CapabilitySet>();
    for capability in query_remote_v2_required_capabilities(true) {
        capabilities.insert(capability);
    }
    RemoteCapabilities::new(
        capabilities,
        RemoteExecutorBinding::new("orm-live-answer-limits", "orm-live-answer-epoch-01")
            .expect("executor binding"),
        signer().public_key(),
    )
}

/// The provider ceilings of a default-configured server.
fn server_ceilings() -> AnswerLimits {
    let proxy = QueryExecutionResourceLimits::default_for(AnswerTransport::Proxy);
    AnswerLimits {
        answer: BoundedAnswerLimits {
            max_items: proxy.items,
            max_bytes: proxy.bytes,
            deadline: None,
            cancellation: AnswerCancellation::default(),
        },
        max_collection_members: proxy.collection_members,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_answer_limits_follow_the_transport_live() {
    let _guard = crate::common::integration_test_guard().await;
    let suffix = unique_schema_suffix("rust", "answer-limits");
    let item_type = format!("{suffix}-item");
    let value_attr = format!("{suffix}-value");

    let registry = DescriptorRegistry::new();
    registry
        .register_entity(EntityDescriptor {
            type_name: item_type.clone(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![attr("value", &value_attr, ValueType::Long, false)],
            doc: None,
            meta: Default::default(),
        })
        .expect("item descriptor registers");
    let define = generate_define_block(&SchemaInfo::from_descriptors(&registry.snapshot()));
    let db = setup_dynamic_typeql(&define).await;

    // Fixture setup has its own deadline, independent of either query read.
    tokio::time::timeout(Duration::from_millis(TEST_TIMEOUT_MILLISECONDS), async {
        for start in (0..ENTITIES).step_by(usize::try_from(INSERT_BATCH).unwrap()) {
            let statements = (start..(start + INSERT_BATCH).min(ENTITIES))
                .map(|index| format!("$e{index} isa {item_type}, has {value_attr} {index};"))
                .collect::<Vec<_>>()
                .join(" ");
            db.execute_raw(&format!("insert {statements}"), TxType::Write)
                .await
                .unwrap_or_else(|error| panic!("insert batch at {start}: {error}"));
        }
    })
    .await
    .expect("answer-limit fixture inserts complete before the setup deadline");

    // This large scan tests answer-size defaults, with headroom for host load.
    let direct_defaults = QueryExecutionResourceLimits::default_for(AnswerTransport::Direct);
    assert_eq!(direct_defaults.items, 100_000);
    assert_eq!(direct_defaults.bytes, 64 * 1024 * 1024);
    assert_eq!(direct_defaults.timeout_milliseconds, 30_000);
    let direct = QueryExecutionResourceLimits {
        timeout_milliseconds: TEST_TIMEOUT_MILLISECONDS,
        ..direct_defaults
    };
    assert!(ENTITIES > DEFAULT_PROXY_QUERY_ITEMS && ENTITIES < direct.items);
    let (_, counted) = QueryTarget::Database(&db)
        .execute(
            &registry,
            count_request(&registry, &item_type),
            direct,
            &AnswerCancellation::default(),
            QueryExecutionDeadline::for_limits(direct),
        )
        .await
        .unwrap_or_else(|error| panic!("direct count at default answer-size limits: {error}"));
    assert!(
        matches!(counted.result(), MatchResult::Count { value, .. } if *value == ENTITIES),
        "direct count returns every entity: {:?}",
        counted.result()
    );

    // Proxy at default answer-size limits: the client prepares under those defaults,
    // a default server executes the request in process, and the reply is
    // refused at the items wire limit.
    let advertisement = advertisement();
    let authority = Arc::new(authority(&define));
    let proxy_defaults = QueryExecutionResourceLimits::default_for(AnswerTransport::Proxy);
    assert_eq!(proxy_defaults.items, 65_536);
    assert_eq!(proxy_defaults.bytes, 32 * 1024 * 1024);
    assert_eq!(proxy_defaults.timeout_milliseconds, 30_000);
    let proxy = QueryExecutionResourceLimits {
        timeout_milliseconds: TEST_TIMEOUT_MILLISECONDS,
        ..proxy_defaults
    };
    let connection = ProxyQueryConnection::with_exchange(
        advertisement.encode().expect("advertisement bytes"),
        Arc::clone(&authority),
        proxy,
    )
    .expect("proxy connection");
    assert_eq!(connection.answer_limits(), proxy);
    let cancellation = AnswerCancellation::default();
    let deadline = QueryExecutionDeadline::for_limits(proxy);
    let pending = connection
        .prepare(
            &registry,
            count_request(&registry, &item_type),
            proxy,
            &cancellation,
            deadline,
        )
        .unwrap_or_else(|error| panic!("proxy request prepares: {error}"));
    let admitted = preflight_remote_request_v2(
        pending.request_bytes(),
        &authority.context(),
        &advertisement,
        server_ceilings(),
    )
    .unwrap_or_else(|rejection| panic!("server admits: {}", rejection.diagnostic_code()));
    let mut server_transaction = db.read_transaction().await.expect("server transaction");
    let response =
        execute_admitted_remote_request_v2(admitted, &mut server_transaction, &signer()).await;
    let error = ProxyQueryConnection::finish(&pending, &response, &cancellation, deadline)
        .expect_err("the proxy refuses an answer past the wire item limit");
    let QueryTargetError::Reply(RemoteModelQueryV2Error::Diagnostic(refusal)) = error else {
        panic!("expected an authenticated refusal, got {error:?}");
    };
    assert!(
        matches!(
            refusal.code().as_str(),
            "query_v2_model_solution_limit" | "solution_scan_limit"
        ),
        "item-limit refusal code: {}",
        refusal.code().as_str()
    );
    let detail = |name: &str| refusal.details().get(name).cloned();
    assert_eq!(
        (detail("limit"), detail("maximum"), detail("source")),
        (
            Some(DiagnosticDetailValue::Text("items".into())),
            Some(DiagnosticDetailValue::Long(
                i64::try_from(DEFAULT_PROXY_QUERY_ITEMS).unwrap()
            )),
            Some(DiagnosticDetailValue::Text("default".into())),
        )
    );

    db.execute_raw(
        &format!("match $e isa {item_type}; delete $e;"),
        TxType::Write,
    )
    .await
    .expect("remove the fixture entities");
}
