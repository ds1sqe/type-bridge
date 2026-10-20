//! The migration lease as conditional `query-plan/v3` writes.
//!
//! Every lease change is one write pipeline whose match carries its
//! condition: a takeover matches the control row by the fence and sequence
//! its caller observed, a release matches the exact active holder and
//! fence. `Matched(1)` means the change applied and `Matched(0)` means the
//! condition no longer held. Two concurrent writers of one control row both
//! write its `@card(1..1)` fence or state ownership, so TypeDB serializes
//! them through the per-owner cardinality lock: the second committer gets
//! `STC2`. A first acquisition inserts the `@key` scope, whose uniqueness
//! lock serializes concurrent first acquirers the same way.
//!
//! Fence and sequence values are canonical decimal strings in the frozen
//! control schemas (they span the full `u64` domain), so the successor fence
//! is computed here and travels as an input, not as a TypeQL `let`.

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::v3::{
    DeleteItem, InsertThing, Operation, Pattern, PlanSpec, QueryPlanV3, Set, Stage, WriteReturns,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, QueryOperand, QueryOutput, QueryPattern,
};
use type_bridge_contract::schema::{DeclaredSchema, DocumentId, OwnsFactId};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
use type_bridge_schema_compat::typeql_to_declared;
use type_bridge_schema_migration::{ExecutionFence, ExecutionScope, LeaseHolderId, MigrationLease};

use crate::control_schema::{
    CONTROL_ENTITY, CONTROL_SCOPE, JOURNAL_CONTROL_SCHEMA_TYPEQL, LEASE_FENCE, LEASE_FREE,
    LEASE_HELD, LEASE_HOLDER, LEASE_STATE, MANAGED_FENCE_SCHEMA_TYPEQL, NEXT_SEQUENCE,
};

const CONTROL: u16 = 0;

/// Lease plans for one control row kind: the journal's authoritative row,
/// which also carries the journal sequence, or the managed fence mirror.
pub(crate) struct LeasePlans {
    schema: DeclaredSchema,
    semantics: ManagedSemanticSchemaFingerprint,
    sequenced: bool,
}

/// The values one lease plan writes or compares.
#[derive(Clone, Copy)]
pub(crate) struct LeaseRow<'a> {
    pub(crate) scope: &'a ExecutionScope,
    pub(crate) holder: &'a LeaseHolderId,
    pub(crate) fence: ExecutionFence,
}

impl<'a> LeaseRow<'a> {
    pub(crate) fn of(lease: &'a MigrationLease) -> Self {
        Self {
            scope: lease.scope(),
            holder: lease.holder(),
            fence: lease.fence(),
        }
    }
}

impl LeasePlans {
    /// Plans over the paired journal's authoritative control row.
    pub(crate) fn journal(profile: &SemanticProfileId) -> Result<Self, Diagnostic> {
        Self::new(
            profile,
            JOURNAL_CONTROL_SCHEMA_TYPEQL,
            "journal-control",
            true,
        )
    }

    /// Plans over the managed database's fence mirror.
    pub(crate) fn managed(profile: &SemanticProfileId) -> Result<Self, Diagnostic> {
        Self::new(profile, MANAGED_FENCE_SCHEMA_TYPEQL, "managed-fence", false)
    }

    fn new(
        profile: &SemanticProfileId,
        schema: &'static str,
        contract: &'static str,
        sequenced: bool,
    ) -> Result<Self, Diagnostic> {
        let document = DocumentId::new(format!("typebridge-{contract}-lease-plan.typeql"))?;
        Ok(Self {
            schema: typeql_to_declared(document, schema).map_err(|_| plan_shape())?,
            semantics: ManagedSemanticSchemaFingerprint::compute(
                profile.clone(),
                schema.as_bytes(),
            )?,
            sequenced,
        })
    }

