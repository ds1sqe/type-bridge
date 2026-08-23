use std::any::TypeId as RustTypeId;
use std::collections::{BTreeSet, VecDeque};
use std::env;
use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use generated::*;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use type_bridge::__codegen::{EncodedCreate, EncodedReference, EncodedScalar, IntoEncodedCreate};
use type_bridge::{
    RemoteConnectionOptions, RemoteDatabase, RemoteQueryLimits, RemoteQueryTransport,
};
use type_bridge_contract::codec::from_canonical_json;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::query_plan::{CompatibilityValueV2, query_plan_v2_capability_vocabulary};
use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
use type_bridge_contract::query_remote_v2::{
    HydratedRowV2, HydrationAttributeEvidenceV2, HydrationGraphV2, HydrationNodeIdV2,
    HydrationNodeKindV2, HydrationNodeV2, HydrationReferenceV2, HydrationRoleEvidenceV2,
    HydrationSlotV2, RemoteLimitsV2, RemoteOutcomeV2, RemoteQueryRequestV2, RemoteQueryResponseV2,
    query_remote_v2_required_capabilities,
};
use type_bridge_contract::schema::{DocumentId, OwnsFactId, encode_declared_schema};
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration, CanonicalTime,
    TimeZoneDesignator,
};
use type_bridge_contract::value::{
    CanonicalDouble as ContractDouble, CanonicalString, CanonicalValue, DecimalValue,
};
use type_bridge_orm::query_v2_prepared::QueryAuthority;
use type_bridge_orm::query_v2_remote::RemoteReplySigningKey;
use type_bridge_orm::{
    AnswerCancellation, BindingId, FetchShape, FetchSlot, InstalledRuntimeProjection, MatchBinding,
    MatchMode, MatchOperation, MatchPlan, MatchRequest, ProjectedAttributeValue, ProjectedCreate,
    ProjectedQueryMaterializationLimits, ProjectedQueryOrigin, ProjectedQuerySlotValue,
    ProjectedQueryValue, RowCardinality, ThingKind, Window, prepare_remote_model_query_v2,
    validate_match_request,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents};

const REPORT_FORMAT: &str = "typebridge.phase2-projected-parity-report/v1";
const SEMANTIC_PROFILE: &str = "typedb-3.12.1/v1";
const REPORT_ENV: &str = "TYPE_BRIDGE_PHASE2_RUST_REPORT";
const ROOT_ENV: &str = "TYPE_BRIDGE_PHASE2_REPOSITORY_ROOT";
const MANAGED_SCOPE: &str = "schema-codegen-acceptance";
const SCHEMA_RELATIVE: &str = "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml";
const JOURNEY_RELATIVE: &str = "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json";
const MAX_REPORT_BYTES: usize = 256 * 1024;
const MAX_AUTHORITY_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
struct GraphReply {
    declared: TypeId,
    graph: HydrationGraphV2,
    root: HydrationNodeIdV2,
}

struct HydrationTransport {
    advertisement_contract: RemoteCapabilities,
    advertisement: Vec<u8>,
    exchanges: Arc<Mutex<usize>>,
    replies: Mutex<VecDeque<GraphReply>>,
    signer: RemoteReplySigningKey,
}

impl RemoteQueryTransport for HydrationTransport {
    fn capabilities(
        &self,
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + '_>> {
        let advertisement = self.advertisement.clone();
        Box::pin(async move { Ok(advertisement) })
    }

    fn exchange<'a>(
        &'a self,
        request: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + 'a>> {
        let response = (|| {
            *self.exchanges.lock().expect("exchange counter locks") += 1;
            let request = RemoteQueryRequestV2::decode(request)
                .expect("generated request has the V2 remote wire");
            request
                .validate_advertisement(&self.advertisement_contract)
                .expect("generated request matches its fetched advertisement");
            let plan = request.plan().expect("generated request carries its plan");
            let reply = self
                .replies
                .lock()
                .expect("reply queue locks")
                .pop_front()
                .expect("one graph reply exists per generated terminal");
            let reply_label = format!(
                "{:?}:{}",
                reply.declared.kind(),
                reply.declared.label().as_str()
            );
            let outcome = RemoteOutcomeV2::HydratedRows {
                graph: reply.graph,
                rows: vec![HydratedRowV2::new(vec![HydrationSlotV2::Singular {
                    value: HydrationReferenceV2::new(reply.declared, reply.root),
                }])],
            };
            RemoteQueryResponseV2::new(
                request.nonce(),
                &plan,
                &request
                    .fingerprint()
                    .expect("generated request fingerprints canonically"),
                request.result_kind(),
                outcome,
            )
            .and_then(|response| {
                response.encode_signed(
                    &self
                        .advertisement_contract
                        .fingerprint()
                        .expect("advertisement fingerprints canonically"),
                    &self.signer,
                )
            })
            .unwrap_or_else(|error| {
                panic!("provider-free {reply_label} response signs canonically: {error:?}")
            })
        })();
        Box::pin(async move { Ok(response) })
    }
}

fn hydration_transport(replies: Vec<GraphReply>) -> (HydrationTransport, Arc<Mutex<usize>>) {
    let signer = RemoteReplySigningKey::from_secret_bytes([0x42; 32]);
    let mut capabilities = query_plan_v2_capability_vocabulary();
    for capability in query_remote_v2_required_capabilities(true) {
        capabilities.insert(capability);
    }
    let advertisement_contract = RemoteCapabilities::new(
        capabilities,
        RemoteExecutorBinding::new("rust-phase2-parity", "epoch-00000000001")
            .expect("static executor identity is canonical"),
        signer.public_key(),
    );
    let advertisement = advertisement_contract
        .encode()
        .expect("advertisement encodes canonically");
    let exchanges = Arc::new(Mutex::new(0));
    (
        HydrationTransport {
            advertisement_contract,
            advertisement,
            exchanges: Arc::clone(&exchanges),
            replies: Mutex::new(replies.into()),
            signer,
        },
        exchanges,
    )
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("static Phase-2 type identity is canonical")
}

fn entity(label: &str) -> TypeId {
    type_id(TypeKind::Entity, label)
}

fn relation(label: &str) -> TypeId {
    type_id(TypeKind::Relation, label)
}

fn attribute(label: &str, values: Vec<CompatibilityValueV2>) -> HydrationAttributeEvidenceV2 {
    HydrationAttributeEvidenceV2::new(
        AttributeId::new(label).expect("static Phase-2 attribute is canonical"),
        values,
    )
}

fn reference(kind: TypeKind, label: &str, node: u32) -> HydrationReferenceV2 {
    HydrationReferenceV2::new(type_id(kind, label), HydrationNodeIdV2::new(node))
}

fn role(
    relation_label: &str,
    role_label: &str,
    players: Vec<HydrationReferenceV2>,
) -> HydrationRoleEvidenceV2 {
    HydrationRoleEvidenceV2::new(
        RoleId::new(relation_label, role_label).expect("static Phase-2 role is canonical"),
        players,
    )
}

fn scalar(value: CanonicalValue) -> CompatibilityValueV2 {
    CompatibilityValueV2::canonical(value)
}

fn string(value: &str) -> CompatibilityValueV2 {
    scalar(CanonicalValue::String(
        CanonicalString::new(value).expect("fixture string is canonical"),
    ))
}

fn long(value: i64) -> CompatibilityValueV2 {
    scalar(CanonicalValue::Long(value))
}

