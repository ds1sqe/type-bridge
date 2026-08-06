//! Canonical execution of the #171 language-neutral identity fixture.
//!
//! The fixture supplies provider solutions and expected logical identities. A
//! recording backend translates only logical fixture IDs to valid provider
//! IIDs; the production selected-result executor owns tuple/root distinctness,
//! page windows, collection multiplicity, hydration, count, and existence.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Deserialize;
use serde_json::{Value, json};
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan,
};
use type_bridge_orm::_descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor,
};
use type_bridge_orm::_entity::Annotation;
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::session::backend::{
    AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits, BoundedAnswerReader,
    BoundedAnswerStats, BoxFuture, DriverBackend, QueryResult, TransactionOps,
};
use type_bridge_orm::{
    BindingId, BindingPair, CapabilitySet, Database, FetchShape, FetchSlot, MatchBinding,
    MatchExpr, MatchMode, MatchOperation, MatchPlan, MatchRequest, MatchResult, OrmError,
    RoleEdgeId, RowCardinality, SlotValue, ThingKind, TxType, ValueType, Window,
};

const FIXTURE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../tests/contracts/typed_query/expected-results-v1.json"
));
const CORPUS_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../tests/contracts/typed_query/corpus-v1.json"
));
const BINDING_NAMES: [&str; 4] = ["person", "employment", "company", "witness"];

#[derive(Clone, Deserialize)]
struct IdentityFixture {
    fixture_id: String,
    version: u8,
    solutions: Vec<FixtureSolution>,
    expected: Value,
}

#[derive(Clone, Deserialize)]
struct FixtureSolution {
    bindings: BTreeMap<String, String>,
}

#[derive(Default)]
struct RecordingEvents {
    opens: AtomicUsize,
    closes: AtomicUsize,
    selected_statements: AtomicUsize,
    root_statements: AtomicUsize,
    rematch_statements: AtomicUsize,
    hydration_statements: AtomicUsize,
}

struct CorpusBackend {
    fixture: Arc<IdentityFixture>,
    events: Arc<RecordingEvents>,
}

impl DriverBackend for CorpusBackend {
    fn match_capabilities(&self) -> CapabilitySet {
        CapabilitySet::all()
    }

    fn open_transaction(
        &self,
        _database: &str,
        _tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.events.opens.fetch_add(1, Ordering::SeqCst);
        let transaction = CorpusTransaction {
            fixture: Arc::clone(&self.fixture),
            events: Arc::clone(&self.events),
        };
        Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
    }

    fn is_open(&self) -> bool {
        true
    }
}

struct CorpusTransaction {
    fixture: Arc<IdentityFixture>,
    events: Arc<RecordingEvents>,
}

impl TransactionOps for CorpusTransaction {
    fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        Box::pin(async { panic!("semantic-corpus backend used the legacy string query seam") })
    }

    fn query_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.events
            .selected_statements
            .fetch_add(1, Ordering::SeqCst);
        let items = solution_rows(&self.fixture)
            .into_iter()
            .take(usize::try_from(query.limit).unwrap_or(usize::MAX))
            .collect();
        Box::pin(async move { feed(items, limits, consumer) })
    }

    fn supports_exactly_one_tuple_proof(&self) -> bool {
        true
    }

    fn query_tuple_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.events
            .selected_statements
            .fetch_add(1, Ordering::SeqCst);
        let items = tuple_rows(&self.fixture, &query.projection, query.limit);
        Box::pin(async move { feed(items, limits, consumer) })
    }

    fn query_root_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedRootScan,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.events.root_statements.fetch_add(1, Ordering::SeqCst);
        let root_name = binding_name(query.root);
        let mut seen = BTreeSet::new();
        let items = self
            .fixture
            .solutions
            .iter()
            .filter_map(|solution| {
                let concept_id = provider_iid(&solution.bindings[root_name]);
                seen.insert(concept_id).then(|| {
                    AnswerItem::Row(json!({
                        "bindings": [{
                            "binding": query.root,
                            "concept_id": concept_id,
                        }],
                        "satisfied_role_edges": [],
                    }))
                })
            })
            .take(
                query
                    .limit
                    .and_then(|limit| usize::try_from(limit).ok())
                    .unwrap_or(usize::MAX),
            )
            .collect();
        Box::pin(async move { feed(items, limits, consumer) })
    }

    fn rematch_page_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedPageRematch,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.events
            .rematch_statements
            .fetch_add(1, Ordering::SeqCst);
        let fixture = Arc::clone(&self.fixture);
        let selected = query
            .root_concept_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let root_name = binding_name(query.root);
        let items = fixture
            .solutions
            .iter()
            .filter(|solution| selected.contains(provider_iid(&solution.bindings[root_name])))
            .map(|solution| AnswerItem::Document(rematch_document(&fixture, solution)))
            .collect();
        Box::pin(async move { feed(items, limits, consumer) })
    }

    fn hydrate_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedHydrateThings,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.events
            .hydration_statements
            .fetch_add(1, Ordering::SeqCst);
        let mut items = Vec::new();
        for target in &query.targets {
            for concept_id in &target.concept_ids {
                let logical = logical_id(concept_id);
                items.push(AnswerItem::Document(hydrated_binding(
                    &self.fixture,
                    target.binding,
                    logical,
                )));
            }
        }
        Box::pin(async move { feed(items, limits, consumer) })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async { panic!("read-only corpus transaction was committed") })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async { panic!("read-only corpus transaction was rolled back") })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.events.closes.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

