//! Typed scalar event matrices retain the batch executor's bounded streaming shape.

use super::authority::PlanAuthority;
use super::{BuiltPlan, PlanDraft, ValueTransport};
use crate::session::backend::{GivenRowsSpec, GivenValue};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::id::TypeId;
use type_bridge_contract::migration_assertion::{BindingId, ValueComparator};
use type_bridge_contract::query_plan::v3::{Expr, Operation, Pattern, Stage, WriteReturns};
use type_bridge_contract::query_plan::{InputRow, QueryOperand, QueryOutput, QueryPattern};
use type_bridge_contract::temporal::{CanonicalDateTimeTz, CanonicalDuration, TimeZoneDesignator};
use type_bridge_contract::value::{CanonicalDouble, CanonicalString, CanonicalValue, ValueTypeTag};

pub(crate) struct BatchDraft {
    pub(crate) draft: PlanDraft,
    pub(crate) inputs: Vec<QueryOperand>,
    rows: Vec<InputRow>,
}

impl BatchDraft {
    pub(crate) fn new(
        authority: &PlanAuthority,
        rows: GivenRowsSpec,
        columns: &[(ValueTypeTag, bool)],
    ) -> Result<Self, Diagnostic> {
        if rows.variables.len() != columns.len() {
            return Err(invalid());
        }
        let mut draft = authority.draft(ValueTransport::Given);
        let inputs = rows
            .variables
            .iter()
            .zip(columns)
            .map(|(name, (domain, nullable))| {
                draft.input(&name.replace('-', "_"), *domain, *nullable)
            })
            .collect::<Result<_, _>>()?;
        let rows = rows
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(canonical)
                    .collect::<Result<Vec<_>, _>>()
                    .map(InputRow::new)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            draft,
            inputs,
            rows,
        })
    }

    pub(crate) fn bind_input(
        &mut self,
        column: usize,
        stem: &str,
        patterns: &mut Vec<Pattern>,
    ) -> Result<BindingId, Diagnostic> {
        let binding = self.draft.bind(stem)?;
        patterns.push(Pattern::Let {
            binding,
            expr: Expr::Operand(self.inputs[column].clone()),
        });
        Ok(binding)
    }

    pub(crate) fn finish(
        self,
        authority: &PlanAuthority,
        pipeline: Vec<Stage>,
        writes: bool,
        columns: Vec<BindingId>,
    ) -> Result<BuiltPlan, Diagnostic> {
        self.draft.finish_rows(
            authority,
            pipeline,
            if writes {
                Operation::Write {
                    returns: WriteReturns::Rows,
                }
            } else {
                Operation::Rows
            },
            QueryOutput::Rows { columns },
            Some(self.rows),
        )
    }
}

pub(crate) fn isa(binding: BindingId, type_id: &TypeId) -> QueryPattern {
    QueryPattern::Isa {
        binding,
        type_id: type_id.clone(),
        include_subtypes: false,
    }
}

pub(crate) fn equal(left: QueryOperand, right: QueryOperand) -> QueryPattern {
    QueryPattern::Value {
        comparator: ValueComparator::Equal,
        left,
        right,
    }
}

pub(crate) fn literal(value: i64) -> QueryOperand {
    QueryOperand::Literal {
        value: CanonicalValue::Long(value),
    }
}

fn invalid() -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        type_bridge_contract::diagnostic::DiagnosticCode::new("query_plan_batch_input")
            .expect("static code"),
        "batch inputs do not match their typed native columns",
    )
}

pub(crate) fn canonical(value: GivenValue) -> Result<Option<CanonicalValue>, Diagnostic> {
    Ok(Some(match value {
        GivenValue::Empty => return Ok(None),
        GivenValue::String(value) => CanonicalValue::String(CanonicalString::new(value)?),
        GivenValue::Integer(value) => CanonicalValue::Long(value),
        GivenValue::Double(value) => CanonicalValue::Double(CanonicalDouble::new(value)?),
        GivenValue::Boolean(value) => CanonicalValue::Boolean(value),
        GivenValue::Date(value) => CanonicalValue::Date(value.parse()?),
        GivenValue::Datetime(value) => CanonicalValue::DateTime(value.parse()?),
        GivenValue::DatetimeTz(value) => CanonicalValue::DateTimeTz(value.parse()?),
        GivenValue::DatetimeTzExact {
            local,
            named_zone,
            effective_offset_seconds,
        } => CanonicalValue::DateTimeTz(if let Some(name) = named_zone {
            CanonicalDateTimeTz::new_named_resolved(local.parse()?, name, effective_offset_seconds)?
        } else {
            CanonicalDateTimeTz::new_fixed(
                local.parse()?,
                if effective_offset_seconds == 0 {
                    TimeZoneDesignator::Utc
                } else {
                    TimeZoneDesignator::OffsetSeconds(effective_offset_seconds)
                },
            )?
        }),
        GivenValue::Decimal(value) => {
            CanonicalValue::Decimal(type_bridge_contract::value::DecimalValue::new(value)?)
        }
        GivenValue::Duration {
            months,
            days,
            nanos,
        } => CanonicalValue::Duration(CanonicalDuration::new(
            false,
            u64::from(months),
            u64::from(days),
            nanos / 1_000_000_000,
            (nanos % 1_000_000_000) as u32,
        )?),
    }))
}
