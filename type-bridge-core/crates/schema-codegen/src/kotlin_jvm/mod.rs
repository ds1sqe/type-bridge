use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{
    BindingTarget, CodeResourceDigest, ProjectionHandler, RuntimeProjection,
};
use type_bridge_schema::{ResolvedSchema, VerifiedSchemaAuthority};

use crate::{GeneratedPackage, embedded_authority, invalid, verify_projection_evidence};

const SCHEMA_SOURCE: &str = include_str!("schema.kt");
const ATTRIBUTE_SOURCE: &str = include_str!("attribute.kt");

/// Kotlin/JVM emitter for the verified schema-package entry point and resources.
///
/// Kotlin/JVM remains an in-development binding until its complete generated
/// model/runtime surface and independent SDK conformance proofs are available.
#[derive(Clone, Copy, Debug, Default)]
pub struct KotlinJvmEmitter;

impl KotlinJvmEmitter {
    /// Construct the deterministic Kotlin/JVM emitter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Return the exact Kotlin/JVM generator behavior identity.
    #[must_use]
    pub fn generator_handlers(&self) -> Vec<ProjectionHandler> {
        vec![ProjectionHandler::kotlin_jvm_v1()]
    }

    /// Return behavior evidence for the resolved schema.
    #[must_use]
    pub fn generator_handlers_for(&self, _schema: &ResolvedSchema) -> Vec<ProjectionHandler> {
        self.generator_handlers()
    }

    /// Hash the fixed source consumed by the package entry-point renderer.
    pub fn code_resources(&self) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        Ok(vec![
            CodeResourceDigest::from_bytes(
                "typebridge.generator.kotlin-jvm.attribute-source",
                ATTRIBUTE_SOURCE.as_bytes(),
            )?,
            CodeResourceDigest::from_bytes(
                "typebridge.generator.kotlin-jvm.schema-source",
                SCHEMA_SOURCE.as_bytes(),
            )?,
        ])
    }

    /// Return the exact fixed resources selected for the resolved schema.
    pub fn code_resources_for(
        &self,
        _schema: &ResolvedSchema,
    ) -> Result<Vec<CodeResourceDigest>, Diagnostic> {
        self.code_resources()
    }

    /// Emit a package bound to one exact verified authority and Kotlin projection.
    ///
    /// Authority resides in bounded resources rather than JVM string constants,
    /// so schema size does not consume the class-file constant-pool limit.
    pub fn emit(
        &self,
        projection: &RuntimeProjection,
        authority: &VerifiedSchemaAuthority,
    ) -> Result<GeneratedPackage, Diagnostic> {
        if projection.target() != BindingTarget::KotlinJvm {
            return Err(invalid(
                "kotlin_jvm_emitter_target_mismatch",
                "Kotlin/JVM emission requires a Kotlin/JVM projection",
            ));
        }
        verify_projection_evidence(authority, projection)?;
        let embedded = embedded_authority(projection, authority)?;
        let namespace = projection
            .config()
            .kotlin_jvm_package_name()
            .ok_or_else(|| {
                invalid(
                    "kotlin_jvm_emitter_namespace_missing",
                    "Kotlin/JVM projection omits its package namespace",
                )
            })?
            .as_str();
        let package_path = namespace.replace('.', "/");
        let resource_root = format!("{package_path}/typebridge");
        let byte_literals = |bytes: Vec<u8>| {
            bytes
                .into_iter()
                .map(|byte| (byte as i8).to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        let source = SCHEMA_SOURCE
            .replace("__PACKAGE__", namespace)
            .replace("__RESOURCE_ROOT__", &resource_root)
            .replace(
                "__AUTHORITY__",
                &byte_literals(to_canonical_json(authority.authority_fingerprint())?),
            )
            .replace(
                "__BINDING__",
                &byte_literals(to_canonical_json(projection.projection_fingerprint())?),
            );
        let resources = [
            (
                "schema-authority.json",
                embedded.canonical_envelope_json.into_bytes(),
            ),
            ("runtime-projection.json", to_canonical_json(projection)?),
            (
                "semantic-fingerprint.json",
                to_canonical_json(projection.semantic_fingerprint())?,
            ),
            (
                "binding-fingerprint.json",
                to_canonical_json(projection.projection_fingerprint())?,
            ),
        ];
        let mut files = vec![(
            format!("src/main/kotlin/{package_path}/TypeBridgeSchema.kt"),
            source.into_bytes(),
        )];
        files.extend(
            resources
                .into_iter()
                .map(|(name, bytes)| (format!("src/main/resources/{resource_root}/{name}"), bytes)),
        );
        let occupied = projection
            .models()
            .values()
            .map(|model| model.target_name().as_str())
            .chain(
                projection
                    .structs()
                    .values()
                    .map(|value| value.target_name().as_str()),
            )
            .chain(
                projection
                    .functions()
                    .values()
                    .map(|value| value.target_name().as_str()),
            )
            .collect::<std::collections::BTreeSet<_>>();
        let implementation = (0..=occupied.len())
            .map(|index| {
                if index == 0 {
                    "__TypeBridgeValue".to_owned()
                } else {
                    format!("__TypeBridgeValue{index}")
                }
            })
            .find(|candidate| !occupied.contains(candidate.as_str()))
            .expect("one more candidate than occupied names guarantees a free helper");
        for model in projection.models().values() {
            use type_bridge_contract::value::ValueTypeTag;
            let (value_type, operation) = match model.declaration().value_type() {
                Some(ValueTypeTag::Long) => ("Long", "materializeLongAttribute"),
                Some(ValueTypeTag::Boolean) => ("Boolean", "materializeBooleanAttribute"),
                Some(ValueTypeTag::Double) => ("Double", "materializeDoubleAttribute"),
                Some(ValueTypeTag::String) => ("String", "materializeStringAttribute"),
                _ => continue,
            };
            let name = model.target_name().as_str();
            let parent = model
                .declaration()
                .parent()
                .map(|id| {
                    projection
                        .models()
                        .get(id)
                        .map(|parent| format!(" : {}", parent.target_name().as_str()))
                        .ok_or_else(|| {
                            invalid(
                                "kotlin_jvm_attribute_parent_missing",
                                "Projected attribute parent is missing",
                            )
                        })
                })
                .transpose()?
                .unwrap_or_default();
            let label = String::from_utf8(to_canonical_json(model.id().label())?)
                .expect("canonical JSON is UTF-8")
                .replace('$', "\\$");
            let factory = if model.declaration().is_constructible() {
                format!(
                    "    public companion object {{\n        @kotlin.jvm.JvmStatic public fun create(schema: io.github.ds1sqe.typebridge.SchemaPackage, value: kotlin.{value_type}): {name} =\n            {implementation}(schema.{operation}(TypeBridgeSchema.bindingEvidence(), {label}, value))\n    }}\n    private data class {implementation}(override val value: kotlin.{value_type}) : {name}\n"
                )
            } else {
                String::new()
            };
            let source = ATTRIBUTE_SOURCE
                .replace("__PACKAGE__", namespace)
                .replace("__TYPE__", name)
                .replace("__PARENT__", &parent)
                .replace(
                    "__OVERRIDE__",
                    if parent.is_empty() { "" } else { "override " },
                )
                .replace("__VALUE__", value_type)
                .replace("__FACTORY__", &factory);
            files.push((
                format!("src/main/kotlin/{package_path}/{name}.kt"),
                source.into_bytes(),
            ));
        }
        GeneratedPackage::try_new(files)
    }
}
