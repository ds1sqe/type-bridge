//! Private fail-closed wire DTO for plan-bound query invocations.

use serde::{Deserialize, Serialize};

use crate::codec::{from_canonical_json, to_canonical_json};
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::Fingerprint;
use crate::query_plan::{InputRow, QueryInvocation, QueryOperation, QueryPlan, failure};
use crate::value::CanonicalValue;

pub(crate) fn decode_query_invocation(
    plan: &QueryPlan,
    bytes: &[u8],
) -> Result<QueryInvocation, Diagnostic> {
    let wire = from_canonical_json::<QueryInvocationWire>(bytes)?;
    let expected_fingerprint = plan.fingerprint()?;
    if wire.plan_fingerprint != *expected_fingerprint.as_fingerprint() {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_invocation_plan_fingerprint_mismatch",
            "query invocation fingerprint does not bind the supplied plan",
        ));
    }
    let trusted = QueryInvocation::new(
        plan,
        wire.operation.rebuild(),
        wire.inputs.into_iter().map(InputRow::new).collect(),
    )?;
    if to_canonical_json(&trusted)? != bytes {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_invocation_wire_mismatch",
            "query invocation bytes normalize after trusted reconstruction",
        ));
    }
    Ok(trusted)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QueryInvocationWire {
    inputs: Vec<Vec<Option<CanonicalValue>>>,
    operation: QueryOperationWire,
    plan_fingerprint: Fingerprint,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum QueryOperationWire {
    Rows,
    Count,
    Exists,
}

impl QueryOperationWire {
    const fn rebuild(self) -> QueryOperation {
        match self {
            Self::Rows => QueryOperation::Rows,
            Self::Count => QueryOperation::Count,
            Self::Exists => QueryOperation::Exists,
        }
    }
}
