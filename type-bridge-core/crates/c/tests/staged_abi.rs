use std::fs;
use std::path::Path;
use std::process::Command;

#[path = "support/temp.rs"]
mod temp;
use temp::TempDirectory;

/// A strict C17 and C++17 consumer of the ABI 1.7 staged surface: exact
/// prototypes through typed function pointers, and the call sequences of a
/// reusable transaction (commit with retry, rollback then commit, commit then
/// stage again). The fake-provider unit tests in `src/staged.rs` run them.
const STAGED_CONSUMER: &str = r#"#include <stddef.h>
#include <stdint.h>

#include <typebridge/type_bridge.h>

#ifdef __cplusplus
#define STAGED_STATIC_ASSERT(value, message) static_assert(value, message)
#else
#define STAGED_STATIC_ASSERT(value, message) _Static_assert(value, message)
#endif

STAGED_STATIC_ASSERT(TYPE_BRIDGE_C_ABI_MAJOR == 1u, "staged ABI major drifted");
STAGED_STATIC_ASSERT(TYPE_BRIDGE_C_ABI_MINOR == 7u, "staged ABI minor drifted");
STAGED_STATIC_ASSERT(TYPE_BRIDGE_TRANSACTION_READ == 1u, "read kind drifted");
STAGED_STATIC_ASSERT(TYPE_BRIDGE_TRANSACTION_WRITE == 2u, "write kind drifted");
STAGED_STATIC_ASSERT(TYPE_BRIDGE_PLAN_ANSWER_ROWS == 1, "rows answer drifted");
STAGED_STATIC_ASSERT(TYPE_BRIDGE_PLAN_ANSWER_COUNT == 2, "count answer drifted");
STAGED_STATIC_ASSERT(TYPE_BRIDGE_PLAN_ANSWER_EXISTS == 3, "exists answer drifted");
STAGED_STATIC_ASSERT(TYPE_BRIDGE_PLAN_ANSWER_MATCHED == 4, "matched answer drifted");
STAGED_STATIC_ASSERT(sizeof(type_bridge_plan_answer_kind_t) == sizeof(uint32_t),
                     "answer kind is not 32 bits");
/* The typed failure view matches the Rust layout on 64-bit targets. */
STAGED_STATIC_ASSERT(sizeof(void *) != 8 ||
                         sizeof(type_bridge_execution_failure_v1_t) == 80,
                     "failure view size drifted");
STAGED_STATIC_ASSERT(offsetof(type_bridge_execution_failure_v1_t, retry_safe) == 24,
                     "failure retry_safe offset drifted");
STAGED_STATIC_ASSERT(offsetof(type_bridge_execution_failure_v1_t, op_index) == 28,
                     "failure op_index offset drifted");
STAGED_STATIC_ASSERT(offsetof(type_bridge_execution_failure_v1_t, code) == 32,
                     "failure code offset drifted");
STAGED_STATIC_ASSERT(sizeof(void *) != 8 ||
                         offsetof(type_bridge_execution_failure_v1_t, reserved) == 48,
                     "failure reserved offset drifted");
STAGED_STATIC_ASSERT(TYPE_BRIDGE_FAILURE_OUTCOME_REJECTED == 1u &&
                         TYPE_BRIDGE_FAILURE_OUTCOME_INDETERMINATE == 2u,
                     "failure outcomes drifted");
STAGED_STATIC_ASSERT(TYPE_BRIDGE_FAILURE_REASON_CONFLICT == 1u &&
                         TYPE_BRIDGE_FAILURE_REASON_OTHER == 5u,
                     "failure reasons drifted");
STAGED_STATIC_ASSERT(TYPE_BRIDGE_FAILURE_LOCK_EXCLUSIVE == 1u &&
                         TYPE_BRIDGE_FAILURE_LOCK_UNKNOWN == 4u,
                     "failure lock kinds drifted");
STAGED_STATIC_ASSERT(TYPE_BRIDGE_FAILURE_ANNOTATION_DISTINCT == 5u &&
                         TYPE_BRIDGE_FAILURE_ANNOTATION_UNKNOWN == 6u,
                     "failure annotations drifted");

typedef type_bridge_status_t (TYPE_BRIDGE_CALL *plan_decode_fn)(
    const uint8_t *, size_t, const uint8_t *, size_t, type_bridge_plan_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *plan_bytes_fn)(
    const type_bridge_plan_t *, uint8_t *, size_t, size_t *);
