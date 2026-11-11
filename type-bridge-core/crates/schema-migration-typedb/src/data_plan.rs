//! Native scalar data programs over the frozen control and journal schema.
use crate::control_schema::JOURNAL_CONTROL_SCHEMA_TYPEQL;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    DeleteItem, InsertThing, Operation, Pattern, PlanSpec, QueryPlan, Stage, WriteReturns,
};
use type_bridge_contract::query_plan::{QueryOperand, QueryOutput, QueryPattern};
use type_bridge_contract::schema::{DeclaredSchema, DocumentId, OwnsFactId};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_orm::session::staged::PlanAnswer;

#[derive(Debug)]
pub(crate) struct DataQuery {
    pub(crate) plan: QueryPlan,
    aliases: Vec<String>,
}
impl DataQuery {
    pub(crate) fn documents(
        &self,
        answer: PlanAnswer,
    ) -> Result<Vec<serde_json::Value>, Diagnostic> {
        if self.aliases.is_empty() {
            let PlanAnswer::Rows(rows) = answer else {
                return Err(shape());
            };
            return Ok(rows
                .into_iter()
                .map(|_| serde_json::json!({"exists":true}))
                .collect());
        }
        answer
            .scalar_rows(&self.plan)?
            .into_iter()
            .map(|row| {
                let fields = self
                    .aliases
                    .iter()
                    .zip(row)
                    .map(|(name, value)| {
                        let CanonicalValue::String(value) = value else {
                            return Err(shape());
                        };
                        Ok((
                            name.clone(),
                            serde_json::Value::String(value.as_str().to_owned()),
                        ))
                    })
                    .collect::<Result<serde_json::Map<_, _>, _>>()?;
                Ok(serde_json::Value::Object(fields))
            })
            .collect()
    }
}
fn shape() -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new("migration_typedb_data_plan_shape").expect("static code"),
        "migration data program contradicts its frozen schema",
    )
}
fn id(index: usize) -> Result<BindingId, Diagnostic> {
    BindingId::new(u16::try_from(index).map_err(|_| shape())?)
}
fn type_id(kind: &str) -> Result<TypeId, Diagnostic> {
    TypeId::new(TypeKind::Entity, kind)
}
fn literal(value: &str) -> Result<QueryOperand, Diagnostic> {
    Ok(QueryOperand::Literal {
        value: CanonicalValue::String(CanonicalString::new(value)?),
    })
}
static AUTHORITY: std::sync::LazyLock<
    Result<(DeclaredSchema, ManagedSemanticSchemaFingerprint), Diagnostic>,
> = std::sync::LazyLock::new(|| {
    let schema = type_bridge_schema_compat::typeql_to_declared(
        DocumentId::new("migration-control-data-plan.typeql")?,
        JOURNAL_CONTROL_SCHEMA_TYPEQL,
    )
    .map_err(|_| shape())?;
    let semantics = ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1")?,
        JOURNAL_CONTROL_SCHEMA_TYPEQL.as_bytes(),
    )?;
    Ok((schema, semantics))
});