fn feed(
    items: Vec<AnswerItem>,
    limits: BoundedAnswerLimits,
    consumer: &mut dyn AnswerConsumer,
) -> Result<BoundedAnswerStats, OrmError> {
    let mut reader = BoundedAnswerReader::new(limits);
    reader.check_before_read()?;
    for item in items {
        if reader.accept(item, consumer)? == AnswerControl::Stop {
            break;
        }
    }
    Ok(reader.stats())
}

fn solution_rows(fixture: &IdentityFixture) -> Vec<AnswerItem> {
    fixture
        .solutions
        .iter()
        .map(|solution| {
            AnswerItem::Row(json!({
                "bindings": BINDING_NAMES.iter().enumerate().map(|(binding, name)| json!({
                    "binding": binding,
                    "concept_id": provider_iid(&solution.bindings[*name]),
                })).collect::<Vec<_>>(),
                "satisfied_role_edges": [0, 1],
            }))
        })
        .collect()
}

fn tuple_rows(fixture: &IdentityFixture, projection: &[u16], limit: u64) -> Vec<AnswerItem> {
    let mut seen = BTreeSet::new();
    fixture
        .solutions
        .iter()
        .filter_map(|solution| {
            let bindings = projection
                .iter()
                .map(|binding| {
                    let concept_id = provider_iid(&solution.bindings[binding_name(*binding)]);
                    (*binding, concept_id)
                })
                .collect::<Vec<_>>();
            let identity = bindings
                .iter()
                .map(|(_, concept_id)| *concept_id)
                .collect::<Vec<_>>();
            seen.insert(identity).then(|| {
                AnswerItem::Row(json!({
                    "bindings": bindings
                        .iter()
                        .map(|(binding, concept_id)| json!({
                            "binding": binding,
                            "concept_id": concept_id,
                        }))
                        .collect::<Vec<_>>(),
                    "satisfied_role_edges": [],
                }))
            })
        })
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .collect()
}

fn rematch_document(fixture: &IdentityFixture, solution: &FixtureSolution) -> Value {
    json!({
        "bindings": BINDING_NAMES.iter().enumerate().map(|(binding, name)| {
            hydrated_binding(fixture, binding as u16, &solution.bindings[*name])
        }).collect::<Vec<_>>(),
        "satisfied_role_edges": [0, 1],
    })
}

fn hydrated_binding(fixture: &IdentityFixture, binding: u16, logical: &str) -> Value {
    let type_name = logical.split_once(':').unwrap().0;
    let field_name = if type_name == "employment" {
        "code"
    } else {
        "name"
    };
    let field_value = display_value(logical);
    let roles = if type_name == "employment" {
        let solution = fixture
            .solutions
            .iter()
            .find(|solution| solution.bindings["employment"] == logical)
            .expect("every employment fixture identity has a complete solution");
        vec![
            hydrated_role("employee", &solution.bindings["person"]),
            hydrated_role("employer", &solution.bindings["company"]),
        ]
    } else {
        Vec::new()
    };
    json!({
        "binding": binding,
        "concept_id": provider_iid(logical),
        "concrete_type": type_name,
        "kind": if type_name == "employment" { "relation" } else { "entity" },
        "attributes": [{
            "field": field_name,
            "value_type": "string",
            "values": [field_value],
        }],
        "roles": roles,
    })
}

