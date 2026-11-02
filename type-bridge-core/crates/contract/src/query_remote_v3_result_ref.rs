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
/// A singleton answer broadcasts over the input template. Larger answers zip
/// only with an equal number of template rows. Several references in one plan
/// must use the same source; no cross product or data-dependent plan is implied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StagedResultRefV3 {
    /// Input column whose template cells must be absent.
    pub input: InputColumnId,
    /// Zero-based index of an earlier plan in the same stack.
    pub source_plan: u32,
    /// Zero-based selected column in the source's row answer.
    pub source_column: u16,
    /// Schema type accepted by this IID input, including its subtypes.
    pub expected_type: TypeId,
}

impl<'de> Deserialize<'de> for StagedResultRefV3 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            input: u16,
            source_plan: u32,
            source_column: u16,
            expected_type: TypeIdWire,
        }
        let raw = Raw::deserialize(d)?;
        Ok(Self {
            input: InputColumnId::new(raw.input),
            source_plan: raw.source_plan,
            source_column: raw.source_column,
            expected_type: raw
                .expected_type
                .rebuild()
                .map_err(serde::de::Error::custom)?,
        })
    }
}

fn result_failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::stable(DiagnosticCategory::Integrity, code, message)
}

fn subtype(schema: &DeclaredSchema, actual: &TypeId, expected: &TypeId) -> bool {
    let mut current = actual;
    for _ in 0..=schema.facts().len() {
        if current == expected {
            return true;
        }
        let parent = schema.facts().find_map(|fact| match fact {
            SchemaFact::Sub(fact) if fact.id().subtype() == current => Some(fact.id().supertype()),
            _ => None,
        });
        let Some(parent) = parent else { return false };
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
        let Some(StagedAnswerV3::Rows { rows: source }) = answers.get(source_index) else {
            return Err(result_failure(
                "query_stage_result_ref_answer",
                "result reference requires an earlier row answer",
            ));
        };
        if source.len() > 1 && source.len() != self.rows().len() {
            return Err(result_failure(
                "query_stage_result_ref_rows",
                "result reference rows must broadcast a singleton or zip equal counts",
            ));
        }
        let mut rows = self
            .rows()
            .iter()
            .map(|row| row.values().to_vec())
            .collect::<Vec<_>>();
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
                            && subtype(schema, type_id, &reference.expected_type) =>
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
                        .with_detail("source_plan", i64::from(reference.source_plan)));
                    }
                    Some(RemoteValue::Absent) => {
                        return Err(result_failure(
                            "query_stage_result_ref_missing",
                            "a required IID input has an absent source",
                        )
                        .with_detail("source_plan", i64::from(reference.source_plan))
                        .with_detail("source_row", source_row as i64));
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
            if !matches!(
                reference.expected_type.kind(),
                TypeKind::Entity | TypeKind::Relation
            ) || schema
                .fact(&SchemaFactId::Type(reference.expected_type.clone()))
                .is_none()
            {
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
                .any(|actual| !subtype(schema, actual, &reference.expected_type))
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
