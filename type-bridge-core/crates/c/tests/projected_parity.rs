#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, ModelProjection, ProjectedTokenIdentity, ProjectionConfig,
    RuntimeProjection,
};
use type_bridge_contract::schema::{AnnotationKindId, CollectionMode, DocumentId};
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet, build_schema_authority,
    normalize_documents, project, resolve,
};
use type_bridge_schema_codegen::{CEmitter, GeneratedPackage};

const SOURCE: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml");
const CONSUMER: &str = include_str!("../../schema-codegen/tests/c_projected_parity/consumer.c");
const SEMANTIC_PROFILE: &str = "typedb-3.12.1/v1";
const REPORT_FORMAT: &str = "typebridge.projected-parity-report/v1";
const FACT_PREFIX: &str = "TYPE_BRIDGE_C_PROJECTED_FACT\t";
const REPORT_ENV: &str = "TYPE_BRIDGE_PROJECTED_PARITY_REPORT_C";
const REQUIRE_SHARED_ENV: &str = "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER";
const SCHEMA_RELATIVE: &str = "tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml";
const JOURNEY_RELATIVE: &str = "tests/contracts/sdk_conformance/sdk-v3/journey-v3.json";
const MAX_AUTHORITY_BYTES: u64 = 1024 * 1024;
const MAX_REPORT_BYTES: usize = 256 * 1024;
const OBSERVATIONS: [&str; 7] = [
    "canonical_scalar_values",
    "field_name_identity",
    "inherited_relation_role",
    "integer_key_polymorphic_role",
    "ordered_distinct_collections",
    "projected_constraint_validation",
    "token_package_fencing",
];

static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Stage(PathBuf);

impl Stage {
    fn new() -> Self {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "typebridge-c-projected-parity-{}-{sequence}",
            std::process::id(),
        ));
        fs::create_dir(&path).expect("unique C Projected parity stage creates");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("C Projected parity stage removes");
    }
}

struct EmittedFixture {
    local: GeneratedPackage,
    foreign: GeneratedPackage,
    local_projection: RuntimeProjection,
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("C crate lives beneath the repository root")
        .to_path_buf()
}

fn authority_for(
    source: &str,
    document: &str,
    scope: &str,
) -> (
    type_bridge_schema::ResolvedSchema,
    type_bridge_schema::VerifiedSchemaAuthority,
) {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new(document).expect("Projected document ID is valid"),
        source,
    )])
    .expect("Projected schema parses");
    let declared = normalize_documents(&documents).expect("Projected schema normalizes");
    let profile = SemanticProfileId::new(SEMANTIC_PROFILE).expect("Projected profile is valid");
    let resolved = resolve(&declared, &profile).expect("Projected schema resolves");
    let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|id| CapabilityId::new(*id).expect("built-in capability ID is valid"))
        .collect();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new(scope).expect("Projected managed scope is valid"),
        profile,
        available,
    );
    let authority = build_schema_authority(&declared, declared.required_capabilities(), &context)
        .expect("Projected schema authority builds");
    (resolved, authority)
}

