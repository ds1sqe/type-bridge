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
            "typebridge-c-query-remote-abi-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("unique remote-query ABI directory is created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remote-query ABI directory is removed");
    }
}

#[test]
fn strict_c17_and_cpp17_accept_exact_remote_query_function_types() {
    let stage = TempDirectory::new();
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let source = stage.path().join("query-remote-types.c");
    fs::write(
        &source,
        r#"#include <stddef.h>
#include <stdint.h>
#include <typebridge/type_bridge.h>

#ifdef __cplusplus
#define TYPE_BRIDGE_STATIC_ASSERT(value, message) static_assert(value, message)
#else
#define TYPE_BRIDGE_STATIC_ASSERT(value, message) _Static_assert(value, message)
#endif

TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_QUERY_REMOTE_ENVELOPE_BYTES_MAX == 33554432u,
    "remote envelope ceiling drifted");
TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_QUERY_REMOTE_CONTEXT == 28,
    "remote context input tag drifted");
TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_QUERY_REMOTE_PENDING == 29,
    "remote pending input tag drifted");
TYPE_BRIDGE_STATIC_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_QUERY_REMOTE_CLAIM == 30,
    "remote claim input tag drifted");

void type_bridge_query_remote_function_probe(void) {
  type_bridge_status_t (*context_open)(
      const type_bridge_schema_package_t *, type_bridge_byte_view_t,
      const type_bridge_query_execution_limits_v1_t *,
      type_bridge_query_remote_context_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_query_remote_context_open_v1;
  type_bridge_status_t (*context_close)(
      type_bridge_query_remote_context_t **) =
      &type_bridge_query_remote_context_close;
  type_bridge_status_t (*prepare)(
      const type_bridge_query_remote_context_t *,
      const type_bridge_query_terminal_t *, type_bridge_query_terminal_kind_t,
      const type_bridge_cancellation_t *, type_bridge_query_remote_pending_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_query_remote_prepare_v1;
  type_bridge_status_t (*request_bytes)(
      const type_bridge_query_remote_pending_t *, type_bridge_byte_view_t *) =
      &type_bridge_query_remote_pending_request_bytes;
  type_bridge_status_t (*claim)(
      const type_bridge_query_remote_pending_t *,
      const type_bridge_cancellation_t *, type_bridge_query_remote_claim_t **,
      type_bridge_execution_diagnostics_t **) =
      &type_bridge_query_remote_pending_claim;
  type_bridge_status_t (*pending_close)(
      type_bridge_query_remote_pending_t **) =
      &type_bridge_query_remote_pending_close;
  type_bridge_status_t (*snapshot_limit)(
      const type_bridge_query_remote_pending_t *, size_t *) =
      &type_bridge_query_remote_pending_response_snapshot_limit;
  type_bridge_status_t (*decode)(
      const type_bridge_query_remote_claim_t *,
      type_bridge_query_terminal_kind_t,
      const type_bridge_cancellation_t *, type_bridge_byte_view_t,
      type_bridge_query_result_t **, type_bridge_execution_diagnostics_t **) =
      &type_bridge_query_remote_claim_decode_v1;
  type_bridge_status_t (*claim_close)(type_bridge_query_remote_claim_t **) =
      &type_bridge_query_remote_claim_close;
  (void)context_open;
  (void)context_close;
  (void)prepare;
  (void)request_bytes;
  (void)claim;
  (void)pending_close;
  (void)snapshot_limit;
  (void)decode;
  (void)claim_close;
}
"#,
    )
    .expect("remote-query function probe is written");

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
        let object = stage.path().join(if cfg!(windows) {
            format!("query-remote-types-{compiler}-{standard}.obj")
        } else {
            format!("query-remote-types-{compiler}-{standard}.o")
        });

        #[cfg(not(windows))]
        let output = Command::new(compiler)
            .arg(format!("-std={standard}"))
            .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors"])
            .args(["-x", language, "-c"])
            .arg("-I")
            .arg(&include)
            .arg(&source)
            .arg("-o")
            .arg(&object)
            .output()
            .unwrap_or_else(|error| {
                panic!("required {label} compiler is unavailable or failed to launch: {error}")
            });

        #[cfg(windows)]
        let output = Command::new(compiler)
            .args([
                "/nologo",
                if language == "c" { "/TC" } else { "/TP" },
                "/W4",
                "/WX",
                "/c",
            ])
            .arg(format!("/std:{standard}"))
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
            "{label} rejected remote-query ABI types:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
