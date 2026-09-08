//! Deterministic emitters over validated binding-neutral runtime projections.

#![deny(missing_docs)]

use std::collections::BTreeMap;

mod package;
mod python;
mod rust;
mod typescript;

pub use package::GeneratedPackage;
pub use python::PythonEmitter;
pub use rust::RustEmitter;
pub use typescript::TypeScriptEmitter;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::projection::{ModelProjection, ProjectedAnnotation};
use type_bridge_contract::schema::{
    AnnotationFactId, AnnotationKindId, SchemaAnnotationValue, encode_declared_schema,
};
use type_bridge_schema::{VerifiedSchemaAuthority, encode_schema_authority};

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