fn emitted_fixture() -> EmittedFixture {
    let foreign_source = SOURCE.replace(
        "member: { card: { min: 0, max: 2 }, doc: membership player }",
        "member: { card: { min: 0, max: 3 }, doc: membership player }",
    );
    assert_ne!(
        foreign_source, SOURCE,
        "foreign Projected schema must differ"
    );
    let (local_resolved, local_authority) =
        authority_for(SOURCE, "sdk-v3.yaml", "sdk-v3-c-projected");
    let (foreign_resolved, foreign_authority) = authority_for(
        &foreign_source,
        "sdk-v3-foreign.yaml",
        "sdk-v3-c-projected-foreign",
    );
    let emitter = CEmitter::new();
    let emit = |resolved: &type_bridge_schema::ResolvedSchema,
                authority: &type_bridge_schema::VerifiedSchemaAuthority,
                prefix: &str| {
        let handlers = emitter.generator_handlers_for(resolved);
        let resources = emitter
            .code_resources_for(resolved)
            .expect("Projected C resources hash");
        let projection = project(
            resolved,
            BindingTarget::C,
            &ProjectionConfig::c(CSymbolPrefix::new(prefix).expect("C prefix is valid")),
            &handlers,
            &resources,
        )
        .expect("Sdk V3 projects to C");
        let package = emitter
            .emit(&projection, authority)
            .expect("Sdk V3 C package emits");
        (projection, package)
    };
    let (local_projection, local) = emit(&local_resolved, &local_authority, "projected");
    let (foreign_projection, foreign) =
        emit(&foreign_resolved, &foreign_authority, "projected_foreign");
    assert_ne!(
        local_projection.projection_fingerprint(),
        foreign_projection.projection_fingerprint(),
        "the foreign package must have genuinely distinct authority",
    );
    EmittedFixture {
        local,
        foreign,
        local_projection,
    }
}

fn model<'a>(
    projection: &'a RuntimeProjection,
    kind: TypeKind,
    label: &str,
) -> &'a ModelProjection {
    projection
        .models()
        .values()
        .find(|model| model.id().kind() == kind && model.id().label().as_str() == label)
        .unwrap_or_else(|| panic!("projection omitted {kind:?}:{label}"))
}

fn model_ordinal(projection: &RuntimeProjection, kind: TypeKind, label: &str) -> u32 {
    let model = model(projection, kind, label);
    projection
        .projected_token_ordinal(&ProjectedTokenIdentity::Model(model.id().clone()))
        .unwrap_or_else(|| panic!("model {kind:?}:{label} has no token"))
}

fn field_ordinal(
    projection: &RuntimeProjection,
    owner_kind: TypeKind,
    owner_label: &str,
    attribute: &str,
) -> u32 {
    let owner = model(projection, owner_kind, owner_label);
    let field = owner
        .query_tokens()
        .fields()
        .values()
        .find(|field| field.id().attribute().label().as_str() == attribute)
        .unwrap_or_else(|| panic!("projection omitted {owner_label}:{attribute}"));
    projection
        .projected_token_ordinal(&ProjectedTokenIdentity::Field {
            owner: owner.id().clone(),
            field: field.id().clone(),
        })
        .unwrap_or_else(|| panic!("field {owner_label}:{attribute} has no token"))
}

fn role_ordinal(projection: &RuntimeProjection, owner_label: &str, role_label: &str) -> u32 {
    let owner = model(projection, TypeKind::Relation, owner_label);
    let role = owner
        .query_tokens()
        .roles()
        .values()
        .find(|role| role.role().label().as_str() == role_label)
        .unwrap_or_else(|| panic!("projection omitted {owner_label}:{role_label}"));
    projection
        .projected_token_ordinal(&ProjectedTokenIdentity::Role {
            owner: owner.id().clone(),
            role: role.role().clone(),
        })
        .unwrap_or_else(|| panic!("role {owner_label}:{role_label} has no token"))
}