typedef void (TYPE_BRIDGE_CALL *plan_free_fn)(type_bridge_plan_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *transaction_fn)(
    const type_bridge_database_t *, uint32_t,
    type_bridge_staged_transaction_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_transaction_fn)(
    const type_bridge_proxy_connection_t *, uint32_t,
    type_bridge_staged_transaction_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *stage_fn)(
    type_bridge_staged_transaction_t *, const type_bridge_plan_t *,
    type_bridge_pending_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *commit_fn)(
    type_bridge_staged_transaction_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *rollback_fn)(
    type_bridge_staged_transaction_t *);
typedef void (TYPE_BRIDGE_CALL *close_fn)(type_bridge_staged_transaction_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *answer_fn)(
    const type_bridge_pending_t *, type_bridge_plan_answer_kind_t *, uint64_t *);
typedef void (TYPE_BRIDGE_CALL *pending_free_fn)(type_bridge_pending_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *failure_get_fn)(
    const type_bridge_execution_diagnostics_t *, size_t,
    type_bridge_execution_failure_v1_t *);

void staged_prototypes(void);
void staged_prototypes(void) {
  plan_decode_fn decode = &type_bridge_plan_v3_decode;
  plan_bytes_fn bytes = &type_bridge_plan_v3_canonical_bytes;
  plan_free_fn plan_free = &type_bridge_plan_free;
  transaction_fn transaction = &type_bridge_database_transaction;
  proxy_transaction_fn proxy_transaction = &type_bridge_proxy_connection_transaction;
  stage_fn stage = &type_bridge_staged_transaction_stage;
  commit_fn commit = &type_bridge_staged_transaction_commit;
  commit_fn commit_retry = &type_bridge_staged_transaction_commit_retry;
  rollback_fn rollback = &type_bridge_staged_transaction_rollback;
  close_fn close = &type_bridge_staged_transaction_close;
  answer_fn answer = &type_bridge_pending_answer;
  pending_free_fn pending_free = &type_bridge_pending_free;
  failure_get_fn failure_get = &type_bridge_execution_diagnostics_failure_get_v1;
  (void)decode;
  (void)bytes;
  (void)plan_free;
  (void)transaction;
  (void)proxy_transaction;
  (void)stage;
  (void)commit;
  (void)commit_retry;
  (void)rollback;
  (void)close;
  (void)answer;
  (void)pending_free;
  (void)failure_get;
}

/* Decode, stage, commit (retrying an unknown outcome once) and read. */
int staged_consume(const type_bridge_database_t *database,
                   const uint8_t *schema, size_t schema_len,
                   const uint8_t *plan_bytes, size_t plan_len,
                   uint8_t *echo, size_t echo_capacity,
                   uint64_t *out_value);
