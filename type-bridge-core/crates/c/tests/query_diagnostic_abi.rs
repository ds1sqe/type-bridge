use std::fs;
use std::mem::{align_of, offset_of, size_of};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_c::{
    TypeBridgeExecutionDiagnosticDetailKind, TypeBridgeExecutionDiagnosticDetailViewV1,
    TypeBridgeExecutionDiagnosticPathKind, TypeBridgeExecutionDiagnosticPathViewV1,
    TypeBridgeExecutionDiagnosticViewV1, TypeBridgeQueryExecutionLimitsV1,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "typebridge-c-query-diagnostic-abi-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("unique query diagnostic ABI directory is created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("query diagnostic ABI directory is removed");
    }
}

fn parse_layout(output: &[u8]) -> Vec<usize> {
    std::str::from_utf8(output)
        .expect("layout output is UTF-8")
        .split_whitespace()
        .map(|value| value.parse().expect("layout value is an integer"))
        .collect()
}

fn expected_layout() -> Vec<usize> {
    let mut values = vec![
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 255, 1, 2, 3, 4, 5,
        6, 7, 8, 9, 10, 11, 12, 13, 14, 255, 1, 30_000, 65_536, 33_554_432, 65_536, 65_536, 65_536,
        65_536, 3,
    ];
    values.extend([
        size_of::<TypeBridgeQueryExecutionLimitsV1>(),
        align_of::<TypeBridgeQueryExecutionLimitsV1>(),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, struct_size),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, version),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, timeout_milliseconds),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, items),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, bytes),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, graph_nodes),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, attribute_values),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, collection_members),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, role_players),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, statements),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, reserved0),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, reserved),
        size_of::<TypeBridgeExecutionDiagnosticViewV1>(),
        align_of::<TypeBridgeExecutionDiagnosticViewV1>(),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, struct_size),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, version),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, category),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, reserved0),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, code),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, message),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, path_count),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, detail_count),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, reserved),
        size_of::<TypeBridgeExecutionDiagnosticPathViewV1>(),
        align_of::<TypeBridgeExecutionDiagnosticPathViewV1>(),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, struct_size),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, kind),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, index),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, primary),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, secondary),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, tertiary),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, reserved),
        size_of::<TypeBridgeExecutionDiagnosticDetailViewV1>(),
        align_of::<TypeBridgeExecutionDiagnosticDetailViewV1>(),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, struct_size),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, kind),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, boolean_value),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, reserved0),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, unsigned_value),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, key),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, primary),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, secondary),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, tertiary),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, quaternary),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, reserved),
    ]);
    values
}

