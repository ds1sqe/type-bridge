use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{KotlinJvmPackageName, ProjectionConfig, TargetIdentifier};

#[test]
fn kotlin_jvm_names_reject_source_injection_and_reserved_names() {
    for name in [
        "", "_", "__", "class", "when", "fun", "1Name", "a.b", "a/b", "a\\b", "a\0b", "`quoted`",
        "name$", "한글",
    ] {
        assert!(TargetIdentifier::kotlin_jvm(name).is_err(), "{name:?}");
    }
    assert!(TargetIdentifier::kotlin_jvm("a".repeat(256)).is_err());
    for name in ["Person", "personId", "when_", "_field", "A2"] {
        assert_eq!(TargetIdentifier::kotlin_jvm(name).unwrap().as_str(), name);
    }
}

#[test]
fn kotlin_jvm_package_namespace_is_bounded_and_outside_runtime() {
    for package in [
        "",
        ".a",
        "a.",
        "a..b",
        "../escape",
        "a/b",
        "a.class",
        "Example.test",
        "java.lang",
        "kotlin",
        "kotlin.collections",
        "io.github.ds1sqe.typebridge",
        "io.github.ds1sqe.typebridge.internal",
    ] {
        assert!(KotlinJvmPackageName::new(package).is_err(), "{package:?}");
    }
    assert!(KotlinJvmPackageName::new("a".repeat(256)).is_err());
    assert_eq!(
        KotlinJvmPackageName::new("example.schema_v2")
            .unwrap()
            .as_str(),
        "example.schema_v2"
    );
}

#[test]
fn kotlin_jvm_overrides_are_ordered_and_cannot_change_target() {
    let config =
        || ProjectionConfig::kotlin_jvm(KotlinJvmPackageName::new("example.test").unwrap());
    let a = TypeId::new(TypeKind::Entity, "a").unwrap();
    let b = TypeId::new(TypeKind::Entity, "b").unwrap();
    let left = config()
        .with_type_name_override(a.clone(), "First")
        .unwrap()
        .with_type_name_override(b.clone(), "Second")
        .unwrap();
    let right = config()
        .with_type_name_override(b, "Second")
        .unwrap()
        .with_type_name_override(a.clone(), "First")
        .unwrap();
    assert_eq!(
        to_canonical_json(&left).unwrap(),
        to_canonical_json(&right).unwrap()
    );
    assert!(
        left.with_type_name_override(a.clone(), "Duplicate")
            .is_err()
    );
    assert!(config().with_type_name_override(a, "class").is_err());
    assert!(config().c_symbol_prefix().is_none());
    assert!(config().rust_naming_policy().is_none());
}
