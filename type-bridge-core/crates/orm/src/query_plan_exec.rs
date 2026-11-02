//! Lowering and answer resolution for trusted `query-plan/v3` programs.
//!
//! A plain plan lowers to one TypeQL pipeline: a `given` stage carrying
//! every input row when the plan declares inputs, the plan's stages in
//! order, and the projection its operation needs. Plan constants render as
//! literals; caller values reach TypeDB only through `given`. A write's
//! `Matched` answer is the number of rows its pipeline produced, counted from
//! the same answer stream, so a conditional write needs no extra query.
//! Opt-in deletion snapshots first run a read-only capture pipeline with the
//! same prefix and inputs, then delete within that same transaction.

use type_bridge_core_lib::__internal as tql;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId};
use type_bridge_contract::query_plan::v3::{
    ArithOp, AttachItem, BuiltPlans, Builtin, DeleteItem, Expr, InsertThing, Operation, Pattern,
    QueryPlanV3, Set, Stage, ThingDocument, WriteReturns,
};
use type_bridge_contract::query_plan::{
    DocumentSource, InputColumn, OrderDirection, QueryOperand, QueryOutput, QueryPattern,
};
use type_bridge_contract::query_remote::{RemoteFieldValue, RemoteValue};
use type_bridge_contract::query_remote_v3::StagedAnswerV3;
use type_bridge_contract::sdk_diagnostic::FailureContext;
use type_bridge_contract::value::ValueTypeTag;

use crate::query_v2::{
    ast, failure, given_rows_spec, local_variable, lower_functions, lower_operand,
    lower_scoped_patterns_with_values, reducer_keyword,
};
use crate::session::backend::{GivenRowsSpec, GivenValue, QueryResult};
use crate::session::staged::{PlanSource, StagedQuery};

/// The answer a committed plan resolves to.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum PlanAnswer {
    /// Provider rows in provider order.
    Rows(Vec<serde_json::Value>),
    /// Schema-validated fetch documents in provider order.
    Documents(Vec<serde_json::Value>),
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