fn hydrated_role(role: &str, logical_player: &str) -> Value {
    let type_name = logical_player.split_once(':').unwrap().0;
    json!({
        "role": role,
        "players": [{
            "concept_id": provider_iid(logical_player),
            "declared_type": type_name,
            "concrete_type": type_name,
            "kind": "entity",
            "attributes": [{
                "field": "name",
                "value_type": "string",
                "values": [display_value(logical_player)],
            }],
        }],
    })
}

fn provider_iid(logical: &str) -> &'static str {
    match logical {
        "person:alice" => "0x01",
        "person:bob" => "0x02",
        "employment:alice-acme-1" => "0x11",
        "employment:alice-acme-2" => "0x12",
        "employment:bob-beta-1" => "0x13",
        "company:acme" => "0x21",
        "company:beta" => "0x22",
        "skill:rust" => "0x31",
        "skill:typedb" => "0x32",
        "skill:python" => "0x33",
        other => panic!("unmapped fixture identity {other}"),
    }
}

fn logical_id(iid: &str) -> &'static str {
    match iid {
        "0x01" => "person:alice",
        "0x02" => "person:bob",
        "0x11" => "employment:alice-acme-1",
        "0x12" => "employment:alice-acme-2",
        "0x13" => "employment:bob-beta-1",
        "0x21" => "company:acme",
        "0x22" => "company:beta",
        "0x31" => "skill:rust",
        "0x32" => "skill:typedb",
        "0x33" => "skill:python",
        other => panic!("unmapped provider IID {other}"),
    }
}

fn display_value(logical: &str) -> String {
    logical
        .split_once(':')
        .unwrap()
        .1
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn binding_name(binding: u16) -> &'static str {
    BINDING_NAMES[usize::from(binding)]
}

fn key(field_name: &str) -> OwnedAttributeDescriptor {
    OwnedAttributeDescriptor {
        field_name: field_name.into(),
        attr_name: field_name.into(),
        value_type: ValueType::String,
        annotations: vec![Annotation::Key],
        is_optional: false,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    }
}