    /// Insert the first held control row of a scope.
    ///
    /// The journal row starts at sequence zero.
    pub(crate) fn insert_held(
        &self,
        row: LeaseRow<'_>,
        given: bool,
    ) -> Result<QueryPlanV3, Diagnostic> {
        let mut plan = Builder::new(given, &["control"]);
        let mut fields = vec![
            (
                self.owns(CONTROL_SCOPE)?,
                plan.text("scope_id", row.scope.managed_scope_id().as_str())?,
            ),
            (
                self.owns(LEASE_HOLDER)?,
                plan.text("holder_id", row.holder.as_str())?,
            ),
            (
                self.owns(LEASE_FENCE)?,
                plan.text("fence", &row.fence.get().to_string())?,
            ),
            (self.owns(LEASE_STATE)?, literal(LEASE_HELD)?),
        ];
        if self.sequenced {
            fields.push((self.owns(NEXT_SEQUENCE)?, literal("0")?));
        }
        plan.stages.push(Stage::Insert {
            things: vec![InsertThing {
                binding: binding(CONTROL),
                type_id: control_type()?,
                fields,
                players: vec![],
            }],
        });
        plan.build(self)
    }

    /// Take the lease over from the observed fence (and journal sequence).
    ///
    /// The row keeps its sequence; a concurrent journal append changes it,
    /// so the takeover then matches nothing.
    pub(crate) fn take_over(
        &self,
        row: LeaseRow<'_>,
        observed_fence: u64,
        observed_sequence: Option<u64>,
        given: bool,
    ) -> Result<QueryPlanV3, Diagnostic> {
        let mut names = vec!["control", "scope", "fence"];
        if self.sequenced {
            names.push("sequence");
        }
        let mut plan = Builder::new(given, &names);
        let mut patterns = vec![control()?];
        let scope = plan.text("expected_scope", row.scope.managed_scope_id().as_str())?;
        patterns.extend(self.equal(1, CONTROL_SCOPE, scope)?);
        let fence = plan.text("expected_fence", &observed_fence.to_string())?;
        patterns.extend(self.equal(2, LEASE_FENCE, fence)?);
        if self.sequenced {
            let sequence = observed_sequence.ok_or_else(plan_shape)?;
            let sequence = plan.text("expected_sequence", &sequence.to_string())?;
            patterns.extend(self.equal(3, NEXT_SEQUENCE, sequence)?);
        }
        let sets = vec![
            self.set(
                plan.text("next_fence", &row.fence.get().to_string())?,
                LEASE_FENCE,
            )?,
            self.set(plan.text("holder_id", row.holder.as_str())?, LEASE_HOLDER)?,
            self.set(literal(LEASE_HELD)?, LEASE_STATE)?,
        ];
        plan.stages.push(Stage::Match { patterns });
        plan.stages.push(Stage::Update { sets });
        plan.build(self)
    }

    /// Release exactly this held lease: mark the row free and remove its
    /// holder. The fence (and journal sequence) stay as they are.
    pub(crate) fn release(
        &self,
        row: LeaseRow<'_>,
        given: bool,
    ) -> Result<QueryPlanV3, Diagnostic> {
        let mut plan = Builder::new(given, &["control", "scope", "holder", "fence", "state"]);
        let mut patterns = vec![control()?];
        let scope = plan.text("expected_scope", row.scope.managed_scope_id().as_str())?;
        patterns.extend(self.equal(1, CONTROL_SCOPE, scope)?);
        let holder = plan.text("expected_holder", row.holder.as_str())?;
        patterns.extend(self.equal(2, LEASE_HOLDER, holder)?);
        let fence = plan.text("expected_fence", &row.fence.get().to_string())?;
        patterns.extend(self.equal(3, LEASE_FENCE, fence)?);
        patterns.extend(self.equal(4, LEASE_STATE, literal(LEASE_HELD)?)?);
        plan.stages.push(Stage::Match { patterns });
        plan.stages.push(Stage::Update {
            sets: vec![self.set(literal(LEASE_FREE)?, LEASE_STATE)?],
        });
        plan.stages.push(Stage::Delete {
            items: vec![DeleteItem::Has {
                owner: binding(CONTROL),
                field: self.owns(LEASE_HOLDER)?,
            }],
        });
        plan.build(self)
    }

