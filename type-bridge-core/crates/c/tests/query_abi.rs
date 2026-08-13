use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "typebridge-c-query-abi-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("unique query ABI directory is created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("query ABI directory is removed");
    }
}

#[test]
fn public_query_abi_compiles_under_strict_c_and_cpp() {
    let stage = TempDirectory::new();
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let source = stage.path().join("query-abi.c");
    fs::write(
        &source,
        r#"#include <stddef.h>
#include <stdint.h>

#include <typebridge/type_bridge.h>

#ifdef __cplusplus
#define QUERY_STATIC_ASSERT(value, message) static_assert(value, message)
#else
#define QUERY_STATIC_ASSERT(value, message) _Static_assert(value, message)
#endif

QUERY_STATIC_ASSERT(TYPE_BRIDGE_C_ABI_MAJOR == 1u, "query ABI major drifted");
QUERY_STATIC_ASSERT(TYPE_BRIDGE_C_ABI_MINOR == 3u, "query ABI minor drifted");
QUERY_STATIC_ASSERT(TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION == 1u,
                    "query descriptor version drifted");
QUERY_STATIC_ASSERT(TYPE_BRIDGE_QUERY_TERMINAL_FIRST == 8u,
                    "first terminal tag drifted");
QUERY_STATIC_ASSERT(TYPE_BRIDGE_PROJECTED_TOKEN_FUNCTION == 4u,
                    "function token tag drifted");
QUERY_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION == 31,
                    "function input tag drifted");
QUERY_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION_VALUE == 32,
                    "function value input tag drifted");
QUERY_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION_CALL == 33,
                    "function call input tag drifted");
QUERY_STATIC_ASSERT(sizeof(type_bridge_query_function_argument_v1_t) == 72u,
                    "function witness layout drifted");
QUERY_STATIC_ASSERT(sizeof(type_bridge_query_function_argument_member_v1_t) <= 65535u,
                    "function member descriptor exceeds the C object floor");
QUERY_STATIC_ASSERT(sizeof(type_bridge_query_order_descriptor_v1_t) <= 65535u,
                    "order descriptor exceeds the C object floor");
QUERY_STATIC_ASSERT(sizeof(type_bridge_query_selection_descriptor_v1_t) <= 65535u,
                    "selection descriptor exceeds the C object floor");
QUERY_STATIC_ASSERT(sizeof(type_bridge_query_shape_slot_v1_t) <= 65535u,
                    "shape descriptor exceeds the C object floor");
QUERY_STATIC_ASSERT(sizeof(type_bridge_query_terminal_descriptor_v1_t) <= 65535u,
                    "terminal descriptor exceeds the C object floor");

typedef type_bridge_status_t (TYPE_BRIDGE_CALL *query_session_open_fn)(
    const type_bridge_schema_package_t *, type_bridge_query_session_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *query_binding_open_fn)(
    const type_bridge_query_session_t *, const type_bridge_projected_token_v1_t *,
    type_bridge_query_match_mode_t, type_bridge_query_binding_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *query_function_call_open_fn)(
    const type_bridge_query_function_t *,
    const type_bridge_projected_token_v1_t *,
    const type_bridge_query_function_arguments_graph_v1_t *,
    type_bridge_query_function_call_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *query_open_fn)(
    const type_bridge_query_session_t *, const type_bridge_query_descriptor_v1_t *,
    type_bridge_query_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *query_terminal_open_fn)(
    const type_bridge_query_t *,
    const type_bridge_query_terminal_descriptor_v1_t *,
    type_bridge_query_terminal_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_execute_fn)(
    const type_bridge_database_t *, const type_bridge_query_terminal_t *,
    type_bridge_query_terminal_kind_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_query_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *remote_prepare_fn)(
    const type_bridge_query_remote_context_t *,
    const type_bridge_query_terminal_t *, type_bridge_query_terminal_kind_t,
    const type_bridge_cancellation_t *, type_bridge_query_remote_pending_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *row_thing_fn)(
    const type_bridge_query_result_t *, type_bridge_query_result_kind_t,
    size_t, size_t, size_t, const type_bridge_projected_token_v1_t *,
    type_bridge_query_match_mode_t, type_bridge_query_selection_kind_t,
    type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *reduced_metadata_fn)(
    const type_bridge_query_result_t *, type_bridge_query_result_kind_t,
    size_t, size_t, type_bridge_query_reduced_value_kind_t,
    type_bridge_query_reduced_value_metadata_v1_t *,
    type_bridge_execution_diagnostics_t **);

#define QUERY_TAKE(symbol) do { (void)&symbol; } while (0)

