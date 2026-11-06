//! Transactional data-step programs. User code is identified, never serialized.
use serde::Serialize;

use crate::capability::CapabilitySet;
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::migration::MigrationStepId;
use crate::query_plan::QueryPlan;

/// One program executed inside an engine-owned managed write transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DataProgram {
    /// A trusted query-plan v3 program, with no invocation inputs.
    Plan {
        /// Canonical plan bytes are covered by the manifest digest.
        plan: Box<QueryPlan>,
    },
    /// Caller-owned TypeQL; its exact text is covered by the manifest digest.
    Typeql {
        /// One raw TypeQL query.
        text: String,
    },
    /// A callback registered in the executing SDK catalog.
    User {
        /// Stable registration identity. Callback code is not in the manifest.
        step_id: MigrationStepId,
    },
}

impl DataProgram {
    fn validate(&self) -> Result<(), Diagnostic> {
        match self {
            Self::Plan { plan } if plan.format() != crate::query_plan::QUERY_PLAN_FORMAT => {
                Err(Diagnostic::stable(
                    DiagnosticCategory::InvalidContract,
                    "migration_data_plan_format",
                    "data steps require query-plan v3",
                ))
            }
            Self::Plan { plan } if !plan.inputs().is_empty() => Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "migration_data_plan_inputs",
                "migration data plans cannot require invocation inputs",
            )),
            Self::Typeql { text } if text.trim().is_empty() => Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "migration_data_typeql_empty",
                "migration data TypeQL must not be empty",
            )),
            _ => crate::codec::to_canonical_json(self).map(|_| ()),
        }
    }
}

/// One atomic forward program and its optional atomic reverse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataStep {
    id: MigrationStepId,
    forward: DataProgram,
    #[serde(skip_serializing_if = "Option::is_none")]
    reverse: Option<DataProgram>,
    #[serde(skip)]
    required_capabilities: CapabilitySet,
}

impl DataStep {
    /// Validate programs and derive their capability union. No reverse means
    /// irreversible; rollback must refuse before acquiring a write lease.
    pub fn new(
        id: MigrationStepId,
        forward: DataProgram,
        reverse: Option<DataProgram>,
    ) -> Result<Self, Diagnostic> {
        let mut required_capabilities = CapabilitySet::new();
        for program in std::iter::once(&forward).chain(reverse.iter()) {
            program.validate()?;
            if let DataProgram::Plan { plan } = program {
                for capability in plan.required_capabilities().iter().cloned() {
                    required_capabilities.insert(capability);
                }
            }
        }
        Ok(Self {
            id,
            forward,
            reverse,
            required_capabilities,
        })
    }
    /// Return the migration-local marker identity.
    pub const fn id(&self) -> &MigrationStepId {
        &self.id
    }
    /// Return the forward program.
    pub const fn forward(&self) -> &DataProgram {
        &self.forward
    }
    /// Return the reverse program, if one was declared.
    pub const fn reverse(&self) -> Option<&DataProgram> {
        self.reverse.as_ref()
    }
    /// Return the constructor-derived plan capabilities.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }
}