impl PlanAnswer {
    /// Decode scalar output columns in declared order for typed SDK tuples.
    /// Direct provider concepts and verified proxy values share this path.
    pub fn scalar_rows(
        &self,
        plan: &QueryPlanV3,
    ) -> Result<Vec<Vec<type_bridge_contract::value::CanonicalValue>>, Diagnostic> {
        let (Self::Rows(rows), QueryOutput::Rows { columns }) = (self, plan.output()) else {
            return Err(staged_answer_shape());
        };
        let names = columns
            .iter()
            .map(|column| local_variable(plan.bindings(), *column))
            .collect::<Result<Vec<_>, _>>()?;
        rows.iter()
            .map(|row| {
                names
                    .iter()
                    .zip(columns)
                    .map(|(name, binding)| {
                        let value = row_value(row, name, plan.limits().codec)?;
                        match value {
                            RemoteValue::Value { value } | RemoteValue::Attribute { value, .. }
                                if plan.scalar_value_type(*binding) == Some(value.value_type()) =>
                            {
                                Ok(value)
                            }
                            RemoteValue::Value { .. } | RemoteValue::Attribute { .. } => {
                                Err(staged_answer_shape())
                            }
                            RemoteValue::Thing { .. } | RemoteValue::Absent => {
                                Err(staged_answer_shape())
                            }
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// Project this answer onto the staged V3 wire evidence of `plan`.
    ///
    /// Rows carry one provider concept per output binding, keyed by the
    /// binding's variable. Fetch scalars use the validated schema domains.
    pub fn to_staged_v3(&self, plan: &QueryPlanV3) -> Result<StagedAnswerV3, Diagnostic> {
        Ok(match self {
            Self::Count(value) => StagedAnswerV3::Count { value: *value },
            Self::Exists(value) => StagedAnswerV3::Exists { value: *value },
            Self::Matched(value) => StagedAnswerV3::Matched { value: *value },
            Self::Documents(documents) => StagedAnswerV3::Documents {
                documents: documents
                    .iter()
                    .map(|document| remote_document(document, plan))
                    .collect::<Result<_, _>>()?,
            },
            Self::Rows(rows) => {
                // A thing fetch answers documents under an empty row
                // output; projecting them as rows would drop every value.
                if matches!(plan.pipeline().last(), Some(Stage::Fetch(_))) {
                    return Err(staged_answer_shape());
                }
                let QueryOutput::Rows { columns } = plan.output() else {
                    return Err(staged_answer_shape());
                };
                let names = columns
                    .iter()
                    .map(|column| local_variable(plan.bindings(), *column))
                    .collect::<Result<Vec<_>, _>>()?;
                let rows = rows
                    .iter()
                    .map(|row| {
                        names
                            .iter()
                            .map(|name| remote_value(row, name, plan.limits().codec))
                            .collect()
                    })
                    .collect::<Result<_, _>>()?;
                StagedAnswerV3::Rows { rows }
            }
        })
    }

    /// Resolve one answer of a verified staged V3 reply. Rows become
    /// objects keyed by the projected variable, holding the typed remote
    /// value; fetched documents retain their validated scalar domains.
    pub fn from_staged_v3(answer: &StagedAnswerV3, plan: &QueryPlanV3) -> Result<Self, Diagnostic> {
        answer.validate_for(plan)?;
        Self::from_verified_staged_v3(answer, plan)
    }

    pub(crate) fn from_verified_staged_v3(
        answer: &StagedAnswerV3,
        plan: &QueryPlanV3,
    ) -> Result<Self, Diagnostic> {
        let fingerprint = plan.shape_fingerprint()?.digest().to_hex();
        let _span = tracing::debug_span!(target: "type_bridge.plan", "type_bridge.plan.answer", %fingerprint, transport = "proxy").entered();
        tracing::debug!(target: "type_bridge.plan", items = match answer { StagedAnswerV3::Rows { rows } => rows.len(), StagedAnswerV3::Documents { documents } => documents.len(), _ => 1 }, wire_bytes = serde_json::to_vec(answer).map_or(0, |bytes| bytes.len()), "type_bridge.plan.answer");
        Ok(match answer {
            StagedAnswerV3::Count { value } => Self::Count(*value),
            StagedAnswerV3::Exists { value } => Self::Exists(*value),
            StagedAnswerV3::Matched { value } => Self::Matched(*value),
            StagedAnswerV3::Documents { documents } => {
                plan.fetch_document_shape()
                    .ok_or_else(staged_answer_shape)?;
                Self::Documents(
                    documents
                        .iter()
                        .map(document_json)
                        .collect::<Result<_, _>>()?,
                )
            }
            StagedAnswerV3::Rows { rows } => {
                let QueryOutput::Rows { columns } = plan.output() else {
                    return Err(staged_answer_shape());
                };
                let names = columns
                    .iter()
                    .map(|column| local_variable(plan.bindings(), *column))
                    .collect::<Result<Vec<_>, _>>()?;
                Self::Rows(
                    rows.iter()
                        .map(|row| {
                            names
                                .iter()
                                .zip(row)
                                .map(|(name, value)| {
                                    // A value that cannot serialize is a malformed
                                    // answer, never a silent null.
                                    serde_json::to_value(value)
                                        .map(|value| (name.to_string(), value))
                                        .map_err(|_| staged_value_unserializable())
                                })
                                .collect::<Result<serde_json::Map<_, _>, _>>()
                                .map(serde_json::Value::Object)
                        })
                        .collect::<Result<_, _>>()?,
                )
            }
        })
    }
}

pub(crate) fn check_provider(
    plan: &QueryPlanV3,
    version: Option<type_bridge_core_lib::version::Version>,
) -> Result<(), Diagnostic> {
    use type_bridge_contract::query_plan::v3::{CAP_NEGATED_GIVEN_IID, supports_negated_given_iid};
    if plan
        .required_capabilities()
        .iter()
        .any(|capability| capability.as_str() == CAP_NEGATED_GIVEN_IID)
        && !supports_negated_given_iid(
            version
                .as_ref()
                .and_then(type_bridge_core_lib::version::semantic_profile_id),
        )
    {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_negated_given_iid_unsupported",
            "This provider cannot safely execute nonliteral IID predicates under negation",
        )
        .with_detail("capability", CAP_NEGATED_GIVEN_IID));
    }
    Ok(())
}

/// Run a read-only plan stack on one provider snapshot. The owned and
/// borrowed entry points differ only in transaction lifecycle ownership.
pub(crate) async fn execute_plans(
    provider: &mut dyn crate::session::backend::TransactionOps,
    supports_given: bool,
    plans: &[QueryPlanV3],
    policy: crate::QueryExecutionResourceLimits,
    cancellation: &crate::session::backend::AnswerCancellation,
    deadline: crate::QueryExecutionDeadline,
) -> crate::error::Result<Vec<PlanAnswer>> {
    execute_plans_counted(
        provider,
        supports_given,
        plans,
        policy,
        cancellation,
        deadline,
        None,
    )
    .await
    .map(|(answers, _)| answers)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_plans_counted(
    provider: &mut dyn crate::session::backend::TransactionOps,
    supports_given: bool,
    plans: &[QueryPlanV3],
    policy: crate::QueryExecutionResourceLimits,
    cancellation: &crate::session::backend::AnswerCancellation,
    deadline: crate::QueryExecutionDeadline,
    prepared: Option<Vec<StagedQuery>>,
) -> crate::error::Result<(Vec<PlanAnswer>, crate::session::backend::BoundedAnswerStats)> {
    use crate::session::backend::{BoundedAnswerReader, QueryResult};
    let limits = plan_answer_limits(policy, cancellation, deadline)?;
    let check = || BoundedAnswerReader::new(limits.answer.clone()).check_before_read();
    check()?;
    if plans.iter().any(QueryPlanV3::writes) {
        return Err(plan_execution_error(
            "query_plan_read_only",
            "one-shot plan execution requires read-only plans",
        ));
    }
    if plans.len() > policy.statement_limit() as usize {
        return Err(crate::match_request::MatchError::new(
            crate::match_request::MatchErrorCategory::ResourceLimit,
            "statement_count_limit",
            "plan stack exceeds the provider-statement ceiling",
        )
        .into());
    }
    let queries = if let Some(queries) = prepared {
        if queries.len() != plans.len() || queries.iter().any(|query| query.writes) {
            return Err(plan_execution_error(
                "query_plan_read_only",
                "Prepared captures must match the read-only plan stack",
            ));
        }
        queries
    } else {
        let mut queries = Vec::with_capacity(plans.len());
        for plan in plans {
            check()?;
            queries.push(staged_query(plan, supports_given).map_err(plan_diagnostic_error)?);
        }
        queries
    };
    let mut meter = crate::answer_meter::AnswerMeter::new(&limits.answer);
    let mut semantic = crate::answer_meter::PlanMeter::new(&limits);
    let mut answers = Vec::with_capacity(plans.len());
    for (plan, query) in plans.iter().zip(queries) {
        check()?;
        let result = crate::session::staged::bounded(
            &limits.answer,
            query.execute(provider, limits.clone()),
        )
        .await?;
        check()?;
        if let QueryResult::Rows(items) | QueryResult::Documents(items) = &result {
            for item in items {
                meter.accept_value(item)?;
            }
        }
        let ResolvedPlanAnswer { wire, .. } =
            resolve_plan_with_wire(plan, result).map_err(plan_diagnostic_error)?;
        semantic.accept(&wire)?;
        answers
            .push(PlanAnswer::from_verified_staged_v3(&wire, plan).map_err(plan_diagnostic_error)?);
    }
    check()?;
    Ok((
        answers,
        crate::session::backend::BoundedAnswerStats {
            processed_items: meter.items(),
            response_bytes: meter.bytes(),
            stopped_early: false,
        },
    ))
}

pub(crate) fn plan_answer_limits(
    policy: crate::QueryExecutionResourceLimits,
    cancellation: &crate::session::backend::AnswerCancellation,
    deadline: crate::QueryExecutionDeadline,
) -> crate::error::Result<crate::session::backend::AnswerLimits> {
    policy.validate().map_err(|_| invalid_answer_limits())?;
    Ok(crate::session::backend::AnswerLimits {
        answer: crate::session::backend::BoundedAnswerLimits {
            max_items: policy.items,
            max_bytes: policy.bytes,
            deadline: Some(
                deadline
                    .earliest(crate::QueryExecutionDeadline::for_limits(policy))
                    .instant(),
            ),
            cancellation: cancellation.clone(),
        },
        max_graph_nodes: policy.graph_nodes,
        max_attribute_values: policy.attribute_values,
        max_collection_members: policy.collection_members,
        max_role_players: policy.role_players,
    })
}

pub(crate) fn invalid_answer_limits() -> crate::error::OrmError {
    plan_execution_error(
        "invalid_answer_limits",
        "answer policy exceeds supported execution ceilings",
    )
}

fn plan_execution_error(code: &'static str, message: &'static str) -> crate::error::OrmError {
    crate::match_request::MatchError::new(
        crate::match_request::MatchErrorCategory::InvalidPlan,
        code,
        message,
    )
    .into()
}

pub(crate) fn plan_diagnostic_error(diagnostic: Diagnostic) -> crate::error::OrmError {
    crate::match_request::MatchError::new(
        crate::match_request::MatchErrorCategory::ResultDecode,
        diagnostic.code().as_str(),
        diagnostic.message(),
    )
    .into()
}

/// A staged V3 row value that does not serialize as JSON.
fn staged_value_unserializable() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_remote_v3_answer_invalid",
        "a staged reply row value cannot be represented",
    )
}

/// An answer kind must match the validated plan.
/// Resolve one authored call's committed answers, correlating bulk documents
/// with canonical input values and restoring original input order. Every SDK
/// uses this function; no binding assumes provider ordering.
///
/// # Errors
/// Refuses incomplete, extra, mismatched or malformed bulk answers.
pub fn merge_plan_answers(
    plans: &BuiltPlans,
    answers: &[Arc<PlanAnswer>],
    correlations: &[Option<&type_bridge_contract::query_plan::v3::BulkCorrelation>],
) -> Result<Arc<PlanAnswer>, Diagnostic> {
    let bad = || {
        failure(
            DiagnosticCategory::Integrity,
            "query_remote_v3_document_value",
            "bulk answers do not match their input rows",
        )
    };
    let Some(chunks) = plans.bulk_chunks() else {
        return if answers.len() == 1 {
            Ok(Arc::clone(&answers[0]))
        } else {
            Err(bad())
        };
    };
    if chunks.len() == 0 {
        return match answers {
            [] => Ok(Arc::new(if plans.returns_documents() {
                PlanAnswer::Documents(vec![])
            } else {
                PlanAnswer::Matched(0)
            })),
            [answer]
                if matches!(answer.as_ref(), PlanAnswer::Documents(documents) if documents.is_empty())
                    && plans.returns_documents() =>
            {
                Ok(Arc::clone(answer))
            }
            [answer]
                if matches!(answer.as_ref(), PlanAnswer::Matched(0))
                    && !plans.returns_documents() =>
            {
                Ok(Arc::clone(answer))
            }
            _ => Err(bad()),
        };
    }
    if answers.len() != plans.plans().len() || chunks.len() != answers.len() {
        return Err(bad());
    }
    if !plans.returns_documents() {
        let mut total = 0u64;
        for (chunk, answer) in chunks.clone().zip(answers) {
            let PlanAnswer::Matched(count) = answer.as_ref() else {
                return Err(bad());
            };
            if *count != u64::try_from(chunk.positions.len()).map_err(|_| bad())? {
                return Err(bad());
            }
            total = total.checked_add(*count).ok_or_else(bad)?;
        }
        return Ok(Arc::new(PlanAnswer::Matched(total)));
    }
    let count = chunks.clone().map(|chunk| chunk.positions.len()).sum();
    let mut ordered = vec![None; count];
    for (index, ((_plan, chunk), answer)) in
        plans.plans().iter().zip(chunks).zip(answers).enumerate()
    {
        let PlanAnswer::Documents(documents) = answer.as_ref() else {
            return Err(bad());
        };
        let correlation = correlations
            .get(index)
            .and_then(|proof| *proof)
            .ok_or_else(bad)?;
        if correlation.indices().len() != chunk.positions.len() {
            return Err(bad());
        }
        for (position, source) in chunk.positions.iter().zip(correlation.indices()) {
            let target = ordered.get_mut(*position).ok_or_else(bad)?;
            if target.is_some() {
                return Err(bad());
            }
            *target = Some(documents.get(*source).ok_or_else(bad)?.clone());
        }
    }
    let documents = ordered
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(bad)?;
    Ok(Arc::new(PlanAnswer::Documents(documents)))
}

fn staged_answer_shape() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "query_remote_v3_answer_kind",
        "answer kind does not match its validated plan",
    )
}

pub(crate) fn parse_plan_scalar(
    value: &serde_json::Value,
    domain: ValueTypeTag,
    limits: type_bridge_contract::limits::CodecLimits,
) -> Result<type_bridge_contract::value::CanonicalValue, Diagnostic> {
    if domain == ValueTypeTag::String
        && let Some(text) = value.as_str()
    {
        type_bridge_contract::value::CanonicalString::new_with_limits(text, limits)
            .map(type_bridge_contract::value::CanonicalValue::String)
    } else {
        crate::migration_assertion::parse_provider_value(value, domain)
    }
}

pub(crate) fn remote_value(
    row: &serde_json::Value,
    variable: &str,
    limits: type_bridge_contract::limits::CodecLimits,
) -> Result<RemoteValue, Diagnostic> {
    use crate::migration_assertion::{provider_concept_kind, provider_value_type};
    let bad = || {
        failure(
            DiagnosticCategory::Integrity,
            "query_remote_v3_answer_concept",
            "provider row evidence is not a typed concept",
        )
    };
    if let Some(value) = row.get(variable)
        && value.get("kind").is_some()
    {
        return type_bridge_contract::query_remote_v3::decode_remote_value_with_limits(
            value.clone(),
            limits,
        );
    }
    let concept = match row.get(variable) {
        Some(serde_json::Value::Null) => return Ok(RemoteValue::Absent),
        Some(serde_json::Value::Object(concept)) => concept,
        _ => return Err(bad()),
    };
    let field = |name: &str| concept.get(name).and_then(|v| v.as_str()).ok_or_else(bad);
    let scalar = || {
        let tag = provider_value_type(field("value_type")?).ok_or_else(bad)?;
        parse_plan_scalar(concept.get("value").ok_or_else(bad)?, tag, limits)
    };
    let category = field("category")?;
    if matches!(category, "value" | "Value") {
        return Ok(RemoteValue::Value { value: scalar()? });
    }
    let kind = provider_concept_kind(category).ok_or_else(bad)?;
    let type_id = TypeId::new(kind, field("label")?).map_err(|_| bad())?;
    Ok(match kind {
        TypeKind::Attribute => RemoteValue::Attribute {
            type_id,
            value: scalar()?,
        },
        _ => RemoteValue::Thing {
            iid: field("iid")?.to_owned(),
            type_id,
        },
    })
}

fn row_value(
    row: &serde_json::Value,
    name: &str,
    limits: type_bridge_contract::limits::CodecLimits,
) -> Result<RemoteValue, Diagnostic> {
    let raw = row.get(name).ok_or_else(staged_answer_shape)?;
    if raw.get("kind").is_some() {
        serde_json::from_value(raw.clone()).map_err(|_| staged_answer_shape())
    } else {
        remote_value(row, name, limits)
    }
}

/// Lower one trusted plan into the official AST plus its `given` rows.
#[derive(Debug)]
pub(crate) struct LoweredPlan {
    pub(crate) query: tql::Query,
    pub(crate) snapshot: Option<tql::Query>,
    pub(crate) given: Option<GivenRowsSpec>,
    pub(crate) value_bearing_inputs: BTreeSet<String>,
}

pub(crate) fn lower(plan: &QueryPlanV3) -> Result<LoweredPlan, Diagnostic> {
    lower_inner(plan, false)
}

pub(crate) fn lower_redacted(plan: &QueryPlanV3) -> Result<LoweredPlan, Diagnostic> {
    lower_inner(plan, true)
}

fn lower_inner(plan: &QueryPlanV3, redact_inputs: bool) -> Result<LoweredPlan, Diagnostic> {
    // A lifted V2 plan keeps its released predicate and model terminal as
    // frozen compatibility evidence beside the pipeline. Only the pipeline
    // lowers here, so running such a plan would drop its filter and its
    // hydration; refuse it instead of answering unfiltered rows.
    if plan
        .compatibility()
        .is_some_and(|evidence| evidence.predicate().is_some() || evidence.model_query().is_some())
    {
        return Err(failure(
            DiagnosticCategory::UnsupportedCapability,
            "query_plan_compatibility_unsupported",
            "this executor cannot run a plan's frozen V2 predicate or model terminal",
        ));
    }
    let bindings = plan.bindings();
    let inputs = plan.inputs();
    let mut stages = Vec::new();
    let mut given = if inputs.is_empty() {
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
        Some(given_rows_spec(inputs, rows)?)
    };
    let snapshot_iids = snapshot_iid_inputs(plan);
    if !snapshot_iids.is_empty() {
        let encoding_failure = || {
            failure(
                DiagnosticCategory::InvalidContract,
                "query_invocation_inputs_unencodable",
                "snapshot input rows cannot be encoded",
            )
        };
        let row_count = given.as_ref().map_or(1, |given| given.rows.len());
        let existing_bytes = given
            .as_ref()
            .map_or(Ok(4), |given| {
                serde_json::to_vec(&given.rows).map(|bytes| bytes.len())
            })
            .map_err(|_| encoding_failure())?;
        let added_bytes = snapshot_iids.iter().try_fold(0_usize, |total, input| {
            let cell = GivenValue::String(input.iid.to_ascii_lowercase());
            let bytes = serde_json::to_vec(&cell).map_err(|_| encoding_failure())?;
            Ok::<_, Diagnostic>(total.saturating_add(bytes.len().saturating_add(1)))
        })?;
        if !plan.limits().structural.allows_input_rows(row_count)
            || !plan.limits().structural.allows_input_bytes(
                existing_bytes.saturating_add(added_bytes.saturating_mul(row_count)),
            )
        {
            return Err(plan.limits().annotate(failure(
                DiagnosticCategory::ResourceLimit,
                "query_invocation_input_byte_limit",
                "snapshot input rows exceed the structural transport ceiling",
            )));
        }
        let given = given.get_or_insert_with(|| GivenRowsSpec {
            variables: Vec::new(),
            rows: vec![Vec::new()],
        });
        for input in &snapshot_iids {
            given.variables.push(input.input.clone());
            for row in &mut given.rows {
                row.push(GivenValue::String(input.iid.to_ascii_lowercase()));
            }
        }
    }
    if given.is_some() {
        if inputs.iter().any(InputColumn::optional) {
            let mut nullable: Vec<_> = inputs
                .iter()
                .map(|column| {
                    (
                        column.public_name().as_str(),
                        column.value_type(),
                        column.optional(),
                    )
                })
                .collect();
            nullable.extend(
                snapshot_iids
                    .iter()
                    .map(|input| (input.input.as_str(), ValueTypeTag::String, false)),
            );
            stages.push(ast(tql::given_nullable(&nullable))?);
        } else {
            let mut columns: Vec<_> = inputs
                .iter()
                .map(|column| (column.public_name().as_str(), column.value_type()))
                .collect();
            columns.extend(
                snapshot_iids
                    .iter()
                    .map(|input| (input.input.as_str(), ValueTypeTag::String)),
            );
            stages.push(ast(tql::given(&columns))?);
        }
    }
    let literal_policy = LiteralComparisonPolicy {
        inputs,
        // Execution has already charged the real literals before redaction.
        remaining: std::cell::Cell::new(if redact_inputs {
            usize::MAX
        } else {
            plan.limits().codec.max_bytes
        }),
        redact_inputs,
    };
    let mut scope = Scope {
        plan,
        bindings,
        inputs: std::borrow::Cow::Borrowed(inputs),
        snapshot_iids: snapshot_iids.iter(),
        rows: plan.given_rows().unwrap_or(&[]),
        literal_policy: &literal_policy,
    };
    let mut snapshot = None;
    for (index, stage) in plan.pipeline().iter().enumerate() {
        match stage {
            Stage::Match { patterns } => stages.push(tql::match_stage(scope.patterns(patterns)?)),
            Stage::Insert { things } => stages.push(tql::insert(scope.things(things, false)?)),
            // A put matches before it inserts: `isa!` keeps the match to the
            // declared concrete type, so a subtype instance is never taken.
            Stage::Put {
                things,
                include_subtypes,
            } => stages.push(tql::put(scope.things(things, !include_subtypes)?)),
            Stage::Update { sets } => stages.push(tql::update(
                sets.iter()
                    .map(|set| scope.set(set))
                    .collect::<Result<_, _>>()?,
            )),
            Stage::Delete { items } => scope.delete(index, items, &mut stages)?,
            Stage::Select { bindings } => {
                stages.push(ast(tql::select(&scope.names(bindings)?))?);
                scope.retain_input_names(bindings)?;
            }
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
            Stage::Fetch(document) => {
                if matches!(plan.pipeline().last(), Some(Stage::Delete { .. })) {
                    let mut captured = stages.clone();
                    let mut complete = document.clone();
                    // Do not let role expansion remove a root from the snapshot
                    // while the deletion statement still selects that root.
                    for role in &mut complete.roles {
                        role.required = false;
                    }
                    scope.fetch(&complete, &mut captured)?;
                    snapshot = Some(tql::pipeline_with_functions(
                        lower_functions(inputs, plan.functions())?,
                        captured,
                    ));
                } else {
                    scope.fetch(document, &mut stages)?;
                }
            }
            // Additions to bound things are an insert stage over existing
            // variables; nothing fresh is created.
            Stage::Attach { items } => stages.push(tql::insert(
                items
                    .iter()
                    .map(|item| scope.attach(item))
                    .collect::<Result<_, _>>()?,
            )),
            _ => {
                return Err(failure(
                    DiagnosticCategory::UnsupportedCapability,
                    "query_plan_stage_unsupported",
                    "this executor cannot lower the plan stage",
                ));
            }
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
    Ok(LoweredPlan {
        query: tql::pipeline_with_functions(functions, stages),
        value_bearing_inputs: given_regex_inputs(plan),
        snapshot,
        given,
    })
}

/// Lower, render and bound one plan as a staged provider query.
pub(crate) fn staged_query(
    plan: &QueryPlanV3,
    supports_given_rows: bool,
) -> Result<StagedQuery, Diagnostic> {
    staged_query_for_source(plan, supports_given_rows, None)
}

pub(crate) fn staged_query_with_source(
    source: &PlanSource,
    supports_given_rows: bool,
) -> Result<StagedQuery, Diagnostic> {
    staged_query_for_source(source.plan()?, supports_given_rows, Some(source))
}

fn staged_query_for_source(
    plan: &QueryPlanV3,
    supports_given_rows: bool,
    source: Option<&PlanSource>,
) -> Result<StagedQuery, Diagnostic> {
    let _performance = tracing::info_span!("tb.query.compile").entered();
    let span = tracing::debug_span!(target: "type_bridge.plan", "type_bridge.plan.lower", fingerprint = tracing::field::Empty, stages = plan.pipeline().len(), writes = plan.writes(), statements = tracing::field::Empty, outcome = tracing::field::Empty, refusal = tracing::field::Empty);
    let fingerprint = if span.is_disabled() {
        None
    } else {
        let fingerprint = plan.shape_fingerprint()?.digest().to_hex();
        span.record("fingerprint", &fingerprint);
        Some(fingerprint)
    };
    let _entered = span.enter();
    let result = staged_query_inner(plan, supports_given_rows, fingerprint, source);
    if result.is_ok() {
        #[cfg(type_bridge_test_plan_capture)]
        if !span.is_disabled() {
            crate::debug::plan_capture::record(plan);
        }
        span.record(
            "statements",
            plan.statement_counts().0 + plan.statement_counts().1,
        );
    }
    span.record("outcome", if result.is_ok() { "ok" } else { "error" });
    if let Err(error) = &result {
        span.record("refusal", error.code().as_str());
    }
    result
}

fn staged_query_inner(
    plan: &QueryPlanV3,
    supports_given_rows: bool,
    fingerprint: Option<String>,
    source: Option<&PlanSource>,
) -> Result<StagedQuery, Diagnostic> {
    let LoweredPlan {
        query,
        snapshot,
        given,
        value_bearing_inputs,
    } = lower(plan)?;
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
    staged.fingerprint = fingerprint;
    if let Some(snapshot) = snapshot {
        let snapshot = ast(tql::render(&snapshot))?;
        if snapshot.len() > plan.limits().codec.max_bytes {
            return Err(plan.limits().annotate(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_lowered_typeql_byte_limit",
                "lowered snapshot TypeQL exceeds the canonical protocol byte ceiling",
            )));
        }
        staged.snapshot = Some((
            snapshot,
            source
                .cloned()
                .unwrap_or_else(|| PlanSource::Owned(std::sync::Arc::new(plan.clone()))),
        ));
    }
    if !value_bearing_inputs.is_empty() {
        let redacted = lower_redacted(plan)?;
        staged.redacted_typeql = Some(ast(tql::render(&redacted.query))?);
        staged.redacted_snapshot = redacted
            .snapshot
            .map(|query| ast(tql::render(&query)))
            .transpose()?;
    }
    staged.value_bearing_inputs = value_bearing_inputs;
    if let Some(rows) = given {
        staged = staged.with_rows(rows);
    }
    Ok(staged)
}

/// Validate capture evidence before dispatching the destructive statement.
pub(crate) fn validate_snapshot(
    plan: &QueryPlanV3,
    result: &QueryResult,
) -> Result<(), Diagnostic> {
    let QueryResult::Documents(documents) = result else {
        return Err(staged_answer_shape());
    };
    for document in documents {
        remote_document(document, plan)?;
    }
    Ok(())
}

/// Validate document evidence before a transaction can commit.
pub(crate) fn resolve_plan(
    plan: &QueryPlanV3,
    result: QueryResult,
) -> Result<PlanAnswer, Diagnostic> {
    resolve_plan_with_wire(plan, result).map(|resolved| resolved.answer)
}

pub(crate) struct ResolvedPlanAnswer {
    pub answer: PlanAnswer,
    pub wire: StagedAnswerV3,
    pub correlation: Option<Arc<type_bridge_contract::query_plan::v3::BulkCorrelation>>,
}

pub(crate) fn resolve_plan_with_wire(
    plan: &QueryPlanV3,
    result: QueryResult,
) -> Result<ResolvedPlanAnswer, Diagnostic> {
    if plan.fetch_document_shape().is_some() {
        let QueryResult::Documents(documents) = result else {
            return Err(staged_answer_shape());
        };
        let documents = documents
            .iter()
            .map(|document| remote_document(document, plan))
            .collect::<Result<Vec<_>, _>>()?;
        let correlation = plan
            .standalone_entity_rows()
            .map(|expected| expected.correlate_proof(&documents).map(Arc::new))
            .transpose()?;
        let answer = PlanAnswer::Documents(
            documents
                .iter()
                .map(document_json)
                .collect::<Result<_, _>>()?,
        );
        return Ok(ResolvedPlanAnswer {
            answer,
            wire: StagedAnswerV3::Documents { documents },
            correlation,
        });
    }
    if matches!(result, QueryResult::Documents(_)) {
        return Err(staged_answer_shape());
    }
    let answer = resolve(plan.operation(), result)?;
    match &answer {
        PlanAnswer::Matched(count) | PlanAnswer::Count(count) => {
            plan.validate_standalone_entity_row_count(*count)?
        }
        PlanAnswer::Rows(rows) => plan
            .validate_standalone_entity_row_count(u64::try_from(rows.len()).unwrap_or(u64::MAX))?,
        _ => {}
    }
    let wire = answer.to_staged_v3(plan)?;
    wire.validate_for(plan)?;
    Ok(ResolvedPlanAnswer {
        answer,
        wire,
        correlation: None,
    })
}

#[cfg(test)]
thread_local! {
    static DOCUMENT_VALIDATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
pub(crate) fn take_document_validation_count() -> usize {
    DOCUMENT_VALIDATIONS.with(|count| count.replace(0))
}

pub(crate) fn remote_document(
    document: &serde_json::Value,
    plan: &QueryPlanV3,
) -> Result<BTreeMap<String, RemoteFieldValue>, Diagnostic> {
    let shape = plan
        .fetch_document_shape()
        .ok_or_else(staged_answer_shape)?;
    let fields = decode_document(document, plan, shape, None)?;
    shape.validate(&fields)?;
    #[cfg(test)]
    DOCUMENT_VALIDATIONS.with(|count| count.set(count.get() + 1));
    Ok(fields)
}

fn decode_document(
    document: &serde_json::Value,
    plan: &QueryPlanV3,
    shape: &type_bridge_contract::query_plan::v3::FetchDocumentShape,
    context_role: Option<usize>,
) -> Result<BTreeMap<String, RemoteFieldValue>, Diagnostic> {
    let object = document.as_object().ok_or_else(staged_answer_shape)?;
    object
        .iter()
        .map(|(key, value)| {
            let field = if value.is_null() {
                RemoteFieldValue::Absent
            } else if let Some(role) = key
                .strip_prefix("_role_")
                .and_then(|key| key.strip_suffix("_players"))
                .and_then(|key| key.parse().ok())
            {
                RemoteFieldValue::Documents {
                    documents: value
                        .as_array()
                        .ok_or_else(staged_answer_shape)?
                        .iter()
                        .map(|player| decode_document(player, plan, shape, Some(role)))
                        .collect::<Result<_, _>>()?,
                }
            } else if key == "attributes" || key.ends_with("_attributes") {
                let role = if key == "attributes" {
                    context_role
                } else {
                    Some(
                        key.strip_prefix("_role_")
                            .and_then(|s| s.strip_suffix("_attributes"))
                            .and_then(|s| s.parse::<usize>().ok())
                            .ok_or_else(staged_answer_shape)?,
                    )
                };
                let type_key = if key == "attributes" {
                    "_type".to_owned()
                } else {
                    format!("_role_{}_type", role.ok_or_else(staged_answer_shape)?)
                };
                let type_label = object
                    .get(&type_key)
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(staged_answer_shape)?;
                let fields = value
                    .as_object()
                    .ok_or_else(staged_answer_shape)?
                    .iter()
                    .map(|(name, values)| {
                        let domain = shape
                            .attribute_domain(role, type_label, name)
                            .ok_or_else(staged_answer_shape)?;
                        let field = if let Some(values) = values.as_array() {
                            RemoteFieldValue::List {
                                values: values
                                    .iter()
                                    .map(|value| {
                                        parse_plan_scalar(value, domain, plan.limits().codec)
                                    })
                                    .collect::<Result<_, _>>()?,
                            }
                        } else if values.is_null() {
                            RemoteFieldValue::Absent
                        } else {
                            RemoteFieldValue::Scalar {
                                value: parse_plan_scalar(values, domain, plan.limits().codec)?,
                            }
                        };
                        Ok((name.clone(), field))
                    })
                    .collect::<Result<_, Diagnostic>>()?;
                RemoteFieldValue::Document { fields }
            } else {
                RemoteFieldValue::Scalar {
                    value: parse_plan_scalar(
                        value,
                        if context_role.is_none() {
                            shape.scalar_domain(key).unwrap_or(ValueTypeTag::String)
                        } else {
                            ValueTypeTag::String
                        },
                        plan.limits().codec,
                    )?,
                }
            };
            Ok((key.clone(), field))
        })
        .collect()
}

pub(crate) fn document_json(
    fields: &BTreeMap<String, RemoteFieldValue>,
) -> Result<serde_json::Value, Diagnostic> {
    fields
        .iter()
        .map(|(key, field)| {
            let value = match field {
                RemoteFieldValue::Documents { documents } => serde_json::Value::Array(
                    documents
                        .iter()
                        .map(document_json)
                        .collect::<Result<_, _>>()?,
                ),
                RemoteFieldValue::Absent => serde_json::Value::Null,
                RemoteFieldValue::Scalar { value } => scalar_json(value)?,
                RemoteFieldValue::List { values } => serde_json::Value::Array(
                    values.iter().map(scalar_json).collect::<Result<_, _>>()?,
                ),
                RemoteFieldValue::Document { fields } => document_json(fields)?,
            };
            Ok((key.clone(), value))
        })
        .collect::<Result<serde_json::Map<_, _>, _>>()
        .map(serde_json::Value::Object)
}

pub(crate) fn scalar_json(
    value: &type_bridge_contract::value::CanonicalValue,
) -> Result<serde_json::Value, Diagnostic> {
    use crate::value::AttributeValue;
    let value = crate::runtime_projection::attribute_value_from_canonical(value);
    Ok(match value {
        AttributeValue::Long(value) => value.into(),
        AttributeValue::Double(value) => serde_json::Number::from_f64(value)
            .ok_or_else(staged_answer_shape)?
            .into(),
        AttributeValue::Boolean(value) => value.into(),
        AttributeValue::String(value)
        | AttributeValue::Date(value)
        | AttributeValue::DateTime(value)
        | AttributeValue::DateTimeTZ(value)
        | AttributeValue::Decimal(value)
        | AttributeValue::Duration(value) => value.into(),
    })
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
            Stage::Insert { things } | Stage::Put { things, .. } => {
                if let Some(thing) = things.first() {
                    return FailureContext::new(Some(thing.type_id.clone()), None);
                }
            }
            _ => {}
        }
    }
    FailureContext::default()
}

struct SnapshotIidInput<'a> {
    iid: &'a str,
    input: String,
    actual: String,
}