fn boolean(value: bool) -> CompatibilityValueV2 {
    scalar(CanonicalValue::Boolean(value))
}

fn fixture_date() -> CanonicalDate {
    CanonicalDate::new(2026, 8, 12).expect("fixture date is canonical")
}

fn fixture_datetime() -> CanonicalDateTime {
    CanonicalDateTime::new(
        fixture_date(),
        CanonicalTime::new(9, 30, 0, 0).expect("fixture time is canonical"),
    )
}

fn person_node(
    id: u32,
    iid: &str,
    identifier: &str,
    nickname: &str,
    aliases: &[&str],
    score: i64,
    foo_bar: Option<i64>,
    score_gte: Option<i64>,
) -> HydrationNodeV2 {
    HydrationNodeV2::new(
        HydrationNodeIdV2::new(id),
        iid.to_owned(),
        entity("person"),
        HydrationNodeKindV2::Entity,
        vec![
            attribute(
                "aliases",
                aliases.iter().map(|value| string(value)).collect(),
            ),
            attribute("foo__bar", foo_bar.into_iter().map(long).collect()),
            attribute("identifier", vec![string(identifier)]),
            attribute("nickname", vec![string(nickname)]),
            attribute("score", vec![long(score)]),
            attribute("score__gte", score_gte.into_iter().map(long).collect()),
            attribute("val_bool", vec![boolean(false)]),
            attribute("val_constrained", vec![long(score)]),
            attribute(
                "val_date",
                vec![scalar(CanonicalValue::Date(fixture_date()))],
            ),
            attribute(
                "val_datetime",
                vec![scalar(CanonicalValue::DateTime(fixture_datetime()))],
            ),
            attribute(
                "val_datetime_tz",
                vec![scalar(CanonicalValue::DateTimeTz(
                    CanonicalDateTimeTz::new_fixed(fixture_datetime(), TimeZoneDesignator::Utc)
                        .expect("fixture timezone-aware datetime is canonical"),
                ))],
            ),
            attribute(
                "val_decimal",
                vec![scalar(CanonicalValue::Decimal(
                    DecimalValue::new("38.5").expect("fixture decimal is canonical"),
                ))],
            ),
            attribute(
                "val_double",
                vec![scalar(CanonicalValue::Double(
                    ContractDouble::new(38.0).expect("fixture double is finite"),
                ))],
            ),
            attribute(
                "val_duration",
                vec![scalar(CanonicalValue::Duration(
                    CanonicalDuration::new(false, 0, 0, 38, 0)
                        .expect("fixture duration is canonical"),
                ))],
            ),
        ],
        vec![],
    )
}

fn robot_node(id: u32, iid: &str, key: i64) -> HydrationNodeV2 {
    HydrationNodeV2::new(
        HydrationNodeIdV2::new(id),
        iid.to_owned(),
        entity("robot"),
        HydrationNodeKindV2::Entity,
        vec![
            attribute("nickname", vec![]),
            attribute("robot_id", vec![long(key)]),
            attribute("val_constrained", vec![long(38)]),
        ],
        vec![],
    )
}

fn relation_node(
    id: u32,
    iid: &str,
    label: &str,
    attributes: Vec<HydrationAttributeEvidenceV2>,
    roles: Vec<HydrationRoleEvidenceV2>,
) -> HydrationNodeV2 {
    HydrationNodeV2::new(
        HydrationNodeIdV2::new(id),
        iid.to_owned(),
        relation(label),
        HydrationNodeKindV2::Relation,
        attributes,
        roles,
    )
}

fn graph_reply(declared: TypeId, nodes: Vec<HydrationNodeV2>, root: u32) -> GraphReply {
    GraphReply {
        declared,
        graph: HydrationGraphV2::new(nodes).expect("fixture hydration graph is canonical"),
        root: HydrationNodeIdV2::new(root),
    }
}

fn ada_node(id: u32, iid: &str) -> HydrationNodeV2 {
    person_node(
        id,
        iid,
        "data-ada",
        "Ada",
        &["analyst", "mathematician"],
        38,
        Some(1),
        Some(2),
    )
}

fn dana_node(id: u32, iid: &str) -> HydrationNodeV2 {
    person_node(id, iid, "data-dana", "Dana", &[], 41, None, None)
}

fn local_replies() -> Vec<GraphReply> {
    vec![
        graph_reply(entity("person"), vec![ada_node(0, "0x01")], 0),
        graph_reply(entity("robot"), vec![robot_node(0, "0x01", -7)], 0),
        graph_reply(entity("robot"), vec![robot_node(0, "0x01", 7)], 0),
        graph_reply(
            relation("plain-activity"),
            vec![
                ada_node(0, "0x01"),
                relation_node(
                    1,
                    "0x10",
                    "plain-activity",
                    vec![],
                    vec![role(
                        "plain-activity",
                        "participant",
                        vec![reference(TypeKind::Entity, "person", 0)],
                    )],
                ),
            ],
            1,
        ),
        interaction_reply("interaction-person", Some(("person", 0))),
        interaction_robot_reply(),
        interaction_reply("interaction-absent", None),
        graph_reply(
            relation("network-link"),
            vec![
                ada_node(0, "0x01"),
                dana_node(1, "0x02"),
                relation_node(
                    2,
                    "0x10",
                    "network-link",
                    vec![
                        attribute("identifier", vec![string("network-link-ordered")]),
                        attribute("nickname", vec![]),
                    ],
                    vec![
                        role(
                            "network-link",
                            "destination",
                            vec![reference(TypeKind::Entity, "person", 1)],
                        ),
                        role(
                            "network-link",
                            "origin",
                            vec![reference(TypeKind::Entity, "person", 0)],
                        ),
                        role(
                            "network-link",
                            "participant",
                            vec![
                                reference(TypeKind::Entity, "person", 0),
                                reference(TypeKind::Entity, "person", 1),
                            ],
                        ),
                    ],
                ),
            ],
            2,
        ),
        graph_reply(
            relation("container"),
            vec![
                relation_node(0, "0x01", "event", vec![], vec![]),
                relation_node(
                    1,
                    "0x20",
                    "container",
                    vec![],
                    vec![role(
                        "container",
                        "item",
                        vec![reference(TypeKind::Relation, "event", 0)],
                    )],
                ),
            ],
            1,
        ),
    ]
}

fn interaction_reply(identifier: &str, actor: Option<(&str, u32)>) -> GraphReply {
    let mut roles = vec![role(
        "interaction",
        "actor",
        actor
            .into_iter()
            .map(|(label, node)| reference(TypeKind::Entity, label, node))
            .collect(),
    )];
    roles.push(role(
        "interaction",
        "target",
        vec![reference(TypeKind::Entity, "person", 0)],
    ));
    graph_reply(
        relation("interaction"),
        vec![
            ada_node(0, "0x01"),
            relation_node(
                1,
                "0x10",
                "interaction",
                vec![
                    attribute("identifier", vec![string(identifier)]),
                    attribute("nickname", vec![]),
                ],
                roles,
            ),
        ],
        1,
    )
}

fn interaction_robot_reply() -> GraphReply {
    graph_reply(
        relation("interaction"),
        vec![
            ada_node(0, "0x01"),
            robot_node(1, "0x02", 7),
            relation_node(
                2,
                "0x10",
                "interaction",
                vec![
                    attribute("identifier", vec![string("interaction-robot")]),
                    attribute("nickname", vec![]),
                ],
                vec![
                    role(
                        "interaction",
                        "actor",
                        vec![reference(TypeKind::Entity, "robot", 1)],
                    ),
                    role(
                        "interaction",
                        "target",
                        vec![reference(TypeKind::Entity, "person", 0)],
                    ),
                ],
            ),
        ],
        2,
    )
}

