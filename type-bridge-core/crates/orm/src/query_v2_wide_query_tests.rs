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
use type_bridge_contract::query_plan::query_plan_v2_capability_vocabulary;
use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
use type_bridge_contract::query_remote_v2::{
    RemoteLimitsV2, query_remote_v2_required_capabilities,
};
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
use crate::query_v2_model_remote::prepare_remote_model_query_v2;
use crate::query_v2_prepared::QueryAuthority;
use crate::query_v2_remote::{RemoteReplySigningKey, preflight_remote_request_v2};
use crate::session::backend::{AnswerCancellation, AnswerLimits, BoundedAnswerLimits};

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
    let mut capabilities = query_plan_v2_capability_vocabulary()
        .iter()
        .cloned()
        .collect::<CapabilitySet>();
    for capability in query_remote_v2_required_capabilities(true) {
        capabilities.insert(capability);
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
    let pending = prepare_remote_model_query_v2(
        &authority,
        &registry,
        common_definitions(&registry, evidence),
        &advertisement.encode().expect("advertisement bytes"),
        RemoteLimitsV2 {
            deadline_ms: Some(30_000),
            max_bytes: 1 << 22,
            max_items: 500,
            max_collection_members: 1 << 16,
            max_graph_nodes: 1 << 16,
            max_attribute_values: 1 << 16,
            max_role_players: 1 << 16,
            max_statements: 3,
        },
    )
    .unwrap_or_else(|error| panic!("the client prepares the request: {error:?}"));

    // The executor's own admission: decode, validate against the schema, and
    // check capabilities, as the proxy does on receipt.
    preflight_remote_request_v2(
        pending.request_bytes(),
        &authority.context(),
        &advertisement,
        AnswerLimits {
            answer: BoundedAnswerLimits {
                max_items: 500,
                max_bytes: 1 << 22,
                deadline: None,
                cancellation: AnswerCancellation::default(),
            },
            max_collection_members: 1 << 16,
        },
    )
    .unwrap_or_else(|_| panic!("the executor admits the request"));
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

    use type_bridge_contract::query_remote_v2::RemoteQueryRequestV2;

    let registry = automotive_registry();
    let authority = authority(&registry);
    let wanted = RemoteLimitsV2 {
        deadline_ms: Some(30_000),
        max_bytes: 1 << 31,
        max_items: 2_000_000,
        max_collection_members: 1 << 16,
        max_graph_nodes: 3_000_000,
        max_attribute_values: 1 << 16,
        max_role_players: 1 << 16,
        max_statements: 3,
    };
    let prepared_limits = |advertisement: &RemoteCapabilities| {
        let pending = prepare_remote_model_query_v2(
            &authority,
            &registry,
            common_definitions(&registry, false),
            &advertisement.encode().expect("advertisement bytes"),
            wanted,
        )
        .unwrap_or_else(|error| panic!("the client prepares the request: {error:?}"));
        RemoteQueryRequestV2::decode(pending.request_bytes())
            .expect("the prepared request decodes")
            .limits()
    };

    // An executor that advertised nothing has the default ceilings.
    let held = prepared_limits(&executor_advertisement());
    assert_eq!(
        (held.max_bytes, held.max_items, held.max_graph_nodes),
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
        (held.max_bytes, held.max_items, held.max_graph_nodes),
        (1 << 30, 1_000_000, 65_536)
    );
}
