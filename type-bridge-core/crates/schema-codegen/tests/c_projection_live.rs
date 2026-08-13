use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, CSymbolPrefix, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{
    SchemaDocumentSet, encode_schema_authority, normalize_documents, project, resolve,
};
use type_bridge_schema_codegen::{CEmitter, GeneratedPackage};

mod support;

const SCHEMA: &str = include_str!("acceptance/schema.yaml");
const PROVIDER_SCHEMA: &str = include_str!("acceptance/provider-3.12.1.tql");
const SETUP: &str = include_str!("c_projection_live/setup.rs");
const CONSUMER: &str = include_str!("c_projection_live/consumer.c");
const DATABASE_CREATED_MARKER: &str = "generated C isolated database created";
const WORKFORCE_V2_FACT_PREFIX: &str = "TYPE_BRIDGE_C_V2_FACT\t";
const WORKFORCE_MANIFEST_PATH: &str = "tests/contracts/sdk_conformance/manifest-v1.json";
const WORKFORCE_V2_CATALOG_PATH: &str =
    "tests/contracts/sdk_conformance/workforce-v2/catalog-v2.json";
const WORKFORCE_SCHEMA_PATH: &str =
    "type-bridge-core/crates/schema-codegen/tests/acceptance/schema.yaml";
const WORKFORCE_PROVIDER_SCHEMA_PATH: &str =
    "type-bridge-core/crates/schema-codegen/tests/acceptance/provider-3.12.1.tql";
const WORKFORCE_V2_SELECTED_PROOFS: [(&str, &str, &str); 34] = [
    (
        "workforce.crud.entity-single",
        "direct_runtime",
        "entity_lifecycle",
    ),
    (
        "workforce.crud.relation-single",
        "direct_runtime",
        "relation_lifecycle",
    ),
    (
        "workforce.diagnostic.all-workflows",
        "diagnostic",
        "structured_query_diagnostic",
    ),
    (
        "workforce.diagnostic.remote-structured",
        "diagnostic",
        "remote_structured_diagnostic",
    ),
    (
        "workforce.model.values-and-references",
        "direct_runtime",
        "model_values_and_references",
    ),
    (
        "workforce.model.values-and-references",
        "remote_runtime",
        "model_values_and_references",
    ),
    (
        "workforce.query.exact-subtypes",
        "direct_runtime",
        "exact_subtypes",
    ),
    (
        "workforce.query.exact-subtypes",
        "remote_runtime",
        "exact_subtypes",
    ),
    (
        "workforce.query.owner-iid-set",
        "direct_runtime",
        "owner_iid_set",
    ),
    (
        "workforce.query.owner-iid-set",
        "remote_runtime",
        "owner_iid_set",
    ),
    (
        "workforce.query.reducers-direct",
        "direct_runtime",
        "grouped_reducer",
    ),
    (
        "workforce.query.reducers-remote",
        "remote_runtime",
        "grouped_reducer",
    ),
    (
        "workforce.query.remote-hydration",
        "direct_runtime",
        "hydrated_result",
    ),
    (
        "workforce.query.remote-hydration",
        "remote_runtime",
        "hydrated_result",
    ),
    (
        "workforce.query.remote-one-exchange",
        "remote_runtime",
        "remote_one_exchange",
    ),
    ("workforce.query.roles", "direct_runtime", "roles"),
    ("workforce.query.roles", "remote_runtime", "roles"),
    (
        "workforce.query.scalar-boolean-predicates",
        "direct_runtime",
        "scalar_boolean",
    ),
    (
        "workforce.query.scalar-boolean-predicates",
        "remote_runtime",
        "scalar_boolean",
    ),
    (
        "workforce.query.schema-function",
        "direct_runtime",
        "schema_function",
    ),
    (
        "workforce.query.schema-function",
        "remote_runtime",
        "schema_function",
    ),
    (
        "workforce.query.selection-shapes",
        "direct_runtime",
        "selection_shapes",
    ),
    (
        "workforce.query.selection-shapes",
        "remote_runtime",
        "selection_shapes",
    ),
    ("workforce.query.terminals", "direct_runtime", "terminals"),
    ("workforce.query.terminals", "remote_runtime", "terminals"),
    ("workforce.query.topology", "direct_runtime", "topology"),
    ("workforce.query.topology", "remote_runtime", "topology"),
    (
        "workforce.runtime.cancellation",
        "direct_runtime",
        "cancellation_direct",
    ),
    (
        "workforce.runtime.cancellation",
        "remote_runtime",
        "cancellation_remote",
    ),
    (
        "workforce.runtime.explicit-close",
        "lifecycle",
        "query_resource_lifecycle",
    ),
    (
        "workforce.runtime.timeout-resource-limits",
        "direct_runtime",
        "resource_limits",
    ),
    (
        "workforce.runtime.timeout-resource-limits",
        "remote_runtime",
        "resource_limits",
    ),
    (
        "workforce.value.scalar-domain-comparison",
        "direct_runtime",
        "scalar_domain",
    ),
    (
        "workforce.value.scalar-domain-comparison",
        "remote_runtime",
        "scalar_domain",
    ),
];
const WORKFORCE_V2_LIVE_LANES: [(&str, &str); 31] = [
    ("entity_lifecycle", "direct_runtime"),
    ("relation_lifecycle", "direct_runtime"),
    ("structured_query_diagnostic", "diagnostic"),
    ("model_values_and_references", "direct_runtime"),
    ("model_values_and_references", "remote_runtime"),
    ("exact_subtypes", "direct_runtime"),
    ("exact_subtypes", "remote_runtime"),
    ("owner_iid_set", "direct_runtime"),
    ("owner_iid_set", "remote_runtime"),
    ("grouped_reducer", "direct_runtime"),
    ("grouped_reducer", "remote_runtime"),
    ("hydrated_result", "direct_runtime"),
    ("hydrated_result", "remote_runtime"),
    ("remote_one_exchange", "remote_runtime"),
    ("roles", "direct_runtime"),
    ("roles", "remote_runtime"),
    ("scalar_boolean", "direct_runtime"),
    ("scalar_boolean", "remote_runtime"),
    ("schema_function", "direct_runtime"),
    ("schema_function", "remote_runtime"),
    ("selection_shapes", "direct_runtime"),
    ("selection_shapes", "remote_runtime"),
    ("terminals", "direct_runtime"),
    ("terminals", "remote_runtime"),
    ("topology", "direct_runtime"),
    ("topology", "remote_runtime"),
    ("query_resource_lifecycle", "lifecycle"),
    ("resource_limits", "direct_runtime"),
    ("resource_limits", "remote_runtime"),
    ("scalar_domain", "direct_runtime"),
    ("scalar_domain", "remote_runtime"),
];
const WORKFORCE_V2_MANIFEST_TRANSITION_CASES: [&str; 17] = [
    "workforce.model.values-and-references",
    "workforce.crud.entity-single",
    "workforce.crud.relation-single",
    "workforce.query.owner-iid-set",
    "workforce.query.exact-subtypes",
    "workforce.query.scalar-boolean-predicates",
    "workforce.query.roles",
    "workforce.query.topology",
    "workforce.query.selection-shapes",
    "workforce.query.terminals",
    "workforce.query.reducers-direct",
    "workforce.query.remote-one-exchange",
    "workforce.query.remote-hydration",
    "workforce.diagnostic.remote-structured",
    "workforce.value.scalar-domain-comparison",
    "workforce.query.reducers-remote",
    "workforce.query.schema-function",
];

