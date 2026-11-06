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
use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::query_plan::query_plan_capability_vocabulary;
use type_bridge_contract::query_remote::{
    CAP_QUERY_REMOTE_ANSWER_DIMENSIONS, CAP_QUERY_REMOTE_STAGED, StagedAnswer, StagedOutcome,
    StagedReply, StagedRequest,
};
use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
use type_bridge_contract::sdk_diagnostic::{SdkDiagnosticDetailValue, SdkDiagnosticName};
use type_bridge_orm::plan_authority::QueryAuthority;
use type_bridge_orm::remote_signing::RemoteReplySigningKey;
use type_bridge_orm::session::backend::{AnswerLimits, BoundedAnswerLimits};
use type_bridge_orm::session::staged::{TransactionKind, TransactionOptions};
use type_bridge_orm::*;
use type_bridge_schema_compat::released_typeql_to_declared_projection;

/// Past the proxy item default (65,536), well under the direct one (100,000).
const ENTITIES: u64 = 66_000;
const INSERT_BATCH: u64 = 1_000;
const TEST_TIMEOUT_MILLISECONDS: u64 = 120_000;

fn request(
    registry: &DescriptorRegistry,
    type_name: &str,
    operation: MatchOperation,
) -> ValidatedMatchRequest {
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
            operation,
        ),
    )
    .expect("answer-limit request passes validation")
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
    let mut capabilities = query_plan_capability_vocabulary();
    for capability in [CAP_QUERY_REMOTE_STAGED, CAP_QUERY_REMOTE_ANSWER_DIMENSIONS] {
        capabilities.insert(CapabilityId::new(capability).unwrap());
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
        ..Default::default()
    }
}