int staged_consume(const type_bridge_database_t *database,
                   const uint8_t *schema, size_t schema_len,
                   const uint8_t *plan_bytes, size_t plan_len,
                   uint8_t *echo, size_t echo_capacity,
                   uint64_t *out_value) {
  type_bridge_plan_t *plan = NULL;
  type_bridge_staged_transaction_t *transaction = NULL;
  type_bridge_pending_t *pending = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_plan_answer_kind_t kind = TYPE_BRIDGE_PLAN_ANSWER_ROWS;
  type_bridge_status_t status;
  size_t length = 0u;
  int result = 1;

  if (type_bridge_plan_v3_decode(schema, schema_len, plan_bytes, plan_len,
                                 &plan, &diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    goto done;
  }
  if (type_bridge_plan_v3_canonical_bytes(plan, echo, echo_capacity, &length) !=
          TYPE_BRIDGE_STATUS_OK ||
      length != plan_len) {
    goto done;
  }
  if (type_bridge_database_transaction(database, TYPE_BRIDGE_TRANSACTION_WRITE,
                                       &transaction, &diagnostics) !=
      TYPE_BRIDGE_STATUS_OK) {
    goto done;
  }
  if (type_bridge_staged_transaction_stage(transaction, plan, &pending,
                                           &diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    goto done;
  }
  status = type_bridge_staged_transaction_commit(transaction, &diagnostics);
  if (status == TYPE_BRIDGE_STATUS_COMMIT_OUTCOME_UNKNOWN) {
    (void)type_bridge_execution_diagnostics_close(&diagnostics);
    status = type_bridge_staged_transaction_commit_retry(transaction, &diagnostics);
  }
  if (status != TYPE_BRIDGE_STATUS_OK) {
    goto done;
  }
  if (type_bridge_pending_answer(pending, &kind, out_value) ==
          TYPE_BRIDGE_STATUS_OK &&
      kind == TYPE_BRIDGE_PLAN_ANSWER_MATCHED) {
    result = 0;
  }

done:
  type_bridge_staged_transaction_close(&transaction);
  type_bridge_pending_free(&pending);
  type_bridge_plan_free(&plan);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  return result;
}

/* One handle across a rollback: stage, roll back, stage again and commit. */
int staged_rollback_then_commit(type_bridge_staged_transaction_t *transaction,
                                const type_bridge_plan_t *ghost,
                                const type_bridge_plan_t *kept,
                                uint64_t *out_value);
int staged_rollback_then_commit(type_bridge_staged_transaction_t *transaction,
                                const type_bridge_plan_t *ghost,
                                const type_bridge_plan_t *kept,
                                uint64_t *out_value) {
  type_bridge_pending_t *discarded = NULL;
  type_bridge_pending_t *pending = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_plan_answer_kind_t kind = TYPE_BRIDGE_PLAN_ANSWER_ROWS;
  uint64_t ignored = 0u;
  int result = 1;

  if (type_bridge_staged_transaction_stage(transaction, ghost, &discarded,
                                           &diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      type_bridge_staged_transaction_rollback(transaction) != TYPE_BRIDGE_STATUS_OK ||
      type_bridge_pending_answer(discarded, &kind, &ignored) !=
          TYPE_BRIDGE_STATUS_EXECUTION_FAILED ||
      type_bridge_staged_transaction_stage(transaction, kept, &pending,
                                           &diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      type_bridge_staged_transaction_commit(transaction, &diagnostics) !=
          TYPE_BRIDGE_STATUS_OK) {
    goto done;
  }
  if (type_bridge_pending_answer(pending, &kind, out_value) == TYPE_BRIDGE_STATUS_OK) {
    result = 0;
  }

done:
  type_bridge_pending_free(&discarded);
  type_bridge_pending_free(&pending);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  return result;
}

/* One handle across two commits: the second stages after the first commits. */
int staged_commit_then_stage_again(type_bridge_staged_transaction_t *transaction,
                                   const type_bridge_plan_t *plan);
int staged_commit_then_stage_again(type_bridge_staged_transaction_t *transaction,
                                   const type_bridge_plan_t *plan) {
  type_bridge_pending_t *first = NULL;
  type_bridge_pending_t *second = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  int result = 1;

  if (type_bridge_staged_transaction_stage(transaction, plan, &first,
                                           &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
      type_bridge_staged_transaction_commit(transaction, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      type_bridge_staged_transaction_stage(transaction, plan, &second,
                                           &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
      type_bridge_staged_transaction_commit(transaction, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK) {
    result = 0;
  }
  type_bridge_pending_free(&first);
  type_bridge_pending_free(&second);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  return result;
}
"#;

#[test]
fn staged_abi_compiles_under_strict_c17_and_cpp17() {
    let stage = TempDirectory::new("staged_abi");
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let source = stage.path().join("staged-abi.c");
    fs::write(&source, STAGED_CONSUMER).expect("staged ABI probe is written");

    #[cfg(not(windows))]
    let compilers: &[(&str, &str, &str, &str)] = if cfg!(target_os = "macos") {
        &[
            ("clang", "c17", "c", "clang C17"),
            ("clang++", "c++17", "c++", "clang++ C++17"),
        ]
    } else {
        &[
            ("gcc", "c17", "c", "gcc C17"),
            ("clang", "c17", "c", "clang C17"),
            ("g++", "c++17", "c++", "g++ C++17"),
            ("clang++", "c++17", "c++", "clang++ C++17"),
        ]
    };

    #[cfg(not(windows))]
    for &(compiler, standard, language, label) in compilers {
        let object = stage
            .path()
            .join(format!("staged-abi-{compiler}-{standard}.o"));
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
            "{label} rejected the staged ABI probe:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[cfg(windows)]
    for (index, (compiler, standard, language, label)) in [
        ("cl", "/std:c17", "/TC", "MSVC C17"),
        ("cl", "/std:c++17", "/TP", "MSVC C++17"),
        ("clang-cl", "/std:c17", "/TC", "clang-cl C17"),
        ("clang-cl", "/std:c++17", "/TP", "clang-cl C++17"),
    ]
    .into_iter()
    .enumerate()
    {
        let object = stage.path().join(format!("staged-abi-{index}.obj"));
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
            "{label} rejected the staged ABI probe:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