#[cfg(not(windows))]
fn verify_compiler(
    stage: &TempDirectory,
    include: &Path,
    source: &Path,
    compiler: &str,
    standard: &str,
    language: &str,
    label: &str,
) {
    let artifact = format!("{compiler}-{standard}");
    let object = stage
        .path()
        .join(format!("query-diagnostic-types-{artifact}.o"));
    let output = Command::new(compiler)
        .arg(format!("-std={standard}"))
        .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors"])
        .args(["-x", language, "-DTYPE_BRIDGE_FUNCTION_PROBE", "-c"])
        .arg("-I")
        .arg(include)
        .arg(source)
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap_or_else(|error| {
            panic!("required {label} compiler is unavailable or failed to launch: {error}")
        });
    assert!(
        output.status.success(),
        "{label} rejected query diagnostic ABI types:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let executable = stage
        .path()
        .join(format!("query-diagnostic-layout-{artifact}"));
    let output = Command::new(compiler)
        .arg(format!("-std={standard}"))
        .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors"])
        .args(["-x", language])
        .arg("-I")
        .arg(include)
        .arg(source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap_or_else(|error| {
            panic!("required {label} compiler is unavailable or failed to launch: {error}")
        });
    assert!(
        output.status.success(),
        "{label} rejected query diagnostic layout:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let output = Command::new(&executable).output().unwrap_or_else(|error| {
        panic!("{label} query diagnostic layout probe failed to run: {error}")
    });
    assert!(
        output.status.success(),
        "{label} query diagnostic layout probe failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(parse_layout(&output.stdout), expected_layout(), "{label}");
}

#[cfg(windows)]
fn verify_compiler(
    stage: &TempDirectory,
    include: &Path,
    source: &Path,
    compiler: &str,
    standard: &str,
    language: &str,
    label: &str,
) {
    let artifact = format!("{compiler}-{standard}");
    let language_flag = if language == "c" { "/TC" } else { "/TP" };
    let object = stage
        .path()
        .join(format!("query-diagnostic-types-{artifact}.obj"));
    let output = Command::new(compiler)
        .args(["/nologo", language_flag, "/W4", "/WX", "/c"])
        .arg(format!("/std:{standard}"))
        .arg("/DTYPE_BRIDGE_FUNCTION_PROBE")
        .arg(format!("/I{}", include.display()))
        .arg(source)
        .arg(format!("/Fo{}", object.display()))
        .current_dir(stage.path())
        .output()
        .unwrap_or_else(|error| {
            panic!("required {label} compiler is unavailable or failed to launch: {error}")
        });
    assert!(
        output.status.success(),
        "{label} rejected query diagnostic ABI types:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let executable = stage
        .path()
        .join(format!("query-diagnostic-layout-{artifact}.exe"));
    let output = Command::new(compiler)
        .args(["/nologo", language_flag, "/W4", "/WX"])
        .arg(format!("/std:{standard}"))
        .arg(format!("/I{}", include.display()))
        .arg(source)
        .arg(format!("/Fe{}", executable.display()))
        .current_dir(stage.path())
        .output()
        .unwrap_or_else(|error| {
            panic!("required {label} compiler is unavailable or failed to launch: {error}")
        });
    assert!(
        output.status.success(),
        "{label} rejected query diagnostic layout:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let output = Command::new(&executable).output().unwrap_or_else(|error| {
        panic!("{label} query diagnostic layout probe failed to run: {error}")
    });
    assert!(
        output.status.success(),
        "{label} query diagnostic layout probe failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(parse_layout(&output.stdout), expected_layout(), "{label}");
}

#[test]
fn strict_c17_and_cpp17_match_query_diagnostic_numeric_and_layout_contracts() {
    let stage = TempDirectory::new();
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let source = stage.path().join("query-diagnostic-layout.c");
    fs::write(
        &source,
        r#"#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <typebridge/type_bridge.h>

#ifdef __cplusplus
#define TYPE_BRIDGE_ALIGNOF(type) alignof(type)
#else
#define TYPE_BRIDGE_ALIGNOF(type) _Alignof(type)
#endif

#define PRINT_VALUE(value) printf("%llu ", (unsigned long long)(value))

#ifdef TYPE_BRIDGE_FUNCTION_PROBE
void type_bridge_query_diagnostic_function_probe(void) {
  type_bridge_status_t (*count)(
      const type_bridge_execution_diagnostics_t *, size_t *) =
      &type_bridge_execution_diagnostics_count;
  type_bridge_status_t (*get)(
      const type_bridge_execution_diagnostics_t *, size_t,
      type_bridge_execution_diagnostic_view_v1_t *) =
      &type_bridge_execution_diagnostics_get_v1;
  type_bridge_status_t (*path_get)(
      const type_bridge_execution_diagnostics_t *, size_t, size_t,
      type_bridge_execution_diagnostic_path_view_v1_t *) =
      &type_bridge_execution_diagnostics_path_get_v1;
  type_bridge_status_t (*detail_get)(
      const type_bridge_execution_diagnostics_t *, size_t, size_t,
      type_bridge_execution_diagnostic_detail_view_v1_t *) =
      &type_bridge_execution_diagnostics_detail_get_v1;
  type_bridge_status_t (*signed_get)(
      const type_bridge_execution_diagnostics_t *, size_t, size_t, int64_t *) =
      &type_bridge_execution_diagnostics_detail_signed;
  type_bridge_status_t (*list_count)(
      const type_bridge_execution_diagnostics_t *, size_t, size_t, size_t *) =
      &type_bridge_execution_diagnostics_detail_list_count;
  type_bridge_status_t (*list_get)(
      const type_bridge_execution_diagnostics_t *, size_t, size_t, size_t,
      type_bridge_byte_view_t *) =
      &type_bridge_execution_diagnostics_detail_list_get;
  type_bridge_status_t (*database_query)(
      const type_bridge_database_t *, const type_bridge_query_terminal_t *,
      type_bridge_query_terminal_kind_t,
      const type_bridge_query_execution_limits_v1_t *,
      const type_bridge_cancellation_t *, type_bridge_query_result_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_database_query_execute_v1;
  (void)count;
  (void)get;
  (void)path_get;
  (void)detail_get;
  (void)signed_get;
  (void)list_count;
  (void)list_get;
  (void)database_query;
}
#endif

int main(void) {
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_ARGUMENT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_INDEX);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_TYPE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_ROLE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_REQUEST);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_PLAN);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OPERATION);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_PREDICATE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OUTPUT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_PROVIDER_EVIDENCE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_RESULT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_BINDING);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_ROLE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_ROLE_EDGE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OUTPUT_SLOT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OUTPUT_NAME);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_CONTRACT_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_CONTRACT_IDENTITY);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_UNKNOWN);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_BOOLEAN);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_COUNT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_BYTE_COUNT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_CAPABILITY);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_VALUE_TYPE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_TYPE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_ROLE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_FINGERPRINT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_PROVIDER_OPERATION);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_COMMIT_OUTCOME);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_TEXT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_SIGNED);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_TEXT_LIST);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_UNKNOWN);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_VERSION);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_TIMEOUT_MILLISECONDS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_ITEMS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_BYTES);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_GRAPH_NODES);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_ATTRIBUTE_VALUES);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_COLLECTION_MEMBERS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_ROLE_PLAYERS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_STATEMENTS);
  PRINT_VALUE(sizeof(type_bridge_query_execution_limits_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_execution_limits_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, timeout_milliseconds));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, items));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, bytes));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, graph_nodes));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, attribute_values));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, collection_members));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, role_players));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, statements));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_execution_diagnostic_view_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_execution_diagnostic_view_v1_t));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, category));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, code));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, message));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, path_count));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, detail_count));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_execution_diagnostic_path_view_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_execution_diagnostic_path_view_v1_t));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, index));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, primary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, secondary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, tertiary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_execution_diagnostic_detail_view_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_execution_diagnostic_detail_view_v1_t));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, boolean_value));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, unsigned_value));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, key));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, primary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, secondary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, tertiary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, quaternary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, reserved));
  printf("\n");
  return 0;
}
"#,
    )
    .expect("query diagnostic layout source is written");

    #[cfg(not(windows))]
    let compilers: &[(&str, &str, &str, &str)] = if cfg!(target_os = "macos") {
        &[
            ("clang", "c17", "c", "clang C17"),
            ("clang++", "c++17", "c++", "clang++ C++17"),
        ]
    } else {
        &[
            ("gcc", "c17", "c", "gcc C17"),
            ("g++", "c++17", "c++", "g++ C++17"),
            ("clang", "c17", "c", "clang C17"),
            ("clang++", "c++17", "c++", "clang++ C++17"),
        ]
    };
    #[cfg(windows)]
    let compilers: &[(&str, &str, &str, &str)] = &[
        ("cl", "c17", "c", "MSVC C17"),
        ("cl", "c++17", "c++", "MSVC C++17"),
        ("clang-cl", "c17", "c", "clang-cl C17"),
        ("clang-cl", "c++17", "c++", "clang-cl C++17"),
    ];

    for &(compiler, standard, language, label) in compilers {
        verify_compiler(
            &stage, &include, &source, compiler, standard, language, label,
        );
    }
}

