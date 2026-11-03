//! IID-only dependencies between plans in one staged transaction.

use super::*;
use crate::id::{TypeId, TypeKind};
use crate::migration_assertion_wire::TypeIdWire;
use crate::query_plan::InputColumnId;
use crate::schema::{SchemaFact, SchemaFactId};
use crate::value::{CanonicalString, CanonicalValue, ValueTypeTag};
use std::collections::BTreeSet;

/// One later input column obtains an earlier plan's selected thing IID.
///
/// A singleton answer broadcasts over the input template. One fixed template
/// row inherits every row of its single source; otherwise larger answers zip
/// only with an equal number of template rows. References in one plan must use
/// the same source; no cross product or data-dependent plan is implied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StagedResultRefV3 {
    /// Input column whose template cells must be absent.
    pub input: InputColumnId,
    /// Zero-based index of an earlier plan in the same stack.
    pub source_plan: u32,
    /// Zero-based selected column in the source's row answer.
    pub source_column: u16,
    /// Canonical allow-set for this IID input; each type admits its subtypes.
    pub expected_types: Vec<TypeId>,
}

impl<'de> Deserialize<'de> for StagedResultRefV3 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            input: u16,
            source_plan: u32,
            source_column: u16,
            expected_types: Vec<TypeIdWire>,
        }
        let raw = Raw::deserialize(d)?;
        Ok(Self {
            input: InputColumnId::new(raw.input),
            source_plan: raw.source_plan,
            source_column: raw.source_column,
            expected_types: raw
                .expected_types
                .into_iter()
                .map(TypeIdWire::rebuild)
                .collect::<Result<_, _>>()
                .map_err(serde::de::Error::custom)?,
        })
    }
}

fn result_failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::stable(DiagnosticCategory::Integrity, code, message)
}

fn parents(schema: &DeclaredSchema) -> BTreeMap<TypeId, TypeId> {
    schema
        .facts()
        .filter_map(|fact| match fact {
            SchemaFact::Sub(fact) => {
                Some((fact.id().subtype().clone(), fact.id().supertype().clone()))
            }
            _ => None,
        })
        .collect()
}

fn subtype(parents: &BTreeMap<TypeId, TypeId>, actual: &TypeId, expected: &[TypeId]) -> bool {
    let mut current = actual;
    for _ in 0..=parents.len() {
        if expected.binary_search(current).is_ok() {
            return true;
        }
        let Some(parent) = parents.get(current) else {
            return false;
        };
        current = parent;
    }
    false
}

impl StagedPlanV3 {
    /// Stage a typed IID dependency without substituting its future values.
    ///
    /// The dependency columns contain `None` in every invocation template row.
    /// Reference metadata remains visible to hooks; ordinary invocation values
    /// follow the hook's existing visibility policy. The complete stack must
    /// pass [`validate_staged_result_refs_v3`] before provider dispatch.
    pub fn new_with_result_refs(
        plan: &QueryPlanV3,
        rows: Vec<InputRow>,
        mut result_refs: Vec<StagedResultRefV3>,
        schema: &DeclaredSchema,
    ) -> Result<Self, Diagnostic> {
        result_refs.sort_by_key(|reference| reference.input);
        for reference in &mut result_refs {
            reference.expected_types.sort();
            reference.expected_types.dedup();
        }
        if result_refs.is_empty() {
            return Self::new(plan, rows, schema);
        }
        let plan = plan.without_given_rows(schema)?;
        Self::rebuild(
            PlanWire {
                plan_bytes: hex(&plan.canonical_bytes()?),
                rows,
                result_refs,
            },
            schema,
            plan.limits(),
        )
    }

    /// IID dependencies, in ascending target-column order.
    pub fn result_refs(&self) -> &[StagedResultRefV3] {
        &self.wire.result_refs
    }