struct Builder {
    bindings: Vec<AssertionBinding>,
    stages: Vec<Stage>,
}
impl Builder {
    fn bind(&mut self, name: &str) -> Result<BindingId, Diagnostic> {
        let binding = id(self.bindings.len())?;
        self.bindings.push(AssertionBinding::new(
            binding,
            QueryVariable::new(format!("{name}{}", self.bindings.len()))?,
        ));
        Ok(binding)
    }
    fn matched(
        &mut self,
        kind: &str,
        fixed: &[(&str, &str)],
        bound: &[(&str, &str)],
        include_subtypes: bool,
    ) -> Result<(BindingId, Vec<BindingId>), Diagnostic> {
        let root = self.bind("root")?;
        let mut patterns = vec![
            QueryPattern::Isa {
                binding: root,
                type_id: type_id(kind)?,
                include_subtypes,
            }
            .into(),
        ];
        for (field, value) in fixed {
            let attribute = self.bind("predicate")?;
            patterns.extend([
                Pattern::from(QueryPattern::Has {
                    owner: root,
                    attribute,
                    attribute_id: AttributeId::new(*field)?,
                }),
                Pattern::from(QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding { binding: attribute },
                    right: literal(value)?,
                }),
            ]);
        }
        let mut columns = Vec::new();
        for (field, alias) in bound {
            let attribute = self.bind(alias)?;
            patterns.push(
                QueryPattern::Has {
                    owner: root,
                    attribute,
                    attribute_id: AttributeId::new(*field)?,
                }
                .into(),
            );
            columns.push(attribute);
        }
        self.stages.push(Stage::Match { patterns });
        Ok((root, columns))
    }
    fn insert(&mut self, kind: &str, fields: &[(&str, &str)]) -> Result<(), Diagnostic> {
        let root = self.bind("inserted")?;
        let owner = type_id(kind)?;
        self.stages.push(Stage::Insert {
            things: vec![InsertThing {
                binding: root,
                type_id: owner.clone(),
                fields: fields
                    .iter()
                    .map(|(field, value)| {
                        Ok((
                            OwnsFactId::new(owner.clone(), AttributeId::new(*field)?)?,
                            literal(value)?,
                        ))
                    })
                    .collect::<Result<_, Diagnostic>>()?,
                players: vec![],
            }],
        });
        Ok(())
    }
    fn finish(
        self,
        operation: Operation,
        columns: Vec<BindingId>,
    ) -> Result<QueryPlan, Diagnostic> {
        let (schema, semantics) = AUTHORITY.as_ref().map_err(Clone::clone)?;
        self.finish_with(operation, columns, schema, semantics)
    }
    fn finish_with(
        self,
        operation: Operation,
        columns: Vec<BindingId>,
        schema: &DeclaredSchema,
        semantics: &ManagedSemanticSchemaFingerprint,
    ) -> Result<QueryPlan, Diagnostic> {
        QueryPlan::new(
            PlanSpec {
                bindings: self.bindings,
                inputs: vec![],
                given_rows: None,
                operation,
                pipeline: self.stages,
                output: QueryOutput::Rows { columns },
                managed_semantics: semantics.clone(),
                functions: vec![],
            },
            schema,
        )
    }
}
pub(crate) fn read(
    kind: &str,
    fixed: &[(&str, &str)],
    bound: &[(&str, &str)],
) -> Result<DataQuery, Diagnostic> {
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    let (root, mut columns) = builder.matched(kind, fixed, bound, false)?;
    if columns.is_empty() {
        columns.push(root);
    }
    Ok(DataQuery {
        plan: builder.finish(Operation::Rows, columns)?,
        aliases: bound.iter().map(|(_, name)| (*name).to_owned()).collect(),
    })
}
pub(crate) fn read_managed_anchor(bound: &[(&str, &str)]) -> Result<DataQuery, Diagnostic> {
    use crate::control_schema::{LEGACY_CUTOVER_ENTITY, MANAGED_FENCE_SCHEMA_TYPEQL};
    static MANAGED_AUTHORITY: std::sync::LazyLock<
        Result<(DeclaredSchema, ManagedSemanticSchemaFingerprint), Diagnostic>,
    > = std::sync::LazyLock::new(|| {
        let schema = type_bridge_schema_compat::typeql_to_declared(
            DocumentId::new("managed-anchor-data-plan.typeql")?,
            MANAGED_FENCE_SCHEMA_TYPEQL,
        )
        .map_err(|_| shape())?;
        let semantics = ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1")?,
            MANAGED_FENCE_SCHEMA_TYPEQL.as_bytes(),
        )?;
        Ok((schema, semantics))
    });
    let (schema, semantics) = MANAGED_AUTHORITY.as_ref().map_err(Clone::clone)?;
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    let (root, mut columns) = builder.matched(LEGACY_CUTOVER_ENTITY, &[], bound, true)?;
    if columns.is_empty() {
        columns.push(root);
    }
    Ok(DataQuery {
        plan: builder.finish_with(Operation::Rows, columns, schema, semantics)?,
        aliases: bound.iter().map(|(_, name)| (*name).to_owned()).collect(),
    })
}

pub(crate) fn insert(kind: &str, fields: &[(&str, &str)]) -> Result<QueryPlan, Diagnostic> {
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    builder.insert(kind, fields)?;
    builder.finish(
        Operation::Write {
            returns: WriteReturns::Matched,
        },
        vec![],
    )
}
pub(crate) fn delete_kinds(
    kind: &str,
    scope_field: &str,
    scope: &str,
    kind_field: &str,
    kinds: &[&str],
) -> Result<QueryPlan, Diagnostic> {
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    let (root, columns) = builder.matched(
        kind,
        &[(scope_field, scope)],
        &[(kind_field, "kind")],
        false,
    )?;
    let Stage::Match { patterns } = &mut builder.stages[0] else {
        unreachable!("matched creates the initial match stage");
    };
    patterns.push(
        QueryPattern::Or {
            branches: kinds
                .iter()
                .map(|kind| {
                    Ok(vec![QueryPattern::Value {
                        comparator: ValueComparator::Equal,
                        left: QueryOperand::Binding {
                            binding: columns[0],
                        },
                        right: literal(kind)?,
                    }])
                })
                .collect::<Result<_, Diagnostic>>()?,
        }
        .into(),
    );
    builder.stages.push(Stage::Delete {
        items: vec![DeleteItem::Thing { binding: root }],
    });
    builder.finish(
        Operation::Write {
            returns: WriteReturns::Matched,
        },
        vec![],
    )
}
pub(crate) fn replace_and_insert(
    kind: &str,
    expected: &[(&str, &str)],
    replacement: &[(&str, &str)],
    inserted_kind: &str,
    inserted: &[(&str, &str)],
) -> Result<QueryPlan, Diagnostic> {
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    let (root, _) = builder.matched(kind, expected, &[], false)?;
    builder.stages.push(Stage::Delete {
        items: vec![DeleteItem::Thing { binding: root }],
    });
    builder.insert(kind, replacement)?;
    builder.insert(inserted_kind, inserted)?;
    builder.finish(
        Operation::Write {
            returns: WriteReturns::Matched,
        },
        vec![],
    )
}