fn assert_projection_contract(projection: &RuntimeProjection, package: &GeneratedPackage) {
    let scalar_domains = projection
        .models()
        .values()
        .filter_map(|model| model.declaration().value_type())
        .map(|value| value.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        scalar_domains,
        BTreeSet::from([
            "boolean",
            "date",
            "datetime",
            "datetime_tz",
            "decimal",
            "double",
            "duration",
            "long",
            "string",
        ]),
    );

    let actor = model(projection, TypeKind::Entity, "actor");
    assert!(actor.declaration().is_abstract());
    assert!(!actor.declaration().is_constructible());
    assert!(!actor.create().enabled());

    let person = model(projection, TypeKind::Entity, "person");
    let person_fields = person
        .query_tokens()
        .fields()
        .values()
        .map(|field| (field.id().attribute().label().as_str(), field))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        person_fields["nickname"]
            .declaring_id()
            .owner()
            .label()
            .as_str(),
        "actor",
    );
    assert!(!person_fields.contains_key("party_name"));
    assert!(person_fields["aliases"].is_unique());
    assert!(
        person_fields["aliases"]
            .annotations()
            .keys()
            .any(|id| id.kind() == &AnnotationKindId::Distinct),
    );
    assert_eq!(
        person_fields["aliases"].multiplicity().collection_mode(),
        CollectionMode::OrderedList,
    );
    assert_eq!(
        person_fields["aliases"].multiplicity().cardinality().min(),
        0
    );
    assert_eq!(
        person_fields["aliases"].multiplicity().cardinality().max(),
        Some(3)
    );
    assert!(person_fields["identifier"].is_key());
    assert_eq!(
        person_fields["identifier"]
            .multiplicity()
            .cardinality()
            .min(),
        1
    );
    assert_eq!(
        person_fields["identifier"]
            .multiplicity()
            .cardinality()
            .max(),
        Some(1)
    );
    assert_eq!(person_fields["score"].multiplicity().cardinality().min(), 1);
    assert_eq!(
        person_fields["score"].multiplicity().cardinality().max(),
        Some(1)
    );
    assert_ne!(
        field_ordinal(projection, TypeKind::Entity, "person", "foo__bar"),
        field_ordinal(projection, TypeKind::Entity, "person", "score__gte"),
    );

    let plain = model(projection, TypeKind::Relation, "plain-activity");
    let participant = plain
        .query_tokens()
        .roles()
        .values()
        .find(|role| role.role().label().as_str() == "participant")
        .expect("plain-activity retains participant");
    assert_eq!(
        participant.role().declaring_relation().as_str(),
        "base-activity"
    );
    assert_eq!(participant.multiplicity().cardinality().min(), 1);
    assert_eq!(participant.multiplicity().cardinality().max(), Some(1));
    assert_eq!(
        participant
            .accepted_players()
            .iter()
            .map(TypeId::label)
            .map(|label| label.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["person"]),
    );

    let interaction = model(projection, TypeKind::Relation, "interaction");
    let actor_role = interaction
        .query_tokens()
        .roles()
        .values()
        .find(|role| role.role().label().as_str() == "actor")
        .expect("interaction retains actor");
    assert_eq!(
        actor_role
            .accepted_players()
            .iter()
            .map(TypeId::label)
            .map(|label| label.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["person", "robot"]),
    );
    assert!(!actor_role.multiplicity().required());
    assert_eq!(actor_role.multiplicity().cardinality().max(), Some(1));

    let event = model(projection, TypeKind::Relation, "event");
    let event_subject = event
        .query_tokens()
        .roles()
        .values()
        .find(|role| role.role().label().as_str() == "subject")
        .expect("event retains subject");
    assert_eq!(
        event_subject
            .accepted_players()
            .iter()
            .map(TypeId::label)
            .map(|label| label.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["person"]),
    );
    assert_eq!(event_subject.multiplicity().cardinality().min(), 1);
    assert_eq!(event_subject.multiplicity().cardinality().max(), Some(1));

    let employment = model(projection, TypeKind::Relation, "employment");
    assert_eq!(
        employment
            .create()
            .roles()
            .values()
            .map(|role| role.role().label().as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["employee"]),
    );

    let network = model(projection, TypeKind::Relation, "network-link");
    let network_participant = network
        .query_tokens()
        .roles()
        .values()
        .find(|role| role.role().label().as_str() == "participant")
        .expect("network-link retains participant");
    assert_eq!(
        network_participant.multiplicity().collection_mode(),
        CollectionMode::OrderedList,
    );
    assert_eq!(
        network_participant.multiplicity().cardinality().max(),
        Some(3)
    );
    assert!(
        network_participant
            .annotations()
            .keys()
            .any(|id| id.kind() == &AnnotationKindId::Distinct),
    );
    let container = model(projection, TypeKind::Relation, "container");
    let item = container
        .query_tokens()
        .roles()
        .values()
        .find(|role| role.role().label().as_str() == "item")
        .expect("container retains item");
    assert_eq!(
        item.multiplicity().collection_mode(),
        CollectionMode::Unordered
    );
    assert_eq!(item.multiplicity().cardinality().max(), Some(2));

    let counter = model(projection, TypeKind::Entity, "counter");
    let counter_value = counter
        .query_tokens()
        .fields()
        .values()
        .find(|field| field.id().attribute().label().as_str() == "counter_value")
        .expect("counter retains required counter_value");
    assert_eq!(counter_value.multiplicity().cardinality().min(), 1);
    assert_eq!(counter_value.multiplicity().cardinality().max(), Some(1));

    let header = std::str::from_utf8(
        package
            .get("include/projected/models.h")
            .expect("Projected generated header exists"),
    )
    .expect("Projected generated header is UTF-8");
    assert!(!header.contains("projected_actor_create_open("));
    let person_args = header
        .split_once("typedef struct projected_person_create_args_v1_t {")
        .and_then(|(_, tail)| tail.split_once("} projected_person_create_args_v1_t;"))
        .map(|(body, _)| body)
        .expect("person create arguments are emitted");
    assert!(person_args.contains("field_nickname"));
    assert!(!person_args.contains("field_partyzuname"));
    assert!(!header.contains("projected_plainzhactivity_participant_player_from_robot("));
    assert!(!header.contains("projected_event_subject_player_from_robot("));
    let employment_args = header
        .split_once("typedef struct projected_employment_create_args_v1_t {")
        .and_then(|(_, tail)| tail.split_once("} projected_employment_create_args_v1_t;"))
        .map(|(body, _)| body)
        .expect("employment create arguments are emitted");
    assert!(employment_args.contains("role_employee"));
    assert!(!employment_args.contains("role_member"));
}