    pub(super) fn validate_ref_template(&self) -> Result<(), Diagnostic> {
        let mut columns = BTreeSet::new();
        let source = self.result_refs()[0].source_plan;
        let mut previous = None;
        for reference in self.result_refs() {
            if previous.is_some_and(|column| column >= reference.input) {
                return Err(invalid(
                    "query_stage_result_ref_input",
                    "result reference columns must be unique and ascending",
                ));
            }
            previous = Some(reference.input);
            if reference.expected_types.is_empty()
                || reference
                    .expected_types
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])
            {
                return Err(invalid(
                    "query_stage_result_ref_type",
                    "IID input types must be a nonempty canonical allow-set",
                ));
            }
            if reference.source_plan != source {
                return Err(invalid(
                    "query_stage_result_ref_sources",
                    "one plan cannot reference different result sources",
                ));
            }
            let Some(column) = self.plan.inputs().get(usize::from(reference.input.get())) else {
                return Err(invalid(
                    "query_stage_result_ref_input",
                    "result reference input column is outside the plan",
                ));
            };
            if column.value_type() != ValueTypeTag::String {
                return Err(invalid(
                    "query_stage_result_ref_input",
                    "an IID reference requires a string input column",
                ));
            }
            for row in self.rows() {
                if row
                    .values()
                    .get(usize::from(reference.input.get()))
                    .is_some_and(Option::is_some)
                {
                    return Err(invalid(
                        "query_stage_result_ref_template",
                        "result reference cells cannot carry authored values",
                    ));
                }
            }
            columns.insert(reference.input);
        }
        crate::query_plan::validate_input_rows_deferred(
            self.plan.inputs(),
            self.rows(),
            self.plan.limits().structural,
            Some(&columns),
        )
    }

    /// Rebase block-local source indexes when appending this entry to a stack.
    /// The resulting complete stack must pass graph validation before dispatch.
    pub fn rebase_result_refs(mut self, offset: u32) -> Result<Self, Diagnostic> {
        for reference in &mut self.wire.result_refs {
            reference.source_plan = reference.source_plan.checked_add(offset).ok_or_else(|| {
                invalid(
                    "query_stage_result_ref_order",
                    "result reference source index overflowed",
                )
            })?;
        }
        Ok(self)
    }

    /// Bind IID answers immediately before this plan's provider dispatch.
    ///
    /// `answers` contains only earlier, schema-validated answers from this
    /// transaction. A failure must abort the transaction before this plan runs.
    pub fn bind_results(
        &self,
        answers: &[StagedAnswerV3],
        schema: &DeclaredSchema,
    ) -> Result<QueryPlanV3, Diagnostic> {
        if self.result_refs().is_empty() {
            return self.bound_plan(schema);
        }
        let source_index = self.result_refs()[0].source_plan as usize;
        let source = answers.get(source_index).ok_or_else(|| {
            result_failure(
                "query_stage_result_ref_answer",
                "result reference requires an earlier answer",
            )
        })?;
        self.bind_source_result(source, schema)
    }

    /// Bind this entry's single source without copying earlier answers.
    /// The executor selects `source` from the validated ordered dependency graph.
    /// A missing required IID reports `query_stage_result_ref_missing` with
    /// `source_plan`, `source_column`, and `input_column`; an absent cell also
    /// carries `source_row`.
    pub fn bind_source_result(
        &self,
        source: &StagedAnswerV3,
        schema: &DeclaredSchema,
    ) -> Result<QueryPlanV3, Diagnostic> {
        if self.result_refs().is_empty() {
            return self.bound_plan(schema);
        }
        let StagedAnswerV3::Rows { rows: source } = source else {
            return Err(result_failure(
                "query_stage_result_ref_answer",
                "result reference requires an earlier row answer",
            ));
        };
        let inherit = self.rows().len() == 1 && source.len() > 1;
        if source.len() > 1 && source.len() != self.rows().len() && !inherit {
            return Err(result_failure(
                "query_stage_result_ref_rows",
                "result references require a singleton source, one template row, or equal counts",
            ));
        }
        if inherit
            && !self
                .plan
                .limits()
                .structural
                .allows_input_rows(source.len())
        {
            return Err(limit(
                "query_invocation_row_limit",
                "result reference rows exceed the input row limit",
            ));
        }
        let parents = parents(schema);
        let mut rows = if inherit {
            vec![self.rows()[0].values().to_vec(); source.len()]
        } else {
            self.rows()
                .iter()
                .map(|row| row.values().to_vec())
                .collect::<Vec<_>>()
        };
        for reference in self.result_refs() {
            let target = usize::from(reference.input.get());
            let optional = self.plan.inputs()[target].optional();
            for (index, row) in rows.iter_mut().enumerate() {
                let source_row = if source.len() == 1 { 0 } else { index };
                let value = source
                    .get(source_row)
                    .and_then(|row| row.get(usize::from(reference.source_column)));
                row[target] = match value {
                    Some(RemoteValue::Thing { iid, type_id })
                        if crate::id::is_canonical_thing_iid(iid)
                            && subtype(&parents, type_id, &reference.expected_types) =>
                    {
                        Some(CanonicalValue::String(CanonicalString::new(iid.clone())?))
                    }
                    None if source.is_empty() && optional => None,
                    Some(RemoteValue::Absent) if optional => None,
                    None if source.is_empty() => {
                        return Err(result_failure(
                            "query_stage_result_ref_missing",
                            "a required IID input has no source row",
                        )
                        .with_detail("source_plan", i64::from(reference.source_plan))
                        .with_detail("source_column", i64::from(reference.source_column))
                        .with_detail("input_column", i64::from(reference.input.get())));
                    }
                    Some(RemoteValue::Absent) => {
                        return Err(result_failure(
                            "query_stage_result_ref_missing",
                            "a required IID input has an absent source",
                        )
                        .with_detail("source_plan", i64::from(reference.source_plan))
                        .with_detail("source_row", source_row as i64)
                        .with_detail("source_column", i64::from(reference.source_column))
                        .with_detail("input_column", i64::from(reference.input.get())));
                    }
                    _ => {
                        return Err(result_failure(
                            "query_stage_result_ref_answer",
                            "source column is not a compatible thing IID",
                        ));
                    }
                };
            }
        }
        self.plan
            .with_given_rows(rows.into_iter().map(InputRow::new).collect(), schema)
    }
}

