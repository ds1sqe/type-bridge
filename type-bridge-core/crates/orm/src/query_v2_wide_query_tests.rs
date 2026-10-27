//! The two-anchor automotive query, on its real schema shape.
//!
//! "Which catalogue definitions are shared by the direct-child parts of both
//! cars' engines?" joins two identifier-to-definition chains on the
//! definition: 25 bindings and 38 `where` clauses. A generated remote session
//! once refused it before sending, because each clause nested the predicate
//! one level deeper. These tests build the query the way the generated
//! bindings do, one `where_predicate` call per clause, and take it through
//! remote preparation and executor admission.

use std::sync::Arc;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary;
use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
use type_bridge_contract::query_remote_v2::query_remote_v2_required_capabilities;
use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
use type_bridge_schema_compat::released_typeql_to_declared_projection;

use crate::_attribute::ValueType;
use crate::_descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor,
};
use crate::_entity::Annotation;
use crate::_registry::DescriptorRegistry;
use crate::_schema::SchemaInfo;
use crate::AttributeValue;
use crate::match_request::{
    BindingHandle, ComparisonOp, MissingOrder, PredicateHandle, RowCardinality, SessionHandle,
    SortDirection, ValidatedMatchRequest, Window,
};
use crate::query_v2_model_remote::prepare_remote_model_query_with_policy;
use crate::query_v2_prepared::QueryAuthority;
use crate::query_v2_remote::RemoteReplySigningKey;
use crate::session::backend::AnswerCancellation;
use crate::{QueryExecutionDeadline, QueryExecutionResourceLimits};

const PART_KINDS: [&str; 6] = [
    "camshaft",
    "gear",
    "injector",
    "piston",
    "spark_plug",
    "turbocharger",
];

fn attribute(
    name: &str,
    value_type: ValueType,
    key: bool,
    optional: bool,
) -> OwnedAttributeDescriptor {
    OwnedAttributeDescriptor {
        field_name: name.to_owned(),
        attr_name: name.to_owned(),
        value_type,
        annotations: if key {
            vec![Annotation::Key]
        } else if optional {
            vec![Annotation::Card(0, Some(1))]
        } else {
            vec![Annotation::Card(1, Some(1))]
        },
        is_optional: optional,
        is_ordered: false,
        doc: None,
        meta: Default::default(),
    }
}

fn entity(registry: &DescriptorRegistry, name: &str, parent: Option<&str>, key: &str) {
    registry
        .register_entity(EntityDescriptor {
            type_name: name.to_owned(),
            is_abstract: false,
            parent_type: parent.map(str::to_owned),
            // Descriptors are effective projections: a subtype repeats the
            // key it inherits.
            owned_attributes: vec![attribute(key, ValueType::String, true, false)],
            doc: None,
            meta: Default::default(),
        })
        .unwrap_or_else(|error| panic!("register {name}: {error:?}"));
}

fn relation(
    registry: &DescriptorRegistry,
    name: &str,
    key: Option<&str>,
    temporal: bool,
    roles: &[(&str, Vec<String>)],
) {
    let mut owned_attributes = Vec::new();
    if let Some(key) = key {
        owned_attributes.push(attribute(key, ValueType::String, true, false));
    }
    if temporal {
        owned_attributes.push(attribute(
            "effective_from",
            ValueType::DateTime,
            false,
            false,
        ));
        owned_attributes.push(attribute("effective_to", ValueType::DateTime, false, true));
    }
    registry
        .register_relation(RelationDescriptor {
            type_name: name.to_owned(),
            is_abstract: false,
            parent_type: None,
            owned_attributes,
            roles: roles
                .iter()
                .map(|(role, players)| RoleDescriptor {
                    role_name: (*role).to_owned(),
                    player_type_names: players.clone(),
                    ..Default::default()
                })
                .collect(),
            doc: None,
            meta: Default::default(),
        })
        .unwrap_or_else(|error| panic!("register {name}: {error:?}"));
}

