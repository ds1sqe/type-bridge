//! Parse opaque function bodies at the migration boundary, then give the pure
//! schema dependency engine only canonical fact identities.
use std::collections::{BTreeMap, BTreeSet};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{FunctionId, TypeId, TypeKind};
use type_bridge_contract::reserved::is_typebridge_internal_label;
use type_bridge_contract::schema::{DeclaredSchema, SchemaDelta, SchemaFact, SchemaFactId};
use type_bridge_schema::{DeltaError, ManagedDeltaContext};
use type_bridge_schema_compat::{SchemaReference, is_legacy_ledger_label};

type Dependencies = BTreeMap<FunctionId, BTreeSet<SchemaFactId>>;

fn references<'a>(facts: impl Iterator<Item = &'a SchemaFact>) -> Result<Dependencies, DeltaError> {
    let facts: Vec<_> = facts.collect();
    let inventory: BTreeSet<_> = facts.iter().map(|fact| fact.id()).collect();
    let labels: BTreeMap<_, _> = facts
        .iter()
        .filter_map(|fact| match fact {
            SchemaFact::Type(value) => Some((value.id().label().clone(), fact.id())),
            SchemaFact::Struct(value) => Some((value.id().label().clone(), fact.id())),
            _ => None,
        })
        .collect();
    let mut functions = BTreeMap::new();
    for fact in &facts {
        if let SchemaFact::Function(function) = fact
            && let Some(previous) = functions.insert(function.id().clone(), function)
            && previous != function
        {
            return Err(failure("unsupported_function_migration",
                "function body replacement is unsupported; remove the function in one migration and add it in the next").into());
        }
    }
    let mut dependencies = BTreeMap::new();
    for (id, function) in functions {
        let body = type_bridge_schema_compat::__internal::inspect_function_body(function)?;
        if body.has_dynamic_type_reference() {
            return Err(failure("migration_dynamic_function_reference",
                "function body uses a dynamic type position; static schema dependencies are required").into());
        }
        let mut required = BTreeSet::new();
        for reference in body.references() {
            let reserved =
                |label: &str| is_typebridge_internal_label(label) || is_legacy_ledger_label(label);
            let dependency = match reference {
                SchemaReference::Label(label) if !reserved(label.as_str()) => {
                    labels.get(label).cloned()
                }
                SchemaReference::Function(function) if !reserved(function.label().as_str()) => {
                    Some(SchemaFactId::Function(function.clone()))
                }
                SchemaReference::Scoped { scope, name }
                    if !reserved(scope.as_str()) && !reserved(name.as_str()) =>
                {
                    required.extend(scoped_role_dependencies(
                        scope.as_str(),
                        name.as_str(),
                        &facts,
                        &inventory,
                    )?);
                    continue;
                }
                _ => {
                    return Err(failure(
                        "reserved_schema_cross_reference",
                        "function body references the reserved TypeBridge control namespace",
                    )
                    .into());
                }
            }
            .ok_or_else(|| {
                failure(
                    "schema_delta_missing_function_reference",
                    "function body references an absent schema label",
                )
            })?;
            if !inventory.contains(&dependency) {
                return Err(failure(
                    "schema_delta_missing_function_reference",
                    "function body references an absent schema fact",
                )
                .into());
            }
            required.insert(dependency);
        }
        dependencies.insert(id, required);
    }
    Ok(dependencies)
}

fn scoped_role_dependencies(
    scope: &str,
    name: &str,
    facts: &[&SchemaFact],
    inventory: &BTreeSet<SchemaFactId>,
) -> Result<BTreeSet<SchemaFactId>, DeltaError> {
    let root = TypeId::new(TypeKind::Relation, scope)?;
    let mut pending = vec![(root.clone(), BTreeSet::from([SchemaFactId::Type(root)]))];
    let mut visited = BTreeSet::new();
    while let Some((relation, path)) = pending.pop() {
        if !visited.insert(relation.clone()) {
            continue;
        }
        for fact in facts {
            if let SchemaFact::Relates(role) = fact
                && role.id().relation() == &relation
                && role.id().role().label().as_str() == name
            {
                let mut result = path.clone();
                result.insert(fact.id());
                if result.iter().all(|id| inventory.contains(id)) {
                    return Ok(result);
                }
                break;
            }
        }
        for fact in facts {
            if let SchemaFact::Sub(sub) = fact
                && sub.id().subtype() == &relation
            {
                let mut inherited = path.clone();
                inherited.insert(fact.id());
                pending.push((sub.id().supertype().clone(), inherited));
            }
        }
    }
    Err(failure(
        "schema_delta_missing_function_reference",
        "function body references an absent scoped role",
    )
    .into())
}

pub(crate) fn diff_managed(
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> Result<SchemaDelta, DeltaError> {
    for fact in source.facts().chain(target.facts()) {
        if let SchemaFact::Annotation(annotation) = fact
            && let type_bridge_contract::schema::AnnotationSubjectId::Function(id) =
                annotation.id().subject()
            && source.fact(&SchemaFactId::Function(id.clone())).is_some()
            && target.fact(&SchemaFactId::Function(id.clone())).is_some()
            && source.fact(&fact.id()) != target.fact(&fact.id())
        {
            return Err(failure("unsupported_function_metadata_migration",
                "standalone function metadata changes are unsupported; remove the function in one migration and add it in the next").into());
        }
    }
    let dependencies = references(source.facts().chain(target.facts()))?;
    type_bridge_schema::__internal::diff_managed_with_function_dependencies(
        source,
        target,
        context,
        &dependencies,
    )
}

pub(crate) fn apply_delta(
    source: &DeclaredSchema,
    delta: &SchemaDelta,
    context: &ManagedDeltaContext,
) -> Result<DeclaredSchema, DeltaError> {
    let operations = delta.operations().iter().flat_map(|operation| {
        operation
            .defined_facts()
            .into_iter()
            .flatten()
            .chain(operation.expected_fact())
            .chain(operation.replacement_fact())
            .chain(operation.undefined_fact())
    });
    let dependencies = references(source.facts().chain(operations))?;
    type_bridge_schema::__internal::apply_delta_with_function_dependencies(
        source,
        delta,
        context,
        &dependencies,
    )
}

fn failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static function dependency diagnostic"),
        message,
    )
}

pub(crate) fn plan_schema_operations(
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> Result<Vec<type_bridge_contract::schema::SchemaOperation>, DeltaError> {
    Ok(diff_managed(source, target, context)?.operations().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inherited_role_references_keep_the_subtype_path_and_declaring_role() {
        let declared = type_bridge_schema_compat::typeql_to_declared(
            type_bridge_contract::schema::DocumentId::new("inherited-role.tql").unwrap(),
            "define relation base, relates member; relation child sub base;",
        )
        .unwrap();
        let facts: Vec<_> = declared.facts().collect();
        let inventory = facts.iter().map(|fact| fact.id()).collect();
        let dependencies = scoped_role_dependencies("child", "member", &facts, &inventory).unwrap();
        assert_eq!(dependencies.len(), 3);
        assert!(
            dependencies
                .iter()
                .any(|id| matches!(id, SchemaFactId::Sub(_)))
        );
        assert!(dependencies.iter().any(|id| matches!(id, SchemaFactId::Relates(role) if role.relation().label().as_str() == "base")));
        assert!(scoped_role_dependencies("child", "missing", &facts, &inventory).is_err());
    }
}