/// Validate the ordered IID dependency graph before any provider dispatch.
pub fn validate_staged_result_refs_v3(
    plans: &[StagedPlanV3],
    schema: &DeclaredSchema,
) -> Result<(), Diagnostic> {
    if plans.iter().all(|entry| entry.result_refs().is_empty()) {
        return Ok(());
    }
    let parents = parents(schema);
    let mut source_types = BTreeMap::new();
    for (index, entry) in plans.iter().enumerate() {
        for reference in entry.result_refs() {
            let Some(source) = plans
                .get(reference.source_plan as usize)
                .filter(|_| (reference.source_plan as usize) < index)
            else {
                return Err(invalid(
                    "query_stage_result_ref_order",
                    "a result reference must name an earlier plan",
                ));
            };
            if reference.expected_types.iter().any(|type_id| {
                !matches!(type_id.kind(), TypeKind::Entity | TypeKind::Relation)
                    || schema.fact(&SchemaFactId::Type(type_id.clone())).is_none()
            }) {
                return Err(invalid(
                    "query_stage_result_ref_type",
                    "result reference expects a declared thing type",
                ));
            }
            if !matches!(
                source.plan.operation(),
                Operation::Rows
                    | Operation::Write {
                        returns: WriteReturns::Rows
                    }
            ) {
                return Err(invalid(
                    "query_stage_result_ref_column",
                    "source plan must return thing IID rows",
                ));
            }
            let QueryOutput::Rows { columns } = source.plan.output() else {
                return Err(invalid(
                    "query_stage_result_ref_column",
                    "source plan must select a thing IID column",
                ));
            };
            if let std::collections::btree_map::Entry::Vacant(slot) =
                source_types.entry(reference.source_plan)
            {
                slot.insert(source.plan.selected_thing_types(schema)?);
            }
            let types = columns
                .get(usize::from(reference.source_column))
                .filter(|_| source.plan.fetch_document_shape().is_none())
                .and_then(|binding| source_types[&reference.source_plan].get(binding))
                .filter(|types| !types.is_empty())
                .ok_or_else(|| {
                    invalid(
                        "query_stage_result_ref_column",
                        "source column must answer thing IIDs",
                    )
                })?;
            if types
                .iter()
                .any(|actual| !subtype(&parents, actual, &reference.expected_types))
            {
                return Err(invalid(
                    "query_stage_result_ref_type",
                    "source IID types are incompatible with the referenced input",
                ));
            }
        }
    }
    Ok(())
}
