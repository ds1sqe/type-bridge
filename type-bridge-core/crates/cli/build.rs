use std::env;
use std::fs;
use std::path::PathBuf;

const DEVELOPMENT_IDENTITY: &str = "development-uncommitted";

fn candidate_identity(name: &str) -> String {
    println!("cargo:rerun-if-env-changed={name}");
    env::var(name).unwrap_or_else(|_| DEVELOPMENT_IDENTITY.to_owned())
}

fn main() {
    let version = env::var("CARGO_PKG_VERSION").expect("Cargo provides the package version");
    let target = env::var("TARGET").expect("Cargo provides the compilation target");
    let source_commit = candidate_identity("TYPE_BRIDGE_BUILD_SOURCE_COMMIT");
    let source_tree = candidate_identity("TYPE_BRIDGE_BUILD_SOURCE_TREE");
    for (label, value) in [
        ("source commit", source_commit.as_str()),
        ("source tree", source_tree.as_str()),
    ] {
        assert!(
            !value.contains(['\r', '\n']),
            "{label} must fit on one version-output line"
        );
    }
    let report = format!(
        "{version}\n\
         target: {target}\n\
         semantic-profiles: typedb-3.11.5/v1,typedb-3.12.1/v1\n\
         source-commit: {source_commit}\n\
         source-tree: {source_tree}"
    );
    let generated = format!(
        "/// Exact offline build identity emitted by `type-bridge --version`.\n\
         pub const CLI_VERSION: &str = {report:?};\n"
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR"))
        .join("cli_build_identity.rs");
    fs::write(output, generated).expect("write CLI build identity");
}
