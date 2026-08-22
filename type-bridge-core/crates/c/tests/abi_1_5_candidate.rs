#![cfg(feature = "abi-1-5")]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

const ABI_1_5_CANDIDATE_ADDITIONS: [&str; 87] = [
    "type_bridge_database_administration_open",
    "type_bridge_database_administration_exists",
    "type_bridge_database_administration_exists_with_options",
    "type_bridge_database_administration_create",
    "type_bridge_database_administration_create_with_options",
    "type_bridge_database_administration_inspect",
    "type_bridge_database_administration_inspect_with_options",
    "type_bridge_database_administration_plan_delete",
    "type_bridge_database_administration_plan_delete_with_options",
    "type_bridge_database_deletion_plan_inspected_state",
    "type_bridge_database_deletion_plan_execute",
    "type_bridge_database_deletion_plan_execute_with_options",
    "type_bridge_database_deletion_plan_close",
    "type_bridge_database_administration_close",
    "type_bridge_migration_catalog_open",
    "type_bridge_migration_catalog_fingerprint",
    "type_bridge_migration_catalog_count",
    "type_bridge_migration_catalog_entry_at",
    "type_bridge_migration_catalog_head_count",
    "type_bridge_migration_catalog_head_at",
    "type_bridge_migration_catalog_close",
    "type_bridge_migration_history_entry_identity",
    "type_bridge_migration_history_entry_parent_count",
    "type_bridge_migration_history_entry_parent_at",
    "type_bridge_migration_history_entry_manifest_digest",
    "type_bridge_migration_history_entry_step_count",
    "type_bridge_migration_history_entry_safety",
    "type_bridge_migration_history_entry_reversible",
    "type_bridge_migration_history_entry_close",
    "type_bridge_migration_identity_app_label",
    "type_bridge_migration_identity_name",
    "type_bridge_migration_identity_new",
    "type_bridge_migration_identity_close",
    "type_bridge_migration_catalog_preview_apply",
    "type_bridge_migration_catalog_preview_rollback",
    "type_bridge_migration_plan_direction",
    "type_bridge_migration_plan_execution_authorized",
    "type_bridge_migration_plan_count",
    "type_bridge_migration_plan_entry_at",
    "type_bridge_migration_plan_approval_builder",
    "type_bridge_migration_plan_authorize",
    "type_bridge_migration_plan_execute",
    "type_bridge_migration_cancellation_new",
    "type_bridge_migration_cancellation_cancel",
    "type_bridge_migration_cancellation_is_cancelled",
    "type_bridge_migration_cancellation_close",
    "type_bridge_migration_plan_execute_with_options",
    "type_bridge_migration_plan_close",
    "type_bridge_migration_plan_entry_identity",
    "type_bridge_migration_plan_entry_safety",
    "type_bridge_migration_plan_entry_operation_count",
    "type_bridge_migration_plan_entry_transaction_group_count",
    "type_bridge_migration_plan_entry_backfill_count",
    "type_bridge_migration_plan_entry_close",
    "type_bridge_migration_approval_builder_approve",
    "type_bridge_migration_approval_builder_finish",
    "type_bridge_migration_approval_builder_close",
    "type_bridge_migration_approval_set_count",
    "type_bridge_migration_approval_set_close",
    "type_bridge_migration_execution_outcome_direction",
    "type_bridge_migration_execution_outcome_status",
    "type_bridge_migration_execution_outcome_migration_identity",
    "type_bridge_migration_execution_outcome_position_kind",
    "type_bridge_migration_execution_outcome_position_ordinal",
    "type_bridge_migration_execution_outcome_diagnostics",
    "type_bridge_migration_execution_outcome_backfill_count",
    "type_bridge_migration_execution_outcome_backfill_at",
    "type_bridge_migration_execution_outcome_close",
    "type_bridge_migration_backfill_observation_identity",
    "type_bridge_migration_backfill_observation_operation_ordinal",
    "type_bridge_migration_backfill_observation_manifest_step_index",
    "type_bridge_migration_backfill_observation_plan_digest",
    "type_bridge_migration_backfill_observation_direction",
    "type_bridge_migration_backfill_observation_counts",
    "type_bridge_migration_backfill_observation_close",
    "type_bridge_migration_catalog_verify",
    "type_bridge_migration_verification_report_is_clean",
    "type_bridge_migration_verification_report_finding_count",
    "type_bridge_migration_verification_report_finding_at",
    "type_bridge_migration_verification_report_frontier_count",
    "type_bridge_migration_verification_report_frontier_at",
    "type_bridge_migration_verification_report_close",
    "type_bridge_migration_verification_finding_kind",
    "type_bridge_migration_verification_finding_pending_count",
    "type_bridge_migration_verification_finding_pending_at",
    "type_bridge_migration_verification_finding_diagnostics",
    "type_bridge_migration_verification_finding_close",
];

