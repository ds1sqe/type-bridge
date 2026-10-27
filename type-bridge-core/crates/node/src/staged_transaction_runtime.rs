//! N-API seam for the core staged `Transaction`, direct or over the proxy.
//!
//! A [`NodeStagedTransaction`] owns one core transaction. Staging validates
//! on the JavaScript thread; `commit` runs on the provider
//! runtime and settle a Promise. Failures carry the canonical SDK diagnostic
//! host JSON, so a proxy commit raises the same typed failure as a direct one.

use std::sync::Arc;

use napi::bindgen_prelude::{Env, Object, Unknown};
use napi_derive::napi;
use tracing::Instrument as _;
use type_bridge_contract::query_plan::v3::{BuiltPlans, decode_query_plan_v3};
use type_bridge_contract::schema::decode_declared_schema;
use type_bridge_orm::ProviderRuntimeOwner;
use type_bridge_orm::session::staged::{
    Pending, PendingError, PlanAnswer, Transaction, TransactionKind, TransactionOptions,
    merge_plan_answers,
};

use crate::plan_builder_runtime::{NodePlan, NodePlanProgram};
use crate::proxy_connection_runtime::settle;
use crate::query_v2_runtime::non_shared_buffer;

/// Parse the optional JavaScript transaction kind; omitted infers from writes.
pub(crate) fn staged_options(kind: Option<&str>) -> napi::Result<TransactionOptions> {
    let options = TransactionOptions::default();
    let kind = match kind {
        None => return Ok(options),
        Some("read") => TransactionKind::Read,
        Some("write") => TransactionKind::Write,
        Some("schema") => TransactionKind::Schema,
        Some(other) => {
            return Err(napi::Error::new(
                napi::Status::InvalidArg,
                format!("unknown transaction kind '{other}'; expected read, write or schema"),
            ));
        }
    };
    Ok(options.with_kind(kind))
}

fn pending_error(error: &PendingError) -> napi::Error {
    match error {
        PendingError::Failed { diagnostic, .. } => {
            crate::match_runtime::napi_sdk_diagnostic(diagnostic.as_ref().clone())
        }
        PendingError::InvalidPlan(diagnostic) | PendingError::NotAllowedInKind(diagnostic) => {
            crate::query_v2_runtime::napi_error(diagnostic)
        }
        // The server's refusal keeps its stable code.
        PendingError::Refused { diagnostic, .. } => {
            crate::match_runtime::napi_sdk_diagnostic(diagnostic.as_ref().clone())
        }
        other => napi::Error::new(napi::Status::GenericFailure, other.to_string()),
    }
}

fn answer_json(answer: &PlanAnswer) -> napi::Result<String> {
    let value = match answer {
        PlanAnswer::Rows(rows) => serde_json::json!({ "kind": "rows", "rows": rows }),
        PlanAnswer::Documents(documents) => {
            serde_json::json!({ "kind": "documents", "documents": documents })
        }
        PlanAnswer::Count(count) => serde_json::json!({ "kind": "count", "value": count }),
        PlanAnswer::Exists(exists) => serde_json::json!({ "kind": "exists", "value": exists }),
        PlanAnswer::Matched(matched) => serde_json::json!({ "kind": "matched", "value": matched }),
        _ => {
            return Err(napi::Error::from_reason(
                "plan_answer_unsupported: this binding cannot represent the plan answer",
            ));
        }
    };
    serde_json::to_string(&value).map_err(|error| napi::Error::from_reason(error.to_string()))
}

/// Decode the native answer envelope without a JavaScript numeric round trip.
pub(crate) fn parse_plan_answer_json(json: &str) -> napi::Result<PlanAnswer> {
    #[derive(serde::Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum Answer {
        Rows { rows: Vec<serde_json::Value> },
        Documents { documents: Vec<serde_json::Value> },
        Count { value: u64 },
        Exists { value: bool },
        Matched { value: u64 },
    }
    if json.len() > type_bridge_contract::limits::MAX_CANONICAL_BYTES {
        return Err(napi::Error::from_reason(
            "plan_answer_limit: answer exceeds the canonical byte ceiling",
        ));
    }
    let answer: Answer =
        serde_json::from_str(json).map_err(|error| napi::Error::from_reason(error.to_string()))?;
    Ok(match answer {
        Answer::Rows { rows } => PlanAnswer::Rows(rows),
        Answer::Documents { documents } => PlanAnswer::Documents(documents),
        Answer::Count { value } => PlanAnswer::Count(value),
        Answer::Exists { value } => PlanAnswer::Exists(value),
        Answer::Matched { value } => PlanAnswer::Matched(value),
    })
}