async fn execute_staged(
    db: &Database,
    pending: &PendingRemoteModelQuery,
    authority: &QueryAuthority,
    advertisement: &RemoteCapabilities,
) -> (Vec<u8>, Vec<StagedAnswer>) {
    let request =
        StagedRequest::decode(pending.request_bytes(), authority.declared(), advertisement)
            .expect("server admits staged request");
    let mut tx = db.transaction(
        TransactionOptions::default()
            .with_kind(TransactionKind::Read)
            .with_answer_limits(server_ceilings())
            .with_answer_timeout(Duration::from_millis(TEST_TIMEOUT_MILLISECONDS)),
    );
    let plans = request
        .plans()
        .iter()
        .map(|entry| entry.bound_plan(authority.declared()).unwrap())
        .collect::<Vec<_>>();
    let pending = plans
        .iter()
        .map(|plan| tx.stage_plan(plan).unwrap())
        .collect::<Vec<_>>();
    let (outcome, answers) = match tx.commit().await {
        Ok(()) => (
            StagedOutcome::Committed,
            pending
                .iter()
                .zip(&plans)
                .map(|(answer, plan)| answer.get().unwrap().to_staged(plan).unwrap())
                .collect(),
        ),
        Err(error) => (
            StagedOutcome::Failed {
                diagnostic: error.diagnostic(),
            },
            vec![],
        ),
    };
    let response = StagedReply::new(&request, outcome, answers.clone())
        .unwrap()
        .encode_signed(&request, advertisement, &signer())
        .unwrap();
    (response, answers)
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
            owned_attributes: vec![attr("value", &value_attr, ValueType::Long, true)],
            doc: None,
            meta: Default::default(),
        })
        .expect("item descriptor registers");
    let define =
        try_generate_define_block(&SchemaInfo::from_descriptors(&registry.snapshot())).unwrap();
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
    assert_eq!(
        direct_defaults.timeout_milliseconds.milliseconds(),
        Some(60_000)
    );
    let direct = QueryExecutionResourceLimits {
        timeout_milliseconds: type_bridge_orm::QueryTimeout::from_milliseconds(
            TEST_TIMEOUT_MILLISECONDS,
        ),
        ..direct_defaults
    };
    assert!(ENTITIES > DEFAULT_PROXY_QUERY_ITEMS && ENTITIES < direct.items);
    let (_, counted) = QueryTarget::Database(&db)
        .execute(
            &registry,
            request(
                &registry,
                &item_type,
                MatchOperation::CountBy {
                    root: BindingId::new(0),
                },
            ),
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

    // V3 reduces CountBy in the provider, so both transports read one row.
    let advertisement = advertisement();
    let authority = Arc::new(authority(&define));
    let proxy_defaults = QueryExecutionResourceLimits::default_for(AnswerTransport::Proxy);
    assert_eq!(proxy_defaults.items, 65_536);
    assert_eq!(proxy_defaults.bytes, 32 * 1024 * 1024);
    assert_eq!(
        proxy_defaults.timeout_milliseconds.milliseconds(),
        Some(60_000)
    );
    let proxy = QueryExecutionResourceLimits {
        timeout_milliseconds: type_bridge_orm::QueryTimeout::from_milliseconds(
            TEST_TIMEOUT_MILLISECONDS,
        ),
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
            request(
                &registry,
                &item_type,
                MatchOperation::CountBy {
                    root: BindingId::new(0),
                },
            ),
            proxy,
            &cancellation,
            deadline,
        )
        .unwrap_or_else(|error| panic!("proxy request prepares: {error}"));
    let (response, answers) = execute_staged(&db, &pending, &authority, &advertisement).await;
    assert!(
        matches!(answers.as_slice(), [StagedAnswer::Rows { rows }] if rows.len() == 1),
        "V3 CountBy reads one provider aggregate row"
    );
    let (_, counted) = ProxyQueryConnection::finish(&pending, &response, &cancellation, deadline)
        .expect("the proxy counts every entity without scanning its answers");
    assert!(matches!(counted.result(), MatchResult::Count { value, .. } if *value == ENTITIES));

    let fetch = || {
        request(
            &registry,
            &item_type,
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: BindingId::new(0),
                    }],
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: ENTITIES,
                },
                cardinality: RowCardinality::BoundedMany,
            },
        )
    };
    let (_, fetched) = QueryTarget::Database(&db)
        .execute(
            &registry,
            fetch(),
            direct,
            &cancellation,
            QueryExecutionDeadline::for_limits(direct),
        )
        .await
        .expect("direct fetch fits default answer-size limits");
    assert!(
        matches!(fetched.result(), MatchResult::Rows { rows, .. } if rows.len() as u64 == ENTITIES)
    );
    let deadline = QueryExecutionDeadline::for_limits(proxy);
    let pending = connection
        .prepare(&registry, fetch(), proxy, &cancellation, deadline)
        .expect("proxy fetch prepares");
    let (response, _) = execute_staged(&db, &pending, &authority, &advertisement).await;
    let error = ProxyQueryConnection::finish(&pending, &response, &cancellation, deadline)
        .expect_err("the proxy refuses an answer past the wire item limit");
    let QueryTargetError::Reply(RemoteModelQueryError::Execution(refusal)) = error else {
        panic!("expected an authenticated refusal, got {error:?}");
    };
    assert!(
        matches!(
            refusal.code().as_str(),
            "processed_item_limit" | "solution_scan_limit"
        ),
        "item-limit refusal code: {}",
        refusal.code().as_str()
    );
    assert_eq!(
        refusal.category(),
        type_bridge_contract::sdk_diagnostic::SdkDiagnosticCategory::ResourceLimit
    );
    let detail = |name: &str| {
        refusal
            .details()
            .get(&SdkDiagnosticName::new(name).unwrap())
            .cloned()
    };
    assert_eq!(
        detail("maximum"),
        Some(SdkDiagnosticDetailValue::Count(DEFAULT_PROXY_QUERY_ITEMS))
    );
    for (name, expected) in [("limit", "items"), ("source", "default")] {
        assert!(
            matches!(detail(name), Some(SdkDiagnosticDetailValue::QueryIdentity(value)) if value.as_str() == expected)
        );
    }

    db.execute_raw(
        &format!("match $e isa {item_type}; delete $e;"),
        TxType::Write,
    )
    .await
    .expect("remove the fixture entities");
}