/// The part of the automotive ontology the query touches, with its real
/// hierarchy: `Part` and `DefinitionRevision` each have six subtypes and
/// `PartDefinition` has two.
fn automotive_registry() -> Arc<DescriptorRegistry> {
    let registry = Arc::new(DescriptorRegistry::new());
    entity(
        &registry,
        "operational_identifier",
        None,
        "operational_identifier_id",
    );
    entity(&registry, "car", None, "vehicle_id");
    entity(&registry, "component", None, "component_id");
    entity(&registry, "assembly", Some("component"), "component_id");
    entity(&registry, "powertrain", Some("assembly"), "component_id");
    entity(&registry, "engine", Some("assembly"), "component_id");
    entity(&registry, "part", Some("component"), "component_id");
    entity(
        &registry,
        "definition_revision",
        None,
        "definition_revision_id",
    );
    entity(&registry, "part_definition", None, "definition_id");
    for kind in PART_KINDS {
        entity(&registry, kind, Some("part"), "component_id");
        entity(
            &registry,
            &format!("{kind}_definition_revision"),
            Some("definition_revision"),
            "definition_revision_id",
        );
    }
    for kind in ["engineering_part_definition", "service_part_definition"] {
        entity(&registry, kind, Some("part_definition"), "definition_id");
    }

    let names = |base: &[&str], suffix: &str| -> Vec<String> {
        base.iter()
            .map(|name| (*name).to_owned())
            .chain(PART_KINDS.iter().map(|kind| format!("{kind}{suffix}")))
            .collect()
    };
    let parts = names(&["part"], "");
    let revisions = names(&["definition_revision"], "_definition_revision");
    let assemblies = vec!["assembly".to_owned(), "powertrain".into(), "engine".into()];
    let components = names(
        &["component", "assembly", "powertrain", "engine", "part"],
        "",
    );
    let definitions = vec![
        "part_definition".to_owned(),
        "engineering_part_definition".into(),
        "service_part_definition".into(),
    ];

    relation(
        &registry,
        "identifies_car",
        None,
        false,
        &[
            ("car_identifier", vec!["operational_identifier".into()]),
            ("identified_car", vec!["car".into()]),
        ],
    );
    relation(
        &registry,
        "equipped_with",
        Some("equipment_id"),
        true,
        &[
            ("component", vec!["powertrain".into()]),
            ("vehicle", vec!["car".into()]),
        ],
    );
    relation(
        &registry,
        "composition",
        Some("composition_id"),
        true,
        &[("child", components), ("parent", assemblies)],
    );
    relation(
        &registry,
        "conforms_to",
        None,
        false,
        &[("definition", revisions.clone()), ("part", parts)],
    );
    relation(
        &registry,
        "revision_of",
        None,
        false,
        &[("revised_definition", definitions), ("revision", revisions)],
    );
    registry
}

fn authority(registry: &DescriptorRegistry) -> QueryAuthority {
    let schema = SchemaInfo::from_descriptors(&registry.snapshot());
    let declared = released_typeql_to_declared_projection(
        DocumentId::new("automotive.tql").expect("document"),
        &schema.to_typeql().expect("fixture schema"),
    )
    .expect("released descriptor projection");
    QueryAuthority::from_declared_bytes(
        &encode_declared_schema(&declared).expect("declared schema bytes"),
        "typebridge-v1-descriptor-registry",
        "typedb-3.12.1/v1",
    )
    .expect("matching query authority")
}

/// One car's chain: identifier, car, powertrain, engine, direct part, the
/// part's definition revision, and the shared definition.
struct Chain {
    car: BindingHandle,
    part: BindingHandle,
    hidden: Vec<BindingHandle>,
    clauses: Vec<PredicateHandle>,
}

fn active_at(edge: &BindingHandle, as_of: &str) -> [PredicateHandle; 2] {
    let instant = || AttributeValue::DateTime(as_of.to_owned());
    [
        edge.field("effective_from")
            .expect("effective_from")
            .compare_value(ComparisonOp::LessThanOrEqual, instant()),
        edge.field("effective_to")
            .expect("effective_to")
            .compare_value(ComparisonOp::LessThanOrEqual, instant())
            .not(),
    ]
}

fn chain(session: &SessionHandle, identifier_id: &str, definition: &BindingHandle) -> Chain {
    let identifier = session.exact("operational_identifier").expect("identifier");
    let identifies = session.exact("identifies_car").expect("identifies_car");
    let car = session.exact("car").expect("car");
    let equipped = session.exact("equipped_with").expect("equipped_with");
    let powertrain = session.exact("powertrain").expect("powertrain");
    let powertrain_engine = session.exact("composition").expect("composition");
    let engine = session.exact("engine").expect("engine");
    let engine_part = session.exact("composition").expect("composition");
    let part = session.subtypes("part").expect("part");
    let conformance = session.exact("conforms_to").expect("conforms_to");
    let revision = session.subtypes("definition_revision").expect("revision");
    let revision_of = session.exact("revision_of").expect("revision_of");
    let as_of = "2026-01-01T00:00:00";

    let edge = |relation: &BindingHandle, role: &str, player: &BindingHandle| {
        relation
            .role(role)
            .unwrap_or_else(|error| panic!("{role}: {error:?}"))
            .connects(player)
            .unwrap_or_else(|error| panic!("{role} edge: {error:?}"))
    };
    let mut clauses = vec![
        identifier
            .field("operational_identifier_id")
            .expect("identifier key")
            .compare_value(
                ComparisonOp::Equal,
                AttributeValue::String(identifier_id.into()),
            ),
        edge(&identifies, "car_identifier", &identifier),
        edge(&identifies, "identified_car", &car),
        edge(&equipped, "vehicle", &car),
        edge(&equipped, "component", &powertrain),
    ];
    clauses.extend(active_at(&equipped, as_of));
    clauses.push(edge(&powertrain_engine, "parent", &powertrain));
    clauses.push(edge(&powertrain_engine, "child", &engine));
    clauses.extend(active_at(&powertrain_engine, as_of));
    clauses.push(edge(&engine_part, "parent", &engine));
    clauses.push(edge(&engine_part, "child", &part));
    clauses.extend(active_at(&engine_part, as_of));
    clauses.push(edge(&conformance, "part", &part));
    clauses.push(edge(&conformance, "definition", &revision));
    clauses.push(edge(&revision_of, "revision", &revision));
    clauses.push(edge(&revision_of, "revised_definition", definition));

    Chain {
        car: car.clone(),
        part: part.clone(),
        hidden: vec![
            identifier,
            identifies,
            car,
            equipped,
            powertrain,
            powertrain_engine,
            engine,
            engine_part,
            part,
            conformance,
            revision,
            revision_of,
        ],
        clauses,
    }
}