fn token_symbol(prefix: &str, kind: &str, ordinal: u32) -> String {
    format!("{prefix}_projected_{kind}_token_{ordinal}")
}

fn render_consumer(fixture: &EmittedFixture) -> String {
    let local = &fixture.local_projection;
    let mut replacements = BTreeMap::new();
    for (placeholder, kind, label) in [
        ("LOCAL_MODEL_ACTOR", TypeKind::Entity, "actor"),
        ("LOCAL_MODEL_PERSON", TypeKind::Entity, "person"),
        ("LOCAL_MODEL_ROBOT", TypeKind::Entity, "robot"),
        ("LOCAL_MODEL_COUNTER", TypeKind::Entity, "counter"),
        ("LOCAL_MODEL_CONTAINER", TypeKind::Relation, "container"),
        ("LOCAL_MODEL_EMPLOYMENT", TypeKind::Relation, "employment"),
        ("LOCAL_MODEL_EVENT", TypeKind::Relation, "event"),
        ("LOCAL_MODEL_INTERACTION", TypeKind::Relation, "interaction"),
        (
            "LOCAL_MODEL_NETWORK_LINK",
            TypeKind::Relation,
            "network-link",
        ),
        (
            "LOCAL_MODEL_PLAIN_ACTIVITY",
            TypeKind::Relation,
            "plain-activity",
        ),
        ("LOCAL_MODEL_SCORE", TypeKind::Attribute, "score"),
    ] {
        replacements.insert(
            placeholder,
            token_symbol("projected", "model", model_ordinal(local, kind, label)),
        );
    }
    for (placeholder, owner_kind, owner, attribute) in [
        (
            "LOCAL_FIELD_PERSON_ALIASES",
            TypeKind::Entity,
            "person",
            "aliases",
        ),
        (
            "LOCAL_FIELD_PERSON_FOO_BAR",
            TypeKind::Entity,
            "person",
            "foo__bar",
        ),
        (
            "LOCAL_FIELD_PERSON_IDENTIFIER",
            TypeKind::Entity,
            "person",
            "identifier",
        ),
        (
            "LOCAL_FIELD_PERSON_NICKNAME",
            TypeKind::Entity,
            "person",
            "nickname",
        ),
        (
            "LOCAL_FIELD_PERSON_SCORE",
            TypeKind::Entity,
            "person",
            "score",
        ),
        (
            "LOCAL_FIELD_PERSON_SCORE_GTE",
            TypeKind::Entity,
            "person",
            "score__gte",
        ),
        (
            "LOCAL_FIELD_PERSON_VAL_BOOL",
            TypeKind::Entity,
            "person",
            "val_bool",
        ),
        (
            "LOCAL_FIELD_PERSON_VAL_CONSTRAINED",
            TypeKind::Entity,
            "person",
            "val_constrained",
        ),
        (
            "LOCAL_FIELD_PERSON_VAL_DATE",
            TypeKind::Entity,
            "person",
            "val_date",
        ),
        (
            "LOCAL_FIELD_PERSON_VAL_DATETIME",
            TypeKind::Entity,
            "person",
            "val_datetime",
        ),
        (
            "LOCAL_FIELD_PERSON_VAL_DATETIME_TZ",
            TypeKind::Entity,
            "person",
            "val_datetime_tz",
        ),
        (
            "LOCAL_FIELD_PERSON_VAL_DECIMAL",
            TypeKind::Entity,
            "person",
            "val_decimal",
        ),
        (
            "LOCAL_FIELD_PERSON_VAL_DOUBLE",
            TypeKind::Entity,
            "person",
            "val_double",
        ),
        (
            "LOCAL_FIELD_PERSON_VAL_DURATION",
            TypeKind::Entity,
            "person",
            "val_duration",
        ),
        (
            "LOCAL_FIELD_PARTY_PARTY_NAME",
            TypeKind::Entity,
            "party",
            "party_name",
        ),
        (
            "LOCAL_FIELD_ROBOT_ID",
            TypeKind::Entity,
            "robot",
            "robot_id",
        ),
        (
            "LOCAL_FIELD_INTERACTION_IDENTIFIER",
            TypeKind::Relation,
            "interaction",
            "identifier",
        ),
        (
            "LOCAL_FIELD_NETWORK_IDENTIFIER",
            TypeKind::Relation,
            "network-link",
            "identifier",
        ),
    ] {
        replacements.insert(
            placeholder,
            token_symbol(
                "projected",
                "field",
                field_ordinal(local, owner_kind, owner, attribute),
            ),
        );
    }
    for (placeholder, owner, role) in [
        ("LOCAL_ROLE_CONTAINER_ITEM", "container", "item"),
        ("LOCAL_ROLE_EVENT_SUBJECT", "event", "subject"),
        ("LOCAL_ROLE_INTERACTION_ACTOR", "interaction", "actor"),
        ("LOCAL_ROLE_INTERACTION_TARGET", "interaction", "target"),
        (
            "LOCAL_ROLE_NETWORK_DESTINATION",
            "network-link",
            "destination",
        ),
        ("LOCAL_ROLE_NETWORK_ORIGIN", "network-link", "origin"),
        (
            "LOCAL_ROLE_NETWORK_PARTICIPANT",
            "network-link",
            "participant",
        ),
        (
            "LOCAL_ROLE_PLAIN_PARTICIPANT",
            "plain-activity",
            "participant",
        ),
        ("LOCAL_ROLE_MEMBERSHIP_MEMBER", "membership", "member"),
    ] {
        replacements.insert(
            placeholder,
            token_symbol("projected", "role", role_ordinal(local, owner, role)),
        );
    }
    let mut source = CONSUMER.to_owned();
    let mut replacements = replacements.into_iter().collect::<Vec<_>>();
    replacements.sort_by_key(|(token, _)| std::cmp::Reverse(token.len()));
    for (placeholder, symbol) in replacements {
        source = source.replace(placeholder, &symbol);
    }
    assert!(
        !source.contains("LOCAL_") && !source.contains("FOREIGN_"),
        "C Projected consumer retained an unresolved token placeholder",
    );
    source
}

