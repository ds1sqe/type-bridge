//! N-API seam for the core staged `Transaction`, direct or over the proxy.
//!
//! A [`NodeStagedTransaction`] owns one core transaction. Staging validates
//! on the JavaScript thread; `commit` and `commitRetry` run on the provider
//! runtime and settle a Promise. Failures carry the canonical SDK diagnostic
//! host JSON, so a proxy commit raises the same typed failure as a direct one.

use std::sync::Arc;

use napi::bindgen_prelude::{Buffer, Env, Object};
use napi_derive::napi;
use type_bridge_contract::query_plan::v3::decode_query_plan_v3;
use type_bridge_contract::schema::decode_declared_schema;
use type_bridge_orm::ProviderRuntimeOwner;
use type_bridge_orm::session::staged::{
    Pending, PendingError, PlanAnswer, Transaction, TransactionKind, TransactionOptions,
};

use crate::proxy_connection_runtime::settle;

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
        PendingError::Failed {
            failure: Some(failure),
            context,
            ..
        } => crate::match_runtime::napi_sdk_diagnostic(
            type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::from_failure(
                failure,
                context.as_ref().clone(),
            ),
        ),
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

/// One staged plan's answer, visible only after its commit succeeds.
#[napi]
pub struct NodePendingPlanAnswer {
    inner: Pending<PlanAnswer>,
}

#[napi]
impl NodePendingPlanAnswer {
    /// The answer as JSON (`{kind, value}` or `{kind: "rows", rows}`), or
    /// the lifecycle or execution failure every answer of the stack shares.
    #[napi(js_name = "getJson")]
    pub fn get_json(&self) -> napi::Result<String> {
        let answer = self.inner.get().map_err(|error| pending_error(&error))?;
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
}

impl NodeStagedTransaction {
    pub(crate) fn new(transaction: Transaction, runtime: Arc<ProviderRuntimeOwner>) -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(transaction)),
            runtime,
        }
    }

    fn lock(&self) -> napi::Result<tokio::sync::MutexGuard<'_, Transaction>> {
        self.inner.try_lock().map_err(|_| {
            napi::Error::new(
                napi::Status::GenericFailure,
                "transaction_busy: a commit on this transaction is still running",
            )
        })
    }

    fn settle_commit(&self, env: &Env, retry: bool) -> napi::Result<Object<'static>> {
        let inner = Arc::clone(&self.inner);
        settle(env, &self.runtime, async move {
            let mut transaction = inner.lock().await;
            let result = if retry {
                transaction.commit_retry().await
            } else {
                transaction.commit().await
            };
            result.map_err(|error| crate::match_runtime::napi_sdk_diagnostic(error.diagnostic()))
        })
    }
}

#[napi]
impl NodeStagedTransaction {
    /// Stage one canonical `query-plan/v3` program validated under the
    /// declared schema bytes. Nothing is sent until commit.
    #[napi(js_name = "stagePlan")]
    pub fn stage_plan(
        &self,
        plan_bytes: Buffer,
        schema_bytes: Buffer,
    ) -> napi::Result<NodePendingPlanAnswer> {
        let schema = decode_declared_schema(&schema_bytes)
            .map_err(|error| crate::query_v2_runtime::napi_error(&error))?;
        let plan = decode_query_plan_v3(&plan_bytes, &schema)
            .map_err(|error| crate::query_v2_runtime::napi_error(&error))?;
        let inner = self
            .lock()?
            .stage_plan(&plan)
            .map_err(|error| pending_error(&error))?;
        Ok(NodePendingPlanAnswer { inner })
    }

    /// Fire the staged stack; answers become visible only on success.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn commit(&self, env: Env) -> napi::Result<Object<'static>> {
        self.settle_commit(&env, false)
    }

    /// Resend the last proxy commit with its idempotency key after an
    /// unknown outcome. Any other state rejects with a lifecycle error.
    #[napi(js_name = "commitRetry", ts_return_type = "Promise<void>")]
    pub fn commit_retry(&self, env: Env) -> napi::Result<Object<'static>> {
        self.settle_commit(&env, true)
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