#[test]
fn rust_numeric_discriminants_remain_exactly_additive() {
    assert_eq!(TypeBridgeExecutionDiagnosticPathKind::Argument as i32, 1);
    assert_eq!(TypeBridgeExecutionDiagnosticPathKind::Role as i32, 5);
    assert_eq!(
        TypeBridgeExecutionDiagnosticPathKind::QueryRequest as i32,
        6
    );
    assert_eq!(
        TypeBridgeExecutionDiagnosticPathKind::QueryOutputName as i32,
        18
    );
    assert_eq!(
        TypeBridgeExecutionDiagnosticPathKind::ContractField as i32,
        19
    );
    assert_eq!(
        TypeBridgeExecutionDiagnosticPathKind::ContractIdentity as i32,
        20
    );
    assert_eq!(TypeBridgeExecutionDiagnosticPathKind::Unknown as i32, 255);
    assert_eq!(TypeBridgeExecutionDiagnosticDetailKind::Boolean as i32, 1);
    assert_eq!(
        TypeBridgeExecutionDiagnosticDetailKind::CommitOutcome as i32,
        11
    );
    assert_eq!(TypeBridgeExecutionDiagnosticDetailKind::Text as i32, 12);
    assert_eq!(TypeBridgeExecutionDiagnosticDetailKind::Signed as i32, 13);
    assert_eq!(TypeBridgeExecutionDiagnosticDetailKind::TextList as i32, 14);
    assert_eq!(TypeBridgeExecutionDiagnosticDetailKind::Unknown as i32, 255);
}