    fn owns(&self, attribute: &str) -> Result<OwnsFactId, Diagnostic> {
        OwnsFactId::new(control_type()?, AttributeId::new(attribute)?)
    }

    /// `$control has <attribute> $<value>; $<value> == <operand>`.
    fn equal(
        &self,
        value: u16,
        attribute: &str,
        right: QueryOperand,
    ) -> Result<[Pattern; 2], Diagnostic> {
        Ok([
            QueryPattern::Has {
                owner: binding(CONTROL),
                attribute: binding(value),
                attribute_id: AttributeId::new(attribute)?,
            }
            .into(),
            QueryPattern::Value {
                comparator: ValueComparator::Equal,
                left: QueryOperand::Binding {
                    binding: binding(value),
                },
                right,
            }
            .into(),
        ])
    }

    fn set(&self, operand: QueryOperand, attribute: &str) -> Result<Set, Diagnostic> {
        Ok(Set::Field {
            owner: binding(CONTROL),
            field: self.owns(attribute)?,
            operand,
        })
    }
}

/// One plan under construction. Caller values become `given` inputs when the
/// provider transports them; otherwise they stay plan literals.
struct Builder {
    given: bool,
    bindings: Vec<AssertionBinding>,
    inputs: Vec<InputColumn>,
    row: Vec<Option<CanonicalValue>>,
    stages: Vec<Stage>,
}

impl Builder {
    fn new(given: bool, names: &[&str]) -> Self {
        Self {
            given,
            bindings: names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    AssertionBinding::new(
                        binding(u16::try_from(index).expect("fixed lease binding count")),
                        QueryVariable::new(*name).expect("fixed lease variable"),
                    )
                })
                .collect(),
            inputs: Vec::new(),
            row: Vec::new(),
            stages: Vec::new(),
        }
    }

    fn text(&mut self, name: &str, value: &str) -> Result<QueryOperand, Diagnostic> {
        let value = CanonicalValue::String(CanonicalString::new(value)?);
        if !self.given {
            return Ok(QueryOperand::Literal { value });
        }
        let column =
            InputColumnId::new(u16::try_from(self.inputs.len()).map_err(|_| plan_shape())?);
        self.inputs.push(InputColumn::new(
            column,
            QueryVariable::new(name)?,
            ValueTypeTag::String,
            false,
        ));
        self.row.push(Some(value));
        Ok(QueryOperand::Input { column })
    }

    fn build(self, plans: &LeasePlans) -> Result<QueryPlanV3, Diagnostic> {
        let given_rows = (!self.inputs.is_empty()).then(|| vec![InputRow::new(self.row)]);
        QueryPlanV3::new(
            PlanSpec {
                bindings: self.bindings,
                inputs: self.inputs,
                given_rows,
                operation: Operation::Write {
                    returns: WriteReturns::Matched,
                },
                pipeline: self.stages,
                output: QueryOutput::Rows { columns: vec![] },
                managed_semantics: plans.semantics.clone(),
                compatibility: None,
                functions: vec![],
            },
            &plans.schema,
        )
    }
}

fn binding(index: u16) -> BindingId {
    BindingId::new(index).expect("fixed lease binding")
}

/// `$control isa! <control entity>`.
fn control() -> Result<Pattern, Diagnostic> {
    Ok(QueryPattern::Isa {
        binding: binding(CONTROL),
        include_subtypes: false,
        type_id: control_type()?,
    }
    .into())
}

fn control_type() -> Result<TypeId, Diagnostic> {
    TypeId::new(TypeKind::Entity, CONTROL_ENTITY)
}

fn literal(value: &str) -> Result<QueryOperand, Diagnostic> {
    Ok(QueryOperand::Literal {
        value: CanonicalValue::String(CanonicalString::new(value)?),
    })
}

fn plan_shape() -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new("migration_typedb_journal_typeql_shape")
            .expect("static diagnostic code is valid"),
        "journal query cannot be lowered to TypeQL",
    )
}
