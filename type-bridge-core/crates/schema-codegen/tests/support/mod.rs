#![allow(dead_code)]

#[path = "../../../../../tests/support/rust_locks.rs"]
pub mod locks;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet, VerifiedSchemaAuthority,
    build_schema_authority, normalize_documents,
};

pub const TEST_PROFILE: &str = "typedb-3.12.1/v1";
pub const TEST_SCOPE: &str = "schema-codegen-acceptance";
pub const SDK_V2_PROOF_SCHEMA: &str =
    "tests/contracts/sdk_conformance/sdk-v2/proof-fragment-schema-v1.json";
pub const SDK_V2_PROOF_ALLOWLIST: &str =
    "tests/contracts/sdk_conformance/sdk-v2/proof-fragment-allowlist-v1.json";
pub const SDK_V2_JOURNEY: &str = "tests/contracts/sdk_conformance/sdk-v2/journey-v2.json";

const SDK_V2_PROOF_FORMAT: &str = "typebridge.sdk-v2-proof-fragment/v1";
const SDK_V2_ALLOWLIST_FORMAT: &str = "typebridge.sdk-v2-proof-fragment-allowlist/v1";
const SDK_V2_PROOF_MAX_BYTES: u64 = 64 * 1024;
const SDK_V2_PROOF_MAX_RESULTS: usize = 16;
const SDK_V2_PROOF_MAX_SOURCES: usize = 16;

pub type SdkV2ProofLane = (String, String);

#[derive(Clone, Debug, Eq, PartialEq)]
struct SdkV2ProofProducerAuthority {
    sources: Vec<String>,
    results: BTreeMap<SdkV2ProofLane, String>,
}

fn proof_object<'a>(
    value: &'a Value,
    expected: &[&str],
    context: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object"))?;
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "{context} fields differ: expected {expected:?}, got {actual:?}"
        ));
    }
    Ok(object)
}

fn proof_string<'a>(value: &'a Value, context: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{context} must be nonempty text"))
}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_identifier(value: &str, first: impl Fn(u8) -> bool, rest: impl Fn(u8) -> bool) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(&first) && bytes.all(rest)
}

fn proof_relative_path(value: &str, context: &str) -> Result<PathBuf, String> {
    if value.is_empty()
        || value.len() > 512
        || value.contains('\\')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
    {
        return Err(format!("{context} is not a portable source path"));
    }
    let path = PathBuf::from(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path.to_string_lossy() != value
    {
        return Err(format!("{context} is not a canonical relative source path"));
    }
    Ok(path)
}

fn proof_regular_source(root: &Path, relative: &str, context: &str) -> Result<Vec<u8>, String> {
    let relative_path = proof_relative_path(relative, context)?;
    let mut current = root.to_path_buf();
    for component in relative_path.components() {
        let Component::Normal(component) = component else {
            return Err(format!("{context} is not a canonical relative source path"));
        };
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| format!("{context} is not inspectable: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("{context} traverses a symlink"));
        }
    }
    let metadata = fs::metadata(&current)
        .map_err(|error| format!("{context} metadata is not readable: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{context} must be a regular file"));
    }
    fs::read(&current).map_err(|error| format!("{context} is not readable: {error}"))
}

pub fn sdk_v2_source_identity(root: &Path, relative: &str) -> Result<Value, String> {
    let bytes = proof_regular_source(root, relative, "proof source")?;
    Ok(serde_json::json!({
        "path": relative,
        "sha256": format!("{:x}", Sha256::digest(bytes)),
    }))
}

fn validate_proof_source_identity(
    root: &Path,
    value: &Value,
    context: &str,
) -> Result<Value, String> {
    let identity = proof_object(value, &["path", "sha256"], context)?;
    let relative = proof_string(&identity["path"], &format!("{context}.path"))?;
    let digest = proof_string(&identity["sha256"], &format!("{context}.sha256"))?;
    if !is_lower_hex_64(digest) {
        return Err(format!("{context}.sha256 is not lowercase SHA-256"));
    }
    let expected = sdk_v2_source_identity(root, relative)?;
    if value != &expected {
        return Err(format!("{context} digest does not match {relative}"));
    }
    Ok(expected)
}