/// Read the frozen pre-3.0 journal or managed anchor without rewriting TypeQL.
pub(crate) fn read_import_legacy(
    kind: &str,
    fixed: &[(&str, &str)],
    bound: &[(&str, &str)],
) -> Result<DataQuery, Diagnostic> {
    use type_bridge_contract::reserved::{
        IMPORT_LEGACY_JOURNAL_CONTROL_SCHEMA_TYPEQL, IMPORT_LEGACY_MANAGED_FENCE_SCHEMA_TYPEQL,
        import_legacy_control_label as label,
    };
    fn authority(
        source: &str,
    ) -> Result<(DeclaredSchema, ManagedSemanticSchemaFingerprint), Diagnostic> {
        let schema = type_bridge_schema_compat::typeql_to_declared(
            DocumentId::new("import-control.typeql")?,
            source,
        )
        .map_err(|_| shape())?;
        let semantics = ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1")?,
            source.as_bytes(),
        )?;
        Ok((schema, semantics))
    }
    static JOURNAL: std::sync::LazyLock<
        Result<(DeclaredSchema, ManagedSemanticSchemaFingerprint), Diagnostic>,
    > = std::sync::LazyLock::new(|| authority(IMPORT_LEGACY_JOURNAL_CONTROL_SCHEMA_TYPEQL));
    static MANAGED: std::sync::LazyLock<
        Result<(DeclaredSchema, ManagedSemanticSchemaFingerprint), Diagnostic>,
    > = std::sync::LazyLock::new(|| authority(IMPORT_LEGACY_MANAGED_FENCE_SCHEMA_TYPEQL));
    let authority = if kind == crate::control_schema::LEGACY_CUTOVER_ENTITY {
        &MANAGED
    } else {
        &JOURNAL
    };
    let (schema, semantics) = authority.as_ref().map_err(Clone::clone)?;
    let fixed_labels: Vec<_> = fixed.iter().map(|(field, _)| label(field)).collect();
    let bound_labels: Vec<_> = bound.iter().map(|(field, _)| label(field)).collect();
    let fixed: Vec<_> = fixed_labels
        .iter()
        .zip(fixed)
        .map(|(field, (_, value))| (field.as_ref(), *value))
        .collect();
    let bound: Vec<_> = bound_labels
        .iter()
        .zip(bound)
        .map(|(field, (_, alias))| (field.as_ref(), *alias))
        .collect();
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    let (root, mut columns) = builder.matched(&label(kind), &fixed, &bound, true)?;
    if columns.is_empty() {
        columns.push(root);
    }
    Ok(DataQuery {
        plan: builder.finish_with(Operation::Rows, columns, schema, semantics)?,
        aliases: bound.iter().map(|(_, name)| (*name).to_owned()).collect(),
    })
}

pub(crate) fn replace(
    kind: &str,
    expected: &[(&str, &str)],
    replacement: &[(&str, &str)],
) -> Result<QueryPlan, Diagnostic> {
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    let (root, _) = builder.matched(kind, expected, &[], false)?;
    builder.stages.push(Stage::Delete {
        items: vec![DeleteItem::Thing { binding: root }],
    });
    builder.insert(kind, replacement)?;
    builder.finish(
        Operation::Write {
            returns: WriteReturns::Matched,
        },
        vec![],
    )
}

