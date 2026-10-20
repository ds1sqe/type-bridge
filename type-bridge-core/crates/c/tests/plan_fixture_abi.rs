//! Every contract `query-plan/v3` fixture decodes through the C ABI and
//! reproduces its canonical bytes. The fixtures, their manifest and the
//! declared schema are the contract tests' data files under
//! `tests/contracts/plan_v3`, shared without Rust code.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
use std::ptr;

use type_bridge_c::TypeBridgeStatus;

#[cfg(unix)]
#[path = "support/temp.rs"]
mod temp;
#[cfg(unix)]
use temp::TempDirectory;

unsafe extern "C" {
    fn type_bridge_plan_v3_decode(
        schema: *const u8,
        schema_len: usize,
        plan: *const u8,
        plan_len: usize,
        out_plan: *mut *mut c_void,
        out_diagnostics: *mut *mut c_void,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_v3_canonical_bytes(
        plan: *const c_void,
        buffer: *mut u8,
        capacity: usize,
        out_len: *mut usize,
    ) -> TypeBridgeStatus;
    fn type_bridge_plan_free(plan: *mut *mut c_void);
}

fn fixture_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/plan_v3")
        .canonicalize()
        .expect("the contract plan-v3 fixtures exist")
}

/// The fixtures the manifest lists (it maps each name to the plan's
/// semantic fingerprint), with their bytes.
fn fixtures(directory: &Path) -> Vec<(String, Vec<u8>)> {
    let manifest: BTreeMap<String, String> = serde_json::from_slice(
        &fs::read(directory.join("manifest.json")).expect("the fixture manifest exists"),
    )
    .expect("the fixture manifest is a name-to-fingerprint map");
    assert!(manifest.len() >= 30, "the manifest lists every fixture");
    manifest
        .into_keys()
        .map(|name| {
            let bytes = fs::read(directory.join(&name)).expect("a listed fixture exists");
            (name, bytes)
        })
        .collect()
}

#[test]
fn every_contract_plan_fixture_round_trips_through_the_c_abi() {
    let directory = fixture_directory();
    let schema = fs::read(directory.join("schema.json")).expect("the declared schema exists");
    for (name, bytes) in fixtures(&directory) {
        let mut plan = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        // SAFETY: both buffers and both output slots live for the call.
        let status = unsafe {
            type_bridge_plan_v3_decode(
                schema.as_ptr(),
                schema.len(),
                bytes.as_ptr(),
                bytes.len(),
                &mut plan,
                &mut diagnostics,
            )
        };
        assert_eq!(status, TypeBridgeStatus::Ok, "{name} decodes");
        assert!(diagnostics.is_null());
        let mut echo = vec![0u8; bytes.len()];
        let mut length = 0;
        // SAFETY: a live plan and a buffer of `echo.len()` bytes.
        let status = unsafe {
            type_bridge_plan_v3_canonical_bytes(plan, echo.as_mut_ptr(), echo.len(), &mut length)
        };
        assert_eq!(status, TypeBridgeStatus::Ok, "{name} re-encodes");
        assert_eq!(length, bytes.len(), "{name} keeps its length");
        assert!(echo == bytes, "{name} keeps its canonical bytes");
        // SAFETY: the plan handle is owned here.
        unsafe { type_bridge_plan_free(&mut plan) };
    }
}

/// A C17 program reads the schema and every fixture and checks each
/// decoded plan's canonical bytes.
#[cfg(unix)]
const ROUND_TRIP: &str = r#"#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <typebridge/type_bridge.h>

static uint8_t *read_file(const char *path, size_t *length) {
  FILE *file = fopen(path, "rb");
  uint8_t *data;
  long size;
  if (file == NULL) return NULL;
  if (fseek(file, 0, SEEK_END) != 0 || (size = ftell(file)) <= 0 ||
      fseek(file, 0, SEEK_SET) != 0) {
    fclose(file);
    return NULL;
  }
  data = (uint8_t *)malloc((size_t)size);
  if (data == NULL || fread(data, 1u, (size_t)size, file) != (size_t)size) {
    free(data);
    fclose(file);
    return NULL;
  }
  fclose(file);
  *length = (size_t)size;
  return data;
}

int main(int argc, char **argv) {
  size_t schema_length = 0u;
  uint8_t *schema;
  int index, same = 0;
  if (argc < 3 || (schema = read_file(argv[1], &schema_length)) == NULL) return 2;
  for (index = 2; index < argc; ++index) {
    size_t length = 0u, echoed = 0u;
    uint8_t *bytes = read_file(argv[index], &length);
    uint8_t *echo;
    type_bridge_plan_t *plan = NULL;
    type_bridge_execution_diagnostics_t *diagnostics = NULL;
    if (bytes == NULL) return 3;
    if (type_bridge_plan_v3_decode(schema, schema_length, bytes, length, &plan,
                                   &diagnostics) != TYPE_BRIDGE_STATUS_OK) {
      fprintf(stderr, "decode failed: %s\n", argv[index]);
      return 4;
    }
    echo = (uint8_t *)malloc(length);
    if (echo == NULL ||
        type_bridge_plan_v3_canonical_bytes(plan, echo, length, &echoed) !=
            TYPE_BRIDGE_STATUS_OK ||
        echoed != length || memcmp(echo, bytes, length) != 0) {
      fprintf(stderr, "bytes differ: %s\n", argv[index]);
      return 5;
    }
    free(echo);
    free(bytes);
    type_bridge_plan_free(&plan);
    ++same;
  }
  free(schema);
  printf("%d\n", same);
  return 0;
}
"#;

#[cfg(unix)]
fn native_library() -> Option<PathBuf> {
    match std::env::var("TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER").as_deref() {
        Ok("1") => {}
        _ => return None,
    }
    if let Some(path) = std::env::var_os("TYPE_BRIDGE_C_SHARED_LIBRARY") {
        return Some(PathBuf::from(path));
    }
    let filename = format!(
        "{}type_bridge_c{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let found = std::env::current_exe()
        .expect("test executable path")
        .ancestors()
        .skip(1)
        .take(6)
        .map(|directory| directory.join(&filename))
        .find(|candidate| candidate.is_file());
    Some(found.expect("run `cargo build -p type-bridge-c --lib` before the shared consumer tests"))
}

#[cfg(unix)]
#[test]
fn compiled_c17_program_round_trips_every_contract_plan_fixture() {
    let Some(library) = native_library() else {
        return;
    };
    let directory = fixture_directory();
    let names: Vec<String> = fixtures(&directory)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let stage = TempDirectory::new("plan_fixture");
    let source = stage.path().join("round-trip.c");
    fs::write(&source, ROUND_TRIP).expect("the round-trip program is written");
    let executable = stage.path().join("round-trip");
    let native = library.parent().expect("the library has a directory");
    let compile = Command::new("cc")
        .args([
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
        ])
        .arg("-I")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("include"))
        .arg(&source)
        .arg("-L")
        .arg(native)
        .arg("-ltype_bridge_c")
        .arg(format!("-Wl,-rpath,{}", native.display()))
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("a C compiler is required");
    assert!(
        compile.status.success(),
        "the round-trip program failed to build:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&executable)
        .arg(directory.join("schema.json"))
        .args(names.iter().map(|name| directory.join(name)))
        .output()
        .expect("the round-trip program runs");
    assert!(
        run.status.success(),
        "the round-trip program failed with {}:\n{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap().trim(),
        names.len().to_string()
    );
}
