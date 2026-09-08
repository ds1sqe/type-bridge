use std::fs;
use std::mem::{align_of, offset_of, size_of};
use std::path::Path;
use std::process::Command;

use type_bridge_c::{
    TypeBridgeQueryFunctionArgumentMemberV1, TypeBridgeQueryFunctionArgumentV1,
    TypeBridgeQueryFunctionArgumentsGraphV1, TypeBridgeQueryFunctionArgumentsHeaderV1,
};

#[path = "support/temp.rs"]
mod temp;
use temp::TempDirectory;

fn parse_layout(output: &[u8]) -> Vec<usize> {
    std::str::from_utf8(output)
        .expect("layout output is UTF-8")
        .split_whitespace()
        .map(|value| value.parse().expect("layout value is an integer"))
        .collect()
}

fn expected_layout() -> Vec<usize> {
    vec![
        size_of::<TypeBridgeQueryFunctionArgumentV1>(),
        align_of::<TypeBridgeQueryFunctionArgumentV1>(),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, binding),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, value),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, call),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, reserved),
        size_of::<TypeBridgeQueryFunctionArgumentMemberV1>(),
        align_of::<TypeBridgeQueryFunctionArgumentMemberV1>(),
        offset_of!(TypeBridgeQueryFunctionArgumentMemberV1, args_offset),
        size_of::<TypeBridgeQueryFunctionArgumentsHeaderV1>(),
        align_of::<TypeBridgeQueryFunctionArgumentsHeaderV1>(),
        size_of::<TypeBridgeQueryFunctionArgumentsGraphV1>(),
        align_of::<TypeBridgeQueryFunctionArgumentsGraphV1>(),
        offset_of!(TypeBridgeQueryFunctionArgumentsGraphV1, args),
        offset_of!(TypeBridgeQueryFunctionArgumentsGraphV1, members),
    ]
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
        .join(format!("query-function-types-{artifact}.o"));
    let output = Command::new(compiler)
        .arg(format!("-std={standard}"))
        .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors"])
        .args(["-x", language, "-c"])
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
        "{label} rejected query-function ABI types:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let executable = stage
        .path()
        .join(format!("query-function-layout-{artifact}"));
    let output = Command::new(compiler)
        .arg(format!("-std={standard}"))
        .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors"])
        .args(["-x", language, "-DTYPE_BRIDGE_LAYOUT_PROBE"])
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
        "{label} rejected query-function layout probe:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let output = Command::new(&executable).output().unwrap_or_else(|error| {
        panic!("{label} query-function layout probe failed to run: {error}")
    });
    assert!(
        output.status.success(),
        "{label} query-function layout probe failed:\nstdout:\n{}\nstderr:\n{}",
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
        .join(format!("query-function-types-{artifact}.obj"));
    let output = Command::new(compiler)
        .args(["/nologo", language_flag, "/W4", "/WX", "/c"])
        .arg(format!("/std:{standard}"))
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
        "{label} rejected query-function ABI types:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let executable = stage
        .path()
        .join(format!("query-function-layout-{artifact}.exe"));
    let output = Command::new(compiler)
        .args(["/nologo", language_flag, "/W4", "/WX"])
        .arg(format!("/std:{standard}"))
        .arg("/DTYPE_BRIDGE_LAYOUT_PROBE")
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
        "{label} rejected query-function layout probe:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let output = Command::new(&executable).output().unwrap_or_else(|error| {
        panic!("{label} query-function layout probe failed to run: {error}")
    });
    assert!(
        output.status.success(),
        "{label} query-function layout probe failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(parse_layout(&output.stdout), expected_layout(), "{label}");
}

#[test]
fn strict_c17_and_cpp17_accept_exact_query_function_layout_and_types() {
    let stage = TempDirectory::new("query_function_abi");
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let source = stage.path().join("query-function-types.c");
    fs::write(
        &source,
        r#"#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <typebridge/type_bridge.h>

#ifdef __cplusplus
#define TYPE_BRIDGE_STATIC_ASSERT(value, message) static_assert(value, message)
#else
#define TYPE_BRIDGE_STATIC_ASSERT(value, message) _Static_assert(value, message)
#endif

#ifdef __cplusplus
#include <type_traits>
TYPE_BRIDGE_STATIC_ASSERT(std::is_standard_layout<type_bridge_query_function_argument_v1_t>::value,
    "function argument must be standard-layout");
TYPE_BRIDGE_STATIC_ASSERT(std::is_standard_layout<type_bridge_query_function_argument_member_v1_t>::value,
    "function argument member must be standard-layout");
TYPE_BRIDGE_STATIC_ASSERT(std::is_standard_layout<type_bridge_query_function_arguments_header_v1_t>::value,
    "function arguments header must be standard-layout");
TYPE_BRIDGE_STATIC_ASSERT(std::is_standard_layout<type_bridge_query_function_arguments_graph_v1_t>::value,
    "function arguments graph must be standard-layout");
#define TYPE_BRIDGE_ALIGNOF(type) alignof(type)
#else
#define TYPE_BRIDGE_ALIGNOF(type) _Alignof(type)
#endif

TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_PROJECTED_TOKEN_FUNCTION == 4u,
    "function token kind drifted");
TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_BINDING == 1,
    "binding argument kind drifted");
TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_VALUE == 2,
    "value argument kind drifted");
TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_CALL == 3,
    "call argument kind drifted");
TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_MAX == 256u,
    "function argument ceiling drifted");
TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENTS_BYTES_MAX == 65535u,
    "function argument object byte ceiling drifted");
TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION == 31,
    "function opaque-input tag drifted");
TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION_VALUE == 32,
    "function value opaque-input tag drifted");
TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION_CALL == 33,
    "function call opaque-input tag drifted");
TYPE_BRIDGE_STATIC_ASSERT(sizeof(type_bridge_query_function_argument_v1_t) == 72,
    "function argument descriptor size drifted");
TYPE_BRIDGE_STATIC_ASSERT(offsetof(type_bridge_query_function_argument_v1_t, binding) == 16,
    "function argument binding offset drifted");
TYPE_BRIDGE_STATIC_ASSERT(offsetof(type_bridge_query_function_argument_v1_t, value) == 24,
    "function argument value offset drifted");
TYPE_BRIDGE_STATIC_ASSERT(offsetof(type_bridge_query_function_argument_v1_t, call) == 32,
    "function argument call offset drifted");
TYPE_BRIDGE_STATIC_ASSERT(offsetof(type_bridge_query_function_argument_v1_t, reserved) == 40,
    "function argument reserved offset drifted");
TYPE_BRIDGE_STATIC_ASSERT(sizeof(type_bridge_query_function_argument_member_v1_t) == 48,
    "function argument member size drifted");
TYPE_BRIDGE_STATIC_ASSERT(offsetof(type_bridge_query_function_argument_member_v1_t, args_offset) == 8,
    "function argument member offset drifted");
TYPE_BRIDGE_STATIC_ASSERT(sizeof(type_bridge_query_function_arguments_header_v1_t) == 40,
    "function arguments header size drifted");
TYPE_BRIDGE_STATIC_ASSERT(sizeof(type_bridge_query_function_arguments_graph_v1_t) == 64,
    "function arguments graph size drifted");
TYPE_BRIDGE_STATIC_ASSERT(offsetof(type_bridge_query_function_arguments_graph_v1_t, args) == 8,
    "function arguments graph args offset drifted");
TYPE_BRIDGE_STATIC_ASSERT(offsetof(type_bridge_query_function_arguments_graph_v1_t, members) == 24,
    "function arguments graph members offset drifted");

#ifdef TYPE_BRIDGE_LAYOUT_PROBE
#define PRINT_VALUE(value) printf("%llu ", (unsigned long long)(value))
int main(void) {
  PRINT_VALUE(sizeof(type_bridge_query_function_argument_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_function_argument_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, binding));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, value));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, call));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_function_argument_member_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_function_argument_member_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_member_v1_t, args_offset));
  PRINT_VALUE(sizeof(type_bridge_query_function_arguments_header_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_function_arguments_header_v1_t));
  PRINT_VALUE(sizeof(type_bridge_query_function_arguments_graph_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_function_arguments_graph_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_graph_v1_t, args));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_graph_v1_t, members));
  return 0;
}
#else
void type_bridge_query_function_probe(void) {
  type_bridge_status_t (*function_open)(const type_bridge_query_session_t *,
      const type_bridge_projected_token_v1_t *, type_bridge_query_function_t **,
      type_bridge_execution_diagnostics_t **) = &type_bridge_query_function_open;
  type_bridge_status_t (*function_close)(type_bridge_query_function_t **) =
      &type_bridge_query_function_close;
  type_bridge_status_t (*value_open)(const type_bridge_query_session_t *,
      const type_bridge_projected_value_t *, type_bridge_query_function_value_t **,
      type_bridge_execution_diagnostics_t **) = &type_bridge_query_function_value_open;
  type_bridge_status_t (*value_close)(type_bridge_query_function_value_t **) =
      &type_bridge_query_function_value_close;
  type_bridge_status_t (*call_open)(const type_bridge_query_function_t *,
      const type_bridge_projected_token_v1_t *,
      const type_bridge_query_function_arguments_graph_v1_t *,
      type_bridge_query_function_call_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_query_function_call_open_v1;
  type_bridge_status_t (*call_close)(type_bridge_query_function_call_t **) =
      &type_bridge_query_function_call_close;
  type_bridge_status_t (*call_field)(const type_bridge_query_function_call_t *,
      type_bridge_query_comparison_t, const type_bridge_query_field_t *,
      const type_bridge_projected_token_v1_t *, type_bridge_query_predicate_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_query_function_call_compare_field;
  type_bridge_status_t (*call_value)(const type_bridge_query_function_call_t *,
      type_bridge_query_comparison_t, const type_bridge_query_function_value_t *,
      type_bridge_query_predicate_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_query_function_call_compare_value;
  type_bridge_status_t (*call_call)(const type_bridge_query_function_call_t *,
      type_bridge_query_comparison_t, const type_bridge_query_function_call_t *,
      type_bridge_query_predicate_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_query_function_call_compare_call;
  type_bridge_status_t (*field_call)(const type_bridge_query_field_t *,
      const type_bridge_projected_token_v1_t *, type_bridge_query_comparison_t,
      const type_bridge_query_function_call_t *, type_bridge_query_predicate_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_query_field_compare_function;
  (void)function_open;
  (void)function_close;
  (void)value_open;
  (void)value_close;
  (void)call_open;
  (void)call_close;
  (void)call_field;
  (void)call_value;
  (void)call_call;
  (void)field_call;
}
#endif
"#,
    )
    .expect("query-function probe is written");

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