fn snapshot_iid_inputs(plan: &QueryPlanV3) -> Vec<SnapshotIidInput<'_>> {
    if !matches!(plan.pipeline().last(), Some(Stage::Delete { .. }))
        || !plan
            .pipeline()
            .iter()
            .any(|stage| matches!(stage, Stage::Fetch(_)))
    {
        return Vec::new();
    }
    let mut names: BTreeSet<String> = plan
        .bindings()
        .iter()
        .map(|binding| binding.variable().as_str().to_owned())
        .chain(
            plan.inputs()
                .iter()
                .map(|input| input.public_name().as_str().to_owned()),
        )
        .collect();
    let mut ordinal = 0;
    let mut fresh = || loop {
        let name = format!("delete_snapshot_iid{ordinal}");
        ordinal += 1;
        if names.insert(name.clone()) {
            break name;
        }
    };
    let mut result = Vec::new();
    for stage in plan.pipeline() {
        if let Stage::Match { patterns } = stage {
            for pattern in patterns {
                if let Pattern::Iid { iid, .. } = pattern {
                    result.push(SnapshotIidInput {
                        iid,
                        input: fresh(),
                        actual: fresh(),
                    });
                }
            }
        }
    }
    result
}

fn given_regex_inputs(plan: &QueryPlanV3) -> BTreeSet<String> {
    fn collect(patterns: &[QueryPattern], plan: &QueryPlanV3, columns: &mut BTreeSet<String>) {
        use type_bridge_contract::migration_assertion::ValueComparator;
        for pattern in patterns {
            match pattern {
                QueryPattern::Value {
                    comparator:
                        ValueComparator::StartsWith | ValueComparator::EndsWith | ValueComparator::Like,
                    right: QueryOperand::Input { column },
                    ..
                } => {
                    columns.insert(
                        plan.inputs()[usize::from(column.get())]
                            .public_name()
                            .as_str()
                            .to_owned(),
                    );
                }
                QueryPattern::Or { branches } => {
                    for branch in branches {
                        collect(branch, plan, columns);
                    }
                }
                QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
                    collect(patterns, plan, columns)
                }
                _ => {}
            }
        }
    }
    let mut columns = BTreeSet::new();
    for stage in plan.pipeline() {
        if let Stage::Match { patterns } = stage {
            let reads: Vec<_> = patterns
                .iter()
                .filter_map(|pattern| match pattern {
                    Pattern::Read(pattern) => Some(pattern.clone()),
                    _ => None,
                })
                .collect();
            collect(&reads, plan, &mut columns);
        }
    }
    columns
}