fn write_package(package: &GeneratedPackage, root: &Path) {
    for (relative, contents) in package.files() {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("generated path has a parent"))
            .expect("generated package directory creates");
        fs::write(path, contents).expect("generated package file writes");
    }
}

fn command_exists(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn native_library() -> PathBuf {
    if let Some(path) = env::var_os("TYPE_BRIDGE_C_SHARED_LIBRARY") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "explicit TypeBridge C shared library is absent"
        );
        return path;
    }
    let executable = env::current_exe().expect("current test executable is available");
    let dependencies = executable.parent().expect("test executable has a parent");
    let profile = dependencies
        .parent()
        .expect("dependency directory has a parent");
    let filename = format!(
        "{}type_bridge_c{}",
        env::consts::DLL_PREFIX,
        env::consts::DLL_SUFFIX,
    );
    [dependencies.join(&filename), profile.join(filename)]
        .into_iter()
        .find(|path| path.is_file())
        .expect("build type-bridge-c's shared library before the Projected producer")
}

fn shared_consumer_requested() -> bool {
    match env::var(REQUIRE_SHARED_ENV) {
        Err(env::VarError::NotPresent) => false,
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("{REQUIRE_SHARED_ENV} must be unset or exactly `1`, got {value:?}"),
        Err(error) => panic!("{REQUIRE_SHARED_ENV} is not Unicode: {error}"),
    }
}