/// Build the query. `evidence` selects the physical part pairs behind each
/// shared definition instead of the definition alone.
fn common_definitions(registry: &Arc<DescriptorRegistry>, evidence: bool) -> ValidatedMatchRequest {
    let session = SessionHandle::new(Arc::clone(registry));
    let definition = session.subtypes("part_definition").expect("definition");
    let a = chain(&session, "fleet-car-a", &definition);
    let b = chain(&session, "fleet-car-b", &definition);

    let selected: Vec<BindingHandle> = if evidence {
        vec![
            a.car.clone(),
            a.part.clone(),
            definition.clone(),
            b.part.clone(),
            b.car.clone(),
        ]
    } else {
        vec![definition.clone()]
    };
    let shape = session
        .positional(selected.iter().map(BindingHandle::one))
        .expect("shape");
    let mut query = session.query(shape).expect("query");
    let mut clause_count = 0;
    for chain in [&a, &b] {
        for binding in &chain.hidden {
            // A binding the shape selects is already attached; the session
            // refuses to declare it twice, so keep the query as it is.
            if let Ok(next) = query.add_hidden(binding.clone()) {
                query = next;
            }
        }
    }
    for chain in [&a, &b] {
        for clause in &chain.clauses {
            // One call per clause, exactly as a generated `where(...)` does.
            query = query.where_predicate(clause.clone()).expect("where clause");
            clause_count += 1;
        }
    }
    assert_eq!(clause_count, 38, "the query is thirty-eight clauses wide");

    let order = if evidence {
        vec![
            definition
                .field("definition_id")
                .expect("definition key")
                .order(SortDirection::Ascending, MissingOrder::Reject),
            a.part
                .field("component_id")
                .expect("part key")
                .order(SortDirection::Ascending, MissingOrder::Reject),
            b.part
                .field("component_id")
                .expect("part key")
                .order(SortDirection::Ascending, MissingOrder::Reject),
            a.car
                .field("vehicle_id")
                .expect("car key")
                .order(SortDirection::Ascending, MissingOrder::Reject),
            b.car
                .field("vehicle_id")
                .expect("car key")
                .order(SortDirection::Ascending, MissingOrder::Reject),
        ]
    } else {
        vec![
            definition
                .field("definition_id")
                .expect("definition key")
                .order(SortDirection::Ascending, MissingOrder::Reject),
        ]
    };
    query
        .validate_fetch_rows(
            &order,
            Window {
                offset: 0,
                limit: 500,
            },
            RowCardinality::BoundedMany,
        )
        .unwrap_or_else(|error| panic!("the query passes request validation: {error:?}"))
}

fn executor_advertisement() -> RemoteCapabilities {
    let mut capabilities = query_plan_v3_capability_vocabulary()
        .iter()
        .cloned()
        .collect::<CapabilitySet>();
    for capability in query_remote_v2_required_capabilities(true) {
        capabilities.insert(capability);
    }
    for capability in [
        type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_STAGED_V3,
        type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_ANSWER_DIMENSIONS,
    ] {
        capabilities
            .insert(type_bridge_contract::capability::CapabilityId::new(capability).unwrap());
    }
    RemoteCapabilities::new(
        capabilities,
        RemoteExecutorBinding::new("automotive-executor", "epoch-00000000001")
            .expect("executor binding"),
        RemoteReplySigningKey::from_secret_bytes([0x71; 32]).public_key(),
    )
}