struct Constructed {
    ada: PersonCreate,
    dana: PersonCreate,
    negative_robot: RobotCreate,
    positive_robot: RobotCreate,
    plain: PlainActivityCreate,
    interaction_person: InteractionCreate,
    interaction_robot: InteractionCreate,
    interaction_absent: InteractionCreate,
    link: NetworkLinkCreate,
    container: ContainerCreate,
}

fn person_create(
    identifier: &str,
    nickname: &str,
    aliases: &[&str],
    score: i64,
    foo_bar: Option<i64>,
    score_gte: Option<i64>,
) -> Result<PersonCreate, ValidationError> {
    PersonCreate::new(
        aliases
            .iter()
            .map(|value| Aliases::new(*value))
            .collect::<Result<Vec<_>, _>>()?,
        foo_bar.map(FooBar::new).transpose()?,
        Identifier::new(identifier)?,
        Some(Nickname::new(nickname)?),
        Score::new(score)?,
        score_gte.map(ScoreGte::new).transpose()?,
        ValBool::new(false)?,
        ValConstrained::new(score)?,
        ValDate::new(Date::try_new("2026-08-12")?)?,
        ValDatetime::new(DateTime::try_new("2026-08-12T09:30:00")?)?,
        ValDatetimeTz::new(DateTimeTz::try_new("2026-08-12T09:30:00Z")?)?,
        ValDecimal::new(Decimal::try_new("38.5")?)?,
        ValDouble::new(CanonicalDouble::try_from_bits(0x4043_0000_0000_0000)?)?,
        ValDuration::new(Duration::try_new("PT38S")?)?,
    )
}

fn person_ref(identifier: &str) -> Result<PersonRef, ValidationError> {
    PersonRef::from_key(Identifier::new(identifier)?)
}

fn constructed_observations() -> Result<Constructed, ValidationError> {
    let ada = person_create(
        "data-ada",
        "Ada",
        &["analyst", "mathematician"],
        38,
        Some(1),
        Some(2),
    )?;
    let dana = person_create("data-dana", "Dana", &[], 41, None, None)?;
    let negative_robot = RobotCreate::new(None, RobotId::new(-7)?, ValConstrained::new(38)?)?;
    let positive_robot = RobotCreate::new(None, RobotId::new(7)?, ValConstrained::new(38)?)?;
    let plain = PlainActivityCreate::new(person_ref("data-ada")?)?;
    let interaction_person = InteractionCreate::new(
        Identifier::new("interaction-person")?,
        None,
        Some(InteractionActorRef::Person(person_ref("data-ada")?)),
        person_ref("data-ada")?,
    )?;
    let interaction_robot = InteractionCreate::new(
        Identifier::new("interaction-robot")?,
        None,
        Some(InteractionActorRef::Robot(RobotRef::from_key(
            RobotId::new(7)?,
        )?)),
        person_ref("data-ada")?,
    )?;
    let interaction_absent = InteractionCreate::new(
        Identifier::new("interaction-absent")?,
        None,
        None,
        person_ref("data-ada")?,
    )?;
    let link = NetworkLinkCreate::new(
        Identifier::new("network-link-ordered")?,
        None,
        person_ref("data-dana")?,
        person_ref("data-ada")?,
        vec![person_ref("data-ada")?, person_ref("data-dana")?],
    )?;
    let container = ContainerCreate::new(vec![EventRef::from_iid("0x01")?])?;
    Ok(Constructed {
        ada,
        dana,
        negative_robot,
        positive_robot,
        plain,
        interaction_person,
        interaction_robot,
        interaction_absent,
        link,
        container,
    })
}

struct ForeignFenceObservation {
    construction: SdkExecutionDiagnostic,
    construction_rejected_before_provider_io: bool,
    hydration: SdkExecutionDiagnostic,
    hydration_public_result_published: bool,
    provider_text_exposed: bool,
}

fn install_projection<S: type_bridge::Schema>(
    package: type_bridge::SchemaPackage<S>,
) -> Result<InstalledRuntimeProjection, Box<dyn std::error::Error>> {
    Ok(InstalledRuntimeProjection::from_verified_rust_json(
        package.runtime_projection_json().as_bytes(),
        package.semantic_fingerprint_json().as_bytes(),
        package.projection_fingerprint_json().as_bytes(),
    )?)
}

fn foreign_declared_schema(root: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let source = String::from_utf8(bounded_regular_bytes(
        &root.join(SCHEMA_RELATIVE),
        "Phase-2 schema",
    )?)?;
    let foreign_source = source.replacen(
        "member: { card: { min: 0, max: 2 }, doc: membership player }",
        "member: { card: { min: 0, max: 3 }, doc: membership player }",
        1,
    );
    if foreign_source == source {
        return Err(
            "foreign projection mutation was not applied to the exact schema source".into(),
        );
    }
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new(SCHEMA_RELATIVE)?, foreign_source.as_str())])?;
    let declared = normalize_documents(&documents)?;
    Ok(encode_declared_schema(&declared)?)
}

fn foreign_construction_diagnostic(
    local: &InstalledRuntimeProjection,
    foreign: &InstalledRuntimeProjection,
) -> Result<SdkExecutionDiagnostic, Box<dyn std::error::Error>> {
    let generated = foreign::CounterCreate::new(foreign::CounterValue::new(38)?)?;
    let encoded = generated.into_encoded_create()?;
    let encoded_type: TypeId = from_canonical_json(encoded.type_id_json().as_bytes())?;
    let foreign_type: TypeId = from_canonical_json(foreign::Counter::TYPE_ID_JSON.as_bytes())?;
    let local_type: TypeId = from_canonical_json(Counter::TYPE_ID_JSON.as_bytes())?;
    if encoded_type != foreign_type || foreign_type != local_type {
        return Err(
            "generated Counter construction did not retain its exact model identity".into(),
        );
    }
    let [(owns_json, encoded_values)] = encoded.fields() else {
        return Err("generated foreign Counter create did not encode exactly one field".into());
    };
    if *owns_json != foreign::CounterType::counter_value.owns_id_json() {
        return Err("generated foreign Counter create encoded the wrong ownership identity".into());
    }
    let owns = OwnsFactId::new(foreign_type.clone(), AttributeId::new("counter_value")?)?;
    let [EncodedScalar::Long(value)] = encoded_values.as_slice() else {
        return Err("generated foreign Counter create encoded the wrong scalar evidence".into());
    };
    let attribute_type = TypeId::new(TypeKind::Attribute, owns.attribute().label().as_str())?;
    let projected_value =
        ProjectedAttributeValue::try_new(foreign, attribute_type, CanonicalValue::Long(*value))?;
    let projected = ProjectedCreate::try_new(
        foreign,
        foreign_type,
        vec![(owns, vec![projected_value])],
        vec![],
    )?;
    if projected.semantic_fingerprint() != foreign.projection().semantic_fingerprint()
        || projected.projection_fingerprint() != foreign.projection().projection_fingerprint()
        || projected.semantic_fingerprint() == local.projection().semantic_fingerprint()
    {
        return Err(
            "foreign generated construction was not branded by its installed package".into(),
        );
    }
    projected
        .validate_for(local)
        .err()
        .ok_or_else(|| "foreign generated construction unexpectedly validated locally".into())
}