pub(crate) fn insert_managed_anchor(fields: &[(&str, &str)]) -> Result<QueryPlan, Diagnostic> {
    use crate::control_schema::{LEGACY_CUTOVER_ENTITY, MANAGED_FENCE_SCHEMA_TYPEQL};
    static AUTHORITY: std::sync::LazyLock<
        Result<(DeclaredSchema, ManagedSemanticSchemaFingerprint), Diagnostic>,
    > = std::sync::LazyLock::new(|| {
        let schema = type_bridge_schema_compat::typeql_to_declared(
            DocumentId::new("managed-anchor-insert.typeql")?,
            MANAGED_FENCE_SCHEMA_TYPEQL,
        )
        .map_err(|_| shape())?;
        let semantics = ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1")?,
            MANAGED_FENCE_SCHEMA_TYPEQL.as_bytes(),
        )?;
        Ok((schema, semantics))
    });
    let (schema, semantics) = AUTHORITY.as_ref().map_err(Clone::clone)?;
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    builder.insert(LEGACY_CUTOVER_ENTITY, fields)?;
    builder.finish_with(
        Operation::Write {
            returns: WriteReturns::Matched,
        },
        vec![],
        schema,
        semantics,
    )
}

static MANAGED_AUTHORITY: std::sync::LazyLock<
    Result<(DeclaredSchema, ManagedSemanticSchemaFingerprint), Diagnostic>,
> = std::sync::LazyLock::new(|| {
    let source = type_bridge_contract::reserved::MANAGED_FENCE_SCHEMA_TYPEQL;
    let schema = type_bridge_schema_compat::typeql_to_declared(
        DocumentId::new("migration-managed-data.typeql")?,
        source,
    )
    .map_err(|_| shape())?;
    let fingerprint = ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1")?,
        source.as_bytes(),
    )?;
    Ok((schema, fingerprint))
});
fn finish_managed(
    builder: Builder,
    operation: Operation,
    columns: Vec<BindingId>,
) -> Result<QueryPlan, Diagnostic> {
    let (schema, fingerprint) = MANAGED_AUTHORITY.as_ref().map_err(Clone::clone)?;
    builder.finish_with(operation, columns, schema, fingerprint)
}
pub(crate) fn read_data_marker(key: &str) -> Result<DataQuery, Diagnostic> {
    use type_bridge_contract::reserved::*;
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    let fields = [
        (DATA_STEP_SCOPE, "scope"),
        (DATA_STEP_MIGRATION, "migration"),
        (DATA_STEP_ID, "step"),
    ];
    let (_, columns) =
        builder.matched(DATA_STEP_ENTITY, &[(DATA_STEP_KEY, key)], &fields, false)?;
    Ok(DataQuery {
        plan: finish_managed(builder, Operation::Rows, columns)?,
        aliases: fields
            .iter()
            .map(|(_, alias)| (*alias).to_owned())
            .collect(),
    })
}
pub(crate) fn write_data_marker(
    fields: &[(&str, &str)],
    insert: bool,
) -> Result<QueryPlan, Diagnostic> {
    use type_bridge_contract::reserved::DATA_STEP_ENTITY;
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    if insert {
        builder.insert(DATA_STEP_ENTITY, fields)?;
    } else {
        let (root, _) = builder.matched(DATA_STEP_ENTITY, fields, &[], false)?;
        builder.stages.push(Stage::Delete {
            items: vec![DeleteItem::Thing { binding: root }],
        });
    }
    finish_managed(
        builder,
        Operation::Write {
            returns: WriteReturns::Matched,
        },
        vec![],
    )
}
/// Rewrite the exact held mirror in the data transaction to participate in
/// the same write conflict as lease takeover, even for otherwise disjoint rows.
pub(crate) fn claim_data_fence(fields: &[(&str, &str)]) -> Result<QueryPlan, Diagnostic> {
    use type_bridge_contract::reserved::MANAGED_CONTROL_ENTITY;
    let mut builder = Builder {
        bindings: vec![],
        stages: vec![],
    };
    let (root, _) = builder.matched(MANAGED_CONTROL_ENTITY, fields, &[], false)?;
    builder.stages.push(Stage::Delete {
        items: vec![DeleteItem::Thing { binding: root }],
    });
    builder.insert(MANAGED_CONTROL_ENTITY, fields)?;
    finish_managed(
        builder,
        Operation::Write {
            returns: WriteReturns::Matched,
        },
        vec![],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_schema::{CONTROL_SCOPE, JOURNAL_ENTITY, RECORD_KIND};

    #[test]
    fn checkpoint_cleanup_conjoins_record_kinds_in_one_match() {
        let plan = delete_kinds(
            JOURNAL_ENTITY,
            CONTROL_SCOPE,
            "managed",
            RECORD_KIND,
            &["plan", "event", "backfill"],
        )
        .unwrap();
        assert_eq!(plan.statement_counts(), (0, 1));
        assert!(
            matches!(plan.pipeline(), [Stage::Match { patterns }, Stage::Delete { .. }]
            if matches!(patterns.last(), Some(Pattern::Read(QueryPattern::Or { branches })) if branches.len() == 3))
        );
    }
}