/// One staged plan's answer, visible only after its commit succeeds.
#[napi]
pub struct NodePendingPlanAnswer {
    inner: Vec<Pending<PlanAnswer>>,
    plans: Arc<NodePlanProgram>,
    models: Option<Pending<type_bridge_orm::match_request::OwnedPlanFetchResult>>,
    origin: type_bridge_orm::ProjectedQueryOrigin,
    projected: std::sync::OnceLock<
        Result<
            type_bridge_orm::ProjectedQueryValue,
            type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic,
        >,
    >,
}

#[napi]
impl NodePendingPlanAnswer {
    /// Materialize only the core's published model proof, preserving opaque origins.
    #[napi(js_name = "getProjection")]
    pub fn get_projection(
        &self,
    ) -> napi::Result<Option<crate::runtime_projection::NodeAuthoredPlanResult>> {
        let Some(pending) = &self.models else {
            return Ok(None);
        };
        let proof = pending.get().map_err(|error| pending_error(&error))?;
        let NodePlanProgram::Models(model) = self.plans.as_ref() else {
            unreachable!("model pending retains its program")
        };
        if model.tuple.is_some() {
            Ok(Some(crate::runtime_projection::NodeAuthoredPlanResult {
                models: None,
                rows: Some(model.materialize_rows(&proof, self.origin.clone(), &self.projected)?),
            }))
        } else {
            Ok(Some(crate::runtime_projection::NodeAuthoredPlanResult {
                models: Some(model.materialize(&proof, self.origin.clone(), &self.projected)?),
                rows: None,
            }))
        }
    }

    /// The answer as JSON (`{kind, value}` or `{kind: "rows", rows}`), or
    /// the lifecycle or execution failure every answer of the stack shares.
    #[napi(js_name = "getJson")]
    pub fn get_json(&self) -> napi::Result<String> {
        let _entry = crate::perf_stages::entry().entered();
        let answers = self
            .inner
            .iter()
            .map(|pending| pending.get())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| pending_error(&error))?;
        let correlations = self
            .inner
            .iter()
            .map(|pending| pending.correlation())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| pending_error(&error))?;
        if let Some(models) = &self.models {
            return answer_json(
                models
                    .get()
                    .map_err(|error| pending_error(&error))?
                    .selection(),
            );
        }
        let NodePlanProgram::Raw(plans) = self.plans.as_ref() else {
            unreachable!("raw pending retains raw plans")
        };
        let answer = merge_plan_answers(plans, &answers, &correlations)
            .map_err(|error| crate::query_v2_runtime::napi_error(&error))?;
        answer_json(&answer)
    }
}

/// A logical transaction: nothing opens until commit, and each commit is one
/// atomic provider transaction (one signed `query-remote/v3` exchange over
/// the proxy).
#[napi]
pub struct NodeStagedTransaction {
    inner: Arc<tokio::sync::Mutex<Transaction>>,
    runtime: Arc<ProviderRuntimeOwner>,
    origin: type_bridge_orm::ProjectedQueryOrigin,
}

impl NodeStagedTransaction {
    pub(crate) fn new(transaction: Transaction, runtime: Arc<ProviderRuntimeOwner>) -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(transaction)),
            runtime,
            origin: type_bridge_orm::ProjectedQueryOrigin::remote_unbound(),
        }
    }

    pub(crate) fn with_origin(mut self, origin: type_bridge_orm::ProjectedQueryOrigin) -> Self {
        self.origin = origin;
        self
    }

    fn lock(&self) -> napi::Result<tokio::sync::MutexGuard<'_, Transaction>> {
        self.inner.try_lock().map_err(|_| {
            napi::Error::new(
                napi::Status::GenericFailure,
                "transaction_busy: a commit on this transaction is still running",
            )
        })
    }

    fn settle_commit(&self, env: &Env) -> napi::Result<Object<'static>> {
        let entry = crate::perf_stages::entry();
        let _entered = entry.enter();
        let inner = Arc::clone(&self.inner);
        let work = async move {
            let result = inner.lock().await.commit().await;
            result.map_err(|error| crate::match_runtime::napi_sdk_diagnostic(error.diagnostic()))
        };
        // The span closes when the provider runtime finishes the commit.
        settle(env, &self.runtime, work.instrument(entry.clone()))
    }
}