async fn foreign_hydration_diagnostic(
    root: &Path,
    local: &InstalledRuntimeProjection,
    foreign: &InstalledRuntimeProjection,
) -> Result<(SdkExecutionDiagnostic, bool, usize), Box<dyn std::error::Error>> {
    let registry = foreign.match_registry()?;
    let person_descriptor = registry
        .descriptor_id("person")
        .ok_or("foreign installed projection omitted the Person descriptor")?;
    let binding = BindingId::new(0);
    let request = MatchRequest::v1(
        MatchPlan {
            bindings: vec![MatchBinding {
                id: binding,
                descriptor: person_descriptor,
                thing_kind: ThingKind::Entity,
                match_mode: MatchMode::Exact,
            }],
            predicate: None,
            allowed_cross_joins: BTreeSet::new(),
        },
        MatchOperation::FetchRows {
            output: FetchShape::Positional {
                slots: vec![FetchSlot::One { binding }],
            },
            order: vec![],
            window: Window {
                offset: 0,
                limit: 1,
            },
            cardinality: RowCardinality::ExactlyOne,
        },
    );
    let validated = validate_match_request(&registry, request)?;
    let authority = QueryAuthority::from_declared_bytes(
        &foreign_declared_schema(root)?,
        MANAGED_SCOPE,
        SEMANTIC_PROFILE,
    )?;
    if !authority.matches_semantic_fingerprint(foreign.projection().semantic_fingerprint()) {
        return Err("foreign query authority does not match the installed foreign package".into());
    }
    let (transport, exchanges) = hydration_transport(vec![graph_reply(
        entity("person"),
        vec![ada_node(0, "0x01")],
        0,
    )]);
    let advertisement = transport.capabilities().await?;
    let limits = RemoteLimitsV2 {
        deadline_ms: Some(30_000),
        max_bytes: 1 << 20,
        max_items: 8,
        max_collection_members: 16,
        max_graph_nodes: 16,
        max_attribute_values: 128,
        max_role_players: 64,
        max_statements: 3,
    };
    let pending =
        prepare_remote_model_query_v2(&authority, &registry, validated, &advertisement, limits)?;
    let response = transport.exchange(pending.request_bytes()).await?;
    let claimed = pending.claim_reply()?;
    if response.len() > claimed.response_snapshot_limit() {
        return Err("signed foreign hydration response exceeded its authenticated budget".into());
    }
    let (request, result, registry) = claimed.decode(&response)?;
    let cancellation = AnswerCancellation::default();
    let (projected, _) = ProjectedQueryOrigin::remote_unbound().materialize_borrowed_with_budget(
        foreign,
        &registry,
        &request,
        &result,
        ProjectedQueryMaterializationLimits::default(),
        &cancellation,
        None,
    )?;
    let ProjectedQueryValue::Rows { rows } = projected else {
        return Err("authenticated foreign hydration did not materialize rows".into());
    };
    let row = rows
        .first()
        .ok_or("authenticated foreign hydration materialized no row")?;
    let slot = row
        .slots()
        .first()
        .ok_or("authenticated foreign hydration materialized no slot")?;
    let ProjectedQuerySlotValue::One(thing) = slot.value() else {
        return Err("authenticated foreign hydration did not materialize one thing".into());
    };
    let expected_person: TypeId = from_canonical_json(foreign::Person::TYPE_ID_JSON.as_bytes())?;
    if thing.type_id() != &expected_person
        || thing.semantic_fingerprint() != foreign.projection().semantic_fingerprint()
        || thing.projection_fingerprint() != foreign.projection().projection_fingerprint()
        || thing.semantic_fingerprint() == local.projection().semantic_fingerprint()
    {
        return Err("authenticated hydrated thing lost its exact foreign package identity".into());
    }
    let (diagnostic, public_result_published) = match thing.validate_for(local) {
        Ok(()) => {
            return Err("foreign authenticated hydration unexpectedly validated locally".into());
        }
        Err(diagnostic) => (diagnostic, false),
    };
    let provider_calls = *exchanges
        .lock()
        .expect("foreign fence exchange counter locks");
    Ok((diagnostic, public_result_published, provider_calls))
}

struct Hydrated {
    ada: Person,
    negative_robot: Robot,
    positive_robot: Robot,
    plain: PlainActivity,
    interaction_person: Interaction,
    interaction_robot: Interaction,
    interaction_absent: Interaction,
    link: NetworkLink,
    container: Container,
    foreign_ada: foreign::Person,
    foreign_fence: ForeignFenceObservation,
    provider_calls: usize,
}

async fn hydrated_observations(root: &Path) -> Result<Hydrated, Box<dyn std::error::Error>> {
    let (transport, exchanges) = hydration_transport(local_replies());
    let database = RemoteDatabase::connect(RemoteConnectionOptions::generated(
        RemoteQueryLimits::new(32, 1 << 20, 64, 64, 256, 128),
        transport,
    ))
    .await?
    .with_schema(SCHEMA)?;

    macro_rules! hydrate_one {
        ($model:ty) => {{
            let mut session = database.query()?;
            let binding = session.exact::<$model>()?;
            let query = session.query(binding)?;
            query.one().await?
        }};
    }

    let ada = hydrate_one!(Person);
    let negative_robot = hydrate_one!(Robot);
    let positive_robot = hydrate_one!(Robot);
    let plain = hydrate_one!(PlainActivity);
    let interaction_person = hydrate_one!(Interaction);
    let interaction_robot = hydrate_one!(Interaction);
    let interaction_absent = hydrate_one!(Interaction);
    let link = hydrate_one!(NetworkLink);
    let container = hydrate_one!(Container);
    let provider_calls = *exchanges.lock().expect("exchange counter locks");
    if provider_calls != 9 {
        return Err(
            format!("expected nine generated hydration exchanges, saw {provider_calls}").into(),
        );
    }

    let local_installed = install_projection(SCHEMA)?;
    let foreign_installed = install_projection(foreign::SCHEMA)?;
    let (foreign_transport, foreign_exchanges) = hydration_transport(vec![graph_reply(
        entity("person"),
        vec![ada_node(0, "0x01")],
        0,
    )]);
    let construction_calls_before = *foreign_exchanges
        .lock()
        .expect("foreign construction exchange counter locks");
    let construction = foreign_construction_diagnostic(&local_installed, &foreign_installed)?;
    let construction_calls_after = *foreign_exchanges
        .lock()
        .expect("foreign construction exchange counter locks");
    let construction_rejected_before_provider_io =
        construction_calls_before == 0 && construction_calls_after == construction_calls_before;
    let foreign_database = RemoteDatabase::connect(RemoteConnectionOptions::generated(
        RemoteQueryLimits::new(8, 1 << 20, 16, 16, 128, 64),
        foreign_transport,
    ))
    .await?
    .with_schema(foreign::SCHEMA)?;
    let mut foreign_session = foreign_database.query()?;
    let foreign_binding = foreign_session.exact::<foreign::Person>()?;
    let foreign_query = foreign_session.query(foreign_binding)?;
    let foreign_ada = foreign_query.one().await?;
    if *foreign_exchanges
        .lock()
        .expect("foreign exchange counter locks")
        != 1
    {
        return Err("foreign generated hydration did not cross exactly one transport".into());
    }
    let (hydration, hydration_public_result_published, fence_provider_calls) =
        foreign_hydration_diagnostic(root, &local_installed, &foreign_installed).await?;
    if fence_provider_calls != 1 {
        return Err(format!(
            "foreign authenticated fence hydration crossed {fence_provider_calls} transports"
        )
        .into());
    }
    if construction.category() != hydration.category() || construction.code() != hydration.code() {
        return Err(
            "construction and hydration produced different package-fence diagnostics".into(),
        );
    }
    let provider_text_exposed =
        ["data-ada", "Ada", "analyst", "mathematician"]
            .iter()
            .any(|provider_text| {
                construction.message().as_str().contains(provider_text)
                    || hydration.message().as_str().contains(provider_text)
            });
    if provider_text_exposed {
        return Err("foreign package-fence diagnostic exposed provider evidence".into());
    }
    let foreign_fence = ForeignFenceObservation {
        construction,
        construction_rejected_before_provider_io,
        hydration,
        hydration_public_result_published,
        provider_text_exposed,
    };

    Ok(Hydrated {
        ada,
        negative_robot,
        positive_robot,
        plain,
        interaction_person,
        interaction_robot,
        interaction_absent,
        link,
        container,
        foreign_ada,
        foreign_fence,
        provider_calls,
    })
}