fn parse_observations(stdout: &[u8]) -> BTreeMap<String, Value> {
    let text = std::str::from_utf8(stdout).expect("C Projected facts are UTF-8");
    let mut observations = BTreeMap::new();
    for line in text.lines() {
        let Some(payload) = line.strip_prefix(FACT_PREFIX) else {
            continue;
        };
        let (name, raw) = payload
            .split_once('\t')
            .expect("C Projected fact has a name and observation");
        assert!(
            OBSERVATIONS.contains(&name),
            "unexpected C Projected fact {name}"
        );
        let value: Value = serde_json::from_str(raw).expect("C Projected fact is valid JSON");
        assert!(value.is_object(), "C Projected fact must be an object");
        assert_eq!(
            to_canonical_json(&value).expect("C Projected fact canonicalizes"),
            raw.as_bytes(),
            "C Projected fact {name} is not canonical JSON",
        );
        assert!(
            observations.insert(name.to_owned(), value).is_none(),
            "C Projected fact {name} was duplicated",
        );
    }
    assert_eq!(
        observations.keys().map(String::as_str).collect::<Vec<_>>(),
        OBSERVATIONS,
        "C Projected fact ledger is incomplete",
    );
    observations
}

fn regular_source_identity(root: &Path, relative: &str) -> Value {
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).expect("Projected authority is inspectable");
    assert!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Projected authority must be a regular non-symlink file",
    );
    assert!(
        metadata.len() <= MAX_AUTHORITY_BYTES,
        "Projected authority exceeds its byte limit",
    );
    let bytes = fs::read(&path).expect("Projected authority is readable");
    assert!(bytes.len() as u64 <= MAX_AUTHORITY_BYTES);
    json!({
        "path": relative,
        "sha256": format!("{:x}", Sha256::digest(&bytes)),
    })
}

fn build_report(root: &Path, observations: BTreeMap<String, Value>) -> Value {
    json!({
        "authority": {
            "journey": regular_source_identity(root, JOURNEY_RELATIVE),
            "schema": regular_source_identity(root, SCHEMA_RELATIVE),
        },
        "binding": "c",
        "format": REPORT_FORMAT,
        "observations": observations,
        "semantic_profile": SEMANTIC_PROFILE,
    })
}

fn canonical_report(report: &Value) -> Vec<u8> {
    let mut bytes = to_canonical_json(report).expect("C Projected report canonicalizes");
    bytes.push(b'\n');
    assert!(
        bytes.len() <= MAX_REPORT_BYTES,
        "C Projected report exceeds its byte limit",
    );
    bytes
}