static TEMP_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = TEMP_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = env::temp_dir().join(format!(
            "typebridge-c-projection-live-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("unique generated C live directory is created");
        Self(directory)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn emitted_package() -> GeneratedPackage {
    emitted_package_authority_and_fingerprints().0
}

fn emitted_package_authority_and_fingerprints() -> (GeneratedPackage, Vec<u8>, Value, Value) {
    let emitter = CEmitter::new();
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-projection-live.yaml").expect("document ID is valid"),
        SCHEMA,
    )])
    .expect("shared acceptance schema parses");
    let declared = normalize_documents(&documents).expect("shared acceptance schema normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("shared acceptance schema resolves");
    let resources = emitter
        .code_resources_for(&resolved)
        .expect("C resources hash");
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("fixture").expect("fixture prefix is valid")),
        &emitter.generator_handlers_for(&resolved),
        &resources,
    )
    .expect("shared acceptance schema projects to C");
    let authority =
        support::authority_for_declared(&declared, "c-projection-live", support::TEST_PROFILE);
    let authority_bytes = encode_schema_authority(&authority);
    let semantic_fingerprint = serde_json::to_value(projection.semantic_fingerprint())
        .expect("C semantic fingerprint serializes");
    let projection_fingerprint = serde_json::to_value(projection.projection_fingerprint())
        .expect("C projection fingerprint serializes");
    let package = emitter
        .emit(&projection, &authority)
        .expect("shared acceptance C package emits");
    (
        package,
        authority_bytes,
        semantic_fingerprint,
        projection_fingerprint,
    )
}

fn write_package(package: &GeneratedPackage, root: &Path) {
    for (relative, contents) in package.files() {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("generated path has a parent"))
            .expect("generated parent directory is created");
        fs::write(path, contents).expect("generated C package file is written");
    }
}

fn command_exists(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn c_compilers() -> Vec<&'static str> {
    ["gcc", "clang"]
        .into_iter()
        .filter(|compiler| command_exists(compiler))
        .collect()
}

fn runtime_include() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists")
}

#[cfg(unix)]
#[test]
fn exact_live_consumer_is_strict_c17_against_the_shared_generated_schema() {
    let compilers = c_compilers();
    assert!(!compilers.is_empty(), "GCC or Clang is required");
    let stage = TempDirectory::new();
    write_package(&emitted_package(), stage.path());
    let consumer = stage.path().join("consumer.c");
    fs::write(&consumer, CONSUMER).expect("generated C live consumer is staged");
    for compiler in compilers {
        let output = Command::new(compiler)
            .args([
                "-std=c17",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
                "-fsyntax-only",
            ])
            .arg("-I")
            .arg(runtime_include())
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg(stage.path().join("src/models.c"))
            .arg(&consumer)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} rejected the exact generated C17 live consumer:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[cfg(unix)]
fn required_live_environment(name: &str) -> String {
    match env::var(name) {
        Ok(value) if !value.is_empty() => value,
        Ok(_) => panic!("{name} must not be empty for exact generated C acceptance"),
        Err(env::VarError::NotPresent) => {
            panic!("{name} must be configured for exact generated C acceptance")
        }
        Err(env::VarError::NotUnicode(_)) => {
            panic!("{name} must contain valid UTF-8 for exact generated C acceptance")
        }
    }
}

#[cfg(unix)]
fn native_library() -> PathBuf {
    let executable = env::current_exe().expect("current test executable path is available");
    let dependency_directory = executable
        .parent()
        .expect("test executable has a dependency directory");
    let profile_directory = dependency_directory
        .parent()
        .expect("dependency directory has a profile parent");
    let filename = format!(
        "{}type_bridge_c{}",
        env::consts::DLL_PREFIX,
        env::consts::DLL_SUFFIX
    );
    [
        dependency_directory.join(&filename),
        profile_directory.join(filename),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .unwrap_or_else(|| {
        panic!("build the TypeBridge C shared library before exact generated C acceptance")
    })
}

#[cfg(unix)]
fn manifest_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

#[cfg(unix)]
fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("schema-codegen lives beneath the repository root")
        .to_path_buf()
}

#[cfg(unix)]
fn requested_workforce_v2_report() -> Option<PathBuf> {
    let raw = env::var_os("TYPE_BRIDGE_WORKFORCE_REPORT_V2")?;
    let raw = raw
        .into_string()
        .expect("TYPE_BRIDGE_WORKFORCE_REPORT_V2 must be UTF-8");
    assert!(
        raw.len() <= 4096,
        "TYPE_BRIDGE_WORKFORCE_REPORT_V2 exceeds 4096 UTF-8 bytes"
    );
    let path = PathBuf::from(raw);
    assert!(
        path.is_absolute(),
        "TYPE_BRIDGE_WORKFORCE_REPORT_V2 must be an absolute path"
    );
    let parent = path
        .parent()
        .expect("TYPE_BRIDGE_WORKFORCE_REPORT_V2 must have a parent directory");
    let metadata = fs::symlink_metadata(parent)
        .expect("TYPE_BRIDGE_WORKFORCE_REPORT_V2 parent must already exist");
    assert!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "TYPE_BRIDGE_WORKFORCE_REPORT_V2 parent must be a non-symlink directory"
    );
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => panic!("TYPE_BRIDGE_WORKFORCE_REPORT_V2 target must not already exist"),
        Err(error) => {
            panic!("TYPE_BRIDGE_WORKFORCE_REPORT_V2 target is not inspectable: {error}")
        }
    }
    Some(path)
}