fn implementation_exports() -> BTreeSet<&'static str> {
    include_str!("../src/migration_abi.rs")
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("pub unsafe extern \"C\" fn ")
                .and_then(|rest| rest.split_once('(').map(|(name, _)| name))
        })
        .collect()
}

fn header_exports() -> BTreeSet<&'static str> {
    let header = include_str!("../include/typebridge/type_bridge_abi_1_5.h");
    ABI_1_5_CANDIDATE_ADDITIONS
        .into_iter()
        .filter(|name| header.contains(&format!("{name}(")))
        .collect()
}

#[test]
fn candidate_additive_export_inventory_is_exact_and_duplicate_free() {
    let expected = ABI_1_5_CANDIDATE_ADDITIONS
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(expected.len(), ABI_1_5_CANDIDATE_ADDITIONS.len());
    assert_eq!(implementation_exports(), expected);
    assert_eq!(header_exports(), expected);
}

#[test]
fn candidate_header_compiles_as_strict_c17_and_cpp17() {
    let directory =
        std::env::temp_dir().join(format!("typebridge-abi-1-5-header-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    let source = directory.join("probe.c");
    fs::write(
        &source,
        r#"#include <typebridge/type_bridge_abi_1_5.h>
_Static_assert(sizeof(type_bridge_migration_execution_options_v1_t) == 48, "options size");
_Static_assert(_Alignof(type_bridge_migration_execution_options_v1_t) == 8, "options alignment");
int main(void) { return TYPE_BRIDGE_MIGRATION_EXECUTION_OPTIONS_V1 == 1u ? 0 : 1; }
"#,
    )
    .unwrap();
    let include = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let status = Command::new("cc")
        .args([
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
            "-c",
        ])
        .arg(&source)
        .arg("-I")
        .arg(&include)
        .arg("-o")
        .arg(directory.join("probe.o"))
        .status()
        .unwrap();
    assert!(status.success());
    let cpp = directory.join("probe.cpp");
    fs::write(
        &cpp,
        r#"#include <typebridge/type_bridge_abi_1_5.h>
static_assert(sizeof(type_bridge_migration_execution_options_v1_t) == 48, "options size");
static_assert(alignof(type_bridge_migration_execution_options_v1_t) == 8, "options alignment");
int main() { return TYPE_BRIDGE_MIGRATION_EXECUTION_OPTIONS_V1 == 1u ? 0 : 1; }
"#,
    )
    .unwrap();
    let status = Command::new("c++")
        .args([
            "-std=c++17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
            "-c",
        ])
        .arg(&cpp)
        .arg("-I")
        .arg(&include)
        .arg("-o")
        .arg(directory.join("probe-cpp.o"))
        .status()
        .unwrap();
    assert!(status.success());
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn activated_shared_library_exports_the_exact_abi_1_5_additions() {
    let executable = std::env::current_exe().unwrap();
    let profile = executable
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "debug"))
        .expect("test executable is beneath the Cargo profile directory");
    let direct = profile.join("libtype_bridge_c.so");
    let library = if direct.is_file() {
        direct
    } else {
        profile.join("deps/libtype_bridge_c.so")
    };
    assert!(library.is_file(), "Cargo must build the activated cdylib");
    let output = Command::new("nm")
        .args(["-D", "--defined-only"])
        .arg(library)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let exported = stdout
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .filter(|name| ABI_1_5_CANDIDATE_ADDITIONS.contains(name))
        .collect::<BTreeSet<_>>();
    assert_eq!(exported, ABI_1_5_CANDIDATE_ADDITIONS.into_iter().collect());
}

#[test]
fn cmake_install_inventory_carries_the_abi_1_5_header() {
    let cmake = include_str!("../CMakeLists.txt");
    assert!(cmake.contains("project(TypeBridge VERSION 1.6.0 LANGUAGES NONE)"));
    assert!(cmake.contains("include/typebridge/type_bridge_abi_1_5.h"));
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn installed_abi_1_5_c17_consumer_links_and_runs() {
    let executable = std::env::current_exe().unwrap();
    let profile = executable
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "debug"))
        .expect("test executable is beneath the Cargo profile directory");
    let direct = profile.join("libtype_bridge_c.so");
    let library = if direct.is_file() {
        direct
    } else {
        profile.join("deps/libtype_bridge_c.so")
    };
    assert!(library.is_file(), "Cargo must build the activated cdylib");

    let stage = std::env::temp_dir().join(format!(
        "typebridge-abi-1-5-installed-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&stage);
    let build = stage.join("build");
    let install = stage.join("install");
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("cmake")
        .arg("-S")
        .arg(crate_root)
        .arg("-B")
        .arg(&build)
        .arg(format!("-DTYPE_BRIDGE_C_LIBRARY={}", library.display()))
        .args([
            "-DCMAKE_INSTALL_LIBDIR=lib",
            "-DCMAKE_INSTALL_INCLUDEDIR=include",
            "-DCMAKE_INSTALL_BINDIR=bin",
        ])
        .output()
        .expect("ABI 1.5 runtime package configure launches");
    assert!(
        output.status.success(),
        "ABI 1.5 package configure failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new("cmake")
        .arg("--install")
        .arg(&build)
        .arg("--prefix")
        .arg(&install)
        .output()
        .expect("ABI 1.5 runtime package install launches");
    assert!(
        output.status.success(),
        "ABI 1.5 package install failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let source = stage.join("consumer.c");
    fs::write(
        &source,
        r#"#include <stdint.h>
#include <typebridge/type_bridge_abi_1_5.h>

int main(void) {
  type_bridge_migration_cancellation_t *cancellation = NULL;
  type_bridge_migration_identity_t *identity = NULL;
  type_bridge_diagnostics_t *diagnostics = NULL;
  static const uint8_t app[] = "workforcev4";
  static const uint8_t name[] = "9999_unknown";
  uint8_t cancelled = 9u;
  if (type_bridge_migration_cancellation_new(&cancellation) != TYPE_BRIDGE_STATUS_OK || cancellation == NULL) return 1;
  if (type_bridge_migration_cancellation_is_cancelled(cancellation, &cancelled) != TYPE_BRIDGE_STATUS_OK || cancelled != 0u) return 2;
  if (type_bridge_migration_cancellation_cancel(cancellation) != TYPE_BRIDGE_STATUS_OK) return 3;
  if (type_bridge_migration_cancellation_is_cancelled(cancellation, &cancelled) != TYPE_BRIDGE_STATUS_OK || cancelled != 1u) return 4;
  if (type_bridge_migration_cancellation_close(&cancellation) != TYPE_BRIDGE_STATUS_OK || cancellation != NULL) return 5;
  if (type_bridge_migration_identity_new((type_bridge_byte_view_t){app, sizeof(app) - 1u}, (type_bridge_byte_view_t){name, sizeof(name) - 1u}, &identity, &diagnostics) != TYPE_BRIDGE_STATUS_OK || identity == NULL || diagnostics != NULL) return 6;
  if (type_bridge_migration_identity_close(&identity) != TYPE_BRIDGE_STATUS_OK || identity != NULL) return 7;
  return 0;
}
"#,
    )
    .unwrap();
    let consumer = stage.join("consumer");
    let output = Command::new("cc")
        .args([
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
        ])
        .arg(&source)
        .arg("-I")
        .arg(install.join("include"))
        .arg("-L")
        .arg(install.join("lib"))
        .arg("-ltype_bridge_c")
        .arg(format!("-Wl,-rpath,{}", install.join("lib").display()))
        .arg("-o")
        .arg(&consumer)
        .output()
        .expect("installed ABI 1.5 consumer compile launches");
    assert!(
        output.status.success(),
        "installed ABI 1.5 consumer compile failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(&consumer)
        .output()
        .expect("installed ABI 1.5 consumer launches");
    assert!(output.status.success(), "installed ABI 1.5 consumer failed");
    fs::remove_dir_all(stage).unwrap();
}

#[test]
fn candidate_options_layout_is_fixed_on_supported_64_bit_targets() {
    #[repr(C)]
    struct MigrationExecutionOptionsV1 {
        struct_size: u64,
        version: u32,
        flags: u32,
        timeout_milliseconds: u64,
        max_transaction_groups: u64,
        max_backfill_observations: u64,
        cancellation: *const std::ffi::c_void,
    }

    assert_eq!(
        std::mem::size_of::<usize>(),
        8,
        "ABI 1.5 target must be 64-bit"
    );
    assert_eq!(std::mem::size_of::<MigrationExecutionOptionsV1>(), 48);
    assert_eq!(std::mem::align_of::<MigrationExecutionOptionsV1>(), 8);
}
