use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

const BASE_HEADER_SNAPSHOT_SHA256: &str =
    "40944d92b0ff41b77c122dd6d9aa8555728d4d4409379e1278d20692b97de2fa";
const SCHEMA_OPEN_V2: &str = "type_bridge_schema_package_open_v2";
const SCHEMA_OPEN_CHUNKED_V2: &str = "type_bridge_schema_package_open_chunked_v2";

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "typebridge-c-abi-1-4-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("unique ABI 1.4 test directory is created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("ABI 1.4 test directory is removed");
    }
}

fn macro_names(header: &str) -> BTreeSet<String> {
    header
        .lines()
        .filter_map(|line| line.strip_prefix("#define "))
        .filter_map(|definition| definition.split_ascii_whitespace().next())
        .map(|name| name.split_once('(').map_or(name, |(name, _)| name))
        .map(str::to_owned)
        .collect()
}

fn declaration_names(header: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (offset, _) in header.match_indices("type_bridge_") {
        let candidate = &header[offset..];
        let length = candidate
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            .count();
        if candidate[length..].trim_start().starts_with('(') {
            names.insert(candidate[..length].to_owned());
        }
    }
    names
}

fn rust_string_array(source: &str, name: &str) -> Vec<String> {
    let marker = format!("const {name}:");
    let declaration = source
        .split_once(&marker)
        .unwrap_or_else(|| panic!("missing exact export ledger {name}"))
        .1;
    let body = declaration
        .split_once("= [")
        .unwrap_or_else(|| panic!("malformed exact export ledger {name}"))
        .1
        .split_once("\n];")
        .unwrap_or_else(|| panic!("unterminated exact export ledger {name}"))
        .0;
    body.lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix('"')
                .and_then(|line| line.strip_suffix("\","))
                .map(str::to_owned)
        })
        .collect()
}

fn exact_ledgers() -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
    let source = include_str!("schema_package_abi.rs");
    let abi_1_2 = rust_string_array(source, "ABI_1_2_EXPORTED_SYMBOLS")
        .into_iter()
        .collect();
    let abi_1_3 = rust_string_array(source, "ABI_1_3_EXPORTED_SYMBOLS")
        .into_iter()
        .collect();
    let additions = rust_string_array(source, "ABI_1_4_ADDED_EXPORTED_SYMBOLS")
        .into_iter()
        .collect();
    (abi_1_2, abi_1_3, additions)
}

fn command_exists(command: &str) -> bool {
    Command::new(command).arg("--version").output().is_ok()
}

