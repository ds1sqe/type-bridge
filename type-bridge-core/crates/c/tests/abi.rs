use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

#[path = "support/temp.rs"]
mod temp;
use temp::TempDirectory;

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

#[cfg(windows)]
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

const ABI_COMPILER_PROBE: &str = include_str!("support/abi.c");

#[test]
fn aggregate_header_compiles_exact_prototypes_and_hosted_layouts() {
    let header = include_str!("../include/typebridge/type_bridge.h");
    let normalized_header = header.split_whitespace().collect::<Vec<_>>().join(" ");
    let declarations = normalized_header
        .split("TYPE_BRIDGE_CALL ")
        .filter_map(|declaration| declaration.strip_prefix("type_bridge_"))
        .map(|declaration| {
            format!(
                "type_bridge_{}",
                declaration.split_once('(').expect("function declaration").0
            )
        })
        .collect::<Vec<_>>();
    let names = declarations.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(names.len(), declarations.len(), "duplicate declaration");
    let contract: serde_json::Value =
        serde_json::from_str(include_str!("../../../../tests/contracts/c-abi.json"))
            .expect("C ABI contract parses");
    let expected = contract["exports"]
        .as_array()
        .expect("C ABI export inventory")
        .iter()
        .map(|name| name.as_str().expect("export name").to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        expected.len(),
        contract["exports"].as_array().unwrap().len()
    );
    assert_eq!(names, expected, "header declarations differ from the ABI");

    // Ordinary Windows workspace tests need not run inside a Visual Studio
    // developer environment. The dedicated shared-consumer lane opts in.
    #[cfg(windows)]
    if !shared_consumer_required() {
        return;
    }

    assert_eq!(ABI_COMPILER_PROBE.matches("  TAKE(").count(), 45);
    assert!(ABI_COMPILER_PROBE.contains("#if UINTPTR_MAX == UINT64_MAX"));
    assert!(ABI_COMPILER_PROBE.contains("#elif UINTPTR_MAX == UINT32_MAX"));
    let stage = TempDirectory::new("compiler");
    let source = stage.path().join("abi.c");
    fs::write(&source, ABI_COMPILER_PROBE).expect("C ABI compiler probe is written");
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
                &format!("{compiler} {standard} C ABI compiler probe"),
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
                &format!("{compiler} {standard} C ABI compiler probe"),
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
