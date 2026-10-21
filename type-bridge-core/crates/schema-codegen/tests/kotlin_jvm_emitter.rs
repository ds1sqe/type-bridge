use type_bridge_contract::projection::{BindingTarget, KotlinJvmPackageName, ProjectionConfig};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_schema::project;
use type_bridge_schema_codegen::{KotlinJvmEmitter, verify_projection_evidence};

mod support;

const SOURCE: &str = "format: typebridge.schema/v2\nentities:\n  person: {}\n";

#[test]
fn package_resources_round_trip_with_exact_namespace_and_evidence() {
    let authority = support::authority(SOURCE);
    let emitter = KotlinJvmEmitter::new();
    let build = |namespace| {
        project(
            authority.resolved_schema(),
            BindingTarget::KotlinJvm,
            &ProjectionConfig::kotlin_jvm(KotlinJvmPackageName::new(namespace).unwrap()),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap()
    };
    let projection = build("example.people");
    let other = build("example.other");
    assert_eq!(
        projection.semantic_fingerprint(),
        other.semantic_fingerprint()
    );
    assert_ne!(
        projection.projection_fingerprint(),
        other.projection_fingerprint()
    );
    let package = emitter.emit(&projection, &authority).unwrap();
    assert_eq!(
        package.files(),
        emitter.emit(&projection, &authority).unwrap().files()
    );
    let stage = support::Stage::new();
    support::write_package(&package, stage.path());
    let resources = stage
        .path()
        .join("src/main/resources/example/people/typebridge");
    let read = |name| std::fs::read(resources.join(name)).unwrap();
    let restored = decode_runtime_projection_verified(
        &read("runtime-projection.json"),
        &read("semantic-fingerprint.json"),
        &read("binding-fingerprint.json"),
    )
    .unwrap();
    verify_projection_evidence(&authority, &restored).unwrap();
    assert_eq!(restored, projection);
    let source = std::fs::read_to_string(
        stage
            .path()
            .join("src/main/kotlin/example/people/TypeBridgeSchema.kt"),
    )
    .unwrap();
    assert!(source.contains("package example.people"));
    assert!(source.contains("/example/people/typebridge/"));
    assert!(!source.contains("__PACKAGE__"));
}

#[test]
fn emitter_rejects_projection_without_its_source_evidence() {
    let authority = support::authority(SOURCE);
    let emitter = KotlinJvmEmitter::new();
    let projection = project(
        authority.resolved_schema(),
        BindingTarget::KotlinJvm,
        &ProjectionConfig::kotlin_jvm(KotlinJvmPackageName::new("example.people").unwrap()),
        &emitter.generator_handlers(),
        &[],
    )
    .unwrap();
    assert!(emitter.emit(&projection, &authority).is_err());
}

#[test]
fn generated_entry_point_name_is_reserved() {
    let authority =
        support::authority("format: typebridge.schema/v2\nentities:\n  type-bridge-schema: {}\n");
    let emitter = KotlinJvmEmitter::new();
    assert!(
        project(
            authority.resolved_schema(),
            BindingTarget::KotlinJvm,
            &ProjectionConfig::kotlin_jvm(KotlinJvmPackageName::new("example.people").unwrap()),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap()
        )
        .is_err()
    );
}

#[test]
#[ignore = "requires KOTLINC and TYPEBRIDGE_JVM_LIBRARY for native consumer acceptance"]
fn generated_package_opens_in_a_real_jvm() {
    use std::process::Command;
    let compiler = std::env::var_os("KOTLINC").expect("KOTLINC is required");
    let library =
        std::env::var_os("TYPEBRIDGE_JVM_LIBRARY").expect("TYPEBRIDGE_JVM_LIBRARY is required");
    const SCALAR_SOURCE: &str = "format: typebridge.schema/v2\nattributes:\n  measurement:\n    abstract: true\n    value:\n      type: integer\n      range: { min: 0, max: 150 }\n  age: { sub: measurement }\n  active: { value: boolean }\n  score: { value: double }\n  total: { value: integer }\n  value: { value: integer }\n  jvm-static: { value: boolean }\n  string-value: { value: double }\n  text: { value: string }\n  code: { value: { type: string, regex: '^[a-z]+$' } }\nentities:\n  person: {}\n";
    let scalar_config = |namespace| {
        use type_bridge_contract::id::{TypeId, TypeKind};
        ProjectionConfig::kotlin_jvm(KotlinJvmPackageName::new(namespace).unwrap())
            .with_type_name_override(
                TypeId::new(TypeKind::Attribute, "string-value").unwrap(),
                "String",
            )
            .unwrap()
    };
    let authority = support::authority(SCALAR_SOURCE);
    let emitter = KotlinJvmEmitter::new();
    let projection = project(
        authority.resolved_schema(),
        BindingTarget::KotlinJvm,
        &scalar_config("example.people"),
        &emitter.generator_handlers(),
        &emitter.code_resources().unwrap(),
    )
    .unwrap();
    let stage = support::Stage::new();
    support::write_package(
        &emitter.emit(&projection, &authority).unwrap(),
        stage.path(),
    );
    let foreign_projection = project(
        authority.resolved_schema(),
        BindingTarget::KotlinJvm,
        &scalar_config("example.foreign"),
        &emitter.generator_handlers(),
        &emitter.code_resources().unwrap(),
    )
    .unwrap();
    support::write_package(
        &emitter.emit(&foreign_projection, &authority).unwrap(),
        stage.path(),
    );
    let consumer = stage.path().join("Consumer.kt");
    std::fs::write(&consumer, r#"
import example.people.*
import example.foreign.TypeBridgeSchema as ForeignSchema
import io.github.ds1sqe.typebridge.*
import java.nio.file.Path
fun main(args: Array<kotlin.String>) {
    TypeBridgeRuntime.load(Path.of(args[0]))
    if (args.size > 1) {
        try { TypeBridgeSchema.open(); error("substituted resources accepted") }
        catch (e: SchemaPackageException) { check(e.canonicalDiagnostics.contains("jvm_generated_identity_mismatch")) }
        println("Substituted valid resources rejected")
        return
    }
    TypeBridgeSchema.open().use { schema ->
        val age = Age.create(schema, 42)
        val measurement: Measurement = age
        check(measurement.value == 42L && Age.create(schema, 42) == age)
        check(Value.create(schema, 7).value == 7L)
        check(example.people.JvmStatic.create(schema, true).value)
        check(example.people.String.create(schema, 1.5).value == 1.5)
        check(Total.create(schema, Long.MIN_VALUE).value == Long.MIN_VALUE)
        check(Total.create(schema, Long.MAX_VALUE).value == Long.MAX_VALUE)
        ForeignSchema.open().use { foreign ->
            try { Age.create(foreign, 42); error("foreign schema package accepted") }
            catch (e: SdkExecutionException) { check(e.diagnostic.code == "generated_token_package_mismatch") }
        }
        for (text in listOf("", "a\u0000b", "é水😀", "e\u0301", "x".repeat(1024 * 1024))) {
            check(Text.create(schema, text).value == text)
        }
        check(Code.create(schema, "abc").value == "abc")
        try { Code.create(schema, "ABC"); error("regex constraint was ignored") }
        catch (e: SdkExecutionException) { check(e.diagnostic.code == "regex_constraint_violation") }
        for (text in listOf("\uD800", "\uDC00", "a\uD800b")) {
            try { Text.create(schema, text); error("unpaired surrogate accepted") }
            catch (e: SchemaPackageException) { check(e.canonicalDiagnostics.contains("jvm_string_invalid_utf16")) }
        }
        for (text in listOf("x".repeat(1024 * 1024 + 1), "水".repeat(350000))) {
            try { Text.create(schema, text); error("oversized text accepted") }
            catch (e: SchemaPackageException) {
                check(e.canonicalDiagnostics.contains("jvm_input_limit") || e.canonicalDiagnostics.contains("canonical_string_limit_exceeded"))
            }
        }
        check(!Active.create(schema, false).value)
        check(Score.create(schema, -0.0).value.toRawBits() == (-0.0).toRawBits())
        try { Age.create(schema, -1); error("inherited range was ignored") }
        catch (e: SdkExecutionException) {
            check(e.diagnostic.category == ExecutionCategory.INVALID_INPUT)
            check(e.diagnostic.code == "range_constraint_violation")
        }
        try { schema.materializeLongAttribute(byteArrayOf(), "age", 1); error("foreign binding accepted") }
        catch (e: SdkExecutionException) { check(e.diagnostic.code == "generated_token_package_mismatch") }
        try { Score.create(schema, Double.NaN); error("non-finite value accepted") }
        catch (_: SchemaPackageException) {}
    }
    val expected = TypeBridgeSchema.open().use { it.authorityFingerprint to it.bindingFingerprint }
    repeat(100) {
        val schema = TypeBridgeSchema.open()
        check(schema.namespace == "example.people")
        check(schema.modelCount == 11)
        val expected = schema.semanticFingerprint
        val changed = schema.semanticFingerprint
        changed.fill(0)
        check(schema.semanticFingerprint.contentEquals(expected))
        val worker = Thread {
            repeat(100) {
                try { check(schema.modelCount == 11) }
                catch (e: SchemaPackageException) { check(e.canonicalDiagnostics.contains("jvm_package_closed")) }
            }
        }
        worker.setUncaughtExceptionHandler { _, failure -> failure.printStackTrace(); kotlin.system.exitProcess(1) }
        worker.start()
        schema.close()
        schema.close()
        worker.join()
        try { schema.namespace; error("closed package accepted") }
        catch (e: SchemaPackageException) { check(e.canonicalDiagnostics.contains("jvm_package_closed")) }
    }
    try { SchemaPackage.openGenerated(TypeBridgeSchema::class.java, "example.other", "/example/people/typebridge/", expected.first, expected.second); error("namespace mismatch accepted") }
    catch (e: SchemaPackageException) { check(e.canonicalDiagnostics.contains("jvm_package_target_mismatch")) }
    try { SchemaPackage.openGenerated(TypeBridgeSchema::class.java, "example.people", "/absent/", expected.first, expected.second); error("missing resources accepted") }
    catch (e: SchemaPackageException) { check(e.canonicalDiagnostics.contains("jvm_package_resource_missing")) }
    val root = "/example/people/typebridge/"
    fun resource(name: kotlin.String) = TypeBridgeSchema::class.java.getResourceAsStream(root + name)!!.use { it.readAllBytes() }
    val authority = resource("schema-authority.json")
    val projection = resource("runtime-projection.json")
    val semantic = resource("semantic-fingerprint.json")
    val binding = resource("binding-fingerprint.json")
    fun rejected(block: () -> Unit) {
        try { block(); error("invalid native call accepted") }
        catch (e: SchemaPackageException) { check(e.canonicalDiagnostics.startsWith("[")) }
    }
    rejected { NativeBridge.openPackage("example.people".toByteArray(), "{}".toByteArray(), projection, semantic, binding, expected.first, binding) }
    rejected { NativeBridge.openPackage(byteArrayOf(-1), authority, projection, semantic, binding, expected.first, binding) }
    rejected { NativeBridge.openPackage(ByteArray(256), authority, projection, semantic, binding, expected.first, binding) }
    rejected { NativeBridge.openPackage("example.people".toByteArray(), authority, projection, semantic, binding, byteArrayOf(), binding) }
    rejected { NativeBridge.namespace(-1) }
    rejected { NativeBridge.closePackage(0) }
    val method = NativeBridge::class.java.getDeclaredMethod("openPackage", ByteArray::class.java, ByteArray::class.java, ByteArray::class.java, ByteArray::class.java, ByteArray::class.java, ByteArray::class.java, ByteArray::class.java)
    try { method.invoke(null, null, authority, projection, semantic, binding, expected.first, binding); error("null accepted") }
    catch (e: java.lang.reflect.InvocationTargetException) {
        check((e.cause as SchemaPackageException).canonicalDiagnostics.contains("jvm_null_input"))
    }
    println("Kotlin/JNI admission, copies, and concurrent close passed")
}
"#).unwrap();
    let jar = stage.path().join("consumer.jar");
    let runtime =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../jvm/kotlin/src/main/kotlin");
    let runtime_jar = stage.path().join("runtime.jar");
    let generated_jar = stage.path().join("generated.jar");
    let compile = |sources: &std::path::Path,
                   output: &std::path::Path,
                   classpath: Option<&std::ffi::OsStr>,
                   friend: bool,
                   include_runtime: bool| {
        let mut command = Command::new(&compiler);
        command
            .args(["-Werror", "-jvm-target", "21"])
            .arg(sources)
            .arg("-d")
            .arg(output);
        if let Some(classpath) = classpath {
            command.arg("-classpath").arg(classpath);
        }
        if friend {
            command.arg(format!("-Xfriend-paths={}", runtime_jar.display()));
        }
        if include_runtime {
            command.arg("-include-runtime");
        }
        let compiled = command.output().unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
    };
    compile(&runtime, &runtime_jar, None, false, false);
    compile(
        &stage.path().join("src/main/kotlin"),
        &generated_jar,
        Some(runtime_jar.as_os_str()),
        false,
        false,
    );
    let dependencies = std::env::join_paths([&runtime_jar, &generated_jar]).unwrap();
    compile(&consumer, &jar, Some(&dependencies), true, true);
    let classpath = std::env::join_paths([&jar, &runtime_jar, &generated_jar]).unwrap();
    for (name, source, expected) in [
        (
            "WrongScalar",
            "fun bad(s: io.github.ds1sqe.typebridge.SchemaPackage) = example.people.Age.create(s, \"old\")",
            "type mismatch",
        ),
        (
            "WrongNominal",
            "fun bad(s: io.github.ds1sqe.typebridge.SchemaPackage): example.people.Age = example.people.Score.create(s, 1.0)",
            "type mismatch",
        ),
        (
            "Handwritten",
            "package example.people\nclass FakeAge(override val value: Long) : Age",
            "different module",
        ),
        (
            "PrivateValue",
            "val bad = example.people.Age.__TypeBridgeValue(-1)",
            "private",
        ),
        (
            "CopyBypass",
            "fun bad(age: example.people.Age) = age.copy(value = -1)",
            "unresolved reference",
        ),
        (
            "AbstractCreate",
            "fun bad(s: io.github.ds1sqe.typebridge.SchemaPackage) = example.people.Measurement.create(s, 1)",
            "unresolved reference",
        ),
        (
            "ForeignNominal",
            "fun bad(s: io.github.ds1sqe.typebridge.SchemaPackage): example.people.Age = example.foreign.Age.create(s, 42)",
            "type mismatch",
        ),
    ] {
        let path = stage.path().join(format!("{name}.kt"));
        std::fs::write(&path, source).unwrap();
        let rejected = Command::new(&compiler)
            .args(["-Werror", "-jvm-target", "21"])
            .arg(path)
            .arg("-classpath")
            .arg(&dependencies)
            .arg("-d")
            .arg(stage.path().join(format!("{name}.jar")))
            .output()
            .unwrap();
        let diagnostic = String::from_utf8_lossy(&rejected.stderr).to_lowercase();
        assert!(
            !rejected.status.success() && diagnostic.contains(expected),
            "{name}: {diagnostic}"
        );
    }

    let packed = Command::new("jar")
        .arg("uf")
        .arg(&generated_jar)
        .arg("-C")
        .arg(stage.path().join("src/main/resources"))
        .arg(".")
        .output()
        .unwrap();
    assert!(
        packed.status.success(),
        "{}",
        String::from_utf8_lossy(&packed.stderr)
    );
    let run = Command::new("java")
        .args(["-Xcheck:jni", "-cp"])
        .arg(&classpath)
        .arg("ConsumerKt")
        .arg(&library)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        run.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).unwrap().trim(),
        "Kotlin/JNI admission, copies, and concurrent close passed"
    );
    for other_authority in [
        support::authority("format: typebridge.schema/v2\nentities:\n  other-person: {}\n"),
        support::authority_with(SCALAR_SOURCE, "another-scope", support::TEST_PROFILE),
    ] {
        let other_projection = project(
            other_authority.resolved_schema(),
            BindingTarget::KotlinJvm,
            &ProjectionConfig::kotlin_jvm(KotlinJvmPackageName::new("example.people").unwrap()),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();
        let other_stage = support::Stage::new();
        support::write_package(
            &emitter.emit(&other_projection, &other_authority).unwrap(),
            other_stage.path(),
        );
        let packed = Command::new("jar")
            .arg("uf")
            .arg(&generated_jar)
            .arg("-C")
            .arg(other_stage.path().join("src/main/resources"))
            .arg(".")
            .output()
            .unwrap();
        assert!(packed.status.success());
        let rejected = Command::new("java")
            .args(["-Xcheck:jni", "-cp"])
            .arg(&classpath)
            .arg("ConsumerKt")
            .arg(&library)
            .arg("reject")
            .output()
            .unwrap();
        assert!(
            rejected.status.success(),
            "{}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(rejected.stderr.is_empty());
        assert_eq!(
            String::from_utf8(rejected.stdout).unwrap().trim(),
            "Substituted valid resources rejected"
        );
    }
}

#[test]
fn large_authority_does_not_become_a_jvm_string_constant() {
    let mut source = String::from("format: typebridge.schema/v2\nentities:\n");
    for index in 0..500 {
        source.push_str(&format!("  person-{index}: {{}}\n"));
    }
    let authority = support::authority(&source);
    let emitter = KotlinJvmEmitter::new();
    let projection = project(
        authority.resolved_schema(),
        BindingTarget::KotlinJvm,
        &ProjectionConfig::kotlin_jvm(KotlinJvmPackageName::new("example.large").unwrap()),
        &emitter.generator_handlers(),
        &emitter.code_resources().unwrap(),
    )
    .unwrap();
    let package = emitter.emit(&projection, &authority).unwrap();
    let stage = support::Stage::new();
    support::write_package(&package, stage.path());
    let source = std::fs::read(
        stage
            .path()
            .join("src/main/kotlin/example/large/TypeBridgeSchema.kt"),
    )
    .unwrap();
    let authority = std::fs::read(
        stage
            .path()
            .join("src/main/resources/example/large/typebridge/schema-authority.json"),
    )
    .unwrap();
    assert!(authority.len() > 65_535);
    assert!(source.len() < 8_000);
}

#[test]
fn kotlin_qualification_roots_cannot_be_shadowed_by_overrides() {
    use type_bridge_contract::id::{TypeId, TypeKind};
    let authority = support::authority(SOURCE);
    let emitter = KotlinJvmEmitter::new();
    for name in ["TypeBridgeSchema", "kotlin", "io", "java"] {
        let config =
            ProjectionConfig::kotlin_jvm(KotlinJvmPackageName::new("example.people").unwrap())
                .with_type_name_override(TypeId::new(TypeKind::Entity, "person").unwrap(), name)
                .unwrap();
        assert!(
            project(
                authority.resolved_schema(),
                BindingTarget::KotlinJvm,
                &config,
                &emitter.generator_handlers(),
                &emitter.code_resources().unwrap()
            )
            .is_err()
        );
    }
}