fn assert_command_succeeded(output: &Output, description: &str) {
    assert!(
        output.status.success(),
        "{description} failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn shared_consumer_required() -> bool {
    match std::env::var("TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER") {
        Err(std::env::VarError::NotPresent) => false,
        Ok(value) if value == "1" => true,
        Ok(value) => panic!(
            "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER must be unset or exactly `1`, got {value:?}"
        ),
        Err(error) => panic!("TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER is not Unicode: {error}"),
    }
}

fn native_library() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("TYPE_BRIDGE_C_SHARED_LIBRARY") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "explicit TypeBridge C shared library is absent"
        );
        return Some(path);
    }
    let executable = std::env::current_exe().expect("current test executable path is available");
    let dependency_directory = executable
        .parent()
        .expect("test executable has a parent directory");
    let profile_directory = dependency_directory
        .parent()
        .expect("test dependency directory has a profile parent");
    let filename = format!(
        "{}type_bridge_c{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    [
        dependency_directory.join(&filename),
        profile_directory.join(filename),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

#[cfg(target_os = "linux")]
fn shared_library_exports(library: &Path) -> BTreeSet<String> {
    let output = Command::new("nm")
        .args(["-D", "--defined-only", "--format=posix"])
        .arg(library)
        .output()
        .expect("nm is required for the Linux C ABI export audit");
    assert_command_succeeded(&output, "Linux C ABI export inventory");
    String::from_utf8(output.stdout)
        .expect("nm output is UTF-8")
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[cfg(target_os = "macos")]
fn shared_library_exports(library: &Path) -> BTreeSet<String> {
    let output = Command::new("nm")
        .args(["-gjU"])
        .arg(library)
        .output()
        .expect("nm is required for the macOS C ABI export audit");
    assert_command_succeeded(&output, "macOS C ABI export inventory");
    String::from_utf8(output.stdout)
        .expect("nm output is UTF-8")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.strip_prefix('_').unwrap_or(line).to_owned())
        .collect()
}

#[cfg(windows)]
fn shared_library_exports(library: &Path) -> BTreeSet<String> {
    let output = Command::new("dumpbin")
        .args(["/nologo", "/exports"])
        .arg(library)
        .output()
        .expect("dumpbin is required for the Windows C ABI export audit");
    assert_command_succeeded(&output, "Windows C ABI export inventory");
    String::from_utf8(output.stdout)
        .expect("dumpbin output is UTF-8")
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 4
                || fields[0].parse::<u32>().is_err()
                || !fields[1].bytes().all(|byte| byte.is_ascii_hexdigit())
                || !fields[2].bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return None;
            }
            let mut name = fields[3].trim_start_matches('_');
            if let Some((base, suffix)) = name.rsplit_once('@')
                && suffix.bytes().all(|byte| byte.is_ascii_digit())
            {
                name = base;
            }
            Some(name.to_owned())
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn shared_library_exports(_library: &Path) -> BTreeSet<String> {
    panic!("the shared-library ABI export audit is unsupported on this target")
}

#[test]
fn base_header_is_byte_frozen_except_for_the_abi_minor_value() {
    let header = include_str!("../include/typebridge/type_bridge.h");
    let current = "#define TYPE_BRIDGE_C_ABI_MINOR 6u";
    let predecessor = "#define TYPE_BRIDGE_C_ABI_MINOR 3u";
    assert_eq!(header.matches(current).count(), 1);
    let normalized = header.replacen(current, predecessor, 1);
    let digest = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    assert_eq!(digest, BASE_HEADER_SNAPSHOT_SHA256);
    assert_eq!(declaration_names(header).len(), 180);
    assert_eq!(macro_names(header).len(), 205);
}

#[test]
fn aggregate_header_is_the_exact_109_subset_180_subset_225_surface() {
    let base = include_str!("../include/typebridge/type_bridge.h");
    let extension = include_str!("../include/typebridge/type_bridge_abi_1_4.h");
    let (abi_1_2, abi_1_3, additions) = exact_ledgers();
    assert_eq!(abi_1_2.len(), 109, "ABI 1.2 ledger has duplicates");
    assert_eq!(abi_1_3.len(), 180, "ABI 1.3 ledger has duplicates");
    assert_eq!(additions.len(), 45, "ABI 1.4 ledger has duplicates");
    assert!(abi_1_2.is_subset(&abi_1_3));
    assert!(abi_1_3.is_disjoint(&additions));

    let base_declarations = declaration_names(base);
    let extension_declarations = declaration_names(extension);
    assert_eq!(base_declarations, abi_1_3);
    assert_eq!(extension_declarations, additions);
    let aggregate = base_declarations
        .union(&extension_declarations)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(aggregate.len(), 225);
    assert!(abi_1_2.is_subset(&aggregate));

    let expected_macros = [
        "TYPE_BRIDGE_DATABASE_CONFIG_V2_VERSION",
        "TYPE_BRIDGE_DATABASE_CUSTOM_ROOT_CA_BYTES_MAX",
        "TYPE_BRIDGE_TLS_CUSTOM_ROOT_CA",
        "TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT",
        "TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_PUT",
        "TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_UPDATE",
        "TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_DELETE",
        "TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_BUILDER",
        "TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH",
        "TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_RESULT",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    let base_macros = macro_names(base);
    let extension_macros = macro_names(extension);
    assert_eq!(base_macros.len(), 205);
    assert_eq!(extension_macros, expected_macros);
    assert!(base_macros.is_disjoint(&extension_macros));
    assert_eq!(base_macros.union(&extension_macros).count(), 215);
    assert!(extension.starts_with("#pragma once\n\n#include <typebridge/type_bridge.h>\n"));
}

fn direct_export_names(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("#[unsafe(export_name = \"")
                .and_then(|line| line.strip_suffix("\")]"))
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn activated_sources_and_shared_library_match_the_exact_abi_1_4_ledger() {
    let (_, abi_1_3, additions) = exact_ledgers();
    let expected_impl_exports = additions
        .iter()
        .filter(|name| name.as_str() != SCHEMA_OPEN_V2)
        .filter(|name| name.as_str() != SCHEMA_OPEN_CHUNKED_V2)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(expected_impl_exports.len(), 43);

    let runtime = include_str!("../src/runtime.rs");
    let batch = include_str!("../src/projected_batch.rs");
    let crud = include_str!("../src/crud_v2.rs");
    let direct = direct_export_names(runtime)
        .union(&direct_export_names(batch))
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(direct.len(), 15);
    assert!(direct.is_subset(&expected_impl_exports));
    assert_eq!(
        crud.matches("#[unsafe(export_name = $export_name)]")
            .count(),
        6
    );
    let crud_exports = expected_impl_exports
        .difference(&direct)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(crud_exports.len(), 28);
    for name in &crud_exports {
        assert_eq!(
            crud.matches(&format!("\"{name}\"")).count(),
            1,
            "CRUD export {name} must name exactly one generated implementation",
        );
    }

    if !shared_consumer_required() {
        return;
    }
    let library = native_library().unwrap_or_else(|| {
        panic!(
            "the ABI 1.4 export audit requires a built TypeBridge C shared library; run cargo build -p type-bridge-c --lib first"
        )
    });
    let expected = abi_1_3.union(&additions).cloned().collect::<BTreeSet<_>>();
    let actual = shared_library_exports(&library);
    assert!(
        expected.is_subset(&actual),
        "the active shared library omitted frozen ABI 1.4 exports: {:?}",
        expected.difference(&actual).collect::<Vec<_>>(),
    );
}

const ABI_1_4_COMPILER_PROBE: &str = r#"#include <stddef.h>
#include <stdint.h>

#include <typebridge/type_bridge_abi_1_4.h>

#ifdef __cplusplus
#define ABI14_STATIC_ASSERT(value, message) static_assert(value, message)
#else
#define ABI14_STATIC_ASSERT(value, message) _Static_assert(value, message)
#endif

ABI14_STATIC_ASSERT(TYPE_BRIDGE_C_ABI_MAJOR == 1u, "ABI major drifted");
ABI14_STATIC_ASSERT(TYPE_BRIDGE_C_ABI_MINOR >= 4u, "ABI 1.4 is no longer supported");
ABI14_STATIC_ASSERT(TYPE_BRIDGE_DATABASE_CONFIG_V2_VERSION == 2u,
                    "database config version drifted");
ABI14_STATIC_ASSERT(TYPE_BRIDGE_DATABASE_CUSTOM_ROOT_CA_BYTES_MAX == 1048576u,
                    "custom-root ceiling drifted");
ABI14_STATIC_ASSERT(TYPE_BRIDGE_TLS_CUSTOM_ROOT_CA == 2u,
                    "custom-root TLS tag drifted");
ABI14_STATIC_ASSERT(TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT == 1,
                    "insert batch tag drifted");
ABI14_STATIC_ASSERT(TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_PUT == 2,
                    "put batch tag drifted");
ABI14_STATIC_ASSERT(TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_UPDATE == 3,
                    "update batch tag drifted");
ABI14_STATIC_ASSERT(TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_DELETE == 4,
                    "delete batch tag drifted");
ABI14_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_BUILDER == 34,
                    "batch-builder input tag drifted");
ABI14_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH == 35,
                    "batch input tag drifted");
ABI14_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_RESULT == 36,
                    "batch-result input tag drifted");