#[cfg(unix)]
fn validated_workforce_v2_proofs(
    root: &Path,
    report: Option<&Path>,
) -> BTreeMap<support::WorkforceV2ProofLane, Value> {
    let fragments = env::var_os("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS");
    let nonce = env::var_os("TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE");
    match (report, fragments, nonce) {
        (None, None, None) => BTreeMap::new(),
        (None, _, _) => panic!(
            "workforce-v2 proof inputs are valid only when TYPE_BRIDGE_WORKFORCE_REPORT_V2 is requested"
        ),
        (Some(_), Some(fragments), Some(nonce)) => {
            let nonce = nonce
                .into_string()
                .expect("TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE must be UTF-8");
            let paths = support::workforce_v2_proof_paths(&fragments)
                .expect("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS must be a valid path list");
            support::validate_workforce_v2_proof_fragments(root, "c", &nonce, &paths)
                .expect("C workforce-v2 proof fragments must validate")
        }
        (Some(_), _, _) => panic!(
            "TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS and TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE are both required for a V2 report"
        ),
    }
}

#[cfg(unix)]
fn parse_workforce_v2_live_facts(
    root: &Path,
    stdout: &str,
) -> BTreeMap<support::WorkforceV2ProofLane, Value> {
    let expected = WORKFORCE_V2_LIVE_LANES
        .into_iter()
        .map(|(observation_ref, proof_kind)| (observation_ref.to_owned(), proof_kind.to_owned()))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        expected.len(),
        WORKFORCE_V2_LIVE_LANES.len(),
        "C workforce-v2 live lane inventory contains a duplicate"
    );
    let mut observations = BTreeMap::new();
    for line in stdout.lines() {
        let Some(payload) = line.strip_prefix(WORKFORCE_V2_FACT_PREFIX) else {
            continue;
        };
        let mut fields = payload.split('\t');
        let observation_ref = fields.next().expect("fact observation ref exists");
        let proof_kind = fields.next().expect("fact proof kind exists");
        let raw_observation = fields.next().expect("fact observation JSON exists");
        assert!(
            fields.next().is_none()
                && !observation_ref.is_empty()
                && !proof_kind.is_empty()
                && !raw_observation.is_empty(),
            "C workforce-v2 fact is malformed: {line:?}"
        );
        let lane = (observation_ref.to_owned(), proof_kind.to_owned());
        assert!(
            expected.contains(&lane),
            "C workforce-v2 consumer emitted an unexpected lane: {lane:?}"
        );
        let observation: Value = serde_json::from_str(raw_observation)
            .unwrap_or_else(|error| panic!("C workforce-v2 fact is invalid JSON: {error}"));
        assert!(
            observation.is_object(),
            "C workforce-v2 fact must be an object: {lane:?}"
        );
        let canonical =
            to_canonical_json(&observation).expect("C workforce-v2 fact observation canonicalizes");
        assert_eq!(
            canonical,
            raw_observation.as_bytes(),
            "C workforce-v2 fact must be compact canonical JSON: {lane:?}"
        );
        assert!(
            observations.insert(lane.clone(), observation).is_none(),
            "C workforce-v2 consumer duplicated a lane: {lane:?}"
        );
    }
    assert_eq!(
        observations.keys().cloned().collect::<BTreeSet<_>>(),
        expected,
        "C workforce-v2 live fact coverage differs from the frozen lane inventory"
    );

    assert_workforce_v2_observations_match_journey(root, &observations);
    observations
}

#[cfg(unix)]
fn assert_workforce_v2_observations_match_journey(
    root: &Path,
    observations: &BTreeMap<support::WorkforceV2ProofLane, Value>,
) {
    let journey_bytes = fs::read(root.join(support::WORKFORCE_V2_JOURNEY))
        .expect("workforce-v2 journey is readable");
    let journey: Value =
        serde_json::from_slice(&journey_bytes).expect("workforce-v2 journey is valid JSON");
    let expected_observations = journey["expected_observations"]
        .as_object()
        .expect("workforce-v2 expected observations are an object");
    for ((observation_ref, proof_kind), observation) in observations {
        assert_eq!(
            observation,
            expected_observations
                .get(observation_ref)
                .unwrap_or_else(|| panic!("workforce-v2 observation is absent: {observation_ref}")),
            "C workforce-v2 observation diverged: {observation_ref}/{proof_kind}"
        );
    }
}

