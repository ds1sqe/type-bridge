use std::fs;
use std::process::Command;

const ABI_1_6_EXPORTS: [&str; 21] = [
    "type_bridge_canonical_record_encode_attribute_v1",
    "type_bridge_canonical_record_encode_create_v1",
    "type_bridge_canonical_record_encode_reference_v1",
    "type_bridge_canonical_record_encode_snapshot_v1",
    "type_bridge_canonical_record_encode_struct_v1",
    "type_bridge_canonical_record_decode_attribute_v1",
    "type_bridge_canonical_record_decode_create_v1",
    "type_bridge_canonical_record_decode_reference_v1",
    "type_bridge_canonical_record_decode_snapshot_v1",
    "type_bridge_canonical_record_decode_struct_v1",
    "type_bridge_projected_struct_close",
    "type_bridge_canonical_archive_builder_open_v1",
    "type_bridge_canonical_archive_builder_append_record_v1",
    "type_bridge_canonical_archive_builder_finish_v1",
    "type_bridge_canonical_archive_builder_close",
    "type_bridge_canonical_archive_open_v1",
    "type_bridge_canonical_archive_count",
    "type_bridge_canonical_archive_record_at",
    "type_bridge_canonical_archive_close",
    "type_bridge_canonical_bytes_view",
    "type_bridge_canonical_bytes_close",
];

#[test]
fn abi_1_6_header_is_additive_closed_and_strict_c17() {
    let header = include_str!("../include/typebridge/type_bridge_abi_1_6.h");
    assert!(header.contains("#include <typebridge/type_bridge_abi_1_5.h>"));
    for name in ABI_1_6_EXPORTS {
        assert_eq!(header.matches(name).count(), 1, "{name} inventory drifted");
    }
    let directory =
        std::env::temp_dir().join(format!("typebridge-abi-1-6-header-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("ABI 1.6 test directory is created");
    let source = directory.join("abi-1-6.c");
    fs::write(
        &source,
        r#"#include <typebridge/type_bridge_abi_1_6.h>
_Static_assert(TYPE_BRIDGE_C_ABI_MAJOR == 1u, "ABI major drifted");
_Static_assert(TYPE_BRIDGE_C_ABI_MINOR == 6u, "ABI minor drifted");
int main(void) {
  type_bridge_canonical_bytes_t *bytes = 0;
  type_bridge_canonical_archive_builder_t *builder = 0;
  type_bridge_canonical_archive_t *archive = 0;
  return (bytes != 0) || (builder != 0) || (archive != 0);
}
"#,
    )
    .expect("ABI 1.6 probe is written");
    let output = Command::new("cc")
        .args([
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
        ])
        .arg("-I")
        .arg(env!("CARGO_MANIFEST_DIR"))
        .arg("-I")
        .arg(format!("{}/include", env!("CARGO_MANIFEST_DIR")))
        .arg("-c")
        .arg(&source)
        .arg("-o")
        .arg(directory.join("abi-1-6.o"))
        .output()
        .expect("strict C17 compiler launches");
    let _ = fs::remove_dir_all(&directory);
    assert!(
        output.status.success(),
        "strict C17 rejected ABI 1.6:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runtime_package_installs_the_additive_abi_1_6_header() {
    let cmake = include_str!("../CMakeLists.txt");
    assert!(cmake.contains("project(TypeBridge VERSION 1.6.0 LANGUAGES NONE)"));
    assert!(cmake.contains("include/typebridge/type_bridge_abi_1_6.h"));
}