fn registry() -> DescriptorRegistry {
    let registry = DescriptorRegistry::new();
    for type_name in ["person", "company", "skill"] {
        registry
            .register_entity(EntityDescriptor {
                type_name: type_name.into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![key("name")],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
    }
    registry
        .register_relation(RelationDescriptor {
            type_name: "employment".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![key("code")],
            roles: vec![
                RoleDescriptor {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    ..RoleDescriptor::default()
                },
                RoleDescriptor {
                    role_name: "employer".into(),
                    player_type_names: vec!["company".into()],
                    ..RoleDescriptor::default()
                },
            ],
            doc: None,
            meta: Default::default(),
        })
        .unwrap();
    registry
}

fn plan(registry: &DescriptorRegistry) -> MatchPlan {
    let person = BindingId::new(0);
    let employment = BindingId::new(1);
    let company = BindingId::new(2);
    let witness = BindingId::new(3);
    let employment_descriptor = registry.descriptor_id("employment").unwrap();
    MatchPlan {
        bindings: [
            (person, "person", ThingKind::Entity),
            (employment, "employment", ThingKind::Relation),
            (company, "company", ThingKind::Entity),
            (witness, "skill", ThingKind::Entity),
        ]
        .into_iter()
        .map(|(id, name, thing_kind)| MatchBinding {
            id,
            descriptor: registry.descriptor_id(name).unwrap(),
            thing_kind,
            match_mode: MatchMode::Exact,
        })
        .collect(),
        predicate: Some(MatchExpr::And {
            expressions: vec![
                MatchExpr::RoleEdge {
                    id: RoleEdgeId::new(0),
                    relation: employment,
                    role: registry
                        .role_id(&employment_descriptor, "employee")
                        .unwrap(),
                    player: person,
                },
                MatchExpr::RoleEdge {
                    id: RoleEdgeId::new(1),
                    relation: employment,
                    role: registry
                        .role_id(&employment_descriptor, "employer")
                        .unwrap(),
                    player: company,
                },
            ],
        }),
        allowed_cross_joins: BTreeSet::from([BindingPair::new(person, witness)]),
    }
}

fn rows_request(registry: &DescriptorRegistry) -> MatchRequest {
    MatchRequest::v1(
        plan(registry),
        MatchOperation::FetchRows {
            output: FetchShape::Positional {
                slots: [0, 1, 2]
                    .into_iter()
                    .map(|binding| FetchSlot::One {
                        binding: BindingId::new(binding),
                    })
                    .collect(),
            },
            order: Vec::new(),
            window: Window {
                offset: 0,
                limit: 10,
            },
            cardinality: RowCardinality::BoundedMany,
        },
    )
}

fn exact_one_person_request(registry: &DescriptorRegistry) -> MatchRequest {
    MatchRequest::v1(
        plan(registry),
        MatchOperation::FetchRows {
            output: FetchShape::Positional {
                slots: vec![FetchSlot::One {
                    binding: BindingId::new(0),
                }],
            },
            order: Vec::new(),
            window: Window {
                offset: 0,
                limit: 1,
            },
            cardinality: RowCardinality::ExactlyOne,
        },
    )
}

fn page_request(
    registry: &DescriptorRegistry,
    limit: u64,
    distinct: bool,
    include_total: bool,
) -> MatchRequest {
    MatchRequest::v1(
        plan(registry),
        MatchOperation::PageBy {
            root: BindingId::new(0),
            output: FetchShape::Positional {
                slots: vec![
                    FetchSlot::One {
                        binding: BindingId::new(0),
                    },
                    FetchSlot::Collect {
                        binding: BindingId::new(1),
                        distinct,
                        order: Vec::new(),
                    },
                ],
            },
            order: Vec::new(),
            window: Window { offset: 0, limit },
            include_total,
        },
    )
}

fn root_request(
    registry: &DescriptorRegistry,
    operation: impl FnOnce(BindingId) -> MatchOperation,
) -> MatchRequest {
    let root = BindingId::new(0);
    MatchRequest::v1(plan(registry), operation(root))
}

fn logical_slot(slot: &SlotValue) -> Value {
    match slot {
        SlotValue::One(thing) => json!(logical_id(thing.concept_id().as_str())),
        SlotValue::Many(things) => json!(
            things
                .iter()
                .map(|thing| logical_id(thing.concept_id().as_str()))
                .collect::<Vec<_>>()
        ),
    }
}

fn expected<'a>(fixture: &'a IdentityFixture, key: &str) -> &'a Value {
    &fixture.expected[key]
}

