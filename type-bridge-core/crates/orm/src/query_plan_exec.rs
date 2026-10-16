//! Lowering and answer resolution for trusted `query-plan/v3` programs.
//!
//! A plan lowers to exactly one TypeQL pipeline: a `given` stage carrying
//! every input row when the plan declares inputs, the plan's stages in
//! order, and the projection its operation needs. Plan constants render as
//! literals; caller values reach TypeDB only through `given`. A write's
//! `Matched` answer is the number of rows its pipeline produced, counted from
//! the same answer stream, so a conditional write needs no extra query.

use type_bridge_core_lib::__internal as tql;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::id::AttributeId;
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId};
use type_bridge_contract::query_plan::v3::{
    ArithOp, Builtin, DeleteItem, Expr, InsertThing, Operation, Pattern, QueryPlanV3, Set, Stage,
    WriteReturns,
};
use type_bridge_contract::query_plan::{
    DocumentSource, InputColumn, OrderDirection, QueryOperand, QueryOutput, QueryPattern,
};
use type_bridge_contract::sdk_diagnostic::FailureContext;

use crate::query_v2::{
    ast, failure, given_rows_spec, local_variable, lower_functions, lower_operand,
    lower_scoped_patterns, reducer_keyword,
};
use crate::session::backend::{GivenRowsSpec, QueryResult};
use crate::session::staged::StagedQuery;

/// The answer a committed plan resolves to.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum PlanAnswer {
    /// Provider rows, or fetched documents, in provider order.
    Rows(Vec<serde_json::Value>),
    /// Number of rows a read matched.
    ///
    /// The count is taken over the fully loaded answer stream, so the
    /// transaction's row and byte caps bound what can be counted: a read
    /// matching more rows than the cap fails instead of answering.
    Count(u64),
    /// Whether a read matched any row.
    Exists(bool),
    /// Number of rows the write pipeline produced. Zero means nothing was
    /// written, because every write stage runs once per incoming row.
    ///
    /// Like [`Self::Count`], the rows are fully loaded and counted: a write
    /// producing more rows than the transaction's row or byte cap aborts
    /// the commit, so nothing it wrote is kept.
    Matched(u64),
}

/// Lower one trusted plan into the official AST plus its `given` rows.
pub(crate) fn lower(plan: &QueryPlanV3) -> Result<(tql::Query, Option<GivenRowsSpec>), Diagnostic> {
    let bindings = plan.bindings();
    let inputs = plan.inputs();
    let mut stages = Vec::new();
    let given = if inputs.is_empty() {
        None
    } else {
        let rows = plan.given_rows().ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_given_rows_missing",
                "a plan with input columns executes only with bound given rows",
            )
        })?;
        for value in rows.iter().flat_map(|row| row.values()).flatten() {
            type_bridge_schema::validate_provider_temporal_value(value)?;
        }
        stages.push(ast(tql::given(
            &inputs
                .iter()
                .map(|column| (column.public_name().as_str(), column.value_type()))
                .collect::<Vec<_>>(),
        ))?);
        Some(given_rows_spec(inputs, rows)?)
    };
    let scope = Scope { bindings, inputs };
    for (index, stage) in plan.pipeline().iter().enumerate() {
        match stage {
            Stage::Match { patterns } => stages.push(tql::match_stage(scope.patterns(patterns)?)),
            Stage::Insert { things } => stages.push(tql::insert(scope.things(things)?)),
            Stage::Put { things } => stages.push(tql::put(scope.things(things)?)),
            Stage::Update { sets } => stages.push(tql::update(
                sets.iter()
                    .map(|set| scope.set(set))
                    .collect::<Result<_, _>>()?,
            )),
            Stage::Delete { items } => scope.delete(index, items, &mut stages)?,
            Stage::Select { bindings } => stages.push(ast(tql::select(&scope.names(bindings)?))?),
            Stage::Require { bindings } => {
                stages.push(ast(tql::require(&scope.names(bindings)?))?);
            }
            Stage::Distinct => stages.push(tql::distinct()),
            Stage::Reduce {
                assignments,
                groups,
            } => {
                let assignments = assignments
                    .iter()
                    .map(|assignment| {
                        Ok((
                            scope.name(assignment.assigned())?,
                            ast(tql::reducer(
                                reducer_keyword(assignment.reducer()),
                                assignment
                                    .input()
                                    .map(|input| scope.name(input))
                                    .transpose()?,
                            ))?,
                        ))
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?;
                stages.push(ast(tql::reduce_stage(assignments, &scope.names(groups)?))?);
            }
            Stage::Sort { terms } => {
                let fields = terms
                    .iter()
                    .map(|term| {
                        Ok((
                            scope.name(term.binding())?,
                            matches!(term.direction(), OrderDirection::Descending),
                        ))
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?;
                stages.push(ast(tql::sort(&fields))?);
            }
            Stage::Offset { rows } => stages.push(tql::offset(*rows)),
            Stage::Limit { rows } => stages.push(tql::limit(*rows)),
        }
    }
    let projects = matches!(
        plan.operation(),
        Operation::Rows
            | Operation::Write {
                returns: WriteReturns::Rows
            }
    );
    match plan.output() {
        QueryOutput::Rows { columns } if projects && !columns.is_empty() => {
            stages.push(ast(tql::select(&scope.names(columns)?))?);
        }
        QueryOutput::Documents { fields } if projects => {
            let entries = fields
                .iter()
                .map(|field| {
                    let value = match field.source() {
                        DocumentSource::Binding { binding } => {
                            tql::fetch_expr(ast(tql::var(scope.name(*binding)?))?)
                        }
                        DocumentSource::AttributeList { attribute, owner } => {
                            ast(tql::fetch_attribute(
                                scope.name(*owner)?,
                                attribute.label().as_str(),
                                true,
                            ))?
                        }
                    };
                    Ok((field.key().as_str(), value))
                })
                .collect::<Result<Vec<_>, Diagnostic>>()?;
            stages.push(ast(tql::fetch(entries))?);
        }
        _ if plan.operation() == Operation::Exists => stages.push(tql::limit(1)),
        _ => {}
    }
    let functions = lower_functions(inputs, plan.functions())?;
    Ok((tql::pipeline_with_functions(functions, stages), given))
}

/// Lower, render and bound one plan as a staged provider query.
pub(crate) fn staged_query(
    plan: &QueryPlanV3,
    supports_given_rows: bool,
) -> Result<StagedQuery, Diagnostic> {
    let (query, given) = lower(plan)?;
    if given.is_some() && !supports_given_rows {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_given_transport_unsupported",
            "this plan requires the native given transport capability",
        ));
    }
    let typeql = ast(tql::render(&query))?;
    if typeql.len() > plan.limits().codec.max_bytes {
        return Err(plan.limits().annotate(failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_lowered_typeql_byte_limit",
            "lowered TypeQL exceeds the canonical protocol byte ceiling",
        )));
    }
    let mut staged = StagedQuery::new(typeql, plan.writes())
        .canonical()
        .with_failure_context(failure_context(plan));
    if let Some(rows) = given {
        staged = staged.with_rows(rows);
    }
    Ok(staged)
}