ABI14_STATIC_ASSERT(sizeof(type_bridge_query_execution_limits_v1_t) == 104u,
                    "nested limits layout drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, struct_size) == 0u,
                    "config struct_size offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, version) == 4u,
                    "config version offset drifted");

#if UINTPTR_MAX == UINT64_MAX
ABI14_STATIC_ASSERT(sizeof(type_bridge_database_config_v2_t) == 336u,
                    "LP64/LLP64 config size drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, address) == 8u,
                    "LP64/LLP64 address offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, database) == 24u,
                    "LP64/LLP64 database offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, username) == 40u,
                    "LP64/LLP64 username offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, password) == 56u,
                    "LP64/LLP64 password offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, http_port) == 72u,
                    "LP64/LLP64 http_port offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, tls_mode) == 76u,
                    "LP64/LLP64 tls_mode offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, custom_root_ca_pem) == 80u,
                    "LP64/LLP64 custom-root offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, connection_limits) == 96u,
                    "LP64/LLP64 connection limits offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, answer_limits) == 200u,
                    "LP64/LLP64 answer limits offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, reserved) == 304u,
                    "LP64/LLP64 reserved offset drifted");
#elif UINTPTR_MAX == UINT32_MAX
ABI14_STATIC_ASSERT(sizeof(type_bridge_database_config_v2_t) == 296u,
                    "ILP32 config size drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, address) == 8u,
                    "ILP32 address offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, database) == 16u,
                    "ILP32 database offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, username) == 24u,
                    "ILP32 username offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, password) == 32u,
                    "ILP32 password offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, http_port) == 40u,
                    "ILP32 http_port offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, tls_mode) == 44u,
                    "ILP32 tls_mode offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, custom_root_ca_pem) == 48u,
                    "ILP32 custom-root offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, connection_limits) == 56u,
                    "ILP32 connection limits offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, answer_limits) == 160u,
                    "ILP32 answer limits offset drifted");