fn publish_report(path: &Path, report: &Value) {
    assert!(path.is_absolute(), "{REPORT_ENV} must be an absolute path");
    let parent = path.parent().expect("C Projected report path has a parent");
    let metadata = fs::symlink_metadata(parent).expect("report parent is inspectable");
    assert!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "report parent must be a real directory",
    );
    let bytes = canonical_report(report);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .expect("C Projected report creates without replacement");
    file.write_all(&bytes).expect("C Projected report writes");
    file.sync_all().expect("C Projected report synchronizes");
}

fn validate_report(root: &Path, path: &Path) {
    let script = root.join("scripts/ci/compare_projected_parity.py");
    let program = concat!(
        "import importlib.util,pathlib,sys;",
        "p=pathlib.Path(sys.argv[1]);",
        "s=importlib.util.spec_from_file_location('projected_comparator',p);",
        "m=importlib.util.module_from_spec(s);sys.modules[s.name]=m;",
        "s.loader.exec_module(m);",
        "m._load_report(pathlib.Path(sys.argv[2]),m.load_contract())",
    );
    let output = Command::new("python3")
        .args(["-c", program])
        .arg(script)
        .arg(path)
        .current_dir(root)
        .output()
        .expect("committed Projected comparator launches");
    assert!(
        output.status.success(),
        "committed Projected comparator rejected C report:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn canonical_report_publisher_is_bounded_and_create_new() {
    let stage = Stage::new();
    let path = stage.path().join("report.json");
    let report = json!({"binding": "c", "observed": {"value": 38}});
    publish_report(&path, &report);
    assert_eq!(fs::read(&path).unwrap(), canonical_report(&report));
    assert!(std::panic::catch_unwind(|| publish_report(&path, &report)).is_err());
}

#[test]
fn generated_c17_projected_parity_producer_is_exact_and_comparator_accepted() {
    if !shared_consumer_requested() {
        return;
    }
    let root = repository_root();
    let library = native_library();
    let fixture = emitted_fixture();
    assert_projection_contract(&fixture.local_projection, &fixture.local);
    let stage = Stage::new();
    let local = stage.path().join("local");
    let foreign = stage.path().join("foreign");
    write_package(&fixture.local, &local);
    write_package(&fixture.foreign, &foreign);
    let consumer = stage.path().join("projected-parity-consumer.c");
    fs::write(&consumer, render_consumer(&fixture)).expect("C Projected consumer writes");
    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let library_directory = library.parent().expect("shared library has a parent");
    let mut canonical_stdout: Option<Vec<u8>> = None;
    let mut compiler_count = 0;
    for compiler in ["gcc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        compiler_count += 1;
        let executable = stage.path().join(format!("projected-parity-{compiler}"));
        let output = Command::new(compiler)
            .args([
                "-std=c17",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
            ])
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(local.join("include"))
            .arg("-I")
            .arg(foreign.join("include"))
            .arg(local.join("src/models.c"))
            .arg(foreign.join("src/models.c"))
            .arg(&consumer)
            .arg("-L")
            .arg(library_directory)
            .arg("-ltype_bridge_c")
            .arg(format!("-Wl,-rpath,{}", library_directory.display()))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} rejected the strict C17 Projected producer:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {compiler}'s producer: {error}"));
        assert!(
            output.status.success(),
            "{compiler}'s C Projected producer failed with {}:\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        if let Some(expected) = &canonical_stdout {
            assert_eq!(
                &output.stdout, expected,
                "C compilers observed different facts"
            );
        } else {
            canonical_stdout = Some(output.stdout);
        }
    }
    assert!(compiler_count > 0, "GCC or Clang is required");
    let observations = parse_observations(
        canonical_stdout
            .as_deref()
            .expect("one compiler emitted C Projected facts"),
    );
    let report = build_report(&root, observations);
    let requested = env::var_os(REPORT_ENV).map(PathBuf::from);
    let output = requested.unwrap_or_else(|| stage.path().join("c-projected-report.json"));
    publish_report(&output, &report);
    validate_report(&root, &output);
}