/// Resolve the provider answer of one executed plan.
///
/// This runs before the provider commit, so a malformed answer aborts the
/// transaction instead of surfacing after the writes are durable.
pub(crate) fn resolve(operation: Operation, result: QueryResult) -> Result<PlanAnswer, Diagnostic> {
    let items = match result {
        QueryResult::Rows(items) | QueryResult::Documents(items) => items,
        QueryResult::Ok => {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "query_plan_answer_shape",
                "a plan pipeline answered without a row stream",
            ));
        }
    };
    let count = u64::try_from(items.len()).unwrap_or(u64::MAX);
    Ok(match operation {
        Operation::Rows
        | Operation::Write {
            returns: WriteReturns::Rows,
        } => PlanAnswer::Rows(items),
        Operation::Count => PlanAnswer::Count(count),
        Operation::Exists => PlanAnswer::Exists(count != 0),
        Operation::Write {
            returns: WriteReturns::Matched,
        } => PlanAnswer::Matched(count),
    })
}

/// Value-free schema identities of the plan's first write.
fn failure_context(plan: &QueryPlanV3) -> FailureContext {
    for stage in plan.pipeline() {
        match stage {
            Stage::Update { sets } => {
                if let Some(Set::Field { field, .. }) = sets.first() {
                    return FailureContext::new(Some(field.owner().clone()), Some(field.clone()));
                }
            }
            Stage::Delete { items } => {
                if let Some(DeleteItem::Has { field, .. }) = items.first() {
                    return FailureContext::new(Some(field.owner().clone()), Some(field.clone()));
                }
            }
            Stage::Insert { things } | Stage::Put { things } => {
                if let Some(thing) = things.first() {
                    return FailureContext::new(Some(thing.type_id.clone()), None);
                }
            }
            _ => {}
        }
    }
    FailureContext::default()
}

struct Scope<'a> {
    bindings: &'a [AssertionBinding],
    inputs: &'a [InputColumn],
}