ABI14_STATIC_ASSERT(offsetof(type_bridge_database_config_v2_t, reserved) == 264u,
                    "ILP32 reserved offset drifted");
#else
#error "ABI 1.4 supports only frozen 32-bit and 64-bit pointer layouts"
#endif

typedef type_bridge_status_t (TYPE_BRIDGE_CALL *schema_open_fn)(
    const type_bridge_schema_package_descriptor_v1_t *,
    type_bridge_schema_package_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *schema_chunked_open_fn)(
    const type_bridge_schema_package_chunked_descriptor_v1_t *,
    type_bridge_schema_package_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *config_validate_fn)(
    const type_bridge_database_config_v2_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_open_fn)(
    const type_bridge_runtime_t *, const type_bridge_schema_package_t *,
    const type_bridge_database_config_v2_t *,
    const type_bridge_cancellation_t *, type_bridge_database_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *read_open_fn)(
    const type_bridge_database_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_read_transaction_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_open_fn)(
    const type_bridge_database_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_write_transaction_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_commit_fn)(
    type_bridge_write_transaction_t **,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_execution_diagnostics_t **);

typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_create_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_get_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    type_bridge_byte_view_t, const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_update_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    type_bridge_byte_view_t, const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_delete_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    type_bridge_byte_view_t, const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_count_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, uint64_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *read_get_fn)(
    const type_bridge_read_transaction_t *,
    const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *read_count_fn)(
    const type_bridge_read_transaction_t *,
    const type_bridge_projected_token_v1_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, uint64_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_create_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_get_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_update_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_delete_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_count_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, uint64_t *,
    type_bridge_execution_diagnostics_t **);

typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_builder_open_fn)(
    const type_bridge_schema_package_t *,
    const type_bridge_projected_token_v1_t *,
    type_bridge_projected_batch_operation_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_batch_builder_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_builder_add_fn)(
    type_bridge_projected_batch_builder_t *, type_bridge_byte_view_t,
    const type_bridge_projected_create_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_builder_finish_fn)(
    type_bridge_projected_batch_builder_t **, type_bridge_projected_batch_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_builder_close_fn)(
    type_bridge_projected_batch_builder_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_close_fn)(
    type_bridge_projected_batch_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_batch_execute_fn)(
    const type_bridge_database_t *, const type_bridge_projected_batch_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_batch_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_batch_execute_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_batch_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_batch_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_result_count_fn)(
    const type_bridge_projected_batch_result_t *,
    const type_bridge_projected_token_v1_t *,
    type_bridge_projected_batch_operation_t, size_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_result_thing_fn)(
    const type_bridge_projected_batch_result_t *,
    const type_bridge_projected_token_v1_t *,
    type_bridge_projected_batch_operation_t, size_t,
    type_bridge_projected_thing_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_result_close_fn)(
    type_bridge_projected_batch_result_t **);

#define TAKE(type, symbol) do { type value = &symbol; (void)value; } while (0)

void type_bridge_abi_1_4_probe(void) {
  TAKE(schema_open_fn, type_bridge_schema_package_open_v2);
  TAKE(schema_chunked_open_fn, type_bridge_schema_package_open_chunked_v2);
  TAKE(config_validate_fn, type_bridge_database_config_validate_v2);
  TAKE(database_open_fn, type_bridge_database_open_v2);
  TAKE(read_open_fn, type_bridge_read_transaction_open_v2);
  TAKE(write_open_fn, type_bridge_write_transaction_open_v2);
  TAKE(write_commit_fn, type_bridge_write_transaction_commit_v2);
  TAKE(database_create_fn, type_bridge_database_entity_insert_v2);
  TAKE(database_create_fn, type_bridge_database_entity_put_v2);
  TAKE(database_get_fn, type_bridge_database_entity_get_by_iid_v2);
  TAKE(database_update_fn, type_bridge_database_entity_update_v2);
  TAKE(database_delete_fn, type_bridge_database_entity_delete_by_iid_v2);
  TAKE(database_count_fn, type_bridge_database_entity_count_v2);
  TAKE(read_get_fn, type_bridge_read_transaction_entity_get_by_iid_v2);
  TAKE(read_count_fn, type_bridge_read_transaction_entity_count_v2);
  TAKE(write_create_fn, type_bridge_write_transaction_entity_insert_v2);
  TAKE(write_create_fn, type_bridge_write_transaction_entity_put_v2);
  TAKE(write_get_fn, type_bridge_write_transaction_entity_get_by_iid_v2);
  TAKE(write_update_fn, type_bridge_write_transaction_entity_update_v2);
  TAKE(write_delete_fn, type_bridge_write_transaction_entity_delete_by_iid_v2);
  TAKE(write_count_fn, type_bridge_write_transaction_entity_count_v2);
  TAKE(database_create_fn, type_bridge_database_relation_insert_v2);
  TAKE(database_create_fn, type_bridge_database_relation_put_v2);
  TAKE(database_get_fn, type_bridge_database_relation_get_by_iid_v2);
  TAKE(database_update_fn, type_bridge_database_relation_update_v2);
  TAKE(database_delete_fn, type_bridge_database_relation_delete_by_iid_v2);
  TAKE(database_count_fn, type_bridge_database_relation_count_v2);
  TAKE(read_get_fn, type_bridge_read_transaction_relation_get_by_iid_v2);
  TAKE(read_count_fn, type_bridge_read_transaction_relation_count_v2);
  TAKE(write_create_fn, type_bridge_write_transaction_relation_insert_v2);
  TAKE(write_create_fn, type_bridge_write_transaction_relation_put_v2);
  TAKE(write_get_fn, type_bridge_write_transaction_relation_get_by_iid_v2);
  TAKE(write_update_fn, type_bridge_write_transaction_relation_update_v2);
  TAKE(write_delete_fn, type_bridge_write_transaction_relation_delete_by_iid_v2);
  TAKE(write_count_fn, type_bridge_write_transaction_relation_count_v2);
  TAKE(batch_builder_open_fn, type_bridge_projected_batch_builder_open_v1);
  TAKE(batch_builder_add_fn, type_bridge_projected_batch_builder_add_v1);
  TAKE(batch_builder_finish_fn, type_bridge_projected_batch_builder_finish);
  TAKE(batch_builder_close_fn, type_bridge_projected_batch_builder_close);
  TAKE(batch_close_fn, type_bridge_projected_batch_close);
  TAKE(database_batch_execute_fn, type_bridge_database_projected_batch_execute_v1);
  TAKE(write_batch_execute_fn, type_bridge_write_transaction_projected_batch_execute_v1);
  TAKE(batch_result_count_fn, type_bridge_projected_batch_result_count);
  TAKE(batch_result_thing_fn, type_bridge_projected_batch_result_thing_at);
  TAKE(batch_result_close_fn, type_bridge_projected_batch_result_close);
}
"#;