fn proof_lane(value: &Value, context: &str) -> Result<SdkV2ProofLane, String> {
    let lane = proof_object(value, &["observation_ref", "proof_kind"], context)?;
    let observation_ref = proof_string(&lane["observation_ref"], context)?;
    let proof_kind = proof_string(&lane["proof_kind"], context)?;
    if !is_identifier(
        observation_ref,
        |byte| byte.is_ascii_lowercase(),
        |byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_',
    ) || !matches!(
        proof_kind,
        "direct_runtime" | "remote_runtime" | "diagnostic" | "lifecycle"
    ) {
        return Err(format!("{context} is not a valid observation lane"));
    }
    Ok((observation_ref.to_owned(), proof_kind.to_owned()))
}

fn sdk_v2_proof_authority(
    root: &Path,
    binding: &str,
) -> Result<BTreeMap<String, SdkV2ProofProducerAuthority>, String> {
    if !matches!(binding, "python" | "node" | "rust" | "c") {
        return Err(format!("unknown sdk-v2 binding {binding:?}"));
    }
    let raw = proof_regular_source(root, SDK_V2_PROOF_ALLOWLIST, "sdk-v2 proof allowlist")?;
    let value: Value = serde_json::from_slice(&raw)
        .map_err(|error| format!("sdk-v2 proof allowlist is invalid JSON: {error}"))?;
    let allowlist = proof_object(
        &value,
        &["format", "semantic_profile", "bindings"],
        "allowlist",
    )?;
    if proof_string(&allowlist["format"], "allowlist format")? != SDK_V2_ALLOWLIST_FORMAT
        || proof_string(&allowlist["semantic_profile"], "allowlist profile")? != TEST_PROFILE
    {
        return Err("sdk-v2 proof allowlist authority is not v1".to_owned());
    }
    let bindings = proof_object(
        &allowlist["bindings"],
        &["python", "node", "rust", "c"],
        "allowlist bindings",
    )?;
    let producers = bindings[binding]
        .as_array()
        .filter(|values| !values.is_empty())
        .ok_or_else(|| format!("{binding} proof allowlist is empty or malformed"))?;
    let mut authority = BTreeMap::new();
    let mut ordered_producers = Vec::with_capacity(producers.len());
    let mut claimed_lanes = BTreeSet::new();
    for (producer_index, producer) in producers.iter().enumerate() {
        let producer = proof_object(
            producer,
            &["id", "results", "sources"],
            &format!("allowlist producer[{producer_index}]"),
        )?;
        let producer_id = proof_string(
            &producer["id"],
            &format!("allowlist producer[{producer_index}].id"),
        )?;
        if producer_id.len() > 128
            || !is_identifier(
                producer_id,
                |byte| byte.is_ascii_lowercase() || byte.is_ascii_digit(),
                |byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'.' | b'_' | b'-')
                },
            )
        {
            return Err("sdk-v2 proof allowlist producer ID is invalid".to_owned());
        }
        let sources = producer["sources"]
            .as_array()
            .filter(|sources| !sources.is_empty() && sources.len() <= SDK_V2_PROOF_MAX_SOURCES)
            .ok_or_else(|| "proof allowlist producer must bind 1..16 sources".to_owned())?;
        let mut source_paths = Vec::with_capacity(sources.len());
        for (source_index, source) in sources.iter().enumerate() {
            let source = proof_string(
                source,
                &format!("allowlist producer[{producer_index}].sources[{source_index}]"),
            )?;
            proof_relative_path(source, "proof allowlist producer source")?;
            source_paths.push(source.to_owned());
        }
        if !source_paths.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err(
                "proof allowlist producer sources are not unique and path-sorted".to_owned(),
            );
        }

        let results = producer["results"]
            .as_array()
            .filter(|results| !results.is_empty() && results.len() <= SDK_V2_PROOF_MAX_RESULTS)
            .ok_or_else(|| "proof allowlist producer must own 1..16 results".to_owned())?;
        let mut result_authority = BTreeMap::new();
        let mut ordered_lanes = Vec::with_capacity(results.len());
        for (result_index, result) in results.iter().enumerate() {
            let result = proof_object(
                result,
                &["observation_ref", "proof_kind", "test_id"],
                &format!("allowlist producer[{producer_index}].results[{result_index}]"),
            )?;
            let lane = proof_lane(
                &serde_json::json!({
                    "observation_ref": result["observation_ref"],
                    "proof_kind": result["proof_kind"],
                }),
                &format!("allowlist producer[{producer_index}].results[{result_index}]"),
            )?;
            let test_id = proof_string(
                &result["test_id"],
                &format!("allowlist producer[{producer_index}].results[{result_index}].test_id"),
            )?;
            if test_id.len() > 192
                || !is_identifier(
                    test_id,
                    |byte| byte.is_ascii_lowercase() || byte.is_ascii_digit(),
                    |byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'.' | b'_' | b':' | b'-')
                    },
                )
            {
                return Err("sdk-v2 proof allowlist test ID is invalid".to_owned());
            }
            if !claimed_lanes.insert(lane.clone()) {
                return Err("sdk-v2 proof allowlist lane is claimed twice".to_owned());
            }
            result_authority.insert(lane.clone(), test_id.to_owned());
            ordered_lanes.push(lane);
        }
        if !ordered_lanes.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err("proof allowlist producer results are not unique and sorted".to_owned());
        }
        if authority
            .insert(
                producer_id.to_owned(),
                SdkV2ProofProducerAuthority {
                    sources: source_paths,
                    results: result_authority,
                },
            )
            .is_some()
        {
            return Err("sdk-v2 proof allowlist producer ID is duplicated".to_owned());
        }
        ordered_producers.push(producer_id.to_owned());
    }
    if !ordered_producers.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(format!(
            "{binding} proof allowlist producers are not unique and sorted"
        ));
    }
    Ok(authority)
}

