//! Inspection of the same provider program used by staged execution.

use serde::{Deserialize, Serialize};
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::TypeId;
use type_bridge_contract::query_plan::v3::{Operation, QueryPlanV3};

/// Exact provider statements and bounded metadata for a built plan.
///
/// Caller values carried by `given` rows are never included. Embedded plan
/// constants are part of the program and appear exactly as execution sends them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanExplanation {
    /// Canonical shape fingerprint, excluding transported caller values.
    pub fingerprint: type_bridge_contract::fingerprint::Fingerprint,
    /// Plan wire format.
    pub format: String,
    /// Capabilities the receiver must advertise.
    pub required_capabilities: Vec<String>,
    /// Terminal operation.
    pub operation: Operation,
    /// Binding names and schema-derived possible types.
    pub bindings: Vec<ExplainedBinding>,
    /// Public input declarations; never input values.
    pub inputs: Vec<ExplainedInput>,
    /// Transported input size, including internal snapshot inputs.
    pub given: ExplainedGiven,
    /// Complete TypeQL pipelines, in execution order.
    pub statements: Vec<String>,
    /// Read statements charged by execution.
    pub read_statements: usize,
    /// Write statements charged by execution.
    pub write_statements: usize,
}

/// One binding's identity and possible thing types; scalar bindings have none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExplainedBinding {
    /// Dense binding identity.
    pub id: u16,
    /// Provider variable, without the dollar prefix.
    pub variable: String,
    /// Schema-derived types, including admitted subtypes.
    pub possible_types: Vec<TypeId>,
}

/// One public input declaration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExplainedInput {
    /// Public variable name.
    pub public_name: String,
    /// Canonical scalar type name.
    pub value_type: String,
    /// Whether a row may omit the value.
    pub optional: bool,
}

/// Value-free summary of the native `given` transport.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExplainedGiven {
    /// Number of transported rows; zero when the program has no inputs.
    pub row_count: usize,
    /// Canonical JSON byte length of transported rows; zero without inputs.
    pub canonical_bytes: usize,
}

impl PlanExplanation {
    /// Encode the common SDK shape with the bounded canonical contract codec.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        type_bridge_contract::codec::to_canonical_json(self)
    }
}

/// Explain a trusted bound plan through the execution renderer and limits.
pub fn explain(plan: &QueryPlanV3) -> Result<PlanExplanation, Diagnostic> {
    let query = crate::query_plan_exec::staged_query(plan, true)?;
    let mut statements = Vec::new();
    if let Some((snapshot, _)) = query.snapshot {
        statements.push(snapshot);
    }
    let (read_statements, write_statements) = plan.statement_counts();
    statements.push(query.typeql);
    let given = match query.rows {
        Some(rows) => ExplainedGiven {
            row_count: rows.rows.len(),
            canonical_bytes: type_bridge_contract::codec::to_canonical_json(&rows.rows)?.len(),
        },
        None => ExplainedGiven {
            row_count: 0,
            canonical_bytes: 0,
        },
    };
    Ok(PlanExplanation {
        fingerprint: plan.shape_fingerprint()?,
        format: plan.format().to_owned(),
        required_capabilities: plan
            .required_capabilities()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect(),
        operation: plan.operation(),
        bindings: plan
            .bindings()
            .iter()
            .map(|binding| ExplainedBinding {
                id: binding.id().get(),
                variable: binding.variable().as_str().to_owned(),
                possible_types: plan
                    .binding_types(binding.id())
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect(),
            })
            .collect(),
        inputs: plan
            .inputs()
            .iter()
            .map(|input| ExplainedInput {
                public_name: input.public_name().as_str().to_owned(),
                value_type: input.value_type().as_str().to_owned(),
                optional: input.optional(),
            })
            .collect(),
        given,
        statements,
        read_statements,
        write_statements,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explanation_fingerprint_is_independent_of_caller_values() {
        let first = crate::query_plan_exec::race::purchase("short", "I1", "A1");
        let second = crate::query_plan_exec::race::purchase("other", "I2", "A2");
        let explained = explain(&first).unwrap().fingerprint;
        assert_eq!(explained, explain(&second).unwrap().fingerprint);
        assert_ne!(
            explained.digest(),
            first.fingerprint().unwrap().as_fingerprint().digest()
        );
        assert_ne!(
            explained.digest(),
            second.fingerprint().unwrap().as_fingerprint().digest()
        );
    }
}