#[test]
fn aggregate_header_compiles_exact_prototypes_and_hosted_layouts() {
    // Ordinary Windows workspace tests need not run inside a Visual Studio
    // developer environment. The dedicated shared-consumer lane opts in.
    #[cfg(windows)]
    if !shared_consumer_required() {
        return;
    }

    assert_eq!(ABI_1_4_COMPILER_PROBE.matches("  TAKE(").count(), 45);
    assert!(ABI_1_4_COMPILER_PROBE.contains("#if UINTPTR_MAX == UINT64_MAX"));
    assert!(ABI_1_4_COMPILER_PROBE.contains("#elif UINTPTR_MAX == UINT32_MAX"));
    let stage = TempDirectory::new("compiler");
    let source = stage.path().join("abi-1-4.c");
    fs::write(&source, ABI_1_4_COMPILER_PROBE).expect("ABI 1.4 compiler probe is written");
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");

    #[cfg(not(windows))]
    {
        let mut c11 = false;
        let mut c17 = false;
        let mut cpp17 = false;
        for (compiler, standard, language) in [
            ("gcc", "c11", "c"),
            ("gcc", "c17", "c"),
            ("clang", "c11", "c"),
            ("clang", "c17", "c"),
            ("g++", "c++17", "c++"),
            ("clang++", "c++17", "c++"),
        ] {
            if !command_exists(compiler) {
                continue;
            }
            let output = Command::new(compiler)
                .arg(format!("-std={standard}"))
                .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors"])
                .args(["-x", language, "-fsyntax-only", "-I"])
                .arg(&include)
                .arg(&source)
                .output()
                .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
            assert_command_succeeded(
                &output,
                &format!("{compiler} {standard} ABI 1.4 compiler probe"),
            );
            c11 |= standard == "c11";
            c17 |= standard == "c17";
            cpp17 |= standard == "c++17";
        }
        assert!(
            c11 && c17 && cpp17,
            "C11, C17, and C++17 probes are required"
        );
    }

    #[cfg(windows)]
    {
        let mut c11 = false;
        let mut c17 = false;
        let mut cpp17 = false;
        for (compiler, standard, language) in [
            ("cl", "/std:c11", "/TC"),
            ("cl", "/std:c17", "/TC"),
            ("cl", "/std:c++17", "/TP"),
            ("clang-cl", "/std:c11", "/TC"),
            ("clang-cl", "/std:c17", "/TC"),
            ("clang-cl", "/std:c++17", "/TP"),
        ] {
            if !command_exists(compiler) {
                continue;
            }
            let output = Command::new(compiler)
                .args(["/nologo", standard, "/W4", "/WX", "/Zs", language])
                .arg(format!("/I{}", include.display()))
                .arg(&source)
                .current_dir(stage.path())
                .output()
                .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
            assert_command_succeeded(
                &output,
                &format!("{compiler} {standard} ABI 1.4 compiler probe"),
            );
            c11 |= standard == "/std:c11";
            c17 |= standard == "/std:c17";
            cpp17 |= standard == "/std:c++17";
        }
        assert!(
            c11 && c17 && cpp17,
            "C11, C17, and C++17 probes are required"
        );
    }
}