pub fn sdk_v2_proof_lanes(root: &Path, binding: &str) -> Result<BTreeSet<SdkV2ProofLane>, String> {
    Ok(sdk_v2_proof_authority(root, binding)?
        .values()
        .flat_map(|producer| producer.results.keys().cloned())
        .collect())
}

fn load_proof_fragment(path: &Path) -> Result<Value, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!(
            "proof fragment {} must use a normalized absolute path",
            path.display()
        ));
    }
    let mut chain = path.ancestors().collect::<Vec<_>>();
    chain.reverse();
    let mut metadata = None;
    for component in chain {
        let component_metadata = fs::symlink_metadata(component).map_err(|error| {
            format!(
                "proof fragment {} is not inspectable: {error}",
                path.display()
            )
        })?;
        if component_metadata.file_type().is_symlink() {
            return Err(format!(
                "proof fragment {} traverses a symlink",
                path.display()
            ));
        }
        metadata = Some(component_metadata);
    }
    let metadata =
        metadata.ok_or_else(|| format!("proof fragment {} is not inspectable", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "proof fragment {} must be a regular non-symlink file",
            path.display()
        ));
    }
    if metadata.len() > SDK_V2_PROOF_MAX_BYTES {
        return Err(format!(
            "proof fragment {} exceeds {} bytes",
            path.display(),
            SDK_V2_PROOF_MAX_BYTES
        ));
    }
    let raw = fs::read(path)
        .map_err(|error| format!("proof fragment {} is not readable: {error}", path.display()))?;
    let value: Value = serde_json::from_slice(&raw)
        .map_err(|error| format!("proof fragment {} is invalid JSON: {error}", path.display()))?;
    let mut canonical = to_canonical_json(&value)
        .map_err(|_| "proof fragment cannot be canonicalized".to_owned())?;
    canonical.push(b'\n');
    if raw != canonical {
        return Err(format!(
            "proof fragment {} is not compact canonical JSON plus one LF",
            path.display()
        ));
    }
    Ok(value)
}

pub fn sdk_v2_proof_paths(value: &std::ffi::OsStr) -> Result<Vec<PathBuf>, String> {
    let paths = std::env::split_paths(value).collect::<Vec<_>>();
    if paths.is_empty() || paths.iter().any(|path| path.as_os_str().is_empty()) {
        return Err("sdk-v2 proof fragment path list is empty or malformed".to_owned());
    }
    Ok(paths)
}