fn prepares_and_is_admitted(evidence: bool) {
    let registry = automotive_registry();
    let authority = authority(&registry);
    let advertisement = executor_advertisement();
    let request = common_definitions(&registry, evidence);
    let matched = request.request().plan.bindings.len();
    let policy = QueryExecutionResourceLimits {
        timeout_milliseconds: 30_000,
        bytes: 1 << 22,
        items: 500,
        collection_members: 1 << 16,
        graph_nodes: 1 << 16,
        attribute_values: 1 << 16,
        role_players: 1 << 16,
        statements: None,
    };
    let prepare = |request, policy| {
        prepare_remote_model_query_with_policy(
            &authority,
            &registry,
            request,
            &advertisement.encode().unwrap(),
            policy,
            QueryExecutionDeadline::for_limits(policy),
            &AnswerCancellation::default(),
        )
    };
    let pending = prepare(request, policy)
        .unwrap_or_else(|error| panic!("the client prepares the request: {error:?}"));
    let admitted = type_bridge_contract::query_remote_v3::StagedRequestV3::decode(
        pending.request_bytes(),
        authority.declared(),
        &advertisement,
    )
    .unwrap_or_else(|error| panic!("the executor admits the request: {error:?}"));
    let (width, bytes, two_columns_bytes) = admitted
        .plans()
        .iter()
        .filter_map(|entry| {
            let type_bridge_contract::query_plan::QueryOutput::Rows { columns } =
                entry.plan().output()
            else {
                return None;
            };
            let row = columns
                .iter()
                .map(|binding| {
                    use type_bridge_contract::id::TypeKind;
                    use type_bridge_contract::query_remote::RemoteValue;
                    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
                    let type_id = entry
                        .plan()
                        .binding_types(*binding)
                        .unwrap()
                        .first()
                        .unwrap()
                        .clone();
                    if type_id.kind() == TypeKind::Attribute {
                        let value = if type_id.label().as_str().starts_with("effective_") {
                            CanonicalValue::DateTime("2026-01-01T00:00:00".parse().unwrap())
                        } else {
                            CanonicalValue::String(CanonicalString::new("fleet-car-a").unwrap())
                        };
                        RemoteValue::Attribute { type_id, value }
                    } else {
                        RemoteValue::Thing {
                            iid: format!("0x{:016x}", u64::from(binding.get()) + 1),
                            type_id,
                        }
                    }
                })
                .collect::<Vec<_>>();
            Some((
                columns.len(),
                serde_json::to_vec(&row).unwrap().len(),
                serde_json::to_vec(&row[..row.len().min(2)]).unwrap().len(),
            ))
        })
        .max()
        .unwrap();
    println!(
        "WIDE_NATIVE_STACK evidence={evidence} matched={matched} selected_models={} scalar_witness_columns=14 mapping_columns={width} representative_row_bytes={bytes} representative_two_columns_bytes={two_columns_bytes} plans={}",
        if evidence { 5 } else { 1 },
        admitted.plans().len()
    );
    assert_eq!(width, matched + 14);
    assert_eq!(admitted.plans().len(), if evidence { 6 } else { 2 });
    let strict = prepare(
        common_definitions(&registry, evidence),
        QueryExecutionResourceLimits {
            statements: Some(3),
            ..policy
        },
    );
    if evidence {
        assert!(
            matches!(strict, Err(crate::RemoteModelQueryV2Error::Match(error)) if error.code().as_str() == "statement_count_limit")
        );
    } else {
        assert!(strict.is_ok());
    }
}

#[test]
fn shared_definitions_of_two_cars_prepare_and_are_admitted() {
    prepares_and_is_admitted(false);
}

#[test]
fn shared_definition_evidence_of_two_cars_prepares_and_is_admitted() {
    prepares_and_is_admitted(true);
}

/// The client asks for no more than the executor advertised, and no more
/// than it wants itself.
#[test]
fn a_request_budget_is_held_to_the_advertised_answer_ceilings() {
    use std::collections::BTreeMap;

    use type_bridge_contract::query_remote_v3::StagedRequestV3;

    let registry = automotive_registry();
    let authority = authority(&registry);
    let wanted = QueryExecutionResourceLimits {
        timeout_milliseconds: 30_000,
        bytes: 1 << 31,
        items: 2_000_000,
        collection_members: 1 << 16,
        graph_nodes: 3_000_000,
        attribute_values: 1 << 16,
        role_players: 1 << 16,
        statements: None,
    };
    let prepared_limits = |advertisement: &RemoteCapabilities| {
        let pending = prepare_remote_model_query_with_policy(
            &authority,
            &registry,
            common_definitions(&registry, false),
            &advertisement.encode().expect("advertisement bytes"),
            wanted,
            QueryExecutionDeadline::for_limits(wanted),
            &AnswerCancellation::default(),
        )
        .unwrap_or_else(|error| panic!("the client prepares the request: {error:?}"));
        StagedRequestV3::decode(pending.request_bytes(), authority.declared(), advertisement)
            .expect("the prepared request decodes")
            .limits()
    };

    // An executor that advertised nothing has the default ceilings.
    let held = prepared_limits(&executor_advertisement());
    assert_eq!(
        (
            held.max_bytes,
            held.max_items,
            held.max_graph_nodes.unwrap()
        ),
        (32 << 20, 65_536, 65_536)
    );

    // One that advertised more admits more, and one dimension it left alone
    // stays at its default.
    let raised = executor_advertisement()
        .with_answer_limits(BTreeMap::from([
            ("bytes".to_owned(), 1_u64 << 30),
            ("items".to_owned(), 1_000_000),
        ]))
        .expect("raised ceilings");
    let held = prepared_limits(&raised);
    assert_eq!(
        (
            held.max_bytes,
            held.max_items,
            held.max_graph_nodes.unwrap()
        ),
        (1 << 30, 1_000_000, 65_536)
    );
}

