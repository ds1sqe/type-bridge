//! Deterministic emitters over validated binding-neutral runtime projections.

#![deny(missing_docs)]

use std::collections::BTreeMap;

mod c;
mod package;
mod python;
mod rust;
mod typescript;

pub use c::CEmitter;
pub use package::{GeneratedPackage, MIGRATION_HISTORY_BUNDLE_RESOURCE};
pub use python::PythonEmitter;
pub use rust::RustEmitter;
pub use typescript::TypeScriptEmitter;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::projection::{
    BindingTarget, ModelProjection, ProjectedAnnotation, ProjectionConfig, RuntimeProjection,
};
use type_bridge_contract::schema::{
    AnnotationFactId, AnnotationKindId, SchemaAnnotationValue, encode_declared_schema,
};
use type_bridge_schema::{
    ResolvedSchema, VerifiedSchemaAuthority, encode_schema_authority, project,
};

/// Verify that a runtime projection carries the exact feature-selected
/// handler and fixed-resource evidence emitted for its binding target.
///
/// This is the shared package-admission boundary for generated runtimes. It
/// derives all evidence from the verified schema authority, recomputes the
/// complete projection, and rejects self-consistent projections forged with a
/// legacy or foreign emitter ledger.
pub fn verify_projection_evidence(
    authority: &VerifiedSchemaAuthority,
    projection: &RuntimeProjection,
) -> Result<(), Diagnostic> {
    let schema = authority.resolved_schema();
    let reproject =
        |target,
         handlers: &[type_bridge_contract::projection::ProjectionHandler],
         resources: &[type_bridge_contract::projection::CodeResourceDigest]| {
            project(schema, target, projection.config(), handlers, resources).map_err(|_| {
            invalid(
                "schema_codegen_projection_evidence_mismatch",
                "runtime projection cannot be reproduced from the verified schema authority and emitter evidence",
            )
        })
        };
    let expected = match projection.target() {
        BindingTarget::Python => {
            if projection.config() != &ProjectionConfig::python() {
                return Err(invalid(
                    "schema_codegen_projection_evidence_mismatch",
                    "Python runtime projection carries a foreign target configuration",
                ));
            }
            let emitter = PythonEmitter::new();
            reproject(
                BindingTarget::Python,
                &emitter.generator_handlers_for(schema),
                &emitter.code_resources_for(schema)?,
            )?
        }
        BindingTarget::TypeScript => {
            if projection.config() != &ProjectionConfig::typescript() {
                return Err(invalid(
                    "schema_codegen_projection_evidence_mismatch",
                    "TypeScript runtime projection carries a foreign target configuration",
                ));
            }
            let emitter = TypeScriptEmitter::new();
            reproject(
                BindingTarget::TypeScript,
                &emitter.generator_handlers_for(schema),
                &emitter.code_resources_for(schema)?,
            )?
        }
        BindingTarget::Rust => {
            if projection.config() != &ProjectionConfig::rust() {
                return Err(invalid(
                    "schema_codegen_projection_evidence_mismatch",
                    "Rust runtime projection carries a foreign target configuration",
                ));
            }
            let emitter = RustEmitter::new();
            reproject(
                BindingTarget::Rust,
                &emitter.generator_handlers_for(schema),
                &emitter.code_resources_for(schema)?,
            )?
        }
        BindingTarget::C => {
            if projection.config().c_symbol_prefix().is_none() {
                return Err(invalid(
                    "schema_codegen_projection_evidence_mismatch",
                    "C runtime projection omits its generated link-namespace configuration",
                ));
            }
            let emitter = CEmitter::new();
            reproject(
                BindingTarget::C,
                &emitter.generator_handlers_for(schema),
                &emitter.code_resources_for(schema)?,
            )?
        }
        _ => {
            return Err(invalid(
                "schema_codegen_projection_evidence_mismatch",
                "runtime projection targets an unsupported generated binding",
            ));
        }
    };
    if &expected != projection {
        return Err(invalid(
            "schema_codegen_projection_evidence_mismatch",
            "runtime projection differs from the exact feature-selected emitter evidence",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EmbeddedAuthority {
    canonical_envelope_json: String,
    declared_schema_json: String,
    managed_scope_id: String,
    semantic_profile_id: String,
}

fn embedded_authority(
    projection: &type_bridge_contract::projection::RuntimeProjection,
    authority: &VerifiedSchemaAuthority,
) -> Result<EmbeddedAuthority, Diagnostic> {
    if projection.semantic_fingerprint() != authority.resolved_schema().semantic_fingerprint() {
        return Err(invalid(
            "schema_codegen_authority_mismatch",
            "runtime projection and verified schema authority have different semantic fingerprints",
        ));
    }

    let canonical_envelope_json =
        String::from_utf8(encode_schema_authority(authority)).map_err(|_| {
            invalid(
                "schema_codegen_non_utf8_authority",
                "canonical schema-authority JSON must be UTF-8",
            )
        })?;
    let declared_schema_json =
        String::from_utf8(encode_declared_schema(authority.declared_schema())?).map_err(|_| {
            invalid(
                "schema_codegen_non_utf8_declared_schema",
                "canonical declared-schema JSON must be UTF-8",
            )
        })?;

    Ok(EmbeddedAuthority {
        canonical_envelope_json,
        declared_schema_json,
        managed_scope_id: authority.managed_scope().id().as_str().to_owned(),
        semantic_profile_id: authority.semantic_profile().id().as_str().to_owned(),
    })
}

fn invalid(code: &'static str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("schema-codegen diagnostic code is valid"),
        message,
    )
}

fn resolved_schema_uses_ordered_collections(schema: &ResolvedSchema) -> bool {
    schema.types().values().any(|model| {
        model
            .owns()
            .values()
            .any(|owns| !owns.collection_mode().is_unordered())
            || model
                .relates()
                .values()
                .any(|relates| !relates.collection_mode().is_unordered())
    })
}

fn projection_uses_ordered_collections(projection: &RuntimeProjection) -> bool {
    projection.models().values().any(|model| {
        model
            .query_tokens()
            .fields()
            .values()
            .any(|field| !field.multiplicity().collection_mode().is_unordered())
            || model
                .query_tokens()
                .roles()
                .values()
                .any(|role| !role.multiplicity().collection_mode().is_unordered())
    })
}

fn model_documentation(model: &ModelProjection) -> Option<String> {
    let type_doc = documentation_annotation(model.declaration().annotations());
    let direct_sub_doc = model
        .declaration()
        .direct_sub()
        .and_then(|sub| documentation_annotation(sub.annotations()).map(|doc| (sub, doc)));

    match (type_doc, direct_sub_doc) {
        (None, None) => None,
        (Some(type_doc), None) => Some(type_doc.to_owned()),
        (type_doc, Some((sub, sub_doc))) => {
            let edge_doc = format!(
                "Direct subtype of `{}`:\n{sub_doc}",
                sub.id().supertype().label()
            );
            Some(match type_doc {
                Some(type_doc) => format!("{type_doc}\n\n{edge_doc}"),
                None => edge_doc,
            })
        }
    }
}

fn documentation_annotation(
    annotations: &BTreeMap<AnnotationFactId, ProjectedAnnotation>,
) -> Option<&str> {
    annotations.values().find_map(|annotation| {
        if annotation.id().kind() != &AnnotationKindId::Doc {
            return None;
        }
        match annotation.value() {
            SchemaAnnotationValue::Doc(doc) => Some(doc.as_str()),
            _ => None,
        }
    })
}