pub fn validate_sdk_v2_proof_fragments(
    root: &Path,
    binding: &str,
    run_nonce: &str,
    fragment_paths: &[PathBuf],
) -> Result<BTreeMap<SdkV2ProofLane, Value>, String> {
    if !is_lower_hex_64(run_nonce) {
        return Err("sdk-v2 proof run nonce must be 64 lowercase hex characters".to_owned());
    }
    if fragment_paths.is_empty() {
        return Err("at least one sdk-v2 proof fragment is required".to_owned());
    }
    let producer_authority = sdk_v2_proof_authority(root, binding)?;
    let allowed = producer_authority
        .values()
        .flat_map(|producer| producer.results.keys().cloned())
        .collect::<BTreeSet<_>>();
    let expected_contract = serde_json::json!({
        "allowlist": sdk_v2_source_identity(root, SDK_V2_PROOF_ALLOWLIST)?,
        "journey": sdk_v2_source_identity(root, SDK_V2_JOURNEY)?,
        "proof_schema": sdk_v2_source_identity(root, SDK_V2_PROOF_SCHEMA)?,
    });
    let mut normalized_paths = BTreeSet::new();
    let mut producer_ids = BTreeSet::new();
    let mut observations = BTreeMap::new();

    for path in fragment_paths {
        let normalized = path.canonicalize().map_err(|error| {
            format!(
                "proof fragment {} cannot be resolved: {error}",
                path.display()
            )
        })?;
        if !normalized_paths.insert(normalized) {
            return Err("one sdk-v2 proof fragment path was supplied twice".to_owned());
        }
        let fragment = load_proof_fragment(path)?;
        let fragment = proof_object(
            &fragment,
            &[
                "format",
                "binding",
                "semantic_profile",
                "run_nonce",
                "contract",
                "producer",
                "results",
            ],
            "proof fragment",
        )?;
        if proof_string(&fragment["format"], "proof fragment format")? != SDK_V2_PROOF_FORMAT
            || proof_string(&fragment["semantic_profile"], "proof fragment profile")?
                != TEST_PROFILE
        {
            return Err("proof fragment format/profile is not sdk-v2 v1".to_owned());
        }
        if proof_string(&fragment["binding"], "proof fragment binding")? != binding {
            return Err("proof fragment belongs to another binding".to_owned());
        }
        if proof_string(&fragment["run_nonce"], "proof fragment run nonce")? != run_nonce {
            return Err("proof fragment belongs to another run".to_owned());
        }
        let contract = proof_object(
            &fragment["contract"],
            &["allowlist", "journey", "proof_schema"],
            "proof fragment contract",
        )?;
        for (name, identity) in contract {
            validate_proof_source_identity(root, identity, &format!("contract.{name}"))?;
        }
        if fragment["contract"] != expected_contract {
            return Err("proof fragment binds another sdk-v2 contract".to_owned());
        }

        let producer = proof_object(&fragment["producer"], &["id", "sources"], "producer")?;
        let producer_id = proof_string(&producer["id"], "producer.id")?;
        if producer_id.len() > 128
            || !is_identifier(
                producer_id,
                |byte| byte.is_ascii_lowercase() || byte.is_ascii_digit(),
                |byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'.' | b'_' | b'-')
                },
            )
        {
            return Err("proof fragment producer ID is invalid".to_owned());
        }
        if !producer_ids.insert(producer_id.to_owned()) {
            return Err(format!("proof producer {producer_id:?} emitted twice"));
        }
        let authority = producer_authority
            .get(producer_id)
            .ok_or_else(|| format!("proof producer {producer_id:?} is not committed"))?;
        let sources = producer["sources"]
            .as_array()
            .filter(|sources| !sources.is_empty() && sources.len() <= SDK_V2_PROOF_MAX_SOURCES)
            .ok_or_else(|| "proof producer must bind 1..16 sources".to_owned())?;
        let mut source_paths = Vec::with_capacity(sources.len());
        for (index, source) in sources.iter().enumerate() {
            let validated = validate_proof_source_identity(
                root,
                source,
                &format!("producer.sources[{index}]"),
            )?;
            source_paths.push(proof_string(&validated["path"], "producer source path")?.to_owned());
        }
        if !source_paths.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err("proof producer sources are not unique and path-sorted".to_owned());
        }
        if source_paths != authority.sources {
            return Err(format!(
                "proof producer {producer_id:?} source paths differ from committed authority"
            ));
        }

        let results = fragment["results"]
            .as_array()
            .filter(|results| !results.is_empty() && results.len() <= SDK_V2_PROOF_MAX_RESULTS)
            .ok_or_else(|| "proof fragment must carry 1..16 results".to_owned())?;
        let mut result_lanes = Vec::with_capacity(results.len());
        for (index, result) in results.iter().enumerate() {
            let result = proof_object(
                result,
                &[
                    "observation_ref",
                    "proof_kind",
                    "test_id",
                    "outcome",
                    "observation",
                ],
                &format!("results[{index}]"),
            )?;
            let lane = proof_lane(
                &serde_json::json!({
                    "observation_ref": result["observation_ref"],
                    "proof_kind": result["proof_kind"],
                }),
                &format!("results[{index}]"),
            )?;
            let test_id = proof_string(&result["test_id"], &format!("results[{index}].test_id"))?;
            if test_id.len() > 192
                || !is_identifier(
                    test_id,
                    |byte| byte.is_ascii_lowercase() || byte.is_ascii_digit(),
                    |byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'.' | b'_' | b':' | b'-')
                    },
                )
            {
                return Err(format!("results[{index}] test ID is invalid"));
            }
            if proof_string(&result["outcome"], &format!("results[{index}].outcome"))? != "passed"
                || !result["observation"].is_object()
            {
                return Err(format!(
                    "results[{index}] is not a passed object observation"
                ));
            }
            if !allowed.contains(&lane) {
                return Err(format!("proof fragment carries unexpected lane {lane:?}"));
            }
            let expected_test_id = authority.results.get(&lane).ok_or_else(|| {
                format!("proof producer {producer_id:?} does not own lane {lane:?}")
            })?;
            if test_id != expected_test_id {
                return Err(format!(
                    "proof producer {producer_id:?} lane {lane:?} has the wrong test ID"
                ));
            }
            if observations
                .insert(lane.clone(), result["observation"].clone())
                .is_some()
            {
                return Err(format!("proof lane {lane:?} was emitted twice"));
            }
            result_lanes.push(lane);
        }
        if !result_lanes.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err("proof fragment results are not unique and lane-sorted".to_owned());
        }
        if result_lanes.iter().cloned().collect::<BTreeSet<_>>()
            != authority.results.keys().cloned().collect()
        {
            return Err(format!(
                "proof producer {producer_id:?} did not emit its exact committed lanes"
            ));
        }
    }
    if observations.keys().cloned().collect::<BTreeSet<_>>() != allowed {
        return Err("proof fragment lane coverage differs from committed allowlist".to_owned());
    }
    if producer_ids != producer_authority.keys().cloned().collect() {
        return Err("proof fragments did not cover every committed producer".to_owned());
    }
    Ok(observations)
}

pub fn declared(source: &str) -> DeclaredSchema {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema-codegen-authority.yaml").expect("test document ID"),
        source,
    )])
    .expect("test authority source parses");
    normalize_documents(&documents).expect("test authority source normalizes")
}

pub fn authority(source: &str) -> VerifiedSchemaAuthority {
    authority_with(source, TEST_SCOPE, TEST_PROFILE)
}

pub fn authority_with(source: &str, scope: &str, profile: &str) -> VerifiedSchemaAuthority {
    let declared = declared(source);
    authority_for_declared(&declared, scope, profile)
}

pub fn authority_for_declared(
    declared: &DeclaredSchema,
    scope: &str,
    profile: &str,
) -> VerifiedSchemaAuthority {
    let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|id| CapabilityId::new(*id).expect("built-in capability ID"))
        .collect();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new(scope).expect("test managed scope"),
        SemanticProfileId::new(profile).expect("test semantic profile"),
        available,
    );
    build_schema_authority(declared, declared.required_capabilities(), &context)
        .expect("test schema authority builds")
}
