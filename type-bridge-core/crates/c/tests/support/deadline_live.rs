//! Compile and run a public C17 consumer against the shared deadline fixture.
use super::*;

fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
#[ignore = "requires TB_DEADLINE_FIXTURE_DIR, real TypeDB and TYPE_BRIDGE_C_SHARED_LIBRARY"]
fn c_staged_deadline_none_and_typed_timeout_direct_and_proxy() {
    let dir = PathBuf::from(std::env::var("TB_DEADLINE_FIXTURE_DIR").expect("fixture directory"));
    let live: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("fixture.json")).unwrap()).unwrap();
    let library = native_library().expect("C shared library");
    let library_dir = library.parent().unwrap();
    let declared_bytes = unhex(live["declared_hex"].as_str().unwrap());
    let declared = type_bridge_contract::schema::decode_declared_schema(&declared_bytes).unwrap();
    for transport in ["direct", "proxy"] {
        let entry = &live["entries"][format!("c-{transport}")];
        let stage = TempDirectory::new("deadline-live");
        let profile = SemanticProfileId::new(live["profile"].as_str().unwrap()).unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let authority = build_schema_authority(
            &declared,
            declared.required_capabilities(),
            &ManagedDeltaContext::new(
                ManagedScopeId::new(entry["scope"].as_str().unwrap()).unwrap(),
                profile,
                CapabilitySet::new(),
            ),
        )
        .unwrap();
        let emitter = CEmitter::new();
        let projection = project(
            &resolved,
            BindingTarget::C,
            &ProjectionConfig::c(CSymbolPrefix::new("fixture").unwrap()),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();
        write_package(
            &emitter.emit(&projection, &authority).unwrap(),
            stage.path(),
        );
        fs::write(stage.path().join("schema.bin"), &declared_bytes).unwrap();
        fs::write(
            stage.path().join("purchase.bin"),
            unhex(live["purchase_hex"].as_str().unwrap()),
        )
        .unwrap();
        fs::write(
            stage.path().join("consumer.c"),
            include_str!("deadline_live.c"),
        )
        .unwrap();
        let binary = stage.path().join("consumer");
        let compile = Command::new("gcc")
            .args([
                "-std=c17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
            ])
            .arg("-I")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("include"))
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg(stage.path().join("src/models.c"))
            .arg(stage.path().join("consumer.c"))
            .arg("-L")
            .arg(library_dir)
            .arg("-ltype_bridge_c")
            .arg(format!("-Wl,-rpath,{}", library_dir.display()))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "C compile: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(binary)
            .arg(transport)
            .arg(entry["address"].as_str().unwrap())
            .arg(entry["database"].as_str().unwrap())
            .arg(entry["endpoint"].as_str().unwrap_or("unused"))
            .arg(entry["arm"].as_str().unwrap())
            .arg(stage.path().join("schema.bin"))
            .arg(stage.path().join("purchase.bin"))
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "C {transport}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
}