pub(crate) struct LiteralComparisonPolicy<'a> {
    inputs: &'a [InputColumn],
    remaining: std::cell::Cell<usize>,
    redact_inputs: bool,
}

/// TypeDB requires literal regexes. Each input value is guarded by its
/// given-column value, preserving correlated multi-row invocations.
pub(crate) fn string_comparison(
    inputs: &[InputColumn],
    bindings: &[AssertionBinding],
    comparator: type_bridge_contract::migration_assertion::ValueComparator,
    left: &QueryOperand,
    right: &QueryOperand,
    rows: &[type_bridge_contract::query_plan::InputRow],
    literal_policy: Option<&LiteralComparisonPolicy<'_>>,
) -> Result<tql::Pattern, Diagnostic> {
    use type_bridge_contract::migration_assertion::ValueComparator;
    use type_bridge_contract::value::CanonicalValue;
    use type_bridge_core_lib::ast::{TypedComparisonOperator, TypedLiteral};
    let operator = match comparator {
        ValueComparator::StartsWith => TypedComparisonOperator::StartsWith,
        ValueComparator::EndsWith => TypedComparisonOperator::EndsWith,
        ValueComparator::Like => TypedComparisonOperator::Regex,
        _ => unreachable!("only literal regex comparators use the adapter"),
    };
    let bad = || {
        failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_compare_regex_literal",
            "like requires a literal or bound string input",
        )
    };
    let fallback = std::cell::Cell::new(
        type_bridge_contract::limits::QueryLimits::default()
            .codec
            .max_bytes,
    );
    let remaining = literal_policy.map_or(&fallback, |policy| &policy.remaining);
    let charge = |pattern: tql::Pattern| {
        let bytes = ast(tql::render_pattern(&pattern))?.len();
        let Some(next) = remaining.get().checked_sub(bytes) else {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "query_plan_lowered_typeql_byte_limit",
                "literal comparison terms exceed the lowered TypeQL byte ceiling",
            ));
        };
        remaining.set(next);
        Ok(pattern)
    };
    let render = |value: &CanonicalValue| {
        let CanonicalValue::String(value) = value else {
            return Err(bad());
        };
        charge(
            type_bridge_core_lib::compiler::render_typed_comparison(
                lower_operand(inputs, bindings, left, None)?,
                operator,
                &TypedLiteral::String(value.as_str().to_owned()),
            )
            .map_err(|_| bad())?,
        )
    };
    match right {
        QueryOperand::Literal { value } => render(value),
        QueryOperand::Input { column } => {
            let mut branches = Vec::new();
            let mut seen = BTreeSet::new();
            for row in rows {
                let value = row
                    .values()
                    .get(usize::from(column.get()))
                    .and_then(Option::as_ref)
                    .ok_or_else(bad)?;
                let CanonicalValue::String(text) = value else {
                    return Err(bad());
                };
                if seen.insert(text.as_str()) {
                    let placeholder;
                    let value = if literal_policy.is_some_and(|policy| policy.redact_inputs) {
                        placeholder = CanonicalValue::String(
                            type_bridge_contract::value::CanonicalString::new(format!(
                                "<input:{}>",
                                literal_policy.map_or(inputs, |policy| policy.inputs)
                                    [usize::from(column.get())]
                                .public_name()
                                .as_str()
                            ))?,
                        );
                        &placeholder
                    } else {
                        value
                    };
                    branches.push(vec![
                        charge(ast(tql::compare(
                            lower_operand(inputs, bindings, right, None)?,
                            "==",
                            lower_operand(
                                inputs,
                                bindings,
                                &QueryOperand::Literal {
                                    value: value.clone(),
                                },
                                None,
                            )?,
                        ))?)?,
                        render(value)?,
                    ]);
                }
            }
            if branches.is_empty() {
                return Err(bad());
            }
            Ok(tql::or(branches))
        }
        QueryOperand::Binding { .. } => Err(bad()),
    }
}