#[napi]
impl NodeStagedTransaction {
    /// Stage one canonical `query-plan/v3` program validated under the
    /// declared schema bytes. Nothing is sent until commit. Both inputs are
    /// `Uint8Array` views; shared backing storage is refused before Rust
    /// borrows it, like every other raw native byte input.
    #[napi(js_name = "stagePlan")]
    pub fn stage_plan(
        &self,
        env: Env,
        plan_bytes: Unknown,
        schema_bytes: Unknown,
    ) -> napi::Result<NodePendingPlanAnswer> {
        let _entry = crate::perf_stages::entry().entered();
        let plan_bytes = non_shared_buffer(&env, plan_bytes)?;
        let schema_bytes = non_shared_buffer(&env, schema_bytes)?;
        let schema = decode_declared_schema(&schema_bytes)
            .map_err(|error| crate::query_v2_runtime::napi_error(&error))?;
        let plan = decode_query_plan_v3(&plan_bytes, &schema)
            .map_err(|error| crate::query_v2_runtime::napi_error(&error))?;
        let plans = Arc::new(NodePlanProgram::Raw(Box::new(BuiltPlans::single(plan))));
        let inner = self
            .lock()?
            .stage_plans(Arc::clone(&plans), plans.returns_documents())
            .map_err(|error| pending_error(&error))?;
        Ok(NodePendingPlanAnswer {
            inner,
            plans,
            models: None,
            origin: self.origin.clone(),
            projected: std::sync::OnceLock::new(),
        })
    }

    /// Stage one plan built by the plan builder. Nothing is sent until
    /// commit.
    #[napi]
    pub fn stage(&self, plan: &NodePlan) -> napi::Result<NodePendingPlanAnswer> {
        let _entry = crate::perf_stages::entry().entered();
        let (inner, models) = match plan.plans.as_ref() {
            NodePlanProgram::Models(model) => {
                let answer = self
                    .lock()?
                    .stage_plan_fetch(
                        Arc::clone(&plan.plans),
                        Arc::clone(&model.registry),
                        Default::default(),
                    )
                    .map_err(|error| pending_error(&error))?;
                (vec![], Some(answer))
            }
            NodePlanProgram::Raw(_) => {
                let answers = self
                    .lock()?
                    .stage_plans(Arc::clone(&plan.plans), plan.plans.returns_documents())
                    .map_err(|error| pending_error(&error))?;
                (answers, None)
            }
        };
        Ok(NodePendingPlanAnswer {
            inner,
            models,
            plans: Arc::clone(&plan.plans),
            origin: self.origin.clone(),
            projected: std::sync::OnceLock::new(),
        })
    }

    /// Close without committing once any running commit settles; a closed
    /// transaction stays closed.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn dispose(&self, env: Env) -> napi::Result<Object<'static>> {
        let inner = Arc::clone(&self.inner);
        settle(&env, &self.runtime, async move {
            match inner.lock().await.close() {
                Ok(()) | Err(PendingError::Closed) => Ok(()),
                Err(error) => Err(pending_error(&error)),
            }
        })
    }

    /// Fire the staged stack; answers become visible only on success.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn commit(&self, env: Env) -> napi::Result<Object<'static>> {
        self.settle_commit(&env)
    }

    /// Discard staged work and keep the transaction for reuse.
    #[napi]
    pub fn rollback(&self) -> napi::Result<()> {
        self.lock()?
            .rollback()
            .map_err(|error| pending_error(&error))
    }

    /// Discard staged work and make the transaction terminal.
    #[napi]
    pub fn close(&self) -> napi::Result<()> {
        self.lock()?.close().map_err(|error| pending_error(&error))
    }
}

#[cfg(test)]
mod document_answer_tests {
    use super::*;

    #[test]
    fn document_answer_json_preserves_nested_values() {
        let documents =
            vec![serde_json::json!({"_iid":"0x01", "_type":"person", "attributes":{"name":"Ada"}})];
        let encoded = answer_json(&PlanAnswer::Documents(documents.clone())).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
            serde_json::json!({"kind":"documents", "documents":documents})
        );
    }
}

#[cfg(test)]
mod pending_diagnostic_redaction_tests {
    use super::*;
    #[test]
    fn pending_failure_uses_the_canonical_diagnostic_without_server_text() {
        let error = PendingError::Failed {
            diagnostic: Box::new(
                type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::commit_failure(
                    type_bridge_contract::sdk_diagnostic::SdkCommitFailureOutcome::Unknown,
                ),
            ),
            op_index: None,
            message: "signed-server-secret".into(),
            failure: Some(Box::new(
                type_bridge_contract::failure::Failure::Indeterminate { code: None },
            )),
            context: Default::default(),
        };
        assert!(!error.to_string().contains("signed-server-secret"));
        let surfaced = pending_error(&error);
        assert!(!surfaced.reason.contains("signed-server-secret"));
        assert!(surfaced.reason.contains("commit_outcome_unknown"));
    }
}