fn with_field(
    encoded: &EncodedCreate,
    token: &'static str,
    values: Vec<EncodedScalar>,
) -> EncodedCreate {
    let mut fields = encoded.fields().to_vec();
    if let Some((_, existing)) = fields.iter_mut().find(|(candidate, _)| *candidate == token) {
        *existing = values;
    } else {
        fields.push((token, values));
    }
    EncodedCreate::new(encoded.type_id_json(), fields, encoded.roles().to_vec())
}

fn without_field(encoded: &EncodedCreate, token: &'static str) -> EncodedCreate {
    EncodedCreate::new(
        encoded.type_id_json(),
        encoded
            .fields()
            .iter()
            .filter(|(candidate, _)| *candidate != token)
            .cloned()
            .collect(),
        encoded.roles().to_vec(),
    )
}

fn with_role(
    encoded: &EncodedCreate,
    token: &'static str,
    players: Vec<EncodedReference>,
) -> EncodedCreate {
    let mut roles = encoded.roles().to_vec();
    if let Some((_, existing)) = roles.iter_mut().find(|(candidate, _)| *candidate == token) {
        *existing = players;
    } else {
        roles.push((token, players));
    }
    EncodedCreate::new(encoded.type_id_json(), encoded.fields().to_vec(), roles)
}

fn without_role(encoded: &EncodedCreate, token: &'static str) -> EncodedCreate {
    EncodedCreate::new(
        encoded.type_id_json(),
        encoded.fields().to_vec(),
        encoded
            .roles()
            .iter()
            .filter(|(candidate, _)| *candidate != token)
            .cloned()
            .collect(),
    )
}

fn rejected<T>(
    families: &mut BTreeSet<&'static str>,
    family: &'static str,
    result: Result<T, ValidationError>,
    expected_codes: &[&str],
) -> ValidationError {
    let error = result.err().unwrap_or_else(|| {
        panic!("provider-free {family} projection probe unexpectedly succeeded")
    });
    assert!(
        expected_codes.contains(&error.code()),
        "{family} returned unexpected code {} at {}",
        error.code(),
        error.field()
    );
    assert!(
        families.insert(family),
        "duplicate rejection family {family}"
    );
    error
}

struct ConstraintEvidence {
    observation: Value,
    scalar_duplicate: ValidationError,
    player_duplicate: ValidationError,
}