#[test]
fn selected_definition_uses_native_hidden_revision_edge_without_hidden_hydration() {
    use crate::query_plan_exec::PlanAnswer;
    use type_bridge_contract::id::{TypeId, TypeKind};
    use type_bridge_contract::query_plan::{QueryOutput, v3::Stage};
    use type_bridge_contract::query_remote::RemoteValue;
    let registry = automotive_registry();
    let session = SessionHandle::new(Arc::clone(&registry));
    let definition = session.exact("part_definition").unwrap();
    let revision_of = session.exact("revision_of").unwrap();
    let predicate = revision_of
        .role("revised_definition")
        .unwrap()
        .connects(&definition)
        .unwrap();
    let request = session
        .query(session.positional([definition.one()]).unwrap())
        .unwrap()
        .add_hidden(revision_of)
        .unwrap()
        .where_predicate(predicate)
        .unwrap()
        .validate_fetch_rows(
            &[],
            Window {
                offset: 0,
                limit: 5,
            },
            RowCardinality::BoundedMany,
        )
        .unwrap();
    let stack =
        crate::match_request::plan_adapter::adapt_match_request(&registry, &request).unwrap();
    assert_eq!(stack.plans.len(), 2);
    let mut answers = stack.plans.iter().map(|plan| {
        if let Some(Stage::Fetch(document)) = plan.pipeline().last() {
            assert_eq!(document.thing.get(),0,"only selected definition is hydrated");
            return PlanAnswer::Documents(vec![serde_json::json!({"_iid":"0x01","_type":"part_definition","attributes":{"definition_id":"shared"}})]);
        }
        let QueryOutput::Rows {columns}=plan.output() else {panic!("mapping rows");};
        PlanAnswer::Rows(vec![serde_json::Value::Object(columns.iter().map(|binding| {
            let value=RemoteValue::Thing {iid:if binding.get()==0 {"0x01"} else {"0x02"}.into(),type_id:TypeId::new(if binding.get()==0 {TypeKind::Entity} else {TypeKind::Relation},if binding.get()==0 {"part_definition"} else {"revision_of"}).unwrap()};
            (format!("b{}",binding.get()),serde_json::to_value(value).unwrap())
        }).collect())])
    }).collect::<Vec<_>>();
    let policy = QueryExecutionResourceLimits::default();
    crate::match_request::plan_result::resolve(&stack, &request, &answers, policy).unwrap();
    let mapping = answers
        .iter_mut()
        .find_map(|answer| {
            if let PlanAnswer::Rows(rows) = answer {
                Some(rows)
            } else {
                None
            }
        })
        .unwrap();
    mapping[0]["b1"]["type_id"] = serde_json::json!({"kind":"entity","label":"part_definition"});
    assert!(
        crate::match_request::plan_result::resolve(&stack, &request, &answers, policy).is_err(),
        "hidden identities retain their native type fence"
    );
}