impl Scope<'_> {
    fn name(&self, binding: BindingId) -> Result<&str, Diagnostic> {
        local_variable(self.bindings, binding)
    }

    fn names(&self, bindings: &[BindingId]) -> Result<Vec<&str>, Diagnostic> {
        bindings.iter().map(|binding| self.name(*binding)).collect()
    }

    fn operand(&self, operand: &QueryOperand) -> Result<tql::Expr, Diagnostic> {
        lower_operand(self.inputs, self.bindings, operand, None)
    }

    /// Read patterns keep their relative order and lower in one scope, so
    /// generated witness names stay unique. `let` assignments follow in
    /// source order; the validator resolves them after every read pattern.
    fn patterns(&self, patterns: &[Pattern]) -> Result<Vec<tql::Pattern>, Diagnostic> {
        let reads: Vec<QueryPattern> = patterns
            .iter()
            .filter_map(|pattern| match pattern {
                Pattern::Read(pattern) => Some(pattern.clone()),
                Pattern::Let { .. } => None,
            })
            .collect();
        let mut output = lower_scoped_patterns(self.inputs, self.bindings, &reads, None, None)?;
        for pattern in patterns {
            if let Pattern::Let { binding, expr } = pattern {
                output.push(ast(tql::let_value(
                    &[self.name(*binding)?],
                    self.expr(expr)?,
                    false,
                ))?);
            }
        }
        Ok(output)
    }

    fn expr(&self, expr: &Expr) -> Result<tql::Expr, Diagnostic> {
        match expr {
            Expr::Operand(operand) => self.operand(operand),
            Expr::Arith { op, lhs, rhs } => ast(tql::arithmetic(
                self.expr(lhs)?,
                match op {
                    ArithOp::Add => "+",
                    ArithOp::Sub => "-",
                    ArithOp::Mul => "*",
                    ArithOp::Div => "/",
                    ArithOp::Mod => "%",
                    ArithOp::Pow => "^",
                },
                self.expr(rhs)?,
            )),
            Expr::Call { func, args } => ast(tql::function(
                match func {
                    Builtin::Abs => "abs",
                    Builtin::Ceil => "ceil",
                    Builtin::Floor => "floor",
                    Builtin::Round => "round",
                    Builtin::Min => "min",
                    Builtin::Max => "max",
                    Builtin::Length => "len",
                },
                args.iter()
                    .map(|arg| self.expr(arg))
                    .collect::<Result<_, _>>()?,
            )),
        }
    }

    /// `has A == $v` for every bound variable and `has A <literal>` for
    /// plan constants. TypeDB 3.13 reads `has A $v` in a write stage as a
    /// new `$v isa A` and rejects it (WCP1) when `$v` is already bound.
    fn has(
        &self,
        attribute: &AttributeId,
        operand: &QueryOperand,
    ) -> Result<tql::Constraint, Diagnostic> {
        let label = attribute.label().as_str();
        let value = self.operand(operand)?;
        if !matches!(operand, QueryOperand::Literal { .. }) {
            ast(tql::has_comparison(label, "==", value))
        } else {
            ast(tql::has(label, value))
        }
    }

    fn things(&self, things: &[InsertThing]) -> Result<Vec<tql::Pattern>, Diagnostic> {
        things
            .iter()
            .map(|thing| {
                let mut constraints = vec![ast(tql::isa(thing.type_id.label().as_str(), false))?];
                if !thing.players.is_empty() {
                    constraints.push(ast(tql::links(
                        thing
                            .players
                            .iter()
                            .map(|(role, player)| Ok((role.label().as_str(), self.name(*player)?)))
                            .collect::<Result<Vec<_>, Diagnostic>>()?,
                    ))?);
                }
                for (field, operand) in &thing.fields {
                    constraints.push(self.has(field.attribute(), operand)?);
                }
                ast(tql::thing(self.name(thing.binding)?, constraints))
            })
            .collect()
    }

    fn set(&self, set: &Set) -> Result<tql::Pattern, Diagnostic> {
        match set {
            Set::Field {
                owner,
                field,
                operand,
            } => ast(tql::thing(
                self.name(*owner)?,
                vec![self.has(field.attribute(), operand)?],
            )),
            Set::Link {
                relation,
                role,
                player,
            } => ast(tql::thing(
                self.name(*relation)?,
                vec![ast(tql::links(vec![(
                    role.label().as_str(),
                    self.name(*player)?,
                )]))?],
            )),
        }
    }

    /// An ownership deletion binds every value of that ownership in a
    /// preceding optional match. Owners without a value keep their row, so
    /// later stages and `Matched` still see them; each additional value of a
    /// multi-valued ownership adds one row.
    fn delete(
        &self,
        stage: usize,
        items: &[DeleteItem],
        stages: &mut Vec<tql::Stage>,
    ) -> Result<(), Diagnostic> {
        let mut optional = Vec::new();
        let mut deletables = Vec::new();
        for (index, item) in items.iter().enumerate() {
            deletables.push(match item {
                DeleteItem::Thing { binding } => ast(tql::delete_thing(self.name(*binding)?))?,
                DeleteItem::Links {
                    relation,
                    role,
                    player,
                } => ast(tql::delete_links(
                    self.name(*relation)?,
                    vec![(role.label().as_str(), self.name(*player)?)],
                ))?,
                DeleteItem::Has { owner, field } => {
                    // Uppercase cannot collide with validated lowercase plan variables.
                    let value = format!("PlanHas{stage}x{index}");
                    let owner = self.name(*owner)?;
                    let label = field.attribute().label().as_str();
                    optional.push(tql::optional(vec![
                        ast(tql::thing(
                            owner,
                            vec![ast(tql::has(label, ast(tql::var(&value))?))?],
                        ))?,
                        ast(tql::thing(&value, vec![ast(tql::isa(label, true))?]))?,
                    ]));
                    tql::delete_optional(vec![ast(tql::delete_has(owner, &value))?])
                }
            });
        }
        if !optional.is_empty() {
            stages.push(tql::match_stage(optional));
        }
        stages.push(tql::delete(deletables));
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/support/plan_v3_race.rs"]
pub(crate) mod race;

#[cfg(test)]
mod tests {
    use super::*;

    fn typeql(plan: &QueryPlanV3) -> (String, Option<GivenRowsSpec>) {
        let (query, given) = lower(plan).unwrap();
        (tql::render(&query).unwrap(), given)
    }

    fn assert_same_ast(actual: &str, expected: &str) {
        assert!(
            tql::equivalent(actual, expected).unwrap(),
            "lowered:\n{actual}\nexpected:\n{expected}"
        );
    }

    #[test]
    fn purchase_lowers_to_the_one_query_race_pipeline() {
        let plan = race::purchase("O1", "I1", "A1");
        let (actual, given) = typeql(&plan);
        assert_same_ast(&actual, race::PURCHASE_TYPEQL);
        let given = given.unwrap();
        assert_eq!(given.variables, ["order_id", "item_id", "account_id"]);
        assert_eq!(given.rows.len(), 1);
        // Caller values travel only as given rows.
        for value in ["O1", "I1", "A1"] {
            assert!(!actual.contains(&format!("\"{value}\"")));
        }
    }

    #[test]
    fn touch_and_ownership_delete_lower_to_hand_written_pipelines() {
        assert_same_ast(&typeql(&race::touch()).0, race::TOUCH_TYPEQL);
        assert_same_ast(&typeql(&race::drop_price()).0, race::DROP_PRICE_TYPEQL);
    }

    #[test]
    fn write_rows_appends_its_projection_and_reads_keep_their_operation() {
        let (actual, _) = typeql(&race::touch_rows());
        assert_same_ast(&actual, &format!("{} select $doctor;", race::TOUCH_TYPEQL));
        let exists = race::stock_read(Operation::Exists);
        assert_same_ast(
            &typeql(&exists).0,
            &format!("{} limit 1;", race::STOCK_READ_TYPEQL),
        );
        let count = race::stock_read(Operation::Count);
        assert_same_ast(&typeql(&count).0, race::STOCK_READ_TYPEQL);
    }

    #[test]
    fn templates_without_rows_and_providers_without_given_are_refused() {
        let template = race::purchase_template();
        assert_eq!(
            lower(&template).unwrap_err().code().as_str(),
            "query_plan_given_rows_missing"
        );
        let plan = race::purchase("O1", "I1", "A1");
        assert_eq!(
            staged_query(&plan, false).unwrap_err().code().as_str(),
            "query_plan_given_transport_unsupported"
        );
        let staged = staged_query(&plan, true).unwrap();
        assert!(staged.writes);
        assert_eq!(
            staged
                .failure_context
                .attribute
                .as_ref()
                .unwrap()
                .attribute()
                .label()
                .as_str(),
            "stock"
        );
    }

    #[test]
    fn answers_resolve_from_the_same_stream() {
        let rows = || QueryResult::Rows(vec![serde_json::json!({}), serde_json::json!({})]);
        let matched = Operation::Write {
            returns: WriteReturns::Matched,
        };
        assert_eq!(resolve(matched, rows()).unwrap(), PlanAnswer::Matched(2));
        assert_eq!(
            resolve(matched, QueryResult::Rows(vec![])).unwrap(),
            PlanAnswer::Matched(0)
        );
        assert_eq!(
            resolve(Operation::Count, rows()).unwrap(),
            PlanAnswer::Count(2)
        );
        assert_eq!(
            resolve(Operation::Exists, QueryResult::Rows(vec![])).unwrap(),
            PlanAnswer::Exists(false)
        );
        assert!(matches!(
            resolve(Operation::Rows, rows()).unwrap(),
            PlanAnswer::Rows(rows) if rows.len() == 2
        ));
        assert_eq!(
            resolve(matched, QueryResult::Ok)
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_answer_shape"
        );
    }
}