#[tokio::test]
async fn identity_manifest_executes_through_the_canonical_selected_result_backend() {
    let fixture: Arc<IdentityFixture> = Arc::new(serde_json::from_str(FIXTURE_JSON).unwrap());
    assert_eq!(fixture.fixture_id, "identity-main");
    assert_eq!(fixture.version, 1);

    let corpus: Value = serde_json::from_str(CORPUS_JSON).unwrap();
    let result_references = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|case| case["expected"]["result_ref"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        result_references,
        BTreeSet::from([
            "identity-main.alice_collect_distinct_employments",
            "identity-main.alice_collect_employments",
            "identity-main.count_by_person",
            "identity-main.exists_by_person",
            "identity-main.page_by_person_offset_0_limit_1",
            "identity-main.selected_rows",
        ])
    );

    let registry = registry();
    let events = Arc::new(RecordingEvents::default());
    let database = Database::with_backend(
        Box::new(CorpusBackend {
            fixture: Arc::clone(&fixture),
            events: Arc::clone(&events),
        }),
        "semantic-corpus",
    );

    let rows = rows_request(&registry).validate(&registry).unwrap();
    let rows = database.execute_match(&registry, &rows).await.unwrap();
    let MatchResult::Rows { rows } = rows.result() else {
        panic!("expected selected rows")
    };
    let selected_rows = json!(
        rows.iter()
            .map(|row| row.slots().iter().map(logical_slot).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
    assert_eq!(&selected_rows, expected(&fixture, "selected_rows"));

    let all_roots = page_request(&registry, 2, false, false)
        .validate(&registry)
        .unwrap();
    let all_roots = database.execute_match(&registry, &all_roots).await.unwrap();
    let MatchResult::Page { entries, .. } = all_roots.result() else {
        panic!("expected root page")
    };
    let distinct_roots = json!(
        entries
            .iter()
            .map(|row| logical_slot(&row.slots()[0]))
            .collect::<Vec<_>>()
    );
    assert_eq!(&distinct_roots, expected(&fixture, "distinct_roots"));

    let collected = page_request(&registry, 1, false, true)
        .validate(&registry)
        .unwrap();
    let collected = database.execute_match(&registry, &collected).await.unwrap();
    let MatchResult::Page {
        entries,
        window,
        total,
        ..
    } = collected.result()
    else {
        panic!("expected collection page")
    };
    let page = json!({
        "roots": entries.iter().map(|row| logical_slot(&row.slots()[0])).collect::<Vec<_>>(),
        "offset": window.offset,
        "limit": window.limit,
        "total": total,
    });
    assert_eq!(&page, expected(&fixture, "page_by_person_offset_0_limit_1"));
    assert_eq!(
        &logical_slot(&entries[0].slots()[1]),
        expected(&fixture, "alice_collect_employments")
    );

    let collected_distinct = page_request(&registry, 1, true, false)
        .validate(&registry)
        .unwrap();
    let collected_distinct = database
        .execute_match(&registry, &collected_distinct)
        .await
        .unwrap();
    let MatchResult::Page { entries, .. } = collected_distinct.result() else {
        panic!("expected distinct collection page")
    };
    assert_eq!(
        &logical_slot(&entries[0].slots()[1]),
        expected(&fixture, "alice_collect_distinct_employments")
    );

    let count = root_request(&registry, |root| MatchOperation::CountBy { root })
        .validate(&registry)
        .unwrap();
    let count = database.execute_match(&registry, &count).await.unwrap();
    let MatchResult::Count { value, .. } = count.result() else {
        panic!("expected root count")
    };
    assert_eq!(json!(value), *expected(&fixture, "count_by_person"));

    let exists = root_request(&registry, |root| MatchOperation::ExistsBy { root })
        .validate(&registry)
        .unwrap();
    let exists = database.execute_match(&registry, &exists).await.unwrap();
    let MatchResult::Exists { value, .. } = exists.result() else {
        panic!("expected root existence")
    };
    assert_eq!(json!(value), *expected(&fixture, "exists_by_person"));

    assert_eq!(events.opens.load(Ordering::SeqCst), 6);
    assert_eq!(events.closes.load(Ordering::SeqCst), 6);
    assert_eq!(events.selected_statements.load(Ordering::SeqCst), 1);
    assert!(events.root_statements.load(Ordering::SeqCst) >= 6);
    assert_eq!(events.rematch_statements.load(Ordering::SeqCst), 3);
    assert!(events.hydration_statements.load(Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn exact_one_reads_past_duplicate_hidden_witnesses_before_not_unique() {
    let fixture: Arc<IdentityFixture> = Arc::new(serde_json::from_str(FIXTURE_JSON).unwrap());
    assert_eq!(fixture.solutions[0].bindings["person"], "person:alice");
    assert_eq!(fixture.solutions[1].bindings["person"], "person:alice");
    assert_eq!(fixture.solutions[3].bindings["person"], "person:bob");

    let registry = registry();
    let events = Arc::new(RecordingEvents::default());
    let database = Database::with_backend(
        Box::new(CorpusBackend {
            fixture,
            events: Arc::clone(&events),
        }),
        "semantic-corpus-exact-one",
    );
    let request = exact_one_person_request(&registry)
        .validate(&registry)
        .unwrap();

    let error = database
        .execute_match(&registry, &request)
        .await
        .expect_err("Bob must make the selected person identity non-unique");
    let OrmError::Match(error) = error else {
        panic!("expected a canonical typed-match cardinality error")
    };
    assert_eq!(error.code().as_str(), "not_unique");
    assert_eq!(events.selected_statements.load(Ordering::SeqCst), 1);
    assert_eq!(events.closes.load(Ordering::SeqCst), 1);
}
