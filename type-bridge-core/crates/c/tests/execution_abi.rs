use std::fs;
use std::mem::{align_of, offset_of, size_of};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_c::{
    TypeBridgeCancellation, TypeBridgeDatabaseConfigV1, TypeBridgeExecutionDiagnostics,
    TypeBridgeRuntime, TypeBridgeRuntimeConfigV1, TypeBridgeStatus,
};

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn type_bridge_runtime_open_v1(
        config: *const TypeBridgeRuntimeConfigV1,
        out_runtime: *mut *mut TypeBridgeRuntime,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_runtime_close(
        runtime: *mut *mut TypeBridgeRuntime,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_cancellation_open(
        out_cancellation: *mut *mut TypeBridgeCancellation,
    ) -> TypeBridgeStatus;
    fn type_bridge_cancellation_request(
        cancellation: *const TypeBridgeCancellation,
    ) -> TypeBridgeStatus;
    fn type_bridge_cancellation_is_requested(
        cancellation: *const TypeBridgeCancellation,
        out_requested: *mut u8,
    ) -> TypeBridgeStatus;
    fn type_bridge_cancellation_close(
        cancellation: *mut *mut TypeBridgeCancellation,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_close(
        diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
}

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "typebridge-c-execution-abi-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("unique execution ABI directory is created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("execution ABI directory is removed");
    }
}

fn compiler<'candidate>(candidates: &'candidate [&'candidate str]) -> Option<&'candidate str> {
    candidates
        .iter()
        .copied()
        .find(|candidate| Command::new(candidate).arg("--version").output().is_ok())
}

fn expected_layout() -> Vec<usize> {
    vec![
        1,
        1,
        1,
        64,
        4096,
        256,
        4096,
        65536,
        258,
        65536,
        0,
        1,
        size_of::<TypeBridgeRuntimeConfigV1>(),
        align_of::<TypeBridgeRuntimeConfigV1>(),
        offset_of!(TypeBridgeRuntimeConfigV1, struct_size),
        offset_of!(TypeBridgeRuntimeConfigV1, version),
        offset_of!(TypeBridgeRuntimeConfigV1, worker_threads),
        offset_of!(TypeBridgeRuntimeConfigV1, reserved0),
        offset_of!(TypeBridgeRuntimeConfigV1, reserved),
        size_of::<TypeBridgeDatabaseConfigV1>(),
        align_of::<TypeBridgeDatabaseConfigV1>(),
        offset_of!(TypeBridgeDatabaseConfigV1, struct_size),
        offset_of!(TypeBridgeDatabaseConfigV1, version),
        offset_of!(TypeBridgeDatabaseConfigV1, address),
        offset_of!(TypeBridgeDatabaseConfigV1, database),
        offset_of!(TypeBridgeDatabaseConfigV1, username),
        offset_of!(TypeBridgeDatabaseConfigV1, password),
        offset_of!(TypeBridgeDatabaseConfigV1, http_port),
        offset_of!(TypeBridgeDatabaseConfigV1, tls_mode),
        offset_of!(TypeBridgeDatabaseConfigV1, reserved),
    ]
}

fn parse_layout(output: &[u8]) -> Vec<usize> {
    std::str::from_utf8(output)
        .expect("layout output is UTF-8")
        .split_whitespace()
        .map(|value| value.parse().expect("layout value is an integer"))
        .collect()
}

#[test]
fn c17_and_cpp17_headers_match_runtime_layout_and_function_types() {
    let Some(c_compiler) = compiler(&["cc", "gcc", "clang"]) else {
        return;
    };
    let Some(cpp_compiler) = compiler(&["c++", "g++", "clang++"]) else {
        return;
    };
    let stage = TempDirectory::new();
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let source = stage.path().join("layout.c");
    fs::write(
        &source,
        r#"#include <stddef.h>
#include <stdio.h>
#include <typebridge/type_bridge.h>

#ifdef __cplusplus
#define TYPE_BRIDGE_ALIGNOF(type) alignof(type)
#else
#define TYPE_BRIDGE_ALIGNOF(type) _Alignof(type)
#endif

#ifdef TYPE_BRIDGE_FUNCTION_PROBE
void type_bridge_function_type_probe(void) {
  type_bridge_status_t (*runtime_open)(
      const type_bridge_runtime_config_v1_t *, type_bridge_runtime_t **,
      type_bridge_execution_diagnostics_t **) = &type_bridge_runtime_open_v1;
  type_bridge_status_t (*database_open)(
      const type_bridge_runtime_t *, const type_bridge_schema_package_t *,
      const type_bridge_database_config_v1_t *,
      const type_bridge_cancellation_t *, type_bridge_database_t **,
      type_bridge_execution_diagnostics_t **) = &type_bridge_database_open_v1;
  type_bridge_status_t (*read_open)(
      const type_bridge_database_t *, const type_bridge_cancellation_t *,
      type_bridge_read_transaction_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_read_transaction_open;
  type_bridge_status_t (*write_open)(
      const type_bridge_database_t *, const type_bridge_cancellation_t *,
      type_bridge_write_transaction_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_open;
  type_bridge_status_t (*generated_preflight)(
      const type_bridge_generated_opaque_input_v1_t *, size_t,
      const type_bridge_generated_output_range_v1_t *, size_t) =
      &type_bridge_generated_opaque_alias_preflight_v1;
  type_bridge_status_t (*thing_validate_model)(
      const type_bridge_projected_thing_t *,
      const type_bridge_projected_token_v1_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_projected_thing_validate_model;
  type_bridge_status_t (*reference_validate_model)(
      const type_bridge_projected_reference_t *,
      const type_bridge_projected_token_v1_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_projected_reference_validate_model;
  type_bridge_status_t (*reference_validate_role)(
      const type_bridge_projected_reference_t *,
      const type_bridge_projected_token_v1_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_projected_reference_validate_role;
  type_bridge_status_t (*reference_clone)(
      const type_bridge_projected_reference_t *,
      type_bridge_projected_reference_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_projected_reference_clone;
  type_bridge_status_t (*reference_model_ordinal)(
      const type_bridge_projected_reference_t *, uint32_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_projected_reference_model_ordinal;
  type_bridge_status_t (*thing_scalar_role)(
      const type_bridge_projected_thing_t *,
      const type_bridge_projected_token_v1_t *,
      type_bridge_projected_reference_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_projected_thing_scalar_role_reference;
  type_bridge_status_t (*value_validate_model)(
      const type_bridge_projected_value_t *,
      const type_bridge_projected_token_v1_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_projected_value_validate_model;
  type_bridge_status_t (*database_entity_insert)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_database_entity_insert;
  type_bridge_status_t (*database_entity_put)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) = &type_bridge_database_entity_put;
  type_bridge_status_t (*database_entity_get)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      type_bridge_byte_view_t, const type_bridge_cancellation_t *,
      type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_database_entity_get_by_iid;
  type_bridge_status_t (*database_entity_update)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      type_bridge_byte_view_t,
      const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) = &type_bridge_database_entity_update;
  type_bridge_status_t (*database_entity_delete)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      type_bridge_byte_view_t, const type_bridge_cancellation_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_database_entity_delete_by_iid;
  type_bridge_status_t (*database_entity_count)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      const type_bridge_cancellation_t *, uint64_t *,
      type_bridge_execution_diagnostics_t **) = &type_bridge_database_entity_count;
  type_bridge_status_t (*read_entity_get)(
      const type_bridge_read_transaction_t *,
      const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_read_transaction_entity_get_by_iid;
  type_bridge_status_t (*read_entity_count)(
      const type_bridge_read_transaction_t *,
      const type_bridge_projected_token_v1_t *,
      const type_bridge_cancellation_t *, uint64_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_read_transaction_entity_count;
  type_bridge_status_t (*write_entity_insert)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *,
      const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_entity_insert;
  type_bridge_status_t (*write_entity_put)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *,
      const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_entity_put;
  type_bridge_status_t (*write_entity_get)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_entity_get_by_iid;
  type_bridge_status_t (*write_entity_update)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
      const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_entity_update;
  type_bridge_status_t (*write_entity_delete)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
      const type_bridge_cancellation_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_entity_delete_by_iid;
  type_bridge_status_t (*write_entity_count)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *,
      const type_bridge_cancellation_t *, uint64_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_entity_count;
  type_bridge_status_t (*database_relation_insert)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_database_relation_insert;
  type_bridge_status_t (*database_relation_put)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_database_relation_put;
  type_bridge_status_t (*database_relation_get)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      type_bridge_byte_view_t, const type_bridge_cancellation_t *,
      type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_database_relation_get_by_iid;
  type_bridge_status_t (*database_relation_update)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      type_bridge_byte_view_t, const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_database_relation_update;
  type_bridge_status_t (*database_relation_delete)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      type_bridge_byte_view_t, const type_bridge_cancellation_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_database_relation_delete_by_iid;
  type_bridge_status_t (*database_relation_count)(
      const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
      const type_bridge_cancellation_t *, uint64_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_database_relation_count;
  type_bridge_status_t (*read_relation_get)(
      const type_bridge_read_transaction_t *,
      const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_read_transaction_relation_get_by_iid;
  type_bridge_status_t (*read_relation_count)(
      const type_bridge_read_transaction_t *,
      const type_bridge_projected_token_v1_t *,
      const type_bridge_cancellation_t *, uint64_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_read_transaction_relation_count;
  type_bridge_status_t (*write_relation_insert)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *,
      const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_relation_insert;
  type_bridge_status_t (*write_relation_put)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *,
      const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_relation_put;
  type_bridge_status_t (*write_relation_get)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_relation_get_by_iid;
  type_bridge_status_t (*write_relation_update)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
      const type_bridge_projected_create_t *,
      const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_relation_update;
  type_bridge_status_t (*write_relation_delete)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
      const type_bridge_cancellation_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_relation_delete_by_iid;
  type_bridge_status_t (*write_relation_count)(
      const type_bridge_write_transaction_t *,
      const type_bridge_projected_token_v1_t *,
      const type_bridge_cancellation_t *, uint64_t *,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_write_transaction_relation_count;
  (void)runtime_open;
  (void)database_open;
  (void)read_open;
  (void)write_open;
  (void)generated_preflight;
  (void)thing_validate_model;
  (void)reference_validate_model;
  (void)reference_validate_role;
  (void)reference_clone;
  (void)reference_model_ordinal;
  (void)thing_scalar_role;
  (void)value_validate_model;
  (void)database_entity_insert;
  (void)database_entity_put;
  (void)database_entity_get;
  (void)database_entity_update;
  (void)database_entity_delete;
  (void)database_entity_count;
  (void)read_entity_get;
  (void)read_entity_count;
  (void)write_entity_insert;
  (void)write_entity_put;
  (void)write_entity_get;
  (void)write_entity_update;
  (void)write_entity_delete;
  (void)write_entity_count;
  (void)database_relation_insert;
  (void)database_relation_put;
  (void)database_relation_get;
  (void)database_relation_update;
  (void)database_relation_delete;
  (void)database_relation_count;
  (void)read_relation_get;
  (void)read_relation_count;
  (void)write_relation_insert;
  (void)write_relation_put;
  (void)write_relation_get;
  (void)write_relation_update;
  (void)write_relation_delete;
  (void)write_relation_count;
}
#endif

int main(void) {
  printf(
      "%u %u %u %u %u %u %u %u %u %u %u %u "
      "%zu %zu %zu %zu %zu %zu %zu "
      "%zu %zu %zu %zu %zu %zu %zu %zu %zu %zu %zu\n",
      TYPE_BRIDGE_RUNTIME_CONFIG_VERSION,
      TYPE_BRIDGE_DATABASE_CONFIG_VERSION,
      TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MIN,
      TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MAX,
      TYPE_BRIDGE_DATABASE_ADDRESS_BYTES_MAX,
      TYPE_BRIDGE_DATABASE_NAME_BYTES_MAX,
      TYPE_BRIDGE_DATABASE_USERNAME_BYTES_MAX,
      TYPE_BRIDGE_DATABASE_PASSWORD_BYTES_MAX,
      TYPE_BRIDGE_THING_IID_BYTES_MAX,
      TYPE_BRIDGE_PROJECTED_COLLECTION_LEN_MAX,
      TYPE_BRIDGE_TLS_DISABLED,
      TYPE_BRIDGE_TLS_NATIVE_ROOTS,
      sizeof(type_bridge_runtime_config_v1_t),
      (size_t)TYPE_BRIDGE_ALIGNOF(type_bridge_runtime_config_v1_t),
      offsetof(type_bridge_runtime_config_v1_t, struct_size),
      offsetof(type_bridge_runtime_config_v1_t, version),
      offsetof(type_bridge_runtime_config_v1_t, worker_threads),
      offsetof(type_bridge_runtime_config_v1_t, reserved0),
      offsetof(type_bridge_runtime_config_v1_t, reserved),
      sizeof(type_bridge_database_config_v1_t),
      (size_t)TYPE_BRIDGE_ALIGNOF(type_bridge_database_config_v1_t),
      offsetof(type_bridge_database_config_v1_t, struct_size),
      offsetof(type_bridge_database_config_v1_t, version),
      offsetof(type_bridge_database_config_v1_t, address),
      offsetof(type_bridge_database_config_v1_t, database),
      offsetof(type_bridge_database_config_v1_t, username),
      offsetof(type_bridge_database_config_v1_t, password),
      offsetof(type_bridge_database_config_v1_t, http_port),
      offsetof(type_bridge_database_config_v1_t, tls_mode),
      offsetof(type_bridge_database_config_v1_t, reserved));
  return 0;
}
"#,
    )
    .expect("layout source is written");

    for (compiler, standard, suffix) in [
        (c_compiler, "-std=c17", "c"),
        (cpp_compiler, "-std=c++17", "cpp"),
    ] {
        let source_path = if suffix == "cpp" {
            let cpp = stage.path().join("layout.cpp");
            fs::copy(&source, &cpp).expect("C layout source is copied for C++");
            cpp
        } else {
            source.clone()
        };
        let object = stage.path().join(format!("function-types-{suffix}.o"));
        let output = Command::new(compiler)
            .arg(standard)
            .args([
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
                "-DTYPE_BRIDGE_FUNCTION_PROBE",
                "-c",
            ])
            .arg("-I")
            .arg(&include)
            .arg(&source_path)
            .arg("-o")
            .arg(&object)
            .output()
            .expect("function-type compiler launches");
        assert!(
            output.status.success(),
            "{compiler} rejected the execution ABI function types:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let executable = stage.path().join(format!("layout-{suffix}"));
        let output = Command::new(compiler)
            .arg(standard)
            .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors"])
            .arg("-I")
            .arg(&include)
            .arg(&source_path)
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("layout compiler launches");
        assert!(
            output.status.success(),
            "{compiler} rejected the execution ABI header:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = Command::new(&executable)
            .output()
            .expect("layout probe runs");
        assert!(output.status.success());
        assert_eq!(parse_layout(&output.stdout), expected_layout());
    }
}

#[test]
fn read_and_write_transaction_handles_are_not_interchangeable_in_c() {
    let Some(compiler) = compiler(&["cc", "gcc", "clang"]) else {
        return;
    };
    let stage = TempDirectory::new();
    let source = stage.path().join("wrong-transaction.c");
    fs::write(
        &source,
        r#"#include <typebridge/type_bridge.h>
void misuse(type_bridge_read_transaction_t **read,
            type_bridge_execution_diagnostics_t **diagnostics) {
  (void)type_bridge_write_transaction_commit(read, NULL, diagnostics);
}
"#,
    )
    .expect("negative transaction source is written");
    let output = Command::new(compiler)
        .args([
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
            "-c",
        ])
        .arg("-I")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("include"))
        .arg(&source)
        .arg("-o")
        .arg(stage.path().join("wrong-transaction.o"))
        .output()
        .expect("negative transaction compiler launches");
    assert!(
        !output.status.success(),
        "read handles must not compile against write terminals"
    );
}

#[test]
fn runtime_and_cancellation_outputs_are_initialized_and_idempotent() {
    let mut runtime = NonNull::<TypeBridgeRuntime>::dangling().as_ptr();
    let mut diagnostics = NonNull::<TypeBridgeExecutionDiagnostics>::dangling().as_ptr();
    let invalid = TypeBridgeRuntimeConfigV1 {
        struct_size: size_of::<TypeBridgeRuntimeConfigV1>() as u32,
        version: 1,
        worker_threads: 2,
        reserved0: 1,
        reserved: [0; 4],
    };
    assert_eq!(
        unsafe { type_bridge_runtime_open_v1(&invalid, &mut runtime, &mut diagnostics) },
        TypeBridgeStatus::InvalidArgument
    );
    assert!(runtime.is_null());
    assert!(!diagnostics.is_null());
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_close(&mut diagnostics) },
        TypeBridgeStatus::Ok
    );

    for worker_threads in [0, 65] {
        let outside_bounds = TypeBridgeRuntimeConfigV1 {
            worker_threads,
            reserved0: 0,
            ..invalid
        };
        assert_eq!(
            unsafe { type_bridge_runtime_open_v1(&outside_bounds, &mut runtime, &mut diagnostics) },
            TypeBridgeStatus::ResourceLimit
        );
        assert!(runtime.is_null());
        assert!(!diagnostics.is_null());
        assert_eq!(
            unsafe { type_bridge_execution_diagnostics_close(&mut diagnostics) },
            TypeBridgeStatus::Ok
        );
    }

    let valid = TypeBridgeRuntimeConfigV1 {
        worker_threads: 1,
        reserved0: 0,
        ..invalid
    };
    assert_eq!(
        unsafe { type_bridge_runtime_open_v1(&valid, &mut runtime, &mut diagnostics) },
        TypeBridgeStatus::Ok
    );
    assert!(!runtime.is_null());
    assert!(diagnostics.is_null());

    let mut cancellation = NonNull::<TypeBridgeCancellation>::dangling().as_ptr();
    assert_eq!(
        unsafe { type_bridge_cancellation_open(&mut cancellation) },
        TypeBridgeStatus::Ok
    );
    let mut requested = u8::MAX;
    assert_eq!(
        unsafe { type_bridge_cancellation_is_requested(cancellation, &mut requested) },
        TypeBridgeStatus::Ok
    );
    assert_eq!(requested, 0);
    assert_eq!(
        unsafe { type_bridge_cancellation_request(cancellation) },
        TypeBridgeStatus::Ok
    );
    assert_eq!(
        unsafe { type_bridge_cancellation_is_requested(cancellation, &mut requested) },
        TypeBridgeStatus::Ok
    );
    assert_eq!(requested, 1);
    assert_eq!(
        unsafe { type_bridge_cancellation_is_requested(ptr::null(), &mut requested) },
        TypeBridgeStatus::InvalidArgument
    );
    assert_eq!(requested, 0);
    assert_eq!(
        unsafe { type_bridge_cancellation_close(&mut cancellation) },
        TypeBridgeStatus::Ok
    );
    assert!(cancellation.is_null());
    assert_eq!(
        unsafe { type_bridge_cancellation_close(&mut cancellation) },
        TypeBridgeStatus::Ok
    );

    let runtime_before = runtime;
    let runtime_slot = &mut runtime as *mut *mut TypeBridgeRuntime;
    assert_eq!(
        unsafe { type_bridge_runtime_close(runtime_slot, runtime_slot.cast()) },
        TypeBridgeStatus::InvalidArgument
    );
    assert_eq!(runtime, runtime_before);

    assert_eq!(
        unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
        TypeBridgeStatus::Ok
    );
    assert!(runtime.is_null());
    assert!(diagnostics.is_null());
    assert_eq!(
        unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
        TypeBridgeStatus::Ok
    );
    let maximum = TypeBridgeRuntimeConfigV1 {
        worker_threads: 64,
        ..valid
    };
    assert_eq!(
        unsafe { type_bridge_runtime_open_v1(&maximum, &mut runtime, &mut diagnostics) },
        TypeBridgeStatus::Ok
    );
    assert_eq!(
        unsafe { type_bridge_runtime_close(&mut runtime, &mut diagnostics) },
        TypeBridgeStatus::Ok
    );
}