#[test]
fn activated_cmake_package_preserves_the_exact_abi_1_4_predecessor_headers() {
    assert!(
        command_exists("cmake"),
        "cmake is required for the package probe"
    );
    let stage = TempDirectory::new("install");
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let build = stage.path().join("build");
    let install = stage.path().join("install");
    let library = stage.path().join(if cfg!(windows) {
        "type_bridge_c.dll"
    } else {
        "libtype_bridge_c.so"
    });
    fs::write(&library, b"ABI 1.4 package probe").expect("dummy shared library is created");
    let mut configure = Command::new("cmake");
    configure
        .args(["-S"])
        .arg(crate_root)
        .args(["-B"])
        .arg(&build)
        .arg(format!("-DTYPE_BRIDGE_C_LIBRARY={}", library.display()))
        .args([
            "-DCMAKE_INSTALL_LIBDIR=lib",
            "-DCMAKE_INSTALL_INCLUDEDIR=include",
            "-DCMAKE_INSTALL_BINDIR=bin",
        ]);
    #[cfg(windows)]
    {
        let import_library = stage.path().join("type_bridge_c.lib");
        fs::write(&import_library, b"ABI 1.4 import probe")
            .expect("dummy import library is created");
        configure.arg(format!(
            "-DTYPE_BRIDGE_C_IMPORT_LIBRARY={}",
            import_library.display()
        ));
    }
    let output = configure
        .output()
        .expect("runtime package configure launches");
    assert_command_succeeded(&output, "runtime ABI 1.4 package configure");

    let mut install_command = Command::new("cmake");
    install_command
        .args(["--install"])
        .arg(&build)
        .args(["--prefix"])
        .arg(&install);
    #[cfg(windows)]
    install_command.args(["--config", "Debug"]);
    let output = install_command
        .output()
        .expect("runtime package install launches");
    assert_command_succeeded(&output, "runtime ABI 1.4 package install");

    for header in ["type_bridge.h", "type_bridge_abi_1_4.h"] {
        let installed = install.join("include/typebridge").join(header);
        assert_eq!(
            fs::read(&installed).expect("installed header is readable"),
            fs::read(crate_root.join("include/typebridge").join(header))
                .expect("source header is readable"),
            "installed {header} drifted from its source",
        );
    }
    let config = fs::read_to_string(install.join("lib/cmake/TypeBridge/TypeBridgeConfig.cmake"))
        .expect("installed CMake config is UTF-8");
    let pkg_config = fs::read_to_string(install.join("lib/pkgconfig/type-bridge.pc"))
        .expect("installed pkg-config metadata is UTF-8");
    assert!(config.contains("set(TypeBridge_C_ABI_VERSION \"1.6.0\")"));
    assert!(pkg_config.contains("\nVersion: 1.6.0\n"));

    let consumer_source = stage.path().join("consumer");
    let consumer_build = stage.path().join("consumer-build");
    fs::create_dir(&consumer_source).expect("consumer source directory is created");
    fs::write(
        consumer_source.join("CMakeLists.txt"),
        r#"cmake_minimum_required(VERSION 3.20)
project(type_bridge_abi_1_4_consumer LANGUAGES NONE)
find_package(TypeBridge 1.6.0 EXACT CONFIG REQUIRED)
if(NOT TypeBridge_C_ABI_VERSION STREQUAL "1.6.0")
  message(FATAL_ERROR "unexpected TypeBridge C ABI version")
endif()
if(NOT TARGET TypeBridge::C)
  message(FATAL_ERROR "TypeBridge::C target is missing")
endif()
"#,
    )
    .expect("consumer CMake project is written");
    let output = Command::new("cmake")
        .args(["-S"])
        .arg(&consumer_source)
        .args(["-B"])
        .arg(&consumer_build)
        .arg(format!(
            "-DTypeBridge_DIR={}",
            install.join("lib/cmake/TypeBridge").display()
        ))
        .output()
        .expect("ABI 1.6 package consumer configure launches");
    assert_command_succeeded(&output, "exact ABI 1.6 package consumer configure");
}