fn purchase_shop_reads(
    program: &str,
    tuple: bool,
) -> (Arc<DescriptorRegistry>, ValidatedMatchRequest) {
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
    let source=std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../../tests/contracts/sdk_conformance/writes/{program}/schema/fragments/{program}.yaml"))).unwrap();
    let documents = type_bridge_schema::SchemaDocumentSet::parse([(
        DocumentId::new("cost.yaml").unwrap(),
        source.as_str(),
    )])
    .unwrap();
    let declared = type_bridge_schema::normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new("typedb-3.13.6/v1").unwrap();
    let context = type_bridge_schema::ManagedDeltaContext::new(
        type_bridge_contract::managed_scope::ManagedScopeId::new("purchase-shop-cost").unwrap(),
        profile.clone(),
        declared.required_capabilities().clone(),
    );
    let authority = type_bridge_schema::build_schema_authority(
        &declared,
        declared.required_capabilities(),
        &context,
    )
    .unwrap();
    let resolved = type_bridge_schema::resolve(&declared, &profile).unwrap();
    let installed = crate::InstalledRuntimeProjection::try_new(
        type_bridge_schema::project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap()
    .with_schema_authority(&authority);
    let registry = Arc::new(installed.match_registry().unwrap());
    let session = SessionHandle::new(Arc::clone(&registry));
    let buyer = session.exact("account").unwrap();
    let item = session.exact("item").unwrap();
    let line = session
        .exact(if program == "purchase" {
            "purchase"
        } else {
            "order-line"
        })
        .unwrap();
    let selected = if tuple {
        vec![buyer.one(), item.one(), line.one()]
    } else {
        vec![item.one()]
    };
    let mut query = session
        .query(session.positional(selected).unwrap())
        .unwrap();
    if !tuple {
        query = query
            .add_hidden(buyer.clone())
            .unwrap()
            .add_hidden(line.clone())
            .unwrap();
    }
    query = query
        .where_predicate(line.role("buyer").unwrap().connects(&buyer).unwrap())
        .unwrap()
        .where_predicate(line.role("item").unwrap().connects(&item).unwrap())
        .unwrap();
    let request = query
        .validate_fetch_rows(
            &[],
            Window {
                offset: 0,
                limit: 10,
            },
            RowCardinality::Complete,
        )
        .unwrap();
    (registry, request)
}

#[test]
fn purchase_shop_constructor_cost_pins_bounded_prefix_repetition() {
    for program in ["purchase", "shop"] {
        for tuple in [false, true] {
            let (registry, request) = purchase_shop_reads(program, tuple);
            let stack =
                crate::match_request::plan_adapter::adapt_match_request(&registry, &request)
                    .unwrap();
            let queries = stack
                .plans
                .iter()
                .map(|plan| {
                    crate::query_plan_exec::staged_query(plan, true)
                        .unwrap()
                        .typeql
                })
                .collect::<Vec<_>>();
            let plans = if tuple { 4 } else { 2 };
            assert_eq!(queries.len(), plans);
            assert!(
                queries
                    .iter()
                    .all(|query| query.contains("account") && query.contains("item"))
            );
            println!(
                "PURCHASE_SHOP_COST program={program} selected_slots={} prefix_repeats={plans} plans={plans} rendered_typeql_bytes={}",
                if tuple { 3 } else { 1 },
                queries.iter().map(String::len).sum::<usize>()
            );
        }
    }
}

#[cfg(feature = "typedb")]
#[tokio::test]
#[ignore = "informational live sample; requires TYPEDB_ADDRESS/TYPEDB_HTTP_PORT"]
async fn purchase_shop_native_read_latency_sample() {
    use crate::session::backend::TxType;
    use crate::session::database::Database;
    use crate::session::real_driver::ConnectOptions;
    println!("PURCHASE_SHOP_LATENCY_RUNNER pid={}", std::process::id());
    for program in ["purchase", "shop"] {
        let name = format!("tb270_04a_cost_{program}_{}", std::process::id());
        let db = Database::connect_with_options(
            &std::env::var("TYPEDB_ADDRESS").unwrap(),
            &name,
            "admin",
            "password",
            ConnectOptions {
                http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        db.create_database().await.unwrap();
        let execution=crate::match_request::selected_result_executor::catch_execution_unwind(async {
            let schema=if program=="purchase" {
                "define attribute account-id, value string; attribute balance, value integer; attribute item-id, value string; attribute stock, value integer; attribute price, value integer; attribute order-id, value string; attribute name, value string; attribute on-call, value boolean; entity account, owns account-id, owns balance, plays purchase:buyer; entity item, owns item-id, owns stock, owns price, plays purchase:item; entity doctor, owns name, owns on-call; relation purchase, relates buyer, relates item, owns order-id;"
            } else {
                "define attribute account-id, value string; attribute balance, value integer; attribute sku, value string; attribute stock, value integer; attribute price, value integer; attribute order-id, value string; attribute quantity, value integer; entity account, owns account-id @key, owns balance @card(0..1), plays order-line:buyer; entity item, owns sku @key, owns stock @card(0..1), owns price @card(0..1), plays order-line:item; relation order-line, relates buyer, relates item, owns order-id @card(0..1), owns quantity @card(0..1);"
            };
            db.execute_raw(schema,TxType::Schema).await.unwrap();
            let insert=if program=="purchase" {
                "insert $a isa account, has account-id \"A1\", has balance 100; $i isa item, has item-id \"I1\", has stock 10, has price 5; $p isa purchase, links(buyer:$a,item:$i), has order-id \"O1\";"
            } else {
                "insert $a isa account, has account-id \"A1\", has balance 100; $i isa item, has sku \"I1\", has stock 10, has price 5; $p isa order-line, links(buyer:$a,item:$i), has order-id \"O1\", has quantity 1;"
            };
            db.execute_raw(insert,TxType::Write).await.unwrap();
            for tuple in [false,true] {
                let (registry,request)=purchase_shop_reads(program,tuple);
                db.execute_match(&registry,&request).await.unwrap();
                let mut samples=Vec::with_capacity(20);
                for _ in 0..20 {
                    let start=std::time::Instant::now();
                    std::hint::black_box(db.execute_match(&registry,&request).await.unwrap());
                    samples.push(start.elapsed().as_micros());
                }
                samples.sort_unstable();
                println!("PURCHASE_SHOP_LATENCY program={program} selected_slots={} iterations=20 median_us={} p95_us={}",if tuple {3} else {1},(samples[9]+samples[10])/2,samples[18]);
            }
        }).await;
        db.delete_database().await.unwrap();
        if let Err(payload) = execution {
            std::panic::resume_unwind(payload);
        }
    }
}

fn keyed_tuple_window(registry: &Arc<DescriptorRegistry>, offset: u64) -> ValidatedMatchRequest {
    let session = SessionHandle::new(Arc::clone(registry));
    let car = session.exact("car").unwrap();
    let powertrain = session.exact("powertrain").unwrap();
    let equipped = session.exact("equipped_with").unwrap();
    session
        .query(
            session
                .positional([car.one(), powertrain.one(), equipped.one()])
                .unwrap(),
        )
        .unwrap()
        .where_predicate(equipped.role("vehicle").unwrap().connects(&car).unwrap())
        .unwrap()
        .where_predicate(
            equipped
                .role("component")
                .unwrap()
                .connects(&powertrain)
                .unwrap(),
        )
        .unwrap()
        .validate_fetch_rows(
            &[car
                .field("vehicle_id")
                .unwrap()
                .order(SortDirection::Ascending, MissingOrder::Reject)],
            Window { offset, limit: 1 },
            RowCardinality::BoundedMany,
        )
        .unwrap()
}

fn tuple_with_independent_selection(
    registry: &DescriptorRegistry,
    request: &ValidatedMatchRequest,
) -> crate::match_request::plan_adapter::AdaptedMatchRequest {
    use type_bridge_contract::query_plan::v3::{PlanSpec, QueryPlanV3, Stage};
    let mut stack =
        crate::match_request::plan_adapter::adapt_match_request(registry, request).unwrap();
    let crate::match_request::plan_adapter::Layout::Fetch {
        selection,
        mapping: Some(mapping),
        hydration,
        ..
    } = &mut stack.layout
    else {
        panic!("tuple fetch");
    };
    assert!(selection.is_none());
    let plan = &stack.plans[*mapping];
    let rematch = plan
        .pipeline()
        .iter()
        .rposition(|stage| matches!(stage, Stage::Match { .. }))
        .unwrap();
    let prefix = plan.pipeline()[..rematch].to_vec();
    assert!(
        prefix
            .iter()
            .any(|stage| matches!(stage,Stage::Sort {terms} if terms.len()>=4))
    );
    assert!(
        prefix
            .iter()
            .any(|stage| matches!(stage, Stage::Limit { rows: 1 }))
    );
    let authority =
        crate::match_request::descriptor_plan::MatchRequestAdapterAuthority::from_registry(
            registry,
        )
        .unwrap();
    let original = QueryPlanV3::new_with_limits(
        PlanSpec {
            bindings: plan.bindings().to_vec(),
            inputs: plan.inputs().to_vec(),
            given_rows: plan.given_rows().map(<[_]>::to_vec),
            operation: plan.operation(),
            pipeline: prefix,
            output: plan.output().clone(),
            managed_semantics: plan.managed_semantics().clone(),
            compatibility: None,
            functions: plan.functions().to_vec(),
        },
        authority.declared(),
        plan.limits(),
    )
    .unwrap();
    *selection = Some(0);
    *mapping += 1;
    for (index, _, _) in hydration {
        *index += 1;
    }
    stack.plans.insert(0, original);
    assert_eq!(stack.plans.len(), 5);
    stack
}

#[test]
fn typed_tuple_mapping_retains_server_window_order_and_tiebreak() {
    let registry = automotive_registry();
    for offset in [0, 1, 2] {
        let request = keyed_tuple_window(&registry, offset);
        let reference = tuple_with_independent_selection(&registry, &request);
        let mapping = &reference.plans[1];
        for plan in &reference.plans[2..] {
            let pipeline = plan.pipeline();
            assert!(
                matches!(&pipeline[pipeline.len()-2],type_bridge_contract::query_plan::v3::Stage::Reduce {groups,..} if groups.len()==1)
            );
        }
        assert_eq!(
            &mapping.pipeline()[..mapping
                .pipeline()
                .iter()
                .rposition(|stage| matches!(
                    stage,
                    type_bridge_contract::query_plan::v3::Stage::Match { .. }
                ))
                .unwrap()],
            reference.plans[0].pipeline()
        );
        if offset > 0 {
            assert!(mapping.pipeline().iter().any(|stage|matches!(stage,type_bridge_contract::query_plan::v3::Stage::Offset {rows} if *rows==offset)));
        }
        assert_eq!(
            crate::match_request::plan_adapter::adapt_match_request(&registry, &request)
                .unwrap()
                .plans
                .len(),
            4
        );
    }
}

#[cfg(feature = "typedb")]
#[tokio::test]
#[ignore = "requires live TypeDB; compares four-plan tuple windows to independent five-plan selection"]
async fn typed_tuple_mapping_window_matches_five_plan_baseline_live() {
    use crate::session::backend::{AnswerCancellation, TxType};
    use crate::session::database::Database;
    use crate::session::real_driver::ConnectOptions;
    println!("TUPLE_WINDOW_RUNNER pid={}", std::process::id());
    let name = format!("tb270_04a_tuple_window_{}", std::process::id());
    let db = Database::connect_with_options(
        &std::env::var("TYPEDB_ADDRESS").unwrap(),
        &name,
        "admin",
        "password",
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.create_database().await.unwrap();
    let result=crate::match_request::selected_result_executor::catch_execution_unwind(async {
        db.execute_raw("define attribute vehicle_id, value string; attribute component_id, value string; attribute equipment_id, value string; attribute effective_from, value datetime; attribute effective_to, value datetime; entity car, owns vehicle_id @key, plays equipped_with:vehicle; entity powertrain, owns component_id @key, plays equipped_with:component; relation equipped_with, relates vehicle, relates component, owns equipment_id @key, owns effective_from @card(1), owns effective_to @card(0..1);",TxType::Schema).await.unwrap();
        db.execute_raw("insert $a isa car, has vehicle_id \"V1\"; $b isa car, has vehicle_id \"V2\"; $p isa powertrain, has component_id \"P1\"; $q isa powertrain, has component_id \"P2\"; $r isa powertrain, has component_id \"P3\"; $e1 isa equipped_with, links(vehicle:$a,component:$p), has equipment_id \"E1\", has effective_from 2026-01-01T00:00:00; $e2 isa equipped_with, links(vehicle:$a,component:$q), has equipment_id \"E2\", has effective_from 2026-01-01T00:00:00; $e3 isa equipped_with, links(vehicle:$b,component:$r), has equipment_id \"E3\", has effective_from 2026-01-01T00:00:00;",TxType::Write).await.unwrap();
        let registry=automotive_registry();
        let mut windows=Vec::new();
        for offset in [0,1,2] {
            let request=keyed_tuple_window(&registry,offset);
            let four=crate::match_request::plan_adapter::adapt_match_request(&registry,&request).unwrap();
            let five=tuple_with_independent_selection(&registry,&request);
            let limits=QueryExecutionResourceLimits::default().for_adapter_statements(5);
            let mut outcomes=Vec::new();
            for stack in [&four,&five] {
                let answers=db.execute_plans(&stack.plans,&limits,&AnswerCancellation::default(),QueryExecutionDeadline::for_limits(limits)).await.unwrap();
                outcomes.push(serde_json::to_value(crate::match_request::plan_result::resolve(stack,&request,&answers,limits).unwrap()).unwrap());
            }
            assert_eq!(outcomes[0],outcomes[1]);
            windows.push(outcomes.remove(0));
        }
        assert!(windows.windows(2).all(|pair|pair[0]!=pair[1]));
    }).await;
    db.delete_database().await.unwrap();
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

#[cfg(feature = "typedb")]
#[tokio::test]
#[ignore = "pins TypeDB given-window Distinct quirk on the supported server matrix"]
async fn second_distinct_given_window_provider_quirk() {
    use crate::session::backend::{GivenRowsSpec, GivenValue, QueryResult, TxType};
    use crate::session::database::Database;
    use crate::session::real_driver::ConnectOptions;
    println!("DISTINCT_QUIRK_RUNNER pid={}", std::process::id());
    let name = format!("tb270_04a_distinct_quirk_{}", std::process::id());
    let db = Database::connect_with_options(
        &std::env::var("TYPEDB_ADDRESS").unwrap(),
        &name,
        "admin",
        "password",
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    db.create_database().await.unwrap();
    let execution=crate::match_request::selected_result_executor::catch_execution_unwind(async {
        db.execute_raw("define attribute identifier, value string; entity person, owns identifier;",TxType::Schema).await.unwrap();
        db.execute_raw("insert $a isa person, has identifier \"person-1\"; $b isa person, has identifier \"person-2\";",TxType::Write).await.unwrap();
        let original="$a isa! person; $b isa! person; { $a has identifier $x; $x == $i0; } or { $a has identifier $y; $y == $i1; }; { $b has identifier $z; $z == $i2; } or { $b has identifier $w; $w == $i3; }; ";
        let alias=original.replace("$i0","$v0").replace("$i1","$v1").replace("$i2","$v2").replace("$i3","$v3");
        let prefix=format!("given $i0: string, $i1: string, $i2: string, $i3: string; match {original} let $k=iid($a); let $l=iid($b); let $v0=$i0; let $v1=$i1; let $v2=$i2; let $v3=$i3; select $a,$b,$k,$l,$v0,$v1,$v2,$v3; distinct; sort $k asc,$l asc; limit 4; ");
        for rematch in [String::new(),format!("match {alias}")] {
            for (tail,expected) in [("select $a; distinct; match $a isa! $t; fetch {\"iid\":iid($a),\"type\":label($t)};",4),("reduce $n=count groupby $a; select $a; match $a isa! $t; fetch {\"iid\":iid($a),\"type\":label($t)};",2),("select $a,$b; distinct;",4)] {
                let rows=GivenRowsSpec {variables:(0..4).map(|index|format!("i{index}")).collect(),rows:vec![["person-1","person-2","person-1","person-2"].into_iter().map(|value|GivenValue::String(value.into())).collect()]};
                let answer=db.execute_with_rows(&format!("{prefix}{rematch}{tail}"),TxType::Read,rows).await.unwrap();
                match answer {
                    QueryResult::Documents(documents)=>{
                        assert_eq!(documents.len(),expected,"if fixed, remove unique_partition_iids workaround");
                        assert_eq!(documents.iter().map(|document|document["iid"].as_str().unwrap()).collect::<std::collections::BTreeSet<_>>().len(),2);
                    }
                    QueryResult::Rows(rows)=>{
                        assert_eq!(rows.len(),4,"mapping preserves all four distinct identity tuples");
                        assert_eq!(rows.iter().map(|row|serde_json::to_string(row).unwrap()).collect::<std::collections::BTreeSet<_>>().len(),4);
                    }
                    _=>panic!("raw dedup evidence"),
                }
            }
        }
        let overlap=GivenRowsSpec {variables:(0..4).map(|index|format!("i{index}")).collect(),rows:vec![vec![GivenValue::String("person-1".into());4]]};
        let answer=db.execute_with_rows(&format!("{prefix}match {alias}select $a,$b; distinct;"),TxType::Read,overlap).await.unwrap();
        assert!(matches!(answer,QueryResult::Rows(rows) if rows.len()==1),"mapping deduplicates overlapping branch witnesses");
    }).await;
    db.delete_database().await.unwrap();
    if let Err(payload) = execution {
        std::panic::resume_unwind(payload);
    }
}