#[cfg(unix)]
fn workforce_v2_report_results(
    catalog: &Value,
    observations: &BTreeMap<support::WorkforceV2ProofLane, Value>,
) -> Vec<Value> {
    let mut capabilities = BTreeMap::new();
    for case in catalog["cases"]
        .as_array()
        .expect("workforce-v2 catalog cases must be an array")
    {
        let case_id = case["id"]
            .as_str()
            .expect("workforce-v2 case ID must be text")
            .to_owned();
        let capability_id = case["capability_id"]
            .as_str()
            .expect("workforce-v2 capability ID must be text")
            .to_owned();
        assert!(
            capabilities.insert(case_id, capability_id).is_none(),
            "workforce-v2 catalog contains a duplicate case"
        );
    }
    let selected = catalog["selected_proofs"]
        .as_array()
        .expect("workforce-v2 selected proofs must be an array");
    assert_eq!(selected.len(), 34, "workforce-v2 report requires 34 proofs");
    let selected_contract = selected
        .iter()
        .map(|proof| {
            (
                proof["case_id"].as_str().expect("selected case ID is text"),
                proof["proof_kind"]
                    .as_str()
                    .expect("selected proof kind is text"),
                proof["observation_ref"]
                    .as_str()
                    .expect("selected observation ref is text"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        selected_contract, WORKFORCE_V2_SELECTED_PROOFS,
        "workforce-v2 catalog selected proof ledger drifted"
    );
    let mut required = BTreeSet::new();
    let mut rows = Vec::with_capacity(selected.len());
    for proof in selected {
        let case_id = proof["case_id"]
            .as_str()
            .expect("workforce-v2 selected case ID must be text")
            .to_owned();
        let proof_kind = proof["proof_kind"]
            .as_str()
            .expect("workforce-v2 selected proof kind must be text")
            .to_owned();
        let observation_ref = proof["observation_ref"]
            .as_str()
            .expect("workforce-v2 selected observation ref must be text")
            .to_owned();
        let lane = (observation_ref.clone(), proof_kind.clone());
        assert!(
            required.insert(lane.clone()),
            "workforce-v2 catalog duplicated a selected lane: {lane:?}"
        );
        let capability_id = capabilities
            .get(&case_id)
            .unwrap_or_else(|| panic!("selected workforce-v2 case is unknown: {case_id}"));
        let observation = observations
            .get(&lane)
            .unwrap_or_else(|| panic!("C observation is absent: {lane:?}"));
        rows.push(serde_json::json!({
            "capability_id": capability_id,
            "case_id": case_id,
            "observation": observation,
            "outcome": "passed",
            "proof_kind": proof_kind,
        }));
    }
    assert_eq!(
        observations.keys().cloned().collect::<BTreeSet<_>>(),
        required,
        "C workforce-v2 observations differ from the catalog selected lanes"
    );
    assert!(
        rows.windows(2).all(|pair| {
            let left = (
                pair[0]["case_id"].as_str().expect("case ID"),
                pair[0]["proof_kind"].as_str().expect("proof kind"),
            );
            let right = (
                pair[1]["case_id"].as_str().expect("case ID"),
                pair[1]["proof_kind"].as_str().expect("proof kind"),
            );
            left < right
        }),
        "workforce-v2 selected proofs must be sorted by case and proof kind"
    );
    rows
}

#[cfg(unix)]
fn build_workforce_v2_report(
    root: &Path,
    semantic_fingerprint: Value,
    projection_fingerprint: Value,
    observations: &BTreeMap<support::WorkforceV2ProofLane, Value>,
) -> Value {
    let catalog_bytes =
        fs::read(root.join(WORKFORCE_V2_CATALOG_PATH)).expect("workforce-v2 catalog is readable");
    let catalog: Value =
        serde_json::from_slice(&catalog_bytes).expect("workforce-v2 catalog is valid JSON");
    assert_eq!(catalog["format"], "typebridge.workforce-catalog/v2");
    assert_eq!(catalog["fixture"]["id"], "workforce-v2");
    assert_eq!(catalog["fixture"]["version"], 2);
    assert_eq!(
        catalog["fixture"]["semantic_profile"],
        support::TEST_PROFILE
    );
    assert_eq!(catalog["fixture"]["schema_path"], WORKFORCE_SCHEMA_PATH);
    assert_eq!(
        catalog["fixture"]["provider_schema_path"],
        WORKFORCE_PROVIDER_SCHEMA_PATH
    );
    assert_eq!(catalog["journey_path"], support::WORKFORCE_V2_JOURNEY);
    assert_eq!(catalog["projection_targets"]["c"], "c");
    assert_eq!(
        catalog["manifest_transition_cases"],
        serde_json::json!(WORKFORCE_V2_MANIFEST_TRANSITION_CASES),
        "workforce-v2 manifest transition ledger drifted"
    );
    assert_eq!(
        catalog["expected_fingerprints"]["semantic"], semantic_fingerprint,
        "C live semantic fingerprint differs from the catalog"
    );
    assert_eq!(
        catalog["expected_fingerprints"]["projections"]["c"], projection_fingerprint,
        "C live projection fingerprint differs from the catalog"
    );
    serde_json::json!({
        "binding": "c",
        "catalog": support::workforce_v2_source_identity(root, WORKFORCE_V2_CATALOG_PATH)
            .expect("workforce-v2 catalog identity is valid"),
        "fixture": {
            "id": "workforce-v2",
            "journey": support::workforce_v2_source_identity(root, support::WORKFORCE_V2_JOURNEY)
                .expect("workforce-v2 journey identity is valid"),
            "projection_fingerprint": projection_fingerprint,
            "projection_target": "c",
            "provider_schema": support::workforce_v2_source_identity(root, WORKFORCE_PROVIDER_SCHEMA_PATH)
                .expect("workforce-v2 provider schema identity is valid"),
            "schema": support::workforce_v2_source_identity(root, WORKFORCE_SCHEMA_PATH)
                .expect("workforce-v2 schema identity is valid"),
            "semantic_fingerprint": semantic_fingerprint,
            "semantic_profile": support::TEST_PROFILE,
            "version": 2,
        },
        "format": "typebridge.sdk-conformance-report/v2",
        "manifest": support::workforce_v2_source_identity(root, WORKFORCE_MANIFEST_PATH)
            .expect("workforce manifest identity is valid"),
        "results": workforce_v2_report_results(&catalog, observations),
    })
}

#[cfg(unix)]
fn publish_workforce_v2_report(
    path: &Path,
    root: &Path,
    semantic_fingerprint: Value,
    projection_fingerprint: Value,
    observations: &BTreeMap<support::WorkforceV2ProofLane, Value>,
) {
    let report = build_workforce_v2_report(
        root,
        semantic_fingerprint,
        projection_fingerprint,
        observations,
    );
    let mut bytes = to_canonical_json(&report).expect("C workforce-v2 report canonicalizes");
    bytes.push(b'\n');
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .expect("TYPE_BRIDGE_WORKFORCE_REPORT_V2 must name a UTF-8 file");
    let temporary = path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        TEMP_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    struct RemoveTemporary(PathBuf);
    impl Drop for RemoveTemporary {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let temporary_guard = RemoveTemporary(temporary.clone());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .expect("C workforce-v2 temporary report is created without replacement");
    file.write_all(&bytes)
        .expect("C workforce-v2 temporary report is written");
    file.sync_all()
        .expect("C workforce-v2 temporary report is synchronized");
    drop(file);
    fs::hard_link(&temporary, path)
        .expect("C workforce-v2 report is atomically published without replacement");
    fs::remove_file(&temporary).expect("C workforce-v2 temporary link is removed");
    drop(temporary_guard);
    let metadata = fs::symlink_metadata(path).expect("C workforce-v2 report is inspectable");
    assert!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "C workforce-v2 report must be a regular non-symlink file"
    );
}

#[cfg(unix)]
#[test]
fn workforce_v2_c_fact_fan_in_is_exact_canonical_and_closed() {
    use std::fmt::Write as _;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let root = repository_root();
    let fragment_lanes =
        support::workforce_v2_proof_lanes(&root, "c").expect("C proof allowlist validates");
    let selected_live_lanes = WORKFORCE_V2_SELECTED_PROOFS
        .into_iter()
        .map(|(_, proof_kind, observation_ref)| (observation_ref.to_owned(), proof_kind.to_owned()))
        .filter(|lane| !fragment_lanes.contains(lane))
        .collect::<Vec<_>>();
    assert_eq!(
        selected_live_lanes,
        WORKFORCE_V2_LIVE_LANES
            .into_iter()
            .map(|(observation_ref, proof_kind)| {
                (observation_ref.to_owned(), proof_kind.to_owned())
            })
            .collect::<Vec<_>>(),
        "C live facts plus committed fragments must partition the selected ledger"
    );
    let journey: Value = serde_json::from_slice(
        &fs::read(root.join(support::WORKFORCE_V2_JOURNEY))
            .expect("workforce-v2 journey is readable"),
    )
    .expect("workforce-v2 journey is valid JSON");
    let expected = journey["expected_observations"]
        .as_object()
        .expect("workforce-v2 expected observations are an object");
    let mut stdout = String::new();
    for (observation_ref, proof_kind) in WORKFORCE_V2_LIVE_LANES {
        let observation = expected
            .get(observation_ref)
            .unwrap_or_else(|| panic!("expected observation is absent: {observation_ref}"));
        let canonical = String::from_utf8(
            to_canonical_json(observation).expect("expected observation canonicalizes"),
        )
        .expect("canonical observation is UTF-8");
        writeln!(
            stdout,
            "{WORKFORCE_V2_FACT_PREFIX}{observation_ref}\t{proof_kind}\t{canonical}"
        )
        .expect("fact line formats");
    }
    assert_eq!(
        parse_workforce_v2_live_facts(&root, &stdout).len(),
        WORKFORCE_V2_LIVE_LANES.len()
    );

    let duplicated = format!(
        "{stdout}{}",
        stdout.lines().next().expect("one fact line exists")
    );
    assert!(
        catch_unwind(AssertUnwindSafe(|| parse_workforce_v2_live_facts(
            &root,
            &duplicated
        )))
        .is_err(),
        "duplicate fact lanes must fail closed"
    );

    let unexpected =
        format!("{stdout}{WORKFORCE_V2_FACT_PREFIX}not_allowed\tdirect_runtime\t{{}}\n");
    assert!(
        catch_unwind(AssertUnwindSafe(|| parse_workforce_v2_live_facts(
            &root,
            &unexpected
        )))
        .is_err(),
        "unexpected fact lanes must fail closed"
    );

    let canonical_entity = String::from_utf8(
        to_canonical_json(&expected["entity_lifecycle"]).expect("entity observation canonicalizes"),
    )
    .expect("canonical entity observation is UTF-8");
    let noncanonical_entity = canonical_entity.replacen(
        "{\"created\":true,\"deleted\":true",
        "{\"deleted\":true,\"created\":true",
        1,
    );
    assert_ne!(canonical_entity, noncanonical_entity);
    let noncanonical = stdout.replacen(&canonical_entity, &noncanonical_entity, 1);
    assert!(
        catch_unwind(AssertUnwindSafe(|| parse_workforce_v2_live_facts(
            &root,
            &noncanonical
        )))
        .is_err(),
        "noncanonical fact JSON must fail closed"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "requires a same-run deterministic C proof fragment"]
fn emitted_workforce_v2_c_proof_fragment_validates() {
    let root = repository_root();
    let fragments = env::var_os("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS")
        .expect("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS is required");
    let nonce = env::var("TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE")
        .expect("TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE is required");
    let paths = support::workforce_v2_proof_paths(&fragments)
        .expect("C workforce-v2 proof paths must be valid");
    let observations = support::validate_workforce_v2_proof_fragments(&root, "c", &nonce, &paths)
        .expect("emitted C workforce-v2 proof fragment must validate");
    assert_eq!(
        observations.keys().cloned().collect::<BTreeSet<_>>(),
        support::workforce_v2_proof_lanes(&root, "c").expect("C proof allowlist validates"),
    );
}

#[cfg(unix)]
#[test]
#[ignore = "requires a same-run deterministic C proof fragment"]
fn workforce_v2_c_catalog_report_builder_preflight_is_exact_and_nonpublishing() {
    use std::fmt::Write as _;

    assert!(
        env::var_os("TYPE_BRIDGE_WORKFORCE_REPORT_V2").is_none(),
        "provider-free report-builder preflight must not request publication"
    );
    let root = repository_root();
    let stage = TempDirectory::new();
    let report_path = stage.path().join("c.json");
    assert!(!report_path.exists());

    let fragments = env::var_os("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS")
        .expect("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS is required");
    let nonce = env::var("TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE")
        .expect("TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE is required");
    let paths = support::workforce_v2_proof_paths(&fragments)
        .expect("C workforce-v2 proof paths must be valid");
    let fragment_observations =
        support::validate_workforce_v2_proof_fragments(&root, "c", &nonce, &paths)
            .expect("emitted C workforce-v2 proof fragment must validate");
    assert_eq!(fragment_observations.len(), 3);

    let journey: Value = serde_json::from_slice(
        &fs::read(root.join(support::WORKFORCE_V2_JOURNEY))
            .expect("workforce-v2 journey is readable"),
    )
    .expect("workforce-v2 journey is valid JSON");
    let expected = journey["expected_observations"]
        .as_object()
        .expect("workforce-v2 expected observations are an object");
    let mut stdout = String::new();
    for (observation_ref, proof_kind) in WORKFORCE_V2_LIVE_LANES {
        let observation = expected
            .get(observation_ref)
            .unwrap_or_else(|| panic!("expected observation is absent: {observation_ref}"));
        let canonical = String::from_utf8(
            to_canonical_json(observation).expect("expected observation canonicalizes"),
        )
        .expect("canonical observation is UTF-8");
        writeln!(
            stdout,
            "{WORKFORCE_V2_FACT_PREFIX}{observation_ref}\t{proof_kind}\t{canonical}"
        )
        .expect("fact line formats");
    }
    let mut observations = parse_workforce_v2_live_facts(&root, &stdout);
    assert_eq!(observations.len(), 31);
    for (lane, observation) in fragment_observations {
        assert!(
            observations.insert(lane.clone(), observation).is_none(),
            "validated proof lane collides with a live lane: {lane:?}"
        );
    }
    assert_eq!(observations.len(), 34);
    assert_workforce_v2_observations_match_journey(&root, &observations);

    let (_, _, semantic_fingerprint, projection_fingerprint) =
        emitted_package_authority_and_fingerprints();
    let report = build_workforce_v2_report(
        &root,
        semantic_fingerprint.clone(),
        projection_fingerprint.clone(),
        &observations,
    );
    assert_eq!(report["binding"], "c");
    assert_eq!(report["fixture"]["projection_target"], "c");
    assert_eq!(
        report["fixture"]["semantic_fingerprint"],
        semantic_fingerprint
    );
    assert_eq!(
        report["fixture"]["projection_fingerprint"],
        projection_fingerprint
    );
    assert_eq!(
        report["catalog"],
        support::workforce_v2_source_identity(&root, WORKFORCE_V2_CATALOG_PATH)
            .expect("workforce-v2 catalog identity is valid")
    );
    assert_eq!(
        report["results"]
            .as_array()
            .expect("C workforce-v2 report results are an array")
            .len(),
        34
    );
    to_canonical_json(&report).expect("provider-free C workforce-v2 report canonicalizes");
    assert!(
        !report_path.exists(),
        "provider-free report-builder preflight must not publish a report"
    );
}

#[cfg(unix)]
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(third & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

#[cfg(unix)]
fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("loopback port allocation succeeds")
        .local_addr()
        .expect("loopback local address is readable")
        .port()
}

#[cfg(unix)]
struct RemoteServer {
    child: Child,
    log: PathBuf,
}

#[cfg(unix)]
impl RemoteServer {
    fn wait_until_ready(&mut self, port: u16) {
        let deadline = Instant::now() + Duration::from_secs(300);
        while Instant::now() < deadline {
            if let Some(status) = self
                .child
                .try_wait()
                .expect("C remote smoke-server status is readable")
            {
                panic!(
                    "C remote smoke server exited early with {status}\n{}",
                    fs::read_to_string(&self.log).unwrap_or_default()
                );
            }
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(200));
        }
        panic!(
            "C remote smoke server did not become reachable\n{}",
            fs::read_to_string(&self.log).unwrap_or_default()
        );
    }

    fn shutdown(mut self) {
        if self
            .child
            .try_wait()
            .expect("C remote smoke-server status is readable during cleanup")
            .is_none()
        {
            self.child
                .kill()
                .expect("C remote smoke server is terminated during cleanup");
        }
        self.child
            .wait()
            .expect("C remote smoke server is reaped during cleanup");
    }
}

#[cfg(unix)]
impl Drop for RemoteServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
struct IsolatedDatabase {
    cargo: OsString,
    manifest: PathBuf,
    target: PathBuf,
    environment: Vec<(String, String)>,
    active: bool,
}

#[cfg(unix)]
impl IsolatedDatabase {
    fn run(&self, mode: &str) -> Output {
        const EXECUTABLE_BUSY_RETRIES: u32 = 5;

        for retry in 0..=EXECUTABLE_BUSY_RETRIES {
            let mut command = Command::new(&self.cargo);
            command
                .args(["run", "--quiet", "--manifest-path"])
                .arg(&self.manifest)
                .arg("--")
                .arg(mode)
                .env("CARGO_TARGET_DIR", &self.target);
            for (name, value) in &self.environment {
                command.env(name, value);
            }
            match command.output() {
                Ok(output) => return output,
                Err(error)
                    if error.kind() == ErrorKind::ExecutableFileBusy
                        && retry < EXECUTABLE_BUSY_RETRIES =>
                {
                    thread::sleep(Duration::from_millis(10 * (1_u64 << retry)));
                }
                Err(error) => panic!("generated C database {mode} helper failed: {error}"),
            }
        }
        unreachable!("the executable-busy retry loop always returns or panics")
    }

    fn setup(&mut self) {
        let output = self.run("setup");
        let stdout = String::from_utf8_lossy(&output.stdout);
        // The helper flushes this marker immediately after creating the
        // caller-named absent database. It is deliberately absent when setup
        // rejects a pre-existing database, which must never be deleted here.
        self.active = stdout.contains(DATABASE_CREATED_MARKER);
        assert!(
            output.status.success(),
            "generated C isolated database setup failed:\nstdout:\n{}\nstderr:\n{}",
            stdout,
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(
            self.active,
            "setup omitted its flushed database-created marker"
        );
        assert!(stdout.contains("generated C provider schema setup: passed"));
    }

    fn cleanup(&mut self) -> Output {
        let output = self.cleanup_with_retries();
        if output.status.success() {
            self.active = false;
        }
        output
    }

    fn cleanup_with_retries(&self) -> Output {
        let mut output = self.run("cleanup");
        for _ in 1..3 {
            if output.status.success() {
                break;
            }
            output = self.run("cleanup");
        }
        output
    }
}

#[cfg(unix)]
impl Drop for IsolatedDatabase {
    fn drop(&mut self) {
        if self.active {
            let _ = self.cleanup_with_retries();
        }
    }
}

#[cfg(unix)]
#[test]
fn failed_live_setup_still_runs_idempotent_database_cleanup() {
    use std::os::unix::fs::PermissionsExt;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let stage = TempDirectory::new();
    let fake_cargo = stage.path().join("fake-cargo");
    let log = stage.path().join("modes.log");
    fs::write(
        &fake_cargo,
        "#!/bin/sh\nfor argument do mode=$argument; done\nprintf '%s\\n' \"$mode\" >> \"$TYPE_BRIDGE_C_PROJECTION_CLEANUP_LOG\"\nif test \"$mode\" = setup; then\n  if test \"${TYPE_BRIDGE_C_PROJECTION_EMIT_CREATED:-}\" = 1; then\n    printf '%s\\n' 'generated C isolated database created'\n  fi\n  exit 23\nfi\n",
    )
    .expect("fake setup command is written");
    let mut permissions = fs::metadata(&fake_cargo)
        .expect("fake setup command metadata is readable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_cargo, permissions).expect("fake setup command is executable");
    let busy_writer = OpenOptions::new()
        .write(true)
        .open(&fake_cargo)
        .expect("fake setup command can model a briefly busy executable");
    let release_busy_writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        drop(busy_writer);
    });

    let failure = catch_unwind(AssertUnwindSafe(|| {
        let mut isolated = IsolatedDatabase {
            cargo: fake_cargo.clone().into_os_string(),
            manifest: stage.path().join("unused-Cargo.toml"),
            target: stage.path().join("unused-target"),
            environment: vec![
                (
                    "TYPE_BRIDGE_C_PROJECTION_CLEANUP_LOG".to_owned(),
                    log.to_string_lossy().into_owned(),
                ),
                (
                    "TYPE_BRIDGE_C_PROJECTION_EMIT_CREATED".to_owned(),
                    "1".to_owned(),
                ),
            ],
            active: false,
        };
        isolated.setup();
    }));
    release_busy_writer
        .join()
        .expect("brief executable-busy writer is released");
    assert!(failure.is_err(), "the fake setup must fail closed");
    assert_eq!(
        fs::read_to_string(log).expect("setup and cleanup modes are logged"),
        "setup\ncleanup\n",
    );

    let before_create_log = stage.path().join("before-create-modes.log");
    let before_create_failure = catch_unwind(AssertUnwindSafe(|| {
        let mut isolated = IsolatedDatabase {
            cargo: fake_cargo.into_os_string(),
            manifest: stage.path().join("unused-before-create-Cargo.toml"),
            target: stage.path().join("unused-before-create-target"),
            environment: vec![(
                "TYPE_BRIDGE_C_PROJECTION_CLEANUP_LOG".to_owned(),
                before_create_log.to_string_lossy().into_owned(),
            )],
            active: false,
        };
        isolated.setup();
    }));
    assert!(
        before_create_failure.is_err(),
        "the fake pre-create setup must fail closed",
    );
    assert_eq!(
        fs::read_to_string(before_create_log).expect("pre-create setup mode is logged"),
        "setup\n",
        "a pre-existing database rejection must not trigger deletion",
    );
}

/// Exact C acceptance: ordinary generated C invokes nominal Person and
/// Membership CRUD, typed direct query, schema-function, and caller-transport
/// remote query facades against one isolated exact TypeDB 3.12.1 database.
/// Rust is used only for schema setup, the V2 acceptance server, and guaranteed
/// process/database cleanup; the C caller performs the sole remote exchange.
#[cfg(unix)]
#[test]
#[ignore = "requires an isolated exact TypeDB 3.12.1 server and C shared library"]
fn live_c17_generated_person_and_membership_crud_round_trips_exact_3_12_1() {
    let repository_root = repository_root();
    let workforce_v2_report = requested_workforce_v2_report();
    let workforce_v2_proofs =
        validated_workforce_v2_proofs(&repository_root, workforce_v2_report.as_deref());
    let compiler = c_compilers()
        .into_iter()
        .next()
        .expect("GCC or Clang is required for exact generated C acceptance");
    let native_library = native_library();
    let address = required_live_environment("TYPEDB_ADDRESS");
    let http_port = required_live_environment("TYPEDB_HTTP_PORT");
    http_port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .expect("TYPEDB_HTTP_PORT must be an integer from 1 through 65535");
    let database = required_live_environment("TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE");
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());

    let stage = TempDirectory::new();
    let (package, authority_bytes, semantic_fingerprint, projection_fingerprint) =
        emitted_package_authority_and_fingerprints();
    write_package(&package, stage.path());
    let consumer = stage.path().join("consumer.c");
    fs::write(&consumer, CONSUMER).expect("exact generated C consumer is staged");
    let executable = stage.path().join("generated-c-projected-crud-live");
    let native_directory = native_library
        .parent()
        .expect("native C library has a parent directory");
    let compile = Command::new(compiler)
        .args([
            "-std=c17",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
        ])
        .arg("-I")
        .arg(runtime_include())
        .arg("-I")
        .arg(stage.path().join("include"))
        .arg(stage.path().join("src/models.c"))
        .arg(&consumer)
        .arg("-L")
        .arg(native_directory)
        .arg("-ltype_bridge_c")
        .arg(format!("-Wl,-rpath,{}", native_directory.display()))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
    assert!(
        compile.status.success(),
        "{compiler} failed to compile/link exact generated C acceptance:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );

    let setup_root = stage.path().join("setup");
    fs::create_dir_all(setup_root.join("c_projection_live"))
        .expect("setup source directory is created");
    fs::create_dir_all(setup_root.join("acceptance"))
        .expect("shared acceptance fixture directory is created");
    fs::write(setup_root.join("c_projection_live/setup.rs"), SETUP)
        .expect("Rust-only database setup source is staged");
    fs::write(
        setup_root.join("acceptance/provider-3.12.1.tql"),
        PROVIDER_SCHEMA,
    )
    .expect("shared exact provider schema is staged");
    let orm = Path::new(env!("CARGO_MANIFEST_DIR")).join("../orm");
    let manifest = setup_root.join("Cargo.toml");
    fs::write(
        &manifest,
        format!(
            "[package]\nname = \"type-bridge-c-projection-live-setup\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[[bin]]\nname = \"setup\"\npath = \"c_projection_live/setup.rs\"\n\n[dependencies]\ntype-bridge-orm = {{ path = \"{}\" }}\ntokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}\n\n[workspace]\n",
            manifest_path(&orm),
        ),
    )
    .expect("Rust-only setup manifest is staged");
    let target = env::var_os("ACCEPTANCE_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| stage.path().join("target"));
    let environment = vec![
        ("TYPEDB_ADDRESS".to_owned(), address.clone()),
        ("TYPEDB_HTTP_PORT".to_owned(), http_port.clone()),
        (
            "TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE".to_owned(),
            database.clone(),
        ),
        ("TYPEDB_USERNAME".to_owned(), username.clone()),
        ("TYPEDB_PASSWORD".to_owned(), password.clone()),
    ];
    let mut isolated = IsolatedDatabase {
        cargo: env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")),
        manifest,
        target,
        environment: environment.clone(),
        active: false,
    };
    isolated.setup();

    let remote_port = free_port();
    let server_log_path = stage.path().join("v2-smoke-server.log");
    let server_log = File::create(&server_log_path).expect("C V2 server log is created");
    let server_error_log = server_log
        .try_clone()
        .expect("C V2 server log handle is cloned");
    let core = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("schema-codegen has a core workspace parent");
    let mut server_command = if let Some(server) = env::var_os("TYPE_BRIDGE_V2_SMOKE_SERVER") {
        let server = PathBuf::from(server)
            .canonicalize()
            .expect("TYPE_BRIDGE_V2_SMOKE_SERVER is resolvable");
        assert!(
            server.is_file() && !server.is_symlink(),
            "TYPE_BRIDGE_V2_SMOKE_SERVER must be a regular non-symlink file"
        );
        Command::new(server)
    } else {
        let mut command = Command::new(&isolated.cargo);
        command
            .args([
                "run",
                "--quiet",
                "-p",
                "type-bridge-server",
                "--features",
                "v2-query",
                "--example",
                "v2_smoke_server",
            ])
            .current_dir(core)
            .env("CARGO_TARGET_DIR", &isolated.target);
        command
    };
    server_command
        .env("SMOKE_TYPEDB_ADDRESS", &address)
        .env("SMOKE_TYPEDB_USERNAME", &username)
        .env("SMOKE_TYPEDB_PASSWORD", &password)
        .env("SMOKE_TYPEDB_HTTP_PORT", &http_port)
        .env("SMOKE_DATABASE", &database)
        .env("SMOKE_AUTHORITY_B64", base64(&authority_bytes))
        .env("SMOKE_PORT", remote_port.to_string())
        .stdout(Stdio::from(server_log))
        .stderr(Stdio::from(server_error_log));
    let mut remote_server = RemoteServer {
        child: server_command
            .spawn()
            .expect("C V2 caller-transport smoke server starts"),
        log: server_log_path,
    };
    remote_server.wait_until_ready(remote_port);

    let mut run = Command::new(&executable);
    for (name, value) in &environment {
        run.env(name, value);
    }
    run.env_remove("TYPE_BRIDGE_WORKFORCE_REPORT_V2")
        .env_remove("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT")
        .env_remove("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS")
        .env_remove("TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE");
    run.env("TYPE_BRIDGE_C_REMOTE_PORT", remote_port.to_string());
    let output = run
        .output()
        .expect("exact generated C projected CRUD consumer launches");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "exact generated C projected CRUD consumer failed with {}:\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout,
        String::from_utf8_lossy(&output.stderr),
    );
    let markers = include_str!("c_projection_live/consumer.c")
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("puts(\"")
                .and_then(|value| value.strip_suffix("\");"))
                .filter(|value| value.ends_with(": passed"))
        })
        .collect::<Vec<_>>();
    assert_eq!(markers.len(), 61, "live C marker inventory drifted");
    assert_eq!(
        markers
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        markers.len(),
        "live C markers must be unique",
    );
    for marker in markers {
        assert!(stdout.contains(marker), "consumer output omitted {marker}");
    }
    let mut workforce_v2_observations = parse_workforce_v2_live_facts(&repository_root, &stdout);
    if workforce_v2_report.is_some() {
        for (lane, observation) in workforce_v2_proofs {
            assert!(
                workforce_v2_observations
                    .insert(lane.clone(), observation)
                    .is_none(),
                "validated proof lane collides with a live lane: {lane:?}"
            );
        }
        assert_eq!(
            workforce_v2_observations.len(),
            34,
            "C workforce-v2 producer requires exactly 34 observation lanes"
        );
        assert_workforce_v2_observations_match_journey(
            &repository_root,
            &workforce_v2_observations,
        );
    } else {
        assert!(
            workforce_v2_proofs.is_empty(),
            "proof observations are forbidden without a report request"
        );
    }

    let cleanup = isolated.cleanup();
    assert!(
        cleanup.status.success()
            && String::from_utf8_lossy(&cleanup.stdout)
                .contains("generated C isolated database cleanup: passed"),
        "generated C isolated database cleanup failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&cleanup.stdout),
        String::from_utf8_lossy(&cleanup.stderr),
    );
    remote_server.shutdown();
    if let Some(report) = workforce_v2_report {
        publish_workforce_v2_report(
            &report,
            &repository_root,
            semantic_fingerprint,
            projection_fingerprint,
            &workforce_v2_observations,
        );
    }
}