void query_abi_probe(void) {
  query_session_open_fn session_open = &type_bridge_query_session_open;
  query_binding_open_fn binding_open = &type_bridge_query_binding_open_v1;
  query_function_call_open_fn function_call_open =
      &type_bridge_query_function_call_open_v1;
  query_open_fn query_open = &type_bridge_query_open_v1;
  query_terminal_open_fn terminal_open = &type_bridge_query_terminal_open_v1;
  database_execute_fn database_execute = &type_bridge_database_query_execute_v1;
  remote_prepare_fn remote_prepare = &type_bridge_query_remote_prepare_v1;
  row_thing_fn row_thing = &type_bridge_query_result_row_slot_thing_at;
  reduced_metadata_fn reduced_metadata =
      &type_bridge_query_result_reduction_value_metadata_v1;
  type_bridge_query_order_descriptor_v1_t order;
  type_bridge_query_selection_descriptor_v1_t selection;
  type_bridge_query_shape_slot_v1_t shape;
  type_bridge_query_descriptor_v1_t query;
  type_bridge_query_reducer_v1_t reducer;
  type_bridge_query_field_reference_v1_t group_field;
  type_bridge_query_terminal_descriptor_v1_t terminal;
  type_bridge_query_execution_limits_v1_t limits =
      TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;
  type_bridge_query_function_arguments_header_v1_t function_header;
  type_bridge_query_function_argument_v1_t function_argument;
  type_bridge_query_function_argument_member_v1_t function_member;
  type_bridge_query_function_arguments_graph_v1_t function_graph;
  (void)session_open;
  (void)binding_open;
  (void)function_call_open;
  (void)query_open;
  (void)terminal_open;
  (void)database_execute;
  (void)remote_prepare;
  (void)row_thing;
  (void)reduced_metadata;
  (void)order;
  (void)selection;
  (void)shape;
  (void)query;
  (void)reducer;
  (void)group_field;
  (void)terminal;
  (void)limits;
  (void)function_header;
  (void)function_argument;
  (void)function_member;
  (void)function_graph;

  QUERY_TAKE(type_bridge_query_field_compare_field);
  QUERY_TAKE(type_bridge_query_field_compare_function);
  QUERY_TAKE(type_bridge_query_role_connects);
  QUERY_TAKE(type_bridge_query_session_reachable);
  QUERY_TAKE(type_bridge_query_predicate_combine);
  QUERY_TAKE(type_bridge_query_add_hidden);
  QUERY_TAKE(type_bridge_query_where);
  QUERY_TAKE(type_bridge_query_allow_cross_join);
  QUERY_TAKE(type_bridge_read_transaction_query_execute_v1);
  QUERY_TAKE(type_bridge_query_remote_pending_request_bytes);
  QUERY_TAKE(type_bridge_query_remote_pending_response_snapshot_limit);
  QUERY_TAKE(type_bridge_query_remote_pending_claim);
  QUERY_TAKE(type_bridge_query_remote_claim_decode_v1);
  QUERY_TAKE(type_bridge_query_result_page_metadata_v1);
  QUERY_TAKE(type_bridge_query_result_reduction_group_thing);
  QUERY_TAKE(type_bridge_query_result_reduction_group_field_at);
  QUERY_TAKE(type_bridge_query_result_reduction_value_count);
  QUERY_TAKE(type_bridge_query_result_reduction_value_long);
  QUERY_TAKE(type_bridge_query_result_reduction_value_double_bits);
}
"#,
    )
    .expect("query ABI probe is written");

    #[cfg(not(windows))]
    let compilers: &[(&str, &str, &str, &str)] = if cfg!(target_os = "macos") {
        &[
            ("clang", "c11", "c", "clang C11"),
            ("clang", "c17", "c", "clang C17"),
            ("clang++", "c++17", "c++", "clang++ C++17"),
        ]
    } else {
        &[
            ("gcc", "c11", "c", "gcc C11"),
            ("gcc", "c17", "c", "gcc C17"),
            ("clang", "c11", "c", "clang C11"),
            ("clang", "c17", "c", "clang C17"),
            ("g++", "c++17", "c++", "g++ C++17"),
            ("clang++", "c++17", "c++", "clang++ C++17"),
        ]
    };

    #[cfg(not(windows))]
    for &(compiler, standard, language, label) in compilers {
        let object = stage
            .path()
            .join(format!("query-abi-{compiler}-{standard}.o"));
        let output = Command::new(compiler)
            .arg(format!("-std={standard}"))
            .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors", "-c"])
            .args(["-x", language])
            .arg("-I")
            .arg(&include)
            .arg(&source)
            .arg("-o")
            .arg(&object)
            .output()
            .unwrap_or_else(|error| {
                panic!("required {label} compiler is unavailable or failed to launch: {error}")
            });
        assert!(
            output.status.success(),
            "{label} rejected the public query ABI probe:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[cfg(windows)]
    for (index, (compiler, standard, language, label)) in [
        ("cl", "/std:c11", "/TC", "MSVC C11"),
        ("cl", "/std:c17", "/TC", "MSVC C17"),
        ("cl", "/std:c++17", "/TP", "MSVC C++17"),
        ("clang-cl", "/std:c11", "/TC", "clang-cl C11"),
        ("clang-cl", "/std:c17", "/TC", "clang-cl C17"),
        ("clang-cl", "/std:c++17", "/TP", "clang-cl C++17"),
    ]
    .into_iter()
    .enumerate()
    {
        let object = stage.path().join(format!("query-abi-{index}.obj"));
        let output = Command::new(compiler)
            .args(["/nologo", standard, language, "/W4", "/WX", "/c"])
            .arg(format!("/I{}", include.display()))
            .arg(&source)
            .arg(format!("/Fo{}", object.display()))
            .current_dir(stage.path())
            .output()
            .unwrap_or_else(|error| {
                panic!("required {label} compiler is unavailable or failed to launch: {error}")
            });
        assert!(
            output.status.success(),
            "{label} rejected the public query ABI probe:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