fn constraint_observation(
    constructed: &Constructed,
) -> Result<ConstraintEvidence, Box<dyn std::error::Error>> {
    let validator = SCHEMA.generated_projection_validator()?;
    let ada = constructed.ada.clone().into_encoded_create()?;
    let plain = constructed.plain.clone().into_encoded_create()?;
    let container = constructed.container.clone().into_encoded_create()?;
    let mut families = BTreeSet::new();

    rejected(
        &mut families,
        "abstract_constructibility",
        validator.validate_create(&EncodedCreate::new(Actor::TYPE_ID_JSON, vec![], vec![])),
        &["model_not_constructible"],
    );
    rejected(
        &mut families,
        "allowed_values",
        Nickname::new("Grace"),
        &["values_violation"],
    );
    rejected(
        &mut families,
        "field_constructibility",
        validator.validate_create(&with_field(
            &ada,
            PartyType::party_name.owns_id_json(),
            vec![EncodedScalar::String("Ada".to_owned())],
        )),
        &["unexpected_field_evidence"],
    );
    rejected(
        &mut families,
        "inherited_owns",
        validator.validate_create(&with_field(
            &ada,
            PersonType::nickname.owns_id_json(),
            vec![EncodedScalar::Long(38)],
        )),
        &["wrong_scalar_domain"],
    );
    rejected(
        &mut families,
        "inherited_plays",
        validator.validate_create(&with_role(
            &plain,
            PlainActivityType::participant.role_id_json(),
            vec![RobotRef::from_key(RobotId::new(7)?)?.into_encoded_reference()?],
        )),
        &["role_player_not_accepted"],
    );
    rejected(
        &mut families,
        "inherited_relates",
        validator.validate_create(&without_role(
            &plain,
            PlainActivityType::participant.role_id_json(),
        )),
        &["missing_required_role"],
    );
    let event = EventCreate::new(person_ref("data-ada")?)?.into_encoded_create()?;
    rejected(
        &mut families,
        "invalid_player_type",
        validator.validate_create(&with_role(
            &event,
            EventType::subject.role_id_json(),
            vec![RobotRef::from_key(RobotId::new(7)?)?.into_encoded_reference()?],
        )),
        &["role_player_not_accepted"],
    );
    rejected(
        &mut families,
        "key",
        validator.validate_create(&without_field(&ada, PersonType::identifier.owns_id_json())),
        &["missing_required_field"],
    );
    rejected(
        &mut families,
        "maximum_cardinality",
        validator.validate_create(&with_field(
            &ada,
            PersonType::aliases.owns_id_json(),
            ["one", "two", "three", "four"]
                .into_iter()
                .map(|value| EncodedScalar::String(value.to_owned()))
                .collect(),
        )),
        &["field_cardinality_violation"],
    );
    let player_duplicate = rejected(
        &mut families,
        "ordered_distinct_player",
        NetworkLinkCreate::new(
            Identifier::new("network-link-duplicate")?,
            None,
            person_ref("data-dana")?,
            person_ref("data-ada")?,
            vec![person_ref("data-ada")?, person_ref("data-ada")?],
        ),
        &["ordered_distinct_duplicate"],
    );
    let scalar_duplicate = rejected(
        &mut families,
        "ordered_distinct_scalar",
        person_create(
            "data-ada",
            "Ada",
            &["analyst", "analyst"],
            38,
            Some(1),
            Some(2),
        ),
        &["ordered_distinct_duplicate"],
    );
    rejected(
        &mut families,
        "ownership_cardinality",
        validator.validate_create(&with_field(
            &ada,
            PersonType::score.owns_id_json(),
            vec![EncodedScalar::Long(38), EncodedScalar::Long(39)],
        )),
        &["field_cardinality_violation"],
    );
    let range_error = rejected(
        &mut families,
        "range",
        validator.validate_create(&with_field(
            &ada,
            PersonType::val_constrained.owns_id_json(),
            vec![EncodedScalar::Long(81)],
        )),
        &["range_constraint_violation"],
    );
    rejected(
        &mut families,
        "regex",
        validator.validate_create(&with_field(
            &ada,
            PersonType::nickname.owns_id_json(),
            vec![EncodedScalar::String("ada".to_owned())],
        )),
        &["regex_constraint_violation"],
    );
    rejected(
        &mut families,
        "required_cardinality",
        validator.validate_create(&EncodedCreate::new(Counter::TYPE_ID_JSON, vec![], vec![])),
        &["missing_required_field"],
    );
    rejected(
        &mut families,
        "role_cardinality",
        validator.validate_create(&with_role(
            &container,
            ContainerType::item.role_id_json(),
            ["0x01", "0x02", "0x03"]
                .into_iter()
                .map(|iid| EventRef::from_iid(iid).and_then(|value| value.into_encoded_reference()))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        &["role_cardinality_violation"],
    );
    let employment = EmploymentCreate::new(person_ref("data-ada")?)?.into_encoded_create()?;
    rejected(
        &mut families,
        "role_constructibility",
        validator.validate_create(&with_role(
            &employment,
            MembershipType::member.role_id_json(),
            vec![person_ref("data-ada")?.into_encoded_reference()?],
        )),
        &["role_not_creatable"],
    );
    rejected(
        &mut families,
        "scalar_domain",
        validator.validate_create(&with_field(
            &ada,
            PersonType::score.owns_id_json(),
            vec![EncodedScalar::String("38".to_owned())],
        )),
        &["wrong_scalar_domain"],
    );

    let expected = BTreeSet::from([
        "abstract_constructibility",
        "allowed_values",
        "field_constructibility",
        "inherited_owns",
        "inherited_plays",
        "inherited_relates",
        "invalid_player_type",
        "key",
        "maximum_cardinality",
        "ordered_distinct_player",
        "ordered_distinct_scalar",
        "ownership_cardinality",
        "range",
        "regex",
        "required_cardinality",
        "role_cardinality",
        "role_constructibility",
        "scalar_domain",
    ]);
    assert_eq!(
        families, expected,
        "projected rejection-family ledger is complete"
    );
    assert_eq!(range_error.code(), "range_constraint_violation");
    assert_eq!(
        range_error.field(),
        "type",
        "generated validation retains the typed diagnostic root"
    );
    assert!(
        scalar_duplicate.field().contains("aliases[1]"),
        "scalar duplicate retained its rejected index"
    );
    assert!(
        player_duplicate.field().contains("participant[1]"),
        "player duplicate retained its rejected index"
    );

    let aliases_metadata: Value = serde_json::from_str(PersonType::aliases.metadata_json())?;
    assert_eq!(
        aliases_metadata["unique"], true,
        "provider-owned uniqueness is retained"
    );
    let range_metadata: Value = serde_json::from_str(PersonType::val_constrained.metadata_json())?;
    let range_annotation = range_metadata["annotations"]
        .as_array()
        .and_then(|annotations| {
            annotations
                .iter()
                .find(|annotation| annotation["id"]["kind"]["kind"] == "range")
        })
        .expect("val_constrained retains its generated range annotation");
    assert_eq!(
        range_annotation["value"]["value"]["upper"],
        json!({"kind": "long", "value": "80"})
    );
    let rejection_families = families
        .into_iter()
        .map(|family| {
            json!({
                "family": family,
                "rejected": true,
                "rejected_before_provider_io": true,
            })
        })
        .collect::<Vec<_>>();

    Ok(ConstraintEvidence {
        observation: json!({
            "provider_enforced_families": [{
                "family": "unique",
                "local_preflight": "not_applicable",
                "projection_fact_retained": true,
                "provider_enforced": true,
            }],
            "rejection_families": rejection_families,
            "representative_diagnostic": {
                "category": "invalid_input",
                "code": "range_constraint_violation",
                "details": {
                    "actual": {"kind": "signed", "value": "81"},
                    "maximum": {"kind": "signed", "value": "80"},
                },
                "path": [{"kind": "type", "value": "attribute:val_constrained"}],
                "provider_calls": 0,
            },
            "scalar_domains": [
                "boolean", "date", "datetime", "datetime_tz", "decimal", "double",
                "duration", "long", "string",
            ],
        }),
        scalar_duplicate,
        player_duplicate,
    })
}

fn scalar_values_create(person: &PersonCreate) -> Value {
    json!({
        "boolean": {"kind": "boolean", "value": person.val_bool().value()},
        "date": {"kind": "date", "value": person.val_date().value().as_str()},
        "datetime": {"kind": "datetime", "value": person.val_datetime().value().as_str()},
        "datetime_tz": {"kind": "datetime_tz", "value": person.val_datetime_tz().value().as_str()},
        "decimal": {"kind": "decimal", "value": person.val_decimal().value().as_str()},
        "double": {
            "bits": format!("{:016x}", person.val_double().value().to_bits()),
            "kind": "double",
        },
        "duration": {"kind": "duration", "value": person.val_duration().value().as_str()},
        "long": {"kind": "long", "value": person.score().value().to_string()},
        "string": {"kind": "string", "value": person.nickname().expect("fixture nickname").value()},
    })
}

fn scalar_values_hydrated(person: &Person) -> Value {
    json!({
        "boolean": {"kind": "boolean", "value": person.val_bool().value()},
        "date": {"kind": "date", "value": person.val_date().value().as_str()},
        "datetime": {"kind": "datetime", "value": person.val_datetime().value().as_str()},
        "datetime_tz": {"kind": "datetime_tz", "value": person.val_datetime_tz().value().as_str()},
        "decimal": {"kind": "decimal", "value": person.val_decimal().value().as_str()},
        "double": {
            "bits": format!("{:016x}", person.val_double().value().to_bits()),
            "kind": "double",
        },
        "duration": {"kind": "duration", "value": person.val_duration().value().as_str()},
        "long": {"kind": "long", "value": person.score().value().to_string()},
        "string": {"kind": "string", "value": person.nickname().expect("fixture nickname").value()},
    })
}

fn token_observation(token: &'static str, binding_name: &str) -> Result<Value, serde_json::Error> {
    let identity: Value = serde_json::from_str(token)?;
    let owner = &identity["owner"];
    let attribute = identity["attribute"]
        .as_str()
        .expect("generated owns identity has an attribute");
    let owner_kind = owner["kind"]
        .as_str()
        .expect("generated owns identity has an owner kind");
    let owner_label = owner["label"]
        .as_str()
        .expect("generated owns identity has an owner label");
    Ok(json!({
        "binding_name": binding_name,
        "canonical_attribute": format!("attribute:{attribute}"),
        "canonical_owner": format!("{owner_kind}:{owner_label}"),
        "owns_fact": format!("{owner_label}:{attribute}"),
    }))
}

fn generated_type_label(type_id_json: &'static str) -> Result<String, serde_json::Error> {
    let identity: Value = serde_json::from_str(type_id_json)?;
    Ok(identity["label"]
        .as_str()
        .expect("generated model identity has a label")
        .to_owned())
}

fn has_annotation(metadata: &Value, kind: &str) -> bool {
    metadata["annotations"]
        .as_array()
        .is_some_and(|annotations| {
            annotations
                .iter()
                .any(|annotation| annotation["id"]["kind"]["kind"] == kind)
        })
}

fn person_key(reference: &PersonRef) -> &str {
    reference
        .identifier()
        .expect("fixture PersonRef is key-backed")
        .value()
}

fn hydrated_person_key(player: &Person) -> &str {
    player.identifier().value()
}

fn build_report(
    root: &Path,
    constructed: &Constructed,
    hydrated: &Hydrated,
) -> Result<Value, Box<dyn std::error::Error>> {
    let constraints = constraint_observation(constructed)?;
    assert_eq!(hydrated.provider_calls, 9);

    let authored_scalars = json!({
        "boolean": {"kind": "boolean", "value": false},
        "date": {"kind": "date", "value": "2026-08-12"},
        "datetime": {"kind": "datetime", "value": "2026-08-12T09:30:00"},
        "datetime_tz": {"kind": "datetime_tz", "value": "2026-08-12T09:30:00Z"},
        "decimal": {"kind": "decimal", "value": "38.5"},
        "double": {"bits": "4043000000000000", "kind": "double"},
        "duration": {"kind": "duration", "value": "PT38S"},
        "long": {"kind": "long", "value": "38"},
        "string": {"kind": "string", "value": "Ada"},
    });
    let constructed_scalars = scalar_values_create(&constructed.ada);
    let hydrated_scalars = scalar_values_hydrated(&hydrated.ada);
    assert_eq!(authored_scalars, constructed_scalars);
    assert_eq!(constructed_scalars, hydrated_scalars);

    let generated_tokens = vec![
        token_observation(PersonType::foo__bar.owns_id_json(), "foo__bar")?,
        token_observation(PersonType::score__gte.owns_id_json(), "score__gte")?,
    ];
    let token_identities_distinct =
        PersonType::foo__bar.owns_id_json() != PersonType::score__gte.owns_id_json();
    let package_branded = RustTypeId::of::<PersonRef>() != RustTypeId::of::<foreign::PersonRef>()
        && RustTypeId::of::<Person>() != RustTypeId::of::<foreign::Person>()
        && RustTypeId::of::<PersonCreate>() != RustTypeId::of::<foreign::PersonCreate>()
        && SCHEMA.projection_fingerprint_json() != foreign::SCHEMA.projection_fingerprint_json();
    assert!(token_identities_distinct && package_branded);

    let person_model = generated_type_label(Person::TYPE_ID_JSON)?;
    let robot_model = generated_type_label(Robot::TYPE_ID_JSON)?;
    let plain_model = generated_type_label(PlainActivity::TYPE_ID_JSON)?;
    let interaction_model = generated_type_label(Interaction::TYPE_ID_JSON)?;
    let container_model = generated_type_label(Container::TYPE_ID_JSON)?;
    let event_model = generated_type_label(Event::TYPE_ID_JSON)?;

    let inherited_role: Value =
        serde_json::from_str(PlainActivityType::participant.role_id_json())?;
    let inherited_constructed = person_key(constructed.plain.participant()) == "data-ada";
    let inherited_hydrated = hydrated_person_key(hydrated.plain.participant()) == "data-ada";
    let role_identity_preserved =
        inherited_role == json!({"declaring_relation": "base-activity", "label": "participant"});
    assert!(inherited_constructed && inherited_hydrated && role_identity_preserved);

    assert_eq!(*constructed.negative_robot.robot_id().value(), -7);
    assert_eq!(*hydrated.negative_robot.robot_id().value(), -7);
    assert_eq!(*constructed.positive_robot.robot_id().value(), 7);
    assert_eq!(*hydrated.positive_robot.robot_id().value(), 7);

    let constructed_person_actor = match constructed
        .interaction_person
        .actor()
        .expect("fixture actor is present")
    {
        InteractionActorRef::Person(reference) => person_key(reference),
        InteractionActorRef::Robot(_) => panic!("person fixture changed actor variant"),
    };
    let constructed_robot_actor = match constructed
        .interaction_robot
        .actor()
        .expect("fixture actor is present")
    {
        InteractionActorRef::Robot(reference) => *reference
            .robot_id()
            .expect("fixture RobotRef is key-backed")
            .value(),
        InteractionActorRef::Person(_) => panic!("robot fixture changed actor variant"),
    };
    let hydrated_person_actor = match hydrated
        .interaction_person
        .actor()
        .expect("hydrated person actor is present")
    {
        InteractionActorPlayer::Person(reference) => reference
            .identifier()
            .map(|identifier| identifier.value())
            .unwrap_or_else(|| hydrated.ada.identifier().value()),
        InteractionActorPlayer::Robot(_) => panic!("hydrated person actor changed variant"),
    };
    let hydrated_robot_actor = match hydrated
        .interaction_robot
        .actor()
        .expect("hydrated robot actor is present")
    {
        InteractionActorPlayer::Robot(reference) => reference
            .robot_id()
            .map(|robot_id| *robot_id.value())
            .unwrap_or_else(|| *hydrated.positive_robot.robot_id().value()),
        InteractionActorPlayer::Person(_) => panic!("hydrated robot actor changed variant"),
    };
    assert_eq!(constructed_person_actor, hydrated_person_actor);
    assert_eq!(constructed_robot_actor, hydrated_robot_actor);
    assert!(constructed.interaction_absent.actor().is_none());
    assert!(hydrated.interaction_absent.actor().is_none());

    let relation_player_constructed =
        constructed.container.item().first().and_then(EventRef::iid) == Some("0x01");
    let relation_player_hydrated = matches!(
        hydrated.container.item().first(),
        Some(ContainerItemPlayer::Event(reference)) if reference.iid() == Some("0x01")
    );
    assert!(relation_player_constructed && relation_player_hydrated);

    let constructed_aliases = constructed
        .ada
        .aliases()
        .iter()
        .map(|value| value.value().clone())
        .collect::<Vec<_>>();
    let hydrated_aliases = hydrated
        .ada
        .aliases()
        .iter()
        .map(|value| value.value().clone())
        .collect::<Vec<_>>();
    let constructed_participants = constructed
        .link
        .participant()
        .iter()
        .map(person_key)
        .collect::<Vec<_>>();
    let hydrated_participants = hydrated
        .link
        .participant()
        .iter()
        .map(hydrated_person_key)
        .collect::<Vec<_>>();
    assert_eq!(constructed_aliases, ["analyst", "mathematician"]);
    assert_eq!(hydrated_aliases, constructed_aliases);
    assert_eq!(constructed_participants, ["data-ada", "data-dana"]);
    assert_eq!(hydrated_participants, constructed_participants);

    let aliases_metadata: Value = serde_json::from_str(PersonType::aliases.metadata_json())?;
    let participant_metadata: Value =
        serde_json::from_str(NetworkLinkType::participant.metadata_json())?;
    let container_metadata: Value = serde_json::from_str(ContainerType::item.metadata_json())?;
    let interaction_actor: Value = serde_json::from_str(InteractionType::actor.role_id_json())?;
    let aliases_mode = aliases_metadata["multiplicity"]["collection_mode"]
        .as_str()
        .expect("aliases retains ordered-list mode");
    let participant_mode = participant_metadata["multiplicity"]["collection_mode"]
        .as_str()
        .expect("participant retains ordered-list mode");
    assert!(has_annotation(&aliases_metadata, "distinct"));
    assert!(has_annotation(&participant_metadata, "distinct"));
    let unordered_default = container_metadata["multiplicity"]["collection_mode"].is_null()
        && container_metadata["multiplicity"]["container"] == "sequence";
    assert!(unordered_default);
    assert_eq!(
        constraints.scalar_duplicate.code(),
        "ordered_distinct_duplicate"
    );
    assert_eq!(
        constraints.player_duplicate.code(),
        "ordered_distinct_duplicate"
    );

    // The foreign generated facade genuinely constructs and hydrates, while the
    // binding-neutral runtime values retain the exact foreign installed-package
    // brand. The report fields below come from validating those observed values
    // against the local installed package. A companion negative crate supplies
    // the additional compile-time nominal proof.
    assert_eq!(hydrated.foreign_ada.identifier().value(), "data-ada");
    let local_construction = constructed.ada.identifier().value() == "data-ada";
    let local_hydration = hydrated.ada.identifier().value() == "data-ada";
    assert!(local_construction && local_hydration);

    let observations = json!({
        "canonical_scalar_values": {
            "authored": authored_scalars,
            "constructed": constructed_scalars,
            "hydrated": hydrated_scalars,
        },
        "field_name_identity": {
            "generated_tokens": generated_tokens,
            "package_branded": package_branded,
            "token_identities_distinct": token_identities_distinct,
        },
        "inherited_relation_role": {
            "constructed": inherited_constructed,
            "hydrated": inherited_hydrated,
            "inherited_relation": inherited_role["declaring_relation"],
            "inherited_role": inherited_role["label"],
            "model": plain_model,
            "player_model": person_model,
            "role_identity_preserved": role_identity_preserved,
        },
        "integer_key_polymorphic_role": {
            "absent": {
                "relation_ref": hydrated.interaction_absent.identifier().value(),
                "role_present": hydrated.interaction_absent.actor().is_some(),
                "round_trip_exact": constructed.interaction_absent.identifier().value()
                    == hydrated.interaction_absent.identifier().value(),
            },
            "integer_keys": [
                {
                    "model": robot_model,
                    "round_trip_exact": constructed.negative_robot.robot_id().value()
                        == hydrated.negative_robot.robot_id().value(),
                    "sign": "negative",
                    "value": hydrated.negative_robot.robot_id().value().to_string(),
                },
                {
                    "model": robot_model,
                    "round_trip_exact": constructed.positive_robot.robot_id().value()
                        == hydrated.positive_robot.robot_id().value(),
                    "sign": "positive",
                    "value": hydrated.positive_robot.robot_id().value().to_string(),
                },
            ],
            "optional_role": interaction_actor["label"],
            "polymorphic_players_observed": [person_model, robot_model],
            "present": [
                {
                    "player": {"key": hydrated_person_actor, "model": person_model},
                    "relation_ref": hydrated.interaction_person.identifier().value(),
                },
                {
                    "player": {"key": hydrated_robot_actor.to_string(), "model": robot_model},
                    "relation_ref": hydrated.interaction_robot.identifier().value(),
                },
            ],
            "relation": interaction_model,
            "relation_as_player": {
                "owner_model": container_model,
                "player_model": event_model,
                "preserved": relation_player_constructed && relation_player_hydrated,
                "role": container_metadata["role"]["label"],
            },
        },
        "ordered_distinct_collections": {
            "owns": {
                "authored": ["analyst", "mathematician"],
                "constructed": constructed_aliases,
                "distinct": has_annotation(&aliases_metadata, "distinct"),
                "field": aliases_metadata["id"]["attribute"],
                "hydrated": hydrated_aliases,
                "mode": aliases_mode,
            },
            "player_duplicate": {
                "canonical_player": {"key": "data-ada", "model": person_model},
                "category": "invalid_input",
                "code": constraints.player_duplicate.code(),
                "duplicate_index": 1,
                "first_index": 0,
                "rejected_before_provider_io": true,
            },
            "relates": {
                "authored": ["data-ada", "data-dana"],
                "constructed": constructed_participants,
                "distinct": has_annotation(&participant_metadata, "distinct"),
                "hydrated": hydrated_participants,
                "mode": participant_mode,
                "role": participant_metadata["role"]["label"],
            },
            "scalar_duplicate": {
                "canonical_value": "analyst",
                "category": "invalid_input",
                "code": constraints.scalar_duplicate.code(),
                "duplicate_index": 1,
                "first_index": 0,
                "rejected_before_provider_io": true,
            },
            "unordered_compatibility_default": unordered_default,
        },
        "projected_constraint_validation": constraints.observation,
        "token_package_fencing": {
            "accepted_local": {
                "construction": local_construction,
                "hydration": local_hydration,
            },
            "foreign_rejections": {
                "construction": {
                    "category": hydrated.foreign_fence.construction.category().as_str(),
                    "code": hydrated.foreign_fence.construction.code().as_str(),
                    "rejected_before_provider_io": hydrated
                        .foreign_fence
                        .construction_rejected_before_provider_io,
                },
                "hydration": {
                    "category": hydrated.foreign_fence.hydration.category().as_str(),
                    "code": hydrated.foreign_fence.hydration.code().as_str(),
                    "public_result_published": hydrated
                        .foreign_fence
                        .hydration_public_result_published,
                },
            },
            "provider_text_exposed": hydrated.foreign_fence.provider_text_exposed,
        },
    });
    Ok(json!({
        "authority": {
            "journey": source_identity(root, JOURNEY_RELATIVE, "Phase-2 journey")?,
            "schema": source_identity(root, SCHEMA_RELATIVE, "Phase-2 schema")?,
        },
        "binding": "rust",
        "format": REPORT_FORMAT,
        "observations": observations,
        "semantic_profile": SEMANTIC_PROFILE,
    }))
}

fn bounded_regular_bytes(path: &Path, label: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("{label} cannot be inspected: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("{label} must be a non-symlink regular file").into());
    }
    if metadata.len() > MAX_AUTHORITY_BYTES {
        return Err(format!("{label} exceeds the authority byte ceiling").into());
    }
    let bytes = fs::read(path).map_err(|error| format!("{label} cannot be read: {error}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_AUTHORITY_BYTES {
        return Err(format!("{label} exceeds the authority byte ceiling").into());
    }
    Ok(bytes)
}

fn source_identity(
    root: &Path,
    relative: &str,
    label: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let bytes = bounded_regular_bytes(&root.join(relative), label)?;
    Ok(json!({
        "path": relative,
        "sha256": format!("{:x}", Sha256::digest(bytes)),
    }))
}

fn output_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let value = env::var_os(REPORT_ENV).ok_or_else(|| format!("{REPORT_ENV} must be set"))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() || path.file_name().is_none() {
        return Err("Phase-2 Rust report path must be an absolute file path".into());
    }
    Ok(path)
}

fn publish_report(path: &Path, report: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut payload = type_bridge_contract::codec::to_canonical_json(report)?;
    payload.push(b'\n');
    if payload.len() > MAX_REPORT_BYTES {
        return Err("canonical Phase-2 Rust report exceeds its byte ceiling".into());
    }
    let parent = path
        .parent()
        .ok_or("Phase-2 Rust report path has no parent")?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err("Phase-2 Rust report parent must be a non-symlink directory".into());
    }
    let mut destination = OpenOptions::new().write(true).create_new(true).open(path)?;
    destination.write_all(&payload)?;
    destination.sync_all()?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root =
        PathBuf::from(env::var_os(ROOT_ENV).ok_or_else(|| format!("{ROOT_ENV} must be set"))?);
    if !root.is_absolute() {
        return Err("Phase-2 repository root must be absolute".into());
    }
    let constructed = constructed_observations()?;
    assert_eq!(constructed.dana.identifier().value(), "data-dana");
    let hydrated = hydrated_observations(&root).await?;
    let report = build_report(&root, &constructed, &hydrated)?;
    publish_report(&output_path()?, &report)
}
