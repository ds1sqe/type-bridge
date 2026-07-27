//! Deterministic emitters over validated binding-neutral runtime projections.

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
use type_bridge_contract::schema::{AnnotationFactId, AnnotationKindId, SchemaAnnotationValue};

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