struct Scope<'a> {
    plan: &'a QueryPlanV3,
    bindings: &'a [AssertionBinding],
    inputs: std::borrow::Cow<'a, [InputColumn]>,
    snapshot_iids: std::slice::Iter<'a, SnapshotIidInput<'a>>,
    rows: &'a [type_bridge_contract::query_plan::InputRow],
    literal_policy: &'a LiteralComparisonPolicy<'a>,
}

impl Scope<'_> {
    fn retain_input_names(&mut self, retained: &[BindingId]) -> Result<(), Diagnostic> {
        if self.plan.inputs().is_empty()
            || !self
                .plan
                .pipeline()
                .iter()
                .skip(1)
                .any(|stage| matches!(stage, Stage::Match { .. }))
        {
            return Ok(());
        }
        let Some(Stage::Match { patterns }) = self.plan.pipeline().first() else {
            return Ok(());
        };
        if !patterns.iter().any(|pattern| matches!(pattern,Pattern::Let { binding,expr:Expr::Operand(QueryOperand::Input { .. }) } if retained.contains(binding))) { return Ok(()); }
        let mut inputs = Vec::with_capacity(self.plan.inputs().len());
        let mut changed = false;
        for input in self.plan.inputs() {
            let alias = patterns.iter().find_map(|pattern| match pattern {
                Pattern::Let {
                    binding,
                    expr: Expr::Operand(QueryOperand::Input { column }),
                } if *column == input.id() && retained.contains(binding) => Some(*binding),
                _ => None,
            });
            if let Some(alias) = alias {
                inputs.push(InputColumn::new(
                    input.id(),
                    type_bridge_contract::migration_assertion::QueryVariable::new(
                        self.name(alias)?,
                    )?,
                    input.value_type(),
                    input.optional(),
                ));
                changed = true;
            } else {
                inputs.push(input.clone());
            }
        }
        if changed {
            self.inputs = std::borrow::Cow::Owned(inputs);
        }
        Ok(())
    }

    fn name(&self, binding: BindingId) -> Result<&str, Diagnostic> {
        local_variable(self.bindings, binding)
    }

    fn names(&self, bindings: &[BindingId]) -> Result<Vec<&str>, Diagnostic> {
        bindings.iter().map(|binding| self.name(*binding)).collect()
    }

    fn operand(&self, operand: &QueryOperand) -> Result<tql::Expr, Diagnostic> {
        lower_operand(&self.inputs, self.bindings, operand, None)
    }

    /// Read patterns keep their relative order and lower in one scope, so
    /// generated witness names stay unique. IID constraints follow, then
    /// `let` assignments in source order; the validator resolves them after
    /// every read pattern.
    fn patterns(&mut self, patterns: &[Pattern]) -> Result<Vec<tql::Pattern>, Diagnostic> {
        let reads: Vec<QueryPattern> = patterns
            .iter()
            .filter_map(|pattern| match pattern {
                Pattern::Read(pattern) => Some(pattern.clone()),
                Pattern::Let { .. } | Pattern::Iid { .. } => None,
            })
            .collect();
        let mut output = lower_scoped_patterns_with_values(
            &self.inputs,
            self.bindings,
            &reads,
            None,
            Some(crate::query_v2::PatternTypeDomains::Plan(self.plan)),
            Some(self.rows),
            Some(self.literal_policy),
        )?;
        for pattern in patterns {
            if let Pattern::Iid { binding, iid } = pattern {
                if let Some(input) = self.snapshot_iids.next() {
                    output.push(ast(tql::let_value(
                        &[&input.actual],
                        ast(tql::function(
                            "iid",
                            vec![ast(tql::var(self.name(*binding)?))?],
                        ))?,
                        false,
                    ))?);
                    output.push(ast(tql::compare(
                        ast(tql::var(&input.actual))?,
                        "==",
                        ast(tql::var(&input.input))?,
                    ))?);
                } else {
                    output.push(ast(tql::thing(
                        self.name(*binding)?,
                        vec![ast(tql::iid(iid))?],
                    ))?);
                }
            }
        }
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
            Expr::Iid { binding } => ast(tql::function(
                "iid",
                vec![ast(tql::var(self.name(*binding)?))?],
            )),
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

    fn things(&self, things: &[InsertThing], exact: bool) -> Result<Vec<tql::Pattern>, Diagnostic> {
        things
            .iter()
            .map(|thing| {
                let mut constraints = vec![ast(tql::isa(thing.type_id.label().as_str(), exact))?];
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

    fn attach(&self, item: &AttachItem) -> Result<tql::Pattern, Diagnostic> {
        let (pattern, optional) = match item {
            AttachItem::Has {
                owner,
                field,
                operand,
                optional,
            } => (
                ast(tql::thing(
                    self.name(*owner)?,
                    vec![self.has(field.attribute(), operand)?],
                ))?,
                *optional,
            ),
            AttachItem::Links {
                relation,
                role,
                player,
                optional,
            } => (
                ast(tql::thing(
                    self.name(*relation)?,
                    vec![ast(tql::links(vec![(
                        role.label().as_str(),
                        self.name(*player)?,
                    )]))?],
                ))?,
                *optional,
            ),
            _ => {
                return Err(failure(
                    DiagnosticCategory::UnsupportedCapability,
                    "query_plan_stage_unsupported",
                    "this executor cannot lower the attach item",
                ));
            }
        };
        Ok(if optional {
            tql::optional(vec![pattern])
        } else {
            pattern
        })
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

    /// A thing document binds the exact type and each listed role's player
    /// (optional roles inside `try`) in one match, then fetches the fixed
    /// keys. Generated names are uppercase, so they cannot collide with
    /// validated lowercase plan variables.
    fn fetch(
        &self,
        document: &ThingDocument,
        stages: &mut Vec<tql::Stage>,
    ) -> Result<(), Diagnostic> {
        const TYPE: &str = "PlanFetchType";
        let thing = self.name(document.thing)?;
        let mut patterns = vec![ast(tql::thing(
            thing,
            vec![ast(tql::isa(&format!("${TYPE}"), true))?],
        ))?];
        let mut keys = vec![
            "_iid".to_owned(),
            "_type".to_owned(),
            "attributes".to_owned(),
        ];
        let mut values = vec![
            tql::fetch_expr(ast(tql::function("iid", vec![ast(tql::var(thing))?]))?),
            tql::fetch_expr(ast(tql::function("label", vec![ast(tql::var(TYPE))?]))?),
            ast(tql::fetch_attributes(thing))?,
        ];
        for scalar in &document.scalars {
            keys.push(scalar.key.as_str().to_owned());
            values.push(tql::fetch_expr(ast(tql::var(self.name(scalar.binding)?))?));
        }
        for (index, listed) in document.roles.iter().enumerate() {
            let player = format!("PlanFetchRole{index}");
            let player_type = format!("PlanFetchRole{index}Type");
            let mut bound = vec![
                ast(tql::thing(
                    thing,
                    vec![ast(tql::links(vec![(
                        listed.role.label().as_str(),
                        player.as_str(),
                    )]))?],
                ))?,
                ast(tql::thing(
                    &player,
                    vec![ast(tql::isa(&format!("${player_type}"), true))?],
                ))?,
            ];
            if let Some(owner) = self
                .plan
                .fetch_document_shape()
                .and_then(|shape| shape.restricted_role_owner(index))
            {
                bound.insert(
                    0,
                    ast(tql::thing(thing, vec![ast(tql::isa(owner, false))?]))?,
                );
            }
            if listed.collect {
                if listed.required {
                    patterns.push(tql::not(vec![tql::not(bound.clone())]));
                }
                keys.push(format!("_role_{index}_players"));
                values.push(tql::fetch_subquery(vec![
                    tql::match_stage(bound),
                    ast(tql::fetch(vec![
                        (
                            "_iid",
                            tql::fetch_expr(ast(tql::function(
                                "iid",
                                vec![ast(tql::var(&player))?],
                            ))?),
                        ),
                        (
                            "_type",
                            tql::fetch_expr(ast(tql::function(
                                "label",
                                vec![ast(tql::var(&player_type))?],
                            ))?),
                        ),
                        ("attributes", ast(tql::fetch_attributes(&player))?),
                    ]))?,
                ]));
                continue;
            }
            if listed.required {
                patterns.extend(bound);
            } else {
                patterns.push(tql::optional(bound));
            }
            keys.extend([
                format!("_role_{index}_iid"),
                format!("_role_{index}_type"),
                format!("_role_{index}_attributes"),
            ]);
            values.extend([
                tql::fetch_expr(ast(tql::function("iid", vec![ast(tql::var(&player))?]))?),
                tql::fetch_expr(ast(tql::function(
                    "label",
                    vec![ast(tql::var(&player_type))?],
                ))?),
                ast(tql::fetch_attributes(&player))?,
            ]);
        }
        stages.push(tql::match_stage(patterns));
        stages.push(ast(tql::fetch(
            keys.iter().map(String::as_str).zip(values).collect(),
        ))?);
        Ok(())
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
                DeleteItem::Players { relation, role } => {
                    // Like an ownership deletion: bind every player of the
                    // role optionally, then delete those links.
                    let player = format!("PlanPlayers{stage}x{index}");
                    let relation = self.name(*relation)?;
                    let role = role.label().as_str();
                    optional.push(tql::optional(vec![ast(tql::thing(
                        relation,
                        vec![ast(tql::links(vec![(role, player.as_str())]))?],
                    ))?]));
                    tql::delete_optional(vec![ast(tql::delete_links(
                        relation,
                        vec![(role, player.as_str())],
                    ))?])
                }
                _ => {
                    return Err(failure(
                        DiagnosticCategory::UnsupportedCapability,
                        "query_plan_stage_unsupported",
                        "this executor cannot lower the deletion",
                    ));
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
    #[test]
    fn scalar_tuples_share_direct_and_proxy_decoding_and_reject_non_scalars() {
        use type_bridge_contract::query_plan::v3::PlanBuilder;
        use type_bridge_contract::value::CanonicalValue;
        let mut builder = PlanBuilder::new();
        let item = builder.exact(
            type_bridge_contract::id::TypeId::new(
                type_bridge_contract::id::TypeKind::Entity,
                "item",
            )
            .unwrap(),
        );
        let stock = builder.field(
            item,
            type_bridge_contract::id::AttributeId::new("stock").unwrap(),
        );
        builder.returning(vec![stock]);
        let plan = builder
            .build(
                &race::schema(),
                race::purchase_template().managed_semantics().clone(),
            )
            .unwrap();
        let name = local_variable(plan.bindings(), stock).unwrap();
        let row = serde_json::Value::Object(
            [(
                name.to_owned(),
                serde_json::json!({"category":"value","value_type":"integer","value":7}),
            )]
            .into_iter()
            .collect(),
        );
        let direct = PlanAnswer::Rows(vec![row]);
        let proxy =
            PlanAnswer::from_staged_v3(&direct.to_staged_v3(&plan).unwrap(), &plan).unwrap();
        let expected = vec![vec![CanonicalValue::Long(7)]];
        assert_eq!(direct.scalar_rows(&plan).unwrap(), expected);
        assert_eq!(proxy.scalar_rows(&plan).unwrap(), expected);
        let wrong = serde_json::Value::Object([(name.to_owned(), serde_json::json!({"category":"value","value_type":"string","value":"wrong domain"}))].into_iter().collect());
        assert!(resolve_plan(&plan, QueryResult::Rows(vec![wrong])).is_err());
        for value in [
            serde_json::Value::Null,
            serde_json::json!({"kind":"thing","iid":"0x01","type_id":{"kind":"entity","label":"item"}}),
        ] {
            let row = serde_json::Value::Object([(name.to_owned(), value)].into_iter().collect());
            assert!(PlanAnswer::Rows(vec![row]).scalar_rows(&plan).is_err());
        }
        assert!(
            PlanAnswer::Rows(vec![serde_json::json!({})])
                .scalar_rows(&plan)
                .is_err()
        );
    }

    use super::*;

    #[test]
    fn bulk_answers_correlate_values_and_restore_interleaved_input_order() {
        use type_bridge_contract::query_plan::v3::PlanBuilder;
        use type_bridge_contract::schema::OwnsFactId;
        use type_bridge_contract::split::SplitLimits;
        use type_bridge_contract::value::{CanonicalString, CanonicalValue};
        let item = TypeId::new(TypeKind::Entity, "item").unwrap();
        let field = |name| OwnsFactId::new(item.clone(), AttributeId::new(name).unwrap()).unwrap();
        let row = |id: &str, stock| {
            let mut values = vec![(
                field("item-id"),
                CanonicalValue::String(CanonicalString::new(id).unwrap()),
            )];
            if let Some(stock) = stock {
                values.push((field("stock"), CanonicalValue::Long(stock)));
            }
            values
        };
        let mut builder = PlanBuilder::new();
        let thing = builder.insert(item.clone());
        builder
            .bind_model_rows(
                thing,
                vec![
                    row("I1", Some(1)),
                    row("I2", None),
                    row("I3", Some(3)),
                    row("I4", Some(4)),
                ],
            )
            .fetch_held(thing, vec![]);
        let built = builder
            .build_plans(
                &race::schema(),
                race::purchase_template().managed_semantics().clone(),
                false,
                SplitLimits::new(2, usize::MAX),
            )
            .unwrap();
        let document = |id: &str, stock: Option<i64>| {
            let mut attributes = serde_json::json!({"item-id":id});
            if let Some(stock) = stock {
                attributes["stock"] = serde_json::json!(stock);
            }
            serde_json::json!({"_iid":format!("0x{:02x}",id.as_bytes().last().unwrap()),"_type":"item","attributes":attributes})
        };
        let answers = vec![
            Arc::new(PlanAnswer::Documents(vec![
                document("I3", Some(3)),
                document("I1", Some(1)),
            ])),
            Arc::new(PlanAnswer::Documents(vec![document("I4", Some(4))])),
            Arc::new(PlanAnswer::Documents(vec![document("I2", None::<i64>)])),
        ];
        let validated = built
            .plans()
            .iter()
            .zip(&answers)
            .map(|(plan, answer)| {
                let PlanAnswer::Documents(documents) = answer.as_ref() else {
                    panic!("documents")
                };
                resolve_plan_with_wire(plan, QueryResult::Documents(documents.clone()))
            })
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let correlations = validated
            .iter()
            .map(|answer| answer.correlation.as_deref())
            .collect::<Vec<_>>();
        let merged = merge_plan_answers(&built, &answers, &correlations).unwrap();
        let PlanAnswer::Documents(documents) = merged.as_ref() else {
            panic!("expected documents");
        };
        assert_eq!(
            documents
                .iter()
                .map(|document| document["attributes"]["item-id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["I1", "I2", "I3", "I4"]
        );
        let mut wrong = answers.clone();
        wrong[0] = Arc::new(PlanAnswer::Documents(vec![
            document("wrong", Some(3)),
            document("I1", Some(1)),
        ]));
        let PlanAnswer::Documents(documents) = wrong[0].as_ref() else {
            panic!("documents")
        };
        assert_eq!(
            resolve_plan_with_wire(&built.plans()[0], QueryResult::Documents(documents.clone()))
                .err()
                .unwrap()
                .code()
                .as_str(),
            "query_remote_v3_document_value"
        );
    }

    fn typeql(plan: &QueryPlanV3) -> (String, Option<GivenRowsSpec>) {
        let lowered = lower(plan).unwrap();
        (tql::render(&lowered.query).unwrap(), lowered.given)
    }

    fn assert_same_ast(actual: &str, expected: &str) {
        assert!(
            tql::equivalent(actual, expected).unwrap(),
            "lowered:\n{actual}\nexpected:\n{expected}"
        );
    }

    #[test]
    fn negated_given_iids_are_derived_and_refused_on_unsupported_providers() {
        use type_bridge_contract::query_plan::v3::CAP_NEGATED_GIVEN_IID;
        let negative = race::scoped_given_iid("0x10", true, false);
        assert!(
            negative
                .required_capabilities()
                .iter()
                .any(|capability| capability.as_str() == CAP_NEGATED_GIVEN_IID)
        );
        for version in [None, Some("3.12.3".parse().unwrap())] {
            assert_eq!(
                check_provider(&negative, version)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "query_plan_negated_given_iid_unsupported"
            );
        }
        for version in ["3.13.6", "3.14.0"] {
            check_provider(&negative, Some(version.parse().unwrap())).unwrap();
        }
        for optional in [false, true] {
            let positive = race::scoped_given_iid("0x10", false, optional);
            assert!(
                !positive
                    .required_capabilities()
                    .iter()
                    .any(|capability| capability.as_str() == CAP_NEGATED_GIVEN_IID)
            );
            check_provider(&positive, Some("3.12.3".parse().unwrap())).unwrap();
        }
    }

    #[test]
    fn scoped_given_iids_use_fresh_lets_only_under_negation() {
        let (negative, _) = typeql(&race::scoped_given_iid("0x10", true, false));
        assert!(
            negative.contains("let $PlanNestedIid0 = iid($buyer);"),
            "{negative}"
        );
        assert!(
            negative.contains("let $PlanNestedIid1 = iid($buyer);"),
            "{negative}"
        );
        assert!(
            negative.contains("$PlanNestedIid0 == $wanted_iid;"),
            "{negative}"
        );
        assert!(!negative.contains("iid($buyer) =="), "{negative}");
        let branch = |index| {
            format!(
                "$buyer isa! account; $purchase isa! purchase, links (buyer: $buyer); let $PlanNestedIid{index} = iid($buyer); $PlanNestedIid{index} == $wanted_iid;"
            )
        };
        let expected = format!(
            "given $wanted_iid: string; match $purchase isa! purchase; not {{ {{ {} }} or {{ {} }}; }}; select $purchase; distinct; select $purchase;",
            branch(0),
            branch(1)
        );
        assert_eq!(
            negative,
            tql::render(&tql::parse(&expected).unwrap()).unwrap()
        );
        for optional in [false, true] {
            let (positive, _) = typeql(&race::scoped_given_iid("0x10", false, optional));
            assert_eq!(
                positive.matches("iid($buyer) == $wanted_iid;").count(),
                if optional { 1 } else { 2 },
                "{positive}"
            );
            let branch = "$buyer isa! account; $purchase isa! purchase, links (buyer: $buyer); iid($buyer) == $wanted_iid;";
            let scope = if optional {
                format!("try {{ {branch} }};")
            } else {
                format!("{{ {branch} }} or {{ {branch} }};")
            };
            let expected = format!(
                "given $wanted_iid: string; match $purchase isa! purchase; {scope} select $purchase; distinct; select $purchase;"
            );
            assert_eq!(
                positive,
                tql::render(&tql::parse(&expected).unwrap()).unwrap()
            );
            assert!(!positive.contains("PlanNestedIid"), "{positive}");
        }
    }

    #[test]
    fn regex_literals_are_charged_before_accumulating_lowered_patterns() {
        use type_bridge_contract::migration_assertion::ValueComparator;
        let plan = race::string_read(ValueComparator::Like, &["secret"]);
        let policy = LiteralComparisonPolicy {
            inputs: plan.inputs(),
            remaining: std::cell::Cell::new(2),
            redact_inputs: false,
        };
        let error = string_comparison(
            plan.inputs(),
            plan.bindings(),
            ValueComparator::Like,
            &QueryOperand::Binding {
                binding: BindingId::new(1).unwrap(),
            },
            &QueryOperand::Input {
                column: type_bridge_contract::query_plan::InputColumnId::new(0),
            },
            plan.given_rows().unwrap(),
            Some(&policy),
        )
        .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "query_plan_lowered_typeql_byte_limit"
        );
    }

    #[test]
    fn string_comparisons_reuse_compiler_escaping_and_correlate_input_rows() {
        use type_bridge_contract::migration_assertion::ValueComparator;
        for (comparator, expected) in [
            (ValueComparator::Contains, "$name contains $needle;"),
            (ValueComparator::StartsWith, r#"$name like "^x\\.y.*";"#),
            (ValueComparator::EndsWith, r#"$name like ".*x\\.y$";"#),
            (ValueComparator::Like, r#"$name like "x.y";"#),
        ] {
            let (source, given) = typeql(&race::string_read(comparator, &["x.y", "Ada"]));
            assert!(source.contains(expected), "{source}");
            assert_eq!(given.unwrap().rows.len(), 2);
            if comparator != ValueComparator::Contains {
                assert!(source.contains(r#"$needle == "x.y""#), "{source}");
                assert!(source.contains(r#"$needle == "Ada""#), "{source}");
            }
        }
    }

    #[test]
    fn deletion_snapshot_lowers_to_two_statements_from_the_same_prefix() {
        let lowered = lower(&race::delete_doctors_returning()).unwrap();
        assert_same_ast(
            &tql::render(&lowered.query).unwrap(),
            "match $doctor isa! doctor; delete $doctor;",
        );
        assert_same_ast(&tql::render(&lowered.snapshot.unwrap()).unwrap(),
            "match $doctor isa! doctor; match $doctor isa! $PlanFetchType;
             fetch { \"_iid\": iid($doctor), \"_type\": label($PlanFetchType), \"attributes\": { $doctor.* } };"
        );
    }

    #[test]
    fn deletion_snapshot_captures_required_roles_without_filtering_deleted_roots() {
        let lowered = lower(&race::delete_purchases_returning(true)).unwrap();
        assert_same_ast(
            &tql::render(&lowered.query).unwrap(),
            "match $purchase isa! purchase; delete $purchase;",
        );
        assert_same_ast(
            &tql::render(&lowered.snapshot.unwrap()).unwrap(),
            r#"match $purchase isa! purchase;
               match $purchase isa! $PlanFetchType;
                 try { $purchase links (buyer: $PlanFetchRole0);
                       $PlanFetchRole0 isa! $PlanFetchRole0Type; };
                 try { $purchase links (item: $PlanFetchRole1);
                       $PlanFetchRole1 isa! $PlanFetchRole1Type; };
               fetch {
                 "_iid": iid($purchase), "_type": label($PlanFetchType),
                 "attributes": { $purchase.* },
                 "_role_0_iid": iid($PlanFetchRole0),
                 "_role_0_type": label($PlanFetchRole0Type),
                 "_role_0_attributes": { $PlanFetchRole0.* },
                 "_role_1_iid": iid($PlanFetchRole1),
                 "_role_1_type": label($PlanFetchRole1Type),
                 "_role_1_attributes": { $PlanFetchRole1.* }
               };"#,
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
    fn implicit_purchase_keeps_caller_values_out_of_typeql() {
        let (actual, given) = typeql(&race::implicit_purchase(
            "private-order",
            "private-item",
            "private-buyer",
        ));
        let given = given.unwrap();
        assert_eq!(
            given.variables,
            [
                "item_item_id_2",
                "account_account_id_2",
                "purchase_order_id"
            ]
        );
        assert_eq!(given.rows.len(), 1);
        for value in ["private-order", "private-item", "private-buyer"] {
            assert!(!actual.contains(value));
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
    fn removing_role_players_lowers_like_an_ownership_deletion() {
        assert_same_ast(
            &typeql(&race::drop_buyers()).0,
            "match $purchase isa! purchase;
             match try { $purchase links (buyer: $PlanPlayers1x0); };
             delete try { links (buyer: $PlanPlayers1x0) of $purchase; };",
        );
    }

    #[test]
    fn attach_lowers_to_an_insert_over_bound_variables() {
        assert_same_ast(
            &typeql(&race::attach_on_call()).0,
            "match $doctor isa! doctor; insert $doctor has on-call true;",
        );
    }

    #[test]
    fn thing_fetch_lowers_to_the_released_read_back_shape() {
        assert_same_ast(
            &typeql(&race::purchase_fetch()).0,
            race::PURCHASE_FETCH_TYPEQL,
        );
        let (touch, _) = typeql(&race::touch_fetch());
        assert_same_ast(
            &touch,
            &format!(
                "{} match $doctor isa! $PlanFetchType; \
                 fetch {{ \"_iid\": iid($doctor), \"_type\": label($PlanFetchType), \
                 \"attributes\": {{ $doctor.* }} }};",
                race::TOUCH_TYPEQL
            ),
        );
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

    /// A filter adapted from a match request carries its predicate and its
    /// model terminal as frozen V2 evidence, not as pipeline stages. Only
    /// the pipeline lowers here, so the executor refuses the lifted plan
    /// instead of answering every person unfiltered.
    #[test]
    fn a_lifted_filter_plan_is_refused() {
        use type_bridge_contract::fingerprint::SemanticProfileId;
        use type_bridge_contract::managed_scope::ManagedScopeId;
        use type_bridge_contract::projection::{
            BindingTarget, ProjectionConfig, ProjectionHandler,
        };
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_schema::{
            ManagedDeltaContext, SchemaDocumentSet, build_schema_authority, normalize_documents,
            project, resolve, schema_authority_capability_vocabulary,
        };

        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("lifted-filter.yaml").unwrap(),
            "format: typebridge.schema/v2\nattributes:\n  score: { value: integer }\nentities:\n  person:\n    owns:\n      score: { key: true }\n",
        )])
        .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("lifted-filter").unwrap(),
            profile,
            schema_authority_capability_vocabulary(),
        );
        let verified =
            build_schema_authority(&declared, declared.required_capabilities(), &context).unwrap();
        let installed = crate::InstalledRuntimeProjection::try_new(
            project(
                &resolved,
                BindingTarget::Python,
                &ProjectionConfig::python(),
                &[ProjectionHandler::python_v1()],
                &[],
            )
            .unwrap(),
        )
        .unwrap()
        .with_schema_authority(&verified);
        let authority = installed.plan_authority().unwrap();
        let registry = std::sync::Arc::new(installed.match_registry().unwrap());

        let session = crate::match_request::SessionHandle::new(std::sync::Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let query = session
            .query(session.positional([person.one()]).unwrap())
            .unwrap()
            .where_predicate(person.field("score").unwrap().compare_value(
                crate::ComparisonOp::GreaterThanOrEqual,
                crate::AttributeValue::Long(1),
            ))
            .unwrap();
        let adapter =
            crate::query_v2_adapter::MatchRequestAdapterAuthority::from_registry(&registry)
                .unwrap();
        let validated = query.validate_count_by(&person).unwrap();
        let adapted =
            crate::query_v2_adapter::adapt_match_request(&validated, &registry, &adapter.context())
                .unwrap();
        let lifted = adapted
            .validated()
            .plan()
            .to_v3(authority.declared())
            .unwrap();
        assert!(lifted.compatibility().is_some_and(|evidence| {
            evidence.predicate().is_some() && evidence.model_query().is_some()
        }));
        assert_eq!(
            lower(&lifted).unwrap_err().code().as_str(),
            "query_plan_compatibility_unsupported"
        );
    }
}
