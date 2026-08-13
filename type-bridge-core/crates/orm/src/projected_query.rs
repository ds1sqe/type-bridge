//! Binding-neutral materialization of invocation-proven typed-query results.
//!
//! Canonical match validation owns identity, multiplicity, ordering, and
//! hydration semantics. This module performs one final invocation-token gate
//! and atomically converts the already validated result into the projected
//! values shared by language bindings.

use std::collections::BTreeMap;
use std::sync::Arc;

use type_bridge_contract::id::{RoleId as ProjectedRoleId, TypeId, TypeKind};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticName, SdkDiagnosticPathSegment,
    SdkExecutionDiagnostic, SdkQueryDiagnosticCategory, SdkQueryDiagnosticPathKind,
};
use type_bridge_contract::value::{CanonicalDouble, CanonicalValue};

use crate::_registry::DescriptorRegistry;
use crate::match_request::{
    BindingId, BoundFieldId, DescriptorId, FetchShape, FetchSlot, HydratedAttribute,
    HydratedRolePlayer, HydratedThing, MatchMode, MatchOperation, MatchResult, MatchRow,
    ReducedValue, ReductionRow, ResultShapeId, SlotValue, ValidatedMatchRequest,
    ValidatedMatchResult, Window,
};
use crate::projected_model::{
    ProjectedAttributeValue, ProjectedReference, ProjectedRolePlayer, ProjectedThing,
};
use crate::query_execution_limits::QueryExecutionDeadline;
use crate::runtime_projection::InstalledRuntimeProjection;
use crate::session::backend::AnswerCancellation;
use crate::session::database::DatabaseExecutionIdentity;
use crate::session::{Database, TransactionContext, TxType};
use crate::value::AttributeValue;

/// Hard projected-query row or reduction-row ceiling.
pub const MAX_PROJECTED_QUERY_ROWS: usize = 100_000;
/// Hard number of collected identities, page roots, group keys, and reducer cells.
pub const MAX_PROJECTED_QUERY_CELLS: usize = 1_000_000;
/// Hard number of projected things, including collected values and role players.
pub const MAX_PROJECTED_QUERY_THINGS: usize = 1_000_000;
/// Hard number of projected attribute values across one materialized result.
pub const MAX_PROJECTED_QUERY_ATTRIBUTE_VALUES: usize = 4_000_000;
/// Hard recursively charged projected payload byte ceiling.
pub const MAX_PROJECTED_QUERY_BYTES: usize = 64 * 1024 * 1024;

/// Caller policy clamped to the projected-query materializer's hard ceilings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectedQueryMaterializationLimits {
    rows: usize,
    cells: usize,
    things: usize,
    attribute_values: usize,
    bytes: usize,
}

impl ProjectedQueryMaterializationLimits {
    /// Construct an explicitly tightened all-or-nothing materialization policy.
    #[must_use]
    pub const fn tightened(
        rows: usize,
        cells: usize,
        things: usize,
        attribute_values: usize,
        bytes: usize,
    ) -> Self {
        Self {
            rows: min_usize(rows, MAX_PROJECTED_QUERY_ROWS),
            cells: min_usize(cells, MAX_PROJECTED_QUERY_CELLS),
            things: min_usize(things, MAX_PROJECTED_QUERY_THINGS),
            attribute_values: min_usize(attribute_values, MAX_PROJECTED_QUERY_ATTRIBUTE_VALUES),
            bytes: min_usize(bytes, MAX_PROJECTED_QUERY_BYTES),
        }
    }

    /// Return the effective row ceiling after hard-limit clamping.
    #[doc(hidden)]
    #[must_use]
    pub const fn rows(self) -> usize {
        self.rows
    }

    /// Return the effective collection-member/group-key/reducer-cell ceiling.
    #[doc(hidden)]
    #[must_use]
    pub const fn cells(self) -> usize {
        self.cells
    }

    /// Return the effective projected-thing ceiling.
    #[doc(hidden)]
    #[must_use]
    pub const fn things(self) -> usize {
        self.things
    }

    /// Return the effective projected attribute-value ceiling.
    #[doc(hidden)]
    #[must_use]
    pub const fn attribute_values(self) -> usize {
        self.attribute_values
    }

    /// Return the effective recursive projected-payload byte ceiling.
    #[doc(hidden)]
    #[must_use]
    pub const fn bytes(self) -> usize {
        self.bytes
    }
}

impl Default for ProjectedQueryMaterializationLimits {
    fn default() -> Self {
        Self::tightened(
            MAX_PROJECTED_QUERY_ROWS,
            MAX_PROJECTED_QUERY_CELLS,
            MAX_PROJECTED_QUERY_THINGS,
            MAX_PROJECTED_QUERY_ATTRIBUTE_VALUES,
            MAX_PROJECTED_QUERY_BYTES,
        )
    }
}

const fn min_usize(left: usize, right: usize) -> usize {
    if left < right { left } else { right }
}

/// Trusted source of projected-query hydration.
///
/// Constructors are the only way to obtain this value. Remote hydration is
/// deliberately unbound and has no identity-bearing constructor.
#[derive(Clone, Debug)]
pub struct ProjectedQueryOrigin {
    database_identity: Option<DatabaseExecutionIdentity>,
}

impl ProjectedQueryOrigin {
    /// Bind materialized things to one direct database.
    #[doc(hidden)]
    #[must_use]
    pub fn for_database(database: &Database) -> Self {
        Self {
            database_identity: Some(database.execution_identity()),
        }
    }

    /// Bind materialized things to the database of one borrowed read context.
    #[doc(hidden)]
    pub fn for_transaction(
        transaction: &TransactionContext,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        if transaction.tx_type() != TxType::Read {
            return Err(SdkExecutionDiagnostic::invalid_input(
                sdk_code("query_borrowed_transaction_not_read"),
                sdk_message("Typed query terminals require a borrowed read transaction"),
            ));
        }
        Ok(Self {
            database_identity: Some(transaction.execution_identity().clone()),
        })
    }

    /// Materialize authenticated remote evidence without a direct database origin.
    #[doc(hidden)]
    #[must_use]
    pub const fn remote_unbound() -> Self {
        Self {
            database_identity: None,
        }
    }

    /// Materialize an owned projected-value companion while retaining the
    /// caller's exact validated request and result proofs.
    ///
    /// Binding runtimes use this seam once, while constructing a result proof,
    /// when their existing result facade still owns and reads the non-cloneable
    /// invocation proof. They cache the returned companion rather than
    /// rematerializing it for each facade access. The value contains only
    /// validated projected models and their opaque origins; it does not expose
    /// or duplicate the invocation token.
    #[doc(hidden)]
    #[expect(
        clippy::too_many_arguments,
        reason = "the borrowed materializer keeps every invocation proof and budget explicit"
    )]
    pub fn materialize_borrowed_with_budget(
        self,
        installed: &InstalledRuntimeProjection,
        registry: &DescriptorRegistry,
        request: &ValidatedMatchRequest,
        result: &ValidatedMatchResult,
        limits: ProjectedQueryMaterializationLimits,
        cancellation: &AnswerCancellation,
        deadline: Option<QueryExecutionDeadline>,
    ) -> Result<(ProjectedQueryValue, ProjectedQueryResourceMeasure), SdkExecutionDiagnostic> {
        materialize_projected_query_value_with_budget(
            installed,
            registry,
            self,
            request,
            result,
            limits,
            cancellation,
            deadline,
        )
    }
}

/// One owned selected slot value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectedQuerySlotValue {
    /// One singular selected projected thing.
    One(Arc<ProjectedThing>),
    /// One collection preserving validated order and multiplicity.
    Many(Vec<Arc<ProjectedThing>>),
}

/// Invocation-derived metadata and owned value for one selected result slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedQuerySlot {
    binding: BindingId,
    declared_type: TypeId,
    match_mode: MatchMode,
    name: Option<String>,
    value: ProjectedQuerySlotValue,
}

impl ProjectedQuerySlot {
    /// Return the plan-local selected binding identity.
    pub const fn binding(&self) -> BindingId {
        self.binding
    }

    /// Return the generated declared type, before subtype expansion.
    pub const fn declared_type(&self) -> &TypeId {
        &self.declared_type
    }

    /// Return exact or subtype-inclusive matching behavior.
    pub const fn match_mode(&self) -> MatchMode {
        self.match_mode
    }

    /// Return the generated named member, or `None` for positional output.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Return the owned singular or collected projected value.
    pub const fn value(&self) -> &ProjectedQuerySlotValue {
        &self.value
    }
}

/// One positional or declaration-ordered named selected row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedQueryRow {
    slots: Vec<ProjectedQuerySlot>,
}

impl ProjectedQueryRow {
    /// Return selected slots in exact public output order.
    pub fn slots(&self) -> &[ProjectedQuerySlot] {
        &self.slots
    }
}

/// One exact typed reducer output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectedReducedValue {
    /// Lossless count, total on an empty stream.
    Count(u64),
    /// Optional long-domain output.
    Long(Option<i64>),
    /// Optional finite double-domain output, preserving exact IEEE bits.
    Double(Option<CanonicalDouble>),
}

/// One optional typed reduction group key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectedReductionGroup {
    /// Grouped projected thing identity and hydration.
    Thing(Arc<ProjectedThing>),
    /// Grouped projected scalar value.
    Field(Arc<ProjectedAttributeValue>),
    /// Ordered tuple of grouped projected scalar values.
    Fields(Vec<Arc<ProjectedAttributeValue>>),
}

/// One owned typed reduction row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedReductionRow {
    group: Option<ProjectedReductionGroup>,
    values: Vec<ProjectedReducedValue>,
}

impl ProjectedReductionRow {
    /// Return the optional group key.
    pub const fn group(&self) -> Option<&ProjectedReductionGroup> {
        self.group.as_ref()
    }

    /// Return reducer outputs in exact requested order.
    pub fn values(&self) -> &[ProjectedReducedValue] {
        &self.values
    }
}

/// Owned projected result variant for every canonical typed-query terminal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectedQueryValue {
    /// Distinct selected rows.
    Rows {
        /// Rows in exact validated result order.
        rows: Vec<ProjectedQueryRow>,
    },
    /// Distinct-root page and optional same-snapshot total.
    Page {
        /// Plan binding whose distinct identities define the page.
        root: BindingId,
        /// Materialized entries in exact page order.
        entries: Vec<ProjectedQueryRow>,
        /// Exact validated page window.
        window: Window,
        /// Same-snapshot total when requested.
        total: Option<u64>,
    },
    /// Distinct-root count.
    Count {
        /// Counted plan binding.
        root: BindingId,
        /// Lossless distinct identity count.
        value: u64,
    },
    /// Optional binding-grouped reduction rows.
    Reduction {
        /// Root binding whose stream was reduced.
        root: BindingId,
        /// Optional binding group.
        group: Option<BindingId>,
        /// Materialized reduction rows.
        rows: Vec<ProjectedReductionRow>,
    },
    /// Field-grouped reduction rows.
    FieldReduction {
        /// Root binding whose stream was reduced.
        root: BindingId,
        /// Bound scalar field used as the group key.
        group: BoundFieldId,
        /// Materialized reduction rows.
        rows: Vec<ProjectedReductionRow>,
    },
    /// Tuple-field-grouped reduction rows.
    FieldTupleReduction {
        /// Root binding whose stream was reduced.
        root: BindingId,
        /// Ordered bound scalar fields used as the tuple key.
        groups: Vec<BoundFieldId>,
        /// Materialized reduction rows.
        rows: Vec<ProjectedReductionRow>,
    },
    /// Distinct-root existence.
    Exists {
        /// Tested plan binding.
        root: BindingId,
        /// Whether one distinct identity exists.
        value: bool,
    },
}

/// Exact resource measure accumulated while materializing one query result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectedQueryResourceMeasure {
    rows: usize,
    cells: usize,
    things: usize,
    attribute_values: usize,
    bytes: usize,
}

impl ProjectedQueryResourceMeasure {
    /// Return the number of row or reduction-row envelopes.
    pub const fn rows(self) -> usize {
        self.rows
    }
    /// Return the number of collected identities, page roots, group keys, and reducer cells.
    pub const fn cells(self) -> usize {
        self.cells
    }
    /// Return the number of projected things, including nested role players.
    pub const fn things(self) -> usize {
        self.things
    }
    /// Return the number of projected attribute values.
    pub const fn attribute_values(self) -> usize {
        self.attribute_values
    }
    /// Return recursively charged projected payload bytes.
    pub const fn bytes(self) -> usize {
        self.bytes
    }
}

/// One all-or-nothing projected query result retaining its exact invocation proof.
#[derive(Debug)]
pub struct ProjectedQueryResult {
    request: ValidatedMatchRequest,
    shape_id: ResultShapeId,
    value: ProjectedQueryValue,
    measure: ProjectedQueryResourceMeasure,
}

impl ProjectedQueryResult {
    /// Return the exact invocation-bound request proof retained by this result.
    #[doc(hidden)]
    pub const fn request_proof(&self) -> &ValidatedMatchRequest {
        &self.request
    }

    /// Return the exact validated output-shape identity.
    pub const fn shape_id(&self) -> &ResultShapeId {
        &self.shape_id
    }

    /// Return the owned projected result value.
    pub const fn value(&self) -> &ProjectedQueryValue {
        &self.value
    }

    /// Return exact accumulated materialization resources.
    pub const fn resource_measure(&self) -> ProjectedQueryResourceMeasure {
        self.measure
    }
}

/// Consume one fresh validated request/result pair and atomically materialize it.
///
/// The invocation token and shape are checked before any projected value is
/// constructed. No output is returned unless every row, nested role player,
/// scalar group, and reducer cell validates and fits the tightened policy.
#[doc(hidden)]
pub fn materialize_projected_query_result(
    installed: &InstalledRuntimeProjection,
    registry: &DescriptorRegistry,
    origin: ProjectedQueryOrigin,
    request: ValidatedMatchRequest,
    result: ValidatedMatchResult,
    limits: ProjectedQueryMaterializationLimits,
) -> Result<ProjectedQueryResult, SdkExecutionDiagnostic> {
    materialize_projected_query_result_with_cancellation(
        installed,
        registry,
        origin,
        request,
        result,
        limits,
        &AnswerCancellation::default(),
    )
}

/// Consume and atomically materialize one fresh request/result pair with
/// cooperative cancellation checked throughout native graph traversal.
#[doc(hidden)]
pub fn materialize_projected_query_result_with_cancellation(
    installed: &InstalledRuntimeProjection,
    registry: &DescriptorRegistry,
    origin: ProjectedQueryOrigin,
    request: ValidatedMatchRequest,
    result: ValidatedMatchResult,
    limits: ProjectedQueryMaterializationLimits,
    cancellation: &AnswerCancellation,
) -> Result<ProjectedQueryResult, SdkExecutionDiagnostic> {
    materialize_projected_query_result_with_budget(
        installed,
        registry,
        origin,
        request,
        result,
        limits,
        cancellation,
        None,
    )
}

/// Consume and atomically materialize one fresh request/result pair using the
/// same absolute deadline and cancellation owner as terminal execution.
#[doc(hidden)]
#[expect(
    clippy::too_many_arguments,
    reason = "the materializer boundary keeps every invocation proof and budget explicit"
)]
pub fn materialize_projected_query_result_with_budget(
    installed: &InstalledRuntimeProjection,
    registry: &DescriptorRegistry,
    origin: ProjectedQueryOrigin,
    request: ValidatedMatchRequest,
    result: ValidatedMatchResult,
    limits: ProjectedQueryMaterializationLimits,
    cancellation: &AnswerCancellation,
    deadline: Option<QueryExecutionDeadline>,
) -> Result<ProjectedQueryResult, SdkExecutionDiagnostic> {
    let shape_id = request.shape_id().clone();
    let (value, measure) = materialize_projected_query_value_with_budget(
        installed,
        registry,
        origin,
        &request,
        &result,
        limits,
        cancellation,
        deadline,
    )?;
    Ok(ProjectedQueryResult {
        request,
        shape_id,
        value,
        measure,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "the materializer boundary keeps every invocation proof and budget explicit"
)]
fn materialize_projected_query_value_with_budget(
    installed: &InstalledRuntimeProjection,
    registry: &DescriptorRegistry,
    origin: ProjectedQueryOrigin,
    request: &ValidatedMatchRequest,
    result: &ValidatedMatchResult,
    limits: ProjectedQueryMaterializationLimits,
    cancellation: &AnswerCancellation,
    deadline: Option<QueryExecutionDeadline>,
) -> Result<(ProjectedQueryValue, ProjectedQueryResourceMeasure), SdkExecutionDiagnostic> {
    check_materialization_budget(cancellation, deadline)?;
    let canonical = result
        .for_request(request)
        .map_err(|error| super::execution_diagnostic::lower_match_error(&error))?;
    check_materialization_budget(cancellation, deadline)?;
    let mut materializer =
        Materializer::new(installed, registry, origin, limits, cancellation, deadline);
    let value = materializer.materialize_result(request, canonical)?;
    materializer.checkpoint()?;
    Ok((value, materializer.measure))
}

struct Materializer<'a> {
    installed: &'a InstalledRuntimeProjection,
    registry: &'a DescriptorRegistry,
    origin: ProjectedQueryOrigin,
    limits: ProjectedQueryMaterializationLimits,
    cancellation: &'a AnswerCancellation,
    deadline: Option<QueryExecutionDeadline>,
    measure: ProjectedQueryResourceMeasure,
}

impl<'a> Materializer<'a> {
    fn new(
        installed: &'a InstalledRuntimeProjection,
        registry: &'a DescriptorRegistry,
        origin: ProjectedQueryOrigin,
        limits: ProjectedQueryMaterializationLimits,
        cancellation: &'a AnswerCancellation,
        deadline: Option<QueryExecutionDeadline>,
    ) -> Self {
        Self {
            installed,
            registry,
            origin,
            limits,
            cancellation,
            deadline,
            measure: ProjectedQueryResourceMeasure {
                rows: 0,
                cells: 0,
                things: 0,
                attribute_values: 0,
                bytes: 0,
            },
        }
    }

    fn checkpoint(&self) -> Result<(), SdkExecutionDiagnostic> {
        check_materialization_budget(self.cancellation, self.deadline)
    }

    fn materialize_result(
        &mut self,
        request: &ValidatedMatchRequest,
        result: &MatchResult,
    ) -> Result<ProjectedQueryValue, SdkExecutionDiagnostic> {
        self.checkpoint()?;
        match result {
            MatchResult::Rows { rows } => Ok(ProjectedQueryValue::Rows {
                rows: self.rows(request, rows)?,
            }),
            MatchResult::Page {
                root,
                entries,
                window,
                total,
            } => Ok(ProjectedQueryValue::Page {
                root: *root,
                entries: {
                    self.add_cells(entries.len())?;
                    self.rows(request, entries)?
                },
                window: *window,
                total: *total,
            }),
            MatchResult::Count { root, value } => Ok(ProjectedQueryValue::Count {
                root: *root,
                value: *value,
            }),
            MatchResult::Exists { root, value } => Ok(ProjectedQueryValue::Exists {
                root: *root,
                value: *value,
            }),
            MatchResult::Reduction { root, group, rows } => Ok(ProjectedQueryValue::Reduction {
                root: *root,
                group: *group,
                rows: self.reduction_rows(request, rows, None)?,
            }),
            MatchResult::FieldReduction { root, group, rows } => {
                Ok(ProjectedQueryValue::FieldReduction {
                    root: *root,
                    group: group.clone(),
                    rows: self.reduction_rows(request, rows, Some(std::slice::from_ref(group)))?,
                })
            }
            MatchResult::FieldTupleReduction { root, groups, rows } => {
                Ok(ProjectedQueryValue::FieldTupleReduction {
                    root: *root,
                    groups: groups.clone(),
                    rows: self.reduction_rows(request, rows, Some(groups))?,
                })
            }
        }
    }

    fn rows(
        &mut self,
        request: &ValidatedMatchRequest,
        rows: &[MatchRow],
    ) -> Result<Vec<ProjectedQueryRow>, SdkExecutionDiagnostic> {
        self.checkpoint()?;
        let output = request
            .request()
            .operation
            .output()
            .ok_or_else(result_integrity)?;
        self.add_rows(rows.len())?;
        let slots = selected_slot_contracts(request, output)?;
        let mut projected = Vec::new();
        projected
            .try_reserve(rows.len())
            .map_err(|_| materialization_allocation())?;
        for row in rows {
            self.checkpoint()?;
            if row.slots().len() != slots.len() {
                return Err(result_integrity());
            }
            let mut values = Vec::new();
            values
                .try_reserve(slots.len())
                .map_err(|_| materialization_allocation())?;
            for (value, slot) in row.slots().iter().zip(&slots) {
                self.checkpoint()?;
                let value = match (value, slot.is_collection) {
                    (SlotValue::One(thing), false) => {
                        ensure_slot_thing(self.registry, slot, thing)?;
                        ProjectedQuerySlotValue::One(self.thing(thing)?)
                    }
                    (SlotValue::Many(things), true) => {
                        self.add_cells(things.len())?;
                        let mut projected_things = Vec::new();
                        projected_things
                            .try_reserve(things.len())
                            .map_err(|_| materialization_allocation())?;
                        for thing in things {
                            self.checkpoint()?;
                            ensure_slot_thing(self.registry, slot, thing)?;
                            projected_things.push(self.thing(thing)?);
                        }
                        ProjectedQuerySlotValue::Many(projected_things)
                    }
                    (SlotValue::One(_), true) | (SlotValue::Many(_), false) => {
                        return Err(result_integrity());
                    }
                };
                values.push(ProjectedQuerySlot {
                    binding: slot.binding,
                    declared_type: slot.declared_type.clone(),
                    match_mode: slot.match_mode,
                    name: slot.name.clone(),
                    value,
                });
            }
            projected.push(ProjectedQueryRow { slots: values });
        }
        Ok(projected)
    }

    fn reduction_rows(
        &mut self,
        request: &ValidatedMatchRequest,
        rows: &[ReductionRow],
        field_groups: Option<&[BoundFieldId]>,
    ) -> Result<Vec<ProjectedReductionRow>, SdkExecutionDiagnostic> {
        self.checkpoint()?;
        self.add_rows(rows.len())?;
        let binding_group = match &request.request().operation {
            MatchOperation::ReduceBy { group, .. } => *group,
            _ => None,
        };
        let mut output = Vec::new();
        output
            .try_reserve(rows.len())
            .map_err(|_| materialization_allocation())?;
        for row in rows {
            self.checkpoint()?;
            let group = if let Some(thing) = row.group() {
                let Some(group_binding) = binding_group else {
                    return Err(result_integrity());
                };
                let binding = request
                    .request()
                    .plan
                    .bindings
                    .iter()
                    .find(|binding| binding.id == group_binding)
                    .ok_or_else(result_integrity)?;
                ensure_declared_thing(binding.descriptor.as_str(), thing)?;
                Some(ProjectedReductionGroup::Thing(self.thing(thing)?))
            } else if let Some(value) = row.field_group() {
                let [group] = field_groups.unwrap_or_default() else {
                    return Err(result_integrity());
                };
                Some(ProjectedReductionGroup::Field(
                    self.group_value(group, value)?,
                ))
            } else if let Some(values) = row.field_groups() {
                let groups = field_groups.unwrap_or_default();
                if groups.len() != values.len() {
                    return Err(result_integrity());
                }
                let mut projected = Vec::new();
                projected
                    .try_reserve(values.len())
                    .map_err(|_| materialization_allocation())?;
                for (group, value) in groups.iter().zip(values) {
                    self.checkpoint()?;
                    projected.push(self.group_value(group, value)?);
                }
                Some(ProjectedReductionGroup::Fields(projected))
            } else {
                None
            };
            let group_cells = match &group {
                None => 0,
                Some(ProjectedReductionGroup::Thing(_) | ProjectedReductionGroup::Field(_)) => 1,
                Some(ProjectedReductionGroup::Fields(values)) => values.len(),
            };
            self.add_cells(row.values().len().saturating_add(group_cells))?;
            let mut values = Vec::new();
            values
                .try_reserve(row.values().len())
                .map_err(|_| materialization_allocation())?;
            for value in row.values() {
                self.checkpoint()?;
                values.push(match value {
                    ReducedValue::Count(value) => ProjectedReducedValue::Count(*value),
                    ReducedValue::Long(value) => ProjectedReducedValue::Long(*value),
                    ReducedValue::Double(value) => ProjectedReducedValue::Double(
                        value
                            .map(CanonicalDouble::new)
                            .transpose()
                            .map_err(|_| result_integrity())?,
                    ),
                });
            }
            output.push(ProjectedReductionRow { group, values });
        }
        Ok(output)
    }

    fn group_value(
        &mut self,
        field: &BoundFieldId,
        value: &AttributeValue,
    ) -> Result<Arc<ProjectedAttributeValue>, SdkExecutionDiagnostic> {
        self.checkpoint()?;
        let registered = self
            .registry
            .field_id(&field.field.owner, &field.field.name)
            .ok_or_else(result_integrity)?;
        let provider_attribute = self
            .registry
            .provider_attribute_name(&registered)
            .ok_or_else(result_integrity)?;
        let attribute_type =
            TypeId::new(TypeKind::Attribute, provider_attribute).map_err(|_| result_integrity())?;
        let projected = ProjectedAttributeValue::try_new(
            self.installed,
            attribute_type,
            canonical_attribute_value(value).map_err(|_| result_integrity())?,
        )?;
        self.add_attribute_value(&projected)?;
        Ok(Arc::new(projected))
    }

    fn thing(
        &mut self,
        thing: &HydratedThing,
    ) -> Result<Arc<ProjectedThing>, SdkExecutionDiagnostic> {
        self.checkpoint()?;
        let type_id = projected_type_id(self.registry, thing.concrete_descriptor())?;
        let fields = self.attributes(&type_id, thing.attributes())?;
        let roles = self.roles(&type_id, thing)?;
        self.checkpoint()?;
        let projected = match &self.origin.database_identity {
            Some(identity) => ProjectedThing::try_new_for_database(
                self.installed,
                type_id,
                thing.concept_id().as_str().to_owned(),
                fields,
                roles,
                identity.clone(),
            ),
            None => ProjectedThing::try_new(
                self.installed,
                type_id,
                thing.concept_id().as_str().to_owned(),
                fields,
                roles,
            ),
        }?;
        let projected_things = projected
            .roles()
            .values()
            .try_fold(1_usize, |count, players| count.checked_add(players.len()))
            .unwrap_or(usize::MAX);
        let projected_attributes = projected
            .fields()
            .values()
            .map(Vec::len)
            .chain(
                projected
                    .roles()
                    .values()
                    .flatten()
                    .map(|player| player.keys().len()),
            )
            .try_fold(0_usize, usize::checked_add)
            .unwrap_or(usize::MAX);
        self.add_thing(&projected, projected_things, projected_attributes)?;
        Ok(Arc::new(projected))
    }

    fn attributes(
        &mut self,
        owner: &TypeId,
        attributes: &[HydratedAttribute],
    ) -> Result<Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>, SdkExecutionDiagnostic> {
        self.checkpoint()?;
        let model = self
            .installed
            .projection()
            .models()
            .get(owner)
            .ok_or_else(result_integrity)?;
        let mut by_provider_label = BTreeMap::<&str, Vec<&OwnsFactId>>::new();
        for field in model.complete_read().fields() {
            self.checkpoint()?;
            let token = model
                .query_tokens()
                .fields()
                .get(field.token())
                .ok_or_else(result_integrity)?;
            by_provider_label
                .entry(token.id().attribute().label().as_str())
                .or_default()
                .push(field.token());
        }
        let mut evidence = BTreeMap::<OwnsFactId, Vec<&AttributeValue>>::new();
        for attribute in attributes {
            self.checkpoint()?;
            let provider_label = self
                .registry
                .provider_attribute_name(attribute.field())
                .ok_or_else(result_integrity)?;
            let candidates = by_provider_label
                .get(provider_label.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let [field_id] = candidates else {
                return Err(result_integrity());
            };
            evidence
                .entry((*field_id).clone())
                .or_default()
                .extend(attribute.values());
        }

        let mut output = Vec::new();
        output
            .try_reserve(model.complete_read().fields().len())
            .map_err(|_| materialization_allocation())?;
        for field in model.complete_read().fields() {
            self.checkpoint()?;
            let field_id = field.token().clone();
            let attribute_type =
                TypeId::new(TypeKind::Attribute, field_id.attribute().label().as_str())
                    .map_err(|_| result_integrity())?;
            let hydrated = evidence.remove(&field_id).unwrap_or_default();
            let mut values = Vec::new();
            values
                .try_reserve(hydrated.len())
                .map_err(|_| materialization_allocation())?;
            for value in hydrated {
                self.checkpoint()?;
                let value = ProjectedAttributeValue::try_new(
                    self.installed,
                    attribute_type.clone(),
                    canonical_attribute_value(value).map_err(|_| result_integrity())?,
                )?;
                values.push(value);
            }
            output.push((field_id, values));
        }
        if !evidence.is_empty() {
            return Err(result_integrity());
        }
        Ok(output)
    }

    fn roles(
        &mut self,
        type_id: &TypeId,
        thing: &HydratedThing,
    ) -> Result<Vec<(ProjectedRoleId, Vec<ProjectedRolePlayer>)>, SdkExecutionDiagnostic> {
        self.checkpoint()?;
        let model = self
            .installed
            .projection()
            .models()
            .get(type_id)
            .ok_or_else(result_integrity)?;
        let mut by_provider_label = BTreeMap::<&str, Vec<&ProjectedRoleId>>::new();
        for role_id in model.complete_read().roles().keys() {
            self.checkpoint()?;
            let token = model
                .query_tokens()
                .roles()
                .get(role_id)
                .ok_or_else(result_integrity)?;
            by_provider_label
                .entry(token.role().label().as_str())
                .or_default()
                .push(role_id);
        }
        let mut evidence = BTreeMap::<ProjectedRoleId, Vec<&HydratedRolePlayer>>::new();
        for role in thing.roles() {
            self.checkpoint()?;
            let candidates = by_provider_label
                .get(role.role().name.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let [role_id] = candidates else {
                return Err(result_integrity());
            };
            evidence
                .entry((*role_id).clone())
                .or_default()
                .extend(role.players());
        }

        let mut output = Vec::new();
        output
            .try_reserve(model.complete_read().roles().len())
            .map_err(|_| materialization_allocation())?;
        for role_id in model.complete_read().roles().keys() {
            self.checkpoint()?;
            let hydrated = evidence.remove(role_id).unwrap_or_default();
            let mut players = Vec::new();
            players
                .try_reserve(hydrated.len())
                .map_err(|_| materialization_allocation())?;
            for player in hydrated {
                self.checkpoint()?;
                players.push(self.role_player(player)?);
            }
            output.push((role_id.clone(), players));
        }
        if !evidence.is_empty() {
            return Err(result_integrity());
        }
        Ok(output)
    }

    fn role_player(
        &mut self,
        player: &HydratedRolePlayer,
    ) -> Result<ProjectedRolePlayer, SdkExecutionDiagnostic> {
        self.checkpoint()?;
        let type_id = projected_type_id(self.registry, player.concrete_descriptor())?;
        let fields = self.attributes(&type_id, player.attributes())?;
        self.checkpoint()?;
        let keys = fields
            .into_iter()
            .flat_map(|(field, values)| values.into_iter().map(move |value| (field.clone(), value)))
            .filter(|(field, _)| {
                self.installed
                    .projection()
                    .models()
                    .get(&type_id)
                    .is_some_and(|model| model.reference_read().key_fields().contains(field))
            })
            .collect::<Vec<_>>();
        let reference = match &self.origin.database_identity {
            Some(identity) => ProjectedReference::try_new_for_database(
                self.installed,
                type_id,
                Some(player.concept_id().as_str().to_owned()),
                keys,
                identity.clone(),
            ),
            None => ProjectedReference::try_new(
                self.installed,
                type_id,
                Some(player.concept_id().as_str().to_owned()),
                keys,
            ),
        }?;
        ProjectedRolePlayer::try_new(self.installed, reference)
    }

    fn add_rows(&mut self, count: usize) -> Result<(), SdkExecutionDiagnostic> {
        add_limit(
            &mut self.measure.rows,
            count,
            self.limits.rows,
            "projected_query_row_limit",
        )
    }

    fn add_cells(&mut self, count: usize) -> Result<(), SdkExecutionDiagnostic> {
        add_limit(
            &mut self.measure.cells,
            count,
            self.limits.cells,
            "projected_query_cell_limit",
        )
    }

    fn add_attribute_value(
        &mut self,
        value: &ProjectedAttributeValue,
    ) -> Result<(), SdkExecutionDiagnostic> {
        add_limit(
            &mut self.measure.attribute_values,
            1,
            self.limits.attribute_values,
            "projected_query_attribute_limit",
        )?;
        self.add_bytes(value.resource_measure().bytes())
    }

    fn add_thing(
        &mut self,
        thing: &ProjectedThing,
        things: usize,
        attribute_values: usize,
    ) -> Result<(), SdkExecutionDiagnostic> {
        add_limit(
            &mut self.measure.things,
            things,
            self.limits.things,
            "projected_query_thing_limit",
        )?;
        add_limit(
            &mut self.measure.attribute_values,
            attribute_values,
            self.limits.attribute_values,
            "projected_query_attribute_limit",
        )?;
        self.add_bytes(thing.resource_measure().bytes())
    }

    fn add_bytes(&mut self, count: usize) -> Result<(), SdkExecutionDiagnostic> {
        add_limit(
            &mut self.measure.bytes,
            count,
            self.limits.bytes,
            "projected_query_byte_limit",
        )
    }
}

struct SelectedSlotContract {
    binding: BindingId,
    declared_descriptor: DescriptorId,
    declared_type: TypeId,
    match_mode: MatchMode,
    is_collection: bool,
    name: Option<String>,
}

fn selected_slot_contracts(
    request: &ValidatedMatchRequest,
    output: &FetchShape,
) -> Result<Vec<SelectedSlotContract>, SdkExecutionDiagnostic> {
    let raw: Vec<(&FetchSlot, Option<&str>)> = match output {
        FetchShape::Positional { slots } => slots.iter().map(|slot| (slot, None)).collect(),
        FetchShape::Named { slots } => slots
            .iter()
            .map(|slot| (&slot.slot, Some(slot.name.as_str())))
            .collect(),
    };
    raw.into_iter()
        .map(|(slot, name)| {
            let binding_id = slot.binding();
            let binding = request
                .request()
                .plan
                .bindings
                .iter()
                .find(|binding| binding.id == binding_id)
                .ok_or_else(result_integrity)?;
            Ok(SelectedSlotContract {
                binding: binding_id,
                declared_descriptor: binding.descriptor.clone(),
                declared_type: descriptor_type_id(&binding.descriptor)?,
                match_mode: binding.match_mode,
                is_collection: slot.is_collection(),
                name: name.map(str::to_owned),
            })
        })
        .collect()
}

fn ensure_slot_thing(
    registry: &DescriptorRegistry,
    slot: &SelectedSlotContract,
    thing: &HydratedThing,
) -> Result<(), SdkExecutionDiagnostic> {
    ensure_declared_thing(slot.declared_descriptor.as_str(), thing)?;
    let concrete = thing.concrete_descriptor();
    let valid = match slot.match_mode {
        MatchMode::Exact => concrete == &slot.declared_descriptor,
        MatchMode::Subtypes => registry.is_same_or_subtype(concrete, &slot.declared_descriptor),
    };
    if valid {
        Ok(())
    } else {
        Err(result_integrity())
    }
}

fn ensure_declared_thing(
    descriptor: &str,
    thing: &HydratedThing,
) -> Result<(), SdkExecutionDiagnostic> {
    if thing.declared_descriptor().as_str() == descriptor {
        Ok(())
    } else {
        Err(result_integrity())
    }
}

fn projected_type_id(
    registry: &DescriptorRegistry,
    descriptor: &DescriptorId,
) -> Result<TypeId, SdkExecutionDiagnostic> {
    let label = registry
        .descriptor_type_name(descriptor)
        .ok_or_else(result_integrity)?;
    let kind = descriptor.as_str().split_once(':').map(|value| value.0);
    TypeId::new(
        match kind {
            Some("entity") => TypeKind::Entity,
            Some("relation") => TypeKind::Relation,
            _ => return Err(result_integrity()),
        },
        label,
    )
    .map_err(|_| result_integrity())
}

fn descriptor_type_id(descriptor: &DescriptorId) -> Result<TypeId, SdkExecutionDiagnostic> {
    let Some((kind, label)) = descriptor.as_str().split_once(':') else {
        return Err(result_integrity());
    };
    TypeId::new(
        match kind {
            "entity" => TypeKind::Entity,
            "relation" => TypeKind::Relation,
            _ => return Err(result_integrity()),
        },
        label,
    )
    .map_err(|_| result_integrity())
}

fn canonical_attribute_value(value: &AttributeValue) -> Result<CanonicalValue, ()> {
    crate::runtime_projection::canonical_attribute_value(value).map_err(|_| ())
}

fn add_limit(
    current: &mut usize,
    count: usize,
    maximum: usize,
    code: &'static str,
) -> Result<(), SdkExecutionDiagnostic> {
    let actual = current.checked_add(count).unwrap_or(usize::MAX);
    if actual > maximum {
        return Err(SdkExecutionDiagnostic::resource_limit(
            sdk_code(code),
            sdk_message("The projected query result exceeds its materialization ceiling"),
        )
        .try_at(SdkDiagnosticPathSegment::Query(
            SdkQueryDiagnosticPathKind::Result,
        ))
        .expect("the static projected query path is bounded")
        .try_with_detail(
            sdk_name("actual"),
            SdkDiagnosticDetailValue::Count(u64::try_from(actual).unwrap_or(u64::MAX)),
        )
        .expect("static projected query detail is bounded")
        .try_with_detail(
            sdk_name("maximum"),
            SdkDiagnosticDetailValue::Count(u64::try_from(maximum).unwrap_or(u64::MAX)),
        )
        .expect("static projected query detail is bounded"));
    }
    *current = actual;
    Ok(())
}

fn result_integrity() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::integrity(
        sdk_code("projected_query_result_mismatch"),
        sdk_message("Typed query evidence cannot be projected as the validated result shape"),
    )
    .try_at(SdkDiagnosticPathSegment::Query(
        SdkQueryDiagnosticPathKind::Result,
    ))
    .expect("static projected query path is bounded")
}

fn materialization_allocation() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::resource_limit(
        sdk_code("projected_query_allocation_exhausted"),
        sdk_message("The projected query result could not reserve bounded materialization storage"),
    )
}

fn check_materialization_budget(
    cancellation: &AnswerCancellation,
    deadline: Option<QueryExecutionDeadline>,
) -> Result<(), SdkExecutionDiagnostic> {
    if let Some(deadline) = deadline {
        return deadline.check(cancellation);
    }
    if cancellation.is_cancelled() {
        return Err(SdkExecutionDiagnostic::query_failure(
            SdkQueryDiagnosticCategory::Cancelled,
            sdk_code("provider_cancelled"),
        )
        .try_at(SdkDiagnosticPathSegment::Query(
            SdkQueryDiagnosticPathKind::ProviderEvidence,
        ))
        .expect("static projected query cancellation path is bounded"));
    }
    Ok(())
}

fn sdk_code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static projected query code is canonical")
}

fn sdk_message(value: &'static str) -> type_bridge_contract::sdk_diagnostic::SdkDiagnosticMessage {
    type_bridge_contract::sdk_diagnostic::SdkDiagnosticMessage::new(value)
        .expect("static projected query message is valid")
}

fn sdk_name(value: &'static str) -> SdkDiagnosticName {
    SdkDiagnosticName::new(value).expect("static projected query name is canonical")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{RoleId as ProjectedRoleId, TypeId, TypeKind};
    use type_bridge_contract::projection::{
        BindingTarget, CSymbolPrefix, ProjectionConfig, ProjectionHandler,
    };
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::sdk_diagnostic::{
        SdkDiagnosticCategory, SdkDiagnosticDetailValue, SdkDiagnosticName,
    };
    use type_bridge_contract::value::CanonicalValue;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

    use super::*;
    use crate::OrmError;
    use crate::ProjectedCrudExecutor;
    use crate::match_request::result::{
        BoundConceptEvidence, ConceptId, HydratedRole, ProviderResultEvidence,
        ProviderSolutionEvidence, ReductionRow,
    };
    use crate::match_request::result_validation::validate_provider_result;
    use crate::match_request::{
        BindingPair, ComparisonOp, MatchBinding, MatchPlan, MatchRequest, ReduceTerm, Reduction,
        RowCardinality, SessionHandle, ThingKind, validate_match_request,
    };
    use crate::projected_model::ProjectedCreate;
    use crate::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};

    const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  name: { value: string }
  score: { value: integer }
entities:
  person:
    abstract: true
    owns:
      identifier: { key: true }
  employee:
    sub: person
    owns:
      name: { card: { min: 0, max: 1 } }
      score: { card: { min: 0, max: 1 } }
relations:
  membership:
    relates:
      member: { card: { min: 0 } }
plays:
  person:
    membership: [member]
"#;

    fn fixture() -> (InstalledRuntimeProjection, DescriptorRegistry) {
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new("projected-query.yaml").unwrap(), SCHEMA)])
                .unwrap();
        let resolved = resolve(
            &normalize_documents(&documents).unwrap(),
            &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        )
        .unwrap();
        let projection = project(
            &resolved,
            BindingTarget::C,
            &ProjectionConfig::c(CSymbolPrefix::new("pq").unwrap()),
            &[ProjectionHandler::c_v2()],
            &[],
        )
        .unwrap();
        let installed = InstalledRuntimeProjection::try_new(projection).unwrap();
        let registry = installed.match_registry().unwrap();
        (installed, registry)
    }

    fn descriptor(registry: &DescriptorRegistry, name: &str) -> DescriptorId {
        registry.descriptor_id(name).unwrap()
    }

    fn attribute(
        registry: &DescriptorRegistry,
        owner: &str,
        name: &str,
        value: AttributeValue,
    ) -> HydratedAttribute {
        let owner = descriptor(registry, owner);
        HydratedAttribute::new(registry.field_id(&owner, name).unwrap(), vec![value])
    }

    fn employee(
        registry: &DescriptorRegistry,
        iid: &str,
        declared: &str,
        identifier: &str,
        score: i64,
    ) -> HydratedThing {
        HydratedThing::new(
            ConceptId::new(iid),
            descriptor(registry, declared),
            descriptor(registry, "employee"),
            ThingKind::Entity,
            vec![
                attribute(
                    registry,
                    "employee",
                    "identifier",
                    AttributeValue::String(identifier.to_owned()),
                ),
                attribute(
                    registry,
                    "employee",
                    "name",
                    AttributeValue::String(format!("name-{identifier}")),
                ),
                attribute(registry, "employee", "score", AttributeValue::Long(score)),
            ],
            Vec::new(),
        )
    }

    fn minimal_employee(
        registry: &DescriptorRegistry,
        iid: &str,
        declared: &str,
        identifier: &str,
    ) -> HydratedThing {
        HydratedThing::new(
            ConceptId::new(iid),
            descriptor(registry, declared),
            descriptor(registry, "employee"),
            ThingKind::Entity,
            vec![attribute(
                registry,
                "employee",
                "identifier",
                AttributeValue::String(identifier.to_owned()),
            )],
            Vec::new(),
        )
    }

    fn relation(registry: &DescriptorRegistry, iid: &str, player: HydratedThing) -> HydratedThing {
        let relation = descriptor(registry, "membership");
        HydratedThing::new(
            ConceptId::new(iid),
            relation.clone(),
            relation.clone(),
            ThingKind::Relation,
            Vec::new(),
            vec![HydratedRole::new(
                crate::match_request::RoleId::new(relation, "member"),
                vec![HydratedRolePlayer::new(
                    player.concept_id().clone(),
                    descriptor(registry, "person"),
                    player.concrete_descriptor().clone(),
                    player.kind(),
                    player.attributes().to_vec(),
                )],
            )],
        )
    }

    fn binding(
        registry: &DescriptorRegistry,
        id: u16,
        name: &str,
        kind: ThingKind,
        mode: MatchMode,
    ) -> MatchBinding {
        MatchBinding {
            id: BindingId::new(id),
            descriptor: descriptor(registry, name),
            thing_kind: kind,
            match_mode: mode,
        }
    }

    fn solution(values: Vec<(u16, HydratedThing)>) -> ProviderSolutionEvidence {
        ProviderSolutionEvidence::new(
            values
                .into_iter()
                .map(|(binding, thing)| BoundConceptEvidence::new(BindingId::new(binding), thing))
                .collect(),
            Vec::new(),
        )
    }

    fn validate_rows(
        registry: &DescriptorRegistry,
        request: MatchRequest,
        solutions: Vec<ProviderSolutionEvidence>,
    ) -> (ValidatedMatchRequest, ValidatedMatchResult) {
        let request = validate_match_request(registry, request).unwrap();
        let evidence = ProviderResultEvidence::rows(
            request.request_token(),
            request.shape_id().clone(),
            solutions,
        );
        let result = validate_provider_result(registry, &request, evidence).unwrap();
        (request, result)
    }

    fn one_rows_request(registry: &DescriptorRegistry) -> MatchRequest {
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![binding(
                    registry,
                    0,
                    "person",
                    ThingKind::Entity,
                    MatchMode::Subtypes,
                )],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: BindingId::new(0),
                    }],
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 10,
                },
                cardinality: RowCardinality::BoundedMany,
            },
        )
    }

    fn limits_for(measure: ProjectedQueryResourceMeasure) -> ProjectedQueryMaterializationLimits {
        ProjectedQueryMaterializationLimits::tightened(
            measure.rows(),
            measure.cells(),
            measure.things(),
            measure.attribute_values(),
            measure.bytes(),
        )
    }

    fn detail_count(diagnostic: &SdkExecutionDiagnostic, name: &str) -> u64 {
        match diagnostic
            .details()
            .get(&SdkDiagnosticName::new(name).unwrap())
        {
            Some(SdkDiagnosticDetailValue::Count(value)) => *value,
            other => panic!("expected count detail {name}, got {other:?}"),
        }
    }

    fn assert_limit(
        installed: &InstalledRuntimeProjection,
        registry: &DescriptorRegistry,
        request: MatchRequest,
        solutions: Vec<ProviderSolutionEvidence>,
        limits: ProjectedQueryMaterializationLimits,
        code: &str,
        actual: u64,
    ) {
        let (request, result) = validate_rows(registry, request, solutions);
        let error = materialize_projected_query_result(
            installed,
            registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            result,
            limits,
        )
        .unwrap_err();
        assert_eq!(error.category(), SdkDiagnosticCategory::ResourceLimit);
        assert_eq!(error.code().as_str(), code);
        assert_eq!(
            error.path(),
            &[SdkDiagnosticPathSegment::Query(
                SdkQueryDiagnosticPathKind::Result
            )]
        );
        assert_eq!(detail_count(&error, "actual"), actual);
    }

    #[test]
    fn projected_query_invocation_gate_rejects_same_shape_other_token_before_materialization() {
        let (installed, registry) = fixture();
        let request_a = validate_match_request(&registry, one_rows_request(&registry)).unwrap();
        let result_for_a = || {
            validate_provider_result(
                &registry,
                &request_a,
                ProviderResultEvidence::rows(
                    request_a.request_token(),
                    request_a.shape_id().clone(),
                    Vec::new(),
                ),
            )
            .unwrap()
        };
        let request_b = validate_match_request(&registry, one_rows_request(&registry)).unwrap();
        assert_eq!(request_a.shape_id(), request_b.shape_id());

        let wrong_borrowed_result = result_for_a();
        let borrowed_error = ProjectedQueryOrigin::remote_unbound()
            .materialize_borrowed_with_budget(
                &installed,
                &registry,
                &request_b,
                &wrong_borrowed_result,
                ProjectedQueryMaterializationLimits::default(),
                &AnswerCancellation::default(),
                None,
            )
            .unwrap_err();
        assert_eq!(borrowed_error.code().as_str(), "request_token_mismatch");
        let borrowed_result = result_for_a();
        let (borrowed, measure) = ProjectedQueryOrigin::remote_unbound()
            .materialize_borrowed_with_budget(
                &installed,
                &registry,
                &request_a,
                &borrowed_result,
                ProjectedQueryMaterializationLimits::default(),
                &AnswerCancellation::default(),
                None,
            )
            .unwrap();
        assert!(matches!(borrowed, ProjectedQueryValue::Rows { rows } if rows.is_empty()));
        assert_eq!(
            (
                measure.rows(),
                measure.cells(),
                measure.things(),
                measure.bytes()
            ),
            (0, 0, 0, 0)
        );
        assert!(borrowed_result.for_request(&request_a).is_ok());

        let consuming_result = result_for_a();
        let error = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request_b,
            consuming_result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "request_token_mismatch");
        assert_eq!(
            error.path(),
            &[SdkDiagnosticPathSegment::Query(
                SdkQueryDiagnosticPathKind::Result
            )]
        );
    }

    #[test]
    fn projected_query_shape_and_slot_cardinality_are_rechecked_defensively() {
        let (installed, registry) = fixture();
        let request = validate_match_request(&registry, one_rows_request(&registry)).unwrap();
        let wrong_shape = ValidatedMatchResult::new_for_test(
            request.request_token(),
            ResultShapeId::new("different-shape"),
            MatchResult::Rows { rows: Vec::new() },
        );
        let error = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            wrong_shape,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "result_shape_mismatch");

        let request = validate_match_request(&registry, one_rows_request(&registry)).unwrap();
        let wrong_cardinality = ValidatedMatchResult::new_for_test(
            request.request_token(),
            request.shape_id().clone(),
            MatchResult::Rows {
                rows: vec![MatchRow::new(vec![SlotValue::Many(vec![employee(
                    &registry, "0x1", "person", "ada", 7,
                )])])],
            },
        );
        let error = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            wrong_cardinality,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "projected_query_result_mismatch");
        assert_eq!(
            error.path(),
            &[SdkDiagnosticPathSegment::Query(
                SdkQueryDiagnosticPathKind::Result
            )]
        );
    }

    #[test]
    fn subtype_named_rows_keep_declared_slot_metadata_and_concrete_field_tokens() {
        let (installed, registry) = fixture();
        let session = SessionHandle::new(Arc::new(installed.match_registry().unwrap()));
        let person = session.subtypes("person").unwrap();
        let shape = session
            .named([("people", person.one())])
            .expect("canonical named shape");
        let query = session.query(shape).unwrap();
        let request = query
            .validate_fetch_rows(
                &[],
                Window {
                    offset: 0,
                    limit: 1,
                },
                RowCardinality::ExactlyOne,
            )
            .unwrap();
        let result = validate_provider_result(
            &registry,
            &request,
            ProviderResultEvidence::rows(
                request.request_token(),
                request.shape_id().clone(),
                vec![solution(vec![(
                    0,
                    employee(&registry, "0x1", "person", "ada", 7),
                )])],
            ),
        )
        .unwrap();
        let projected = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        let ProjectedQueryValue::Rows { rows } = projected.value() else {
            panic!("expected rows")
        };
        let [slot] = rows[0].slots() else {
            panic!("expected one slot")
        };
        assert_eq!(slot.binding(), BindingId::new(0));
        assert_eq!(
            slot.declared_type(),
            &TypeId::new(TypeKind::Entity, "person").unwrap()
        );
        assert_eq!(slot.match_mode(), MatchMode::Subtypes);
        assert_eq!(slot.name(), Some("people"));
        let ProjectedQuerySlotValue::One(thing) = slot.value() else {
            panic!("expected singular slot")
        };
        assert_eq!(
            thing.type_id(),
            &TypeId::new(TypeKind::Entity, "employee").unwrap()
        );
        assert!(thing.origin_carrier().is_none());
        assert!(
            thing
                .fields()
                .keys()
                .all(|field| field.owner() == thing.type_id())
        );
        let identifier = thing
            .fields()
            .iter()
            .find(|(field, _)| field.attribute().label().as_str() == "identifier")
            .expect("inherited field survives under concrete owner");
        assert_eq!(identifier.0.owner(), thing.type_id());
    }

    #[test]
    fn complete_read_shape_retains_exact_inherited_tokens_and_empty_optional_members() {
        let (installed, registry) = fixture();
        let employee_type = TypeId::new(TypeKind::Entity, "employee").unwrap();
        let employee_model = &installed.projection().models()[&employee_type];
        let inherited = employee_model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token().attribute().label().as_str() == "identifier")
            .unwrap()
            .token();
        let inherited_token = &employee_model.query_tokens().fields()[inherited];
        assert_eq!(inherited.owner(), &employee_type);
        assert_ne!(inherited_token.declaring_id().owner(), inherited.owner());
        assert_ne!(
            inherited_token.target_name().as_str(),
            inherited.attribute().label().as_str()
        );

        let request = MatchRequest::v1(
            MatchPlan {
                bindings: vec![binding(
                    &registry,
                    0,
                    "employee",
                    ThingKind::Entity,
                    MatchMode::Exact,
                )],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: BindingId::new(0),
                    }],
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::ExactlyOne,
            },
        );
        let (request, result) = validate_rows(
            &registry,
            request,
            vec![solution(vec![(
                0,
                minimal_employee(&registry, "0x1", "employee", "ada"),
            )])],
        );
        let projected = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        let thing = first_thing(&projected);
        assert_eq!(
            thing.fields().len(),
            employee_model.complete_read().fields().len()
        );
        assert_eq!(
            thing.fields().keys().collect::<Vec<_>>(),
            employee_model
                .complete_read()
                .fields()
                .iter()
                .map(|field| field.token())
                .collect::<Vec<_>>()
        );
        assert_eq!(thing.fields()[inherited].len(), 1);
        assert!(
            thing
                .fields()
                .iter()
                .all(|(field, values)| { field == inherited || values.is_empty() })
        );

        let relation_type = TypeId::new(TypeKind::Relation, "membership").unwrap();
        let relation_model = &installed.projection().models()[&relation_type];
        let request = MatchRequest::v1(
            MatchPlan {
                bindings: vec![binding(
                    &registry,
                    0,
                    "membership",
                    ThingKind::Relation,
                    MatchMode::Exact,
                )],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: BindingId::new(0),
                    }],
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::ExactlyOne,
            },
        );
        let empty_relation = HydratedThing::new(
            ConceptId::new("0x10"),
            descriptor(&registry, "membership"),
            descriptor(&registry, "membership"),
            ThingKind::Relation,
            Vec::new(),
            Vec::new(),
        );
        let (request, result) = validate_rows(
            &registry,
            request,
            vec![solution(vec![(0, empty_relation)])],
        );
        let projected = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        let thing = first_thing(&projected);
        assert_eq!(
            thing.roles().len(),
            relation_model.complete_read().roles().len()
        );
        for role in relation_model.complete_read().roles().keys() {
            assert!(thing.roles()[role].is_empty());
        }
    }

    #[test]
    fn page_collection_preserves_named_order_multiplicity_and_resource_counts() {
        let (installed, registry) = fixture();
        let make = || {
            let bindings = vec![
                binding(
                    &registry,
                    0,
                    "person",
                    ThingKind::Entity,
                    MatchMode::Subtypes,
                ),
                binding(
                    &registry,
                    1,
                    "person",
                    ThingKind::Entity,
                    MatchMode::Subtypes,
                ),
            ];
            let output = FetchShape::Named {
                slots: vec![
                    crate::match_request::NamedFetchSlot {
                        name: "root".to_owned(),
                        slot: FetchSlot::One {
                            binding: BindingId::new(0),
                        },
                    },
                    crate::match_request::NamedFetchSlot {
                        name: "members".to_owned(),
                        slot: FetchSlot::Collect {
                            binding: BindingId::new(1),
                            distinct: false,
                            order: Vec::new(),
                        },
                    },
                ],
            };
            let request = validate_match_request(
                &registry,
                MatchRequest::v1(
                    MatchPlan {
                        bindings,
                        predicate: None,
                        allowed_cross_joins: BTreeSet::from([BindingPair::new(
                            BindingId::new(0),
                            BindingId::new(1),
                        )]),
                    },
                    MatchOperation::PageBy {
                        root: BindingId::new(0),
                        output,
                        order: Vec::new(),
                        window: Window {
                            offset: 2,
                            limit: 1,
                        },
                        include_total: true,
                    },
                ),
            )
            .unwrap();
            let root = employee(&registry, "0x1", "person", "root", 1);
            let member = employee(&registry, "0x2", "person", "member", 2);
            let evidence = ProviderResultEvidence::page(
                request.request_token(),
                request.shape_id().clone(),
                BindingId::new(0),
                vec![
                    solution(vec![(0, root.clone()), (1, member.clone())]),
                    solution(vec![(0, root), (1, member)]),
                ],
                Window {
                    offset: 2,
                    limit: 1,
                },
                Some(3),
            );
            let result = validate_provider_result(&registry, &request, evidence).unwrap();
            (request, result)
        };
        let (request, result) = make();
        let projected = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        let ProjectedQueryValue::Page {
            root,
            entries,
            window,
            total,
        } = projected.value()
        else {
            panic!("expected page")
        };
        assert_eq!(
            (*root, *window, *total),
            (
                BindingId::new(0),
                Window {
                    offset: 2,
                    limit: 1
                },
                Some(3)
            )
        );
        assert_eq!(entries[0].slots()[0].name(), Some("root"));
        assert_eq!(entries[0].slots()[1].name(), Some("members"));
        let ProjectedQuerySlotValue::Many(members) = entries[0].slots()[1].value() else {
            panic!("expected collection")
        };
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].iid(), "0x2");
        assert_eq!(members[1].iid(), "0x2");
        assert_eq!(projected.resource_measure().rows(), 1);
        assert_eq!(projected.resource_measure().cells(), 3);
        assert_eq!(projected.resource_measure().things(), 3);
        assert_eq!(projected.resource_measure().attribute_values(), 9);

        let exact = projected.resource_measure();
        let (request, result) = make();
        assert_eq!(
            materialize_projected_query_result(
                &installed,
                &registry,
                ProjectedQueryOrigin::remote_unbound(),
                request,
                result,
                limits_for(exact),
            )
            .expect("the exact page-root plus collected-member budget must pass")
            .resource_measure(),
            exact
        );
        let (request, result) = make();
        let error = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            result,
            ProjectedQueryMaterializationLimits::tightened(
                exact.rows(),
                exact.cells() - 1,
                exact.things(),
                exact.attribute_values(),
                exact.bytes(),
            ),
        )
        .expect_err("one page root plus two collected members require three collection cells");
        assert_eq!(error.code().as_str(), "projected_query_cell_limit");
        assert_eq!(detail_count(&error, "actual"), exact.cells() as u64);
    }

    struct NullBackend;

    impl DriverBackend for NullBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async { Err(OrmError::Connection("unused".to_owned())) })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct OriginBackend;

    impl DriverBackend for OriginBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async { Ok(Box::new(OriginTransaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct OriginTransaction;

    impl TransactionOps for OriginTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { Err(OrmError::QueryExecution("unexpected query".into())) })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn borrowed_read_query_origin_matches_the_direct_database_origin() {
        let database = Database::with_backend(Box::new(OriginBackend), "borrowed-origin");
        let direct = ProjectedQueryOrigin::for_database(&database);
        let read = database.transaction_context(TxType::Read).await.unwrap();
        let borrowed = ProjectedQueryOrigin::for_transaction(&read).unwrap();
        assert_eq!(borrowed.database_identity, direct.database_identity);
        read.close().await.unwrap();

        let write = database.transaction_context(TxType::Write).await.unwrap();
        let diagnostic = ProjectedQueryOrigin::for_transaction(&write).unwrap_err();
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::InvalidInput);
        assert_eq!(
            diagnostic.code().as_str(),
            "query_borrowed_transaction_not_read"
        );
        write.close().await.unwrap();
    }

    #[test]
    fn database_origin_binds_outer_thing_and_nested_role_reference_while_remote_is_unbound() {
        let (installed, registry) = fixture();
        let request_for = || {
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![binding(
                        &registry,
                        0,
                        "membership",
                        ThingKind::Relation,
                        MatchMode::Exact,
                    )],
                    predicate: None,
                    allowed_cross_joins: BTreeSet::new(),
                },
                MatchOperation::FetchRows {
                    output: FetchShape::Positional {
                        slots: vec![FetchSlot::One {
                            binding: BindingId::new(0),
                        }],
                    },
                    order: Vec::new(),
                    window: Window {
                        offset: 0,
                        limit: 1,
                    },
                    cardinality: RowCardinality::ExactlyOne,
                },
            )
        };
        let evidence_for = |request: &ValidatedMatchRequest| {
            ProviderResultEvidence::rows(
                request.request_token(),
                request.shape_id().clone(),
                vec![solution(vec![(
                    0,
                    relation(
                        &registry,
                        "0x10",
                        employee(&registry, "0x1", "person", "ada", 7),
                    ),
                )])],
            )
        };

        let database = Database::with_backend(Box::new(NullBackend), "origin-db");
        let direct_request = validate_match_request(&registry, request_for()).unwrap();
        let direct_result =
            validate_provider_result(&registry, &direct_request, evidence_for(&direct_request))
                .unwrap();
        let direct = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::for_database(&database),
            direct_request,
            direct_result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        let direct_thing = first_thing(&direct);
        assert!(direct_thing.origin_carrier().is_some());
        let role = ProjectedRoleId::new("membership", "member").unwrap();
        assert!(
            direct_thing.roles()[&role][0]
                .reference()
                .origin_carrier()
                .is_some()
        );
        assert_eq!(direct.resource_measure().things(), 2);
        assert_eq!(direct.resource_measure().attribute_values(), 1);

        let remote_request = validate_match_request(&registry, request_for()).unwrap();
        let remote_result =
            validate_provider_result(&registry, &remote_request, evidence_for(&remote_request))
                .unwrap();
        let remote = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            remote_request,
            remote_result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        let remote_thing = first_thing(&remote);
        assert!(remote_thing.origin_carrier().is_none());
        assert!(
            remote_thing.roles()[&role][0]
                .reference()
                .origin_carrier()
                .is_none()
        );
        assert!(
            remote_thing
                .try_to_reference(&installed)
                .unwrap()
                .origin_carrier()
                .is_none()
        );
        let remote_create = ProjectedCreate::try_new(
            &installed,
            remote_thing.type_id().clone(),
            vec![],
            vec![(
                role.clone(),
                vec![remote_thing.roles()[&role][0].reference().clone()],
            )],
        )
        .unwrap();
        ProjectedCrudExecutor::new(&installed)
            .preflight_relation_create_for_database_with_compatibility(&database, &remote_create)
            .expect("an explicitly remote/unbound player remains target-database resolvable");
        let measure = remote.resource_measure();
        assert_eq!(measure.attribute_values(), 1);

        let exact_request = validate_match_request(&registry, request_for()).unwrap();
        let exact_result =
            validate_provider_result(&registry, &exact_request, evidence_for(&exact_request))
                .unwrap();
        assert_eq!(
            materialize_projected_query_result(
                &installed,
                &registry,
                ProjectedQueryOrigin::remote_unbound(),
                exact_request,
                exact_result,
                limits_for(measure),
            )
            .unwrap()
            .resource_measure(),
            measure
        );

        let limited_request = validate_match_request(&registry, request_for()).unwrap();
        let limited_result =
            validate_provider_result(&registry, &limited_request, evidence_for(&limited_request))
                .unwrap();
        let error = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            limited_request,
            limited_result,
            ProjectedQueryMaterializationLimits::tightened(
                measure.rows(),
                measure.cells(),
                measure.things(),
                measure.attribute_values() - 1,
                measure.bytes(),
            ),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "projected_query_attribute_limit");
        assert_eq!(detail_count(&error, "actual"), 1);
    }

    fn first_thing(result: &ProjectedQueryResult) -> &ProjectedThing {
        let ProjectedQueryValue::Rows { rows } = result.value() else {
            panic!("expected rows")
        };
        let ProjectedQuerySlotValue::One(thing) = rows[0].slots()[0].value() else {
            panic!("expected singular slot")
        };
        thing
    }

    #[test]
    fn row_resource_limits_accept_exact_measure_and_reject_every_one_below_boundary() {
        let (installed, registry) = fixture();
        let make = || {
            vec![solution(vec![(
                0,
                employee(&registry, "0x1", "person", "ada", 7),
            )])]
        };
        let (request, result) = validate_rows(&registry, one_rows_request(&registry), make());
        let baseline = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        let measure = baseline.resource_measure();
        assert_eq!(measure.cells(), 0);
        let (request, result) = validate_rows(&registry, one_rows_request(&registry), make());
        assert_eq!(
            materialize_projected_query_result(
                &installed,
                &registry,
                ProjectedQueryOrigin::remote_unbound(),
                request,
                result,
                limits_for(measure),
            )
            .unwrap()
            .resource_measure(),
            measure
        );

        let lower = |value: usize| value.checked_sub(1).expect("fixture has nonzero measure");
        assert_limit(
            &installed,
            &registry,
            one_rows_request(&registry),
            make(),
            ProjectedQueryMaterializationLimits::tightened(
                lower(measure.rows()),
                measure.cells(),
                measure.things(),
                measure.attribute_values(),
                measure.bytes(),
            ),
            "projected_query_row_limit",
            measure.rows() as u64,
        );
        assert_limit(
            &installed,
            &registry,
            one_rows_request(&registry),
            make(),
            ProjectedQueryMaterializationLimits::tightened(
                measure.rows(),
                measure.cells(),
                lower(measure.things()),
                measure.attribute_values(),
                measure.bytes(),
            ),
            "projected_query_thing_limit",
            measure.things() as u64,
        );
        assert_limit(
            &installed,
            &registry,
            one_rows_request(&registry),
            make(),
            ProjectedQueryMaterializationLimits::tightened(
                measure.rows(),
                measure.cells(),
                measure.things(),
                lower(measure.attribute_values()),
                measure.bytes(),
            ),
            "projected_query_attribute_limit",
            measure.attribute_values() as u64,
        );
        assert_limit(
            &installed,
            &registry,
            one_rows_request(&registry),
            make(),
            ProjectedQueryMaterializationLimits::tightened(
                measure.rows(),
                measure.cells(),
                measure.things(),
                measure.attribute_values(),
                lower(measure.bytes()),
            ),
            "projected_query_byte_limit",
            measure.bytes() as u64,
        );
    }

    fn reduction_request(
        registry: &DescriptorRegistry,
        operation: impl FnOnce(BindingId, BoundFieldId) -> MatchOperation,
    ) -> ValidatedMatchRequest {
        let root = BindingId::new(0);
        let owner = descriptor(registry, "employee");
        let score = BoundFieldId::new(root, registry.field_id(&owner, "score").unwrap());
        validate_match_request(
            registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![binding(
                        registry,
                        0,
                        "employee",
                        ThingKind::Entity,
                        MatchMode::Exact,
                    )],
                    predicate: Some(crate::match_request::MatchExpr::FieldValue {
                        field: score.clone(),
                        operator: ComparisonOp::GreaterThanOrEqual,
                        value: AttributeValue::Long(0),
                    }),
                    allowed_cross_joins: BTreeSet::new(),
                },
                operation(root, score),
            ),
        )
        .unwrap()
    }

    #[test]
    fn aggregate_terminals_and_reduction_group_kinds_materialize_without_loss() {
        let (installed, registry) = fixture();
        let root = BindingId::new(0);

        let count_request =
            reduction_request(&registry, |root, _| MatchOperation::CountBy { root });
        let count_result = validate_provider_result(
            &registry,
            &count_request,
            ProviderResultEvidence::count(
                count_request.request_token(),
                count_request.shape_id().clone(),
                root,
                u64::MAX,
            ),
        )
        .unwrap();
        let count = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            count_request,
            count_result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        assert!(matches!(
            count.value(),
            ProjectedQueryValue::Count {
                value: u64::MAX,
                ..
            }
        ));

        let exists_request =
            reduction_request(&registry, |root, _| MatchOperation::ExistsBy { root });
        let exists_result = validate_provider_result(
            &registry,
            &exists_request,
            ProviderResultEvidence::exists(
                exists_request.request_token(),
                exists_request.shape_id().clone(),
                root,
                true,
            ),
        )
        .unwrap();
        let exists = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            exists_request,
            exists_result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        assert!(matches!(
            exists.value(),
            ProjectedQueryValue::Exists { value: true, .. }
        ));

        let ungrouped_request =
            reduction_request(&registry, |root, score| MatchOperation::ReduceBy {
                root,
                group: None,
                reducers: vec![
                    ReduceTerm {
                        reduction: Reduction::Count,
                        input: None,
                    },
                    ReduceTerm {
                        reduction: Reduction::Mean,
                        input: Some(score),
                    },
                ],
            });
        let ungrouped_result = validate_provider_result(
            &registry,
            &ungrouped_request,
            ProviderResultEvidence::reduction(
                ungrouped_request.request_token(),
                ungrouped_request.shape_id().clone(),
                root,
                None,
                vec![ReductionRow::new(
                    None,
                    vec![ReducedValue::Count(0), ReducedValue::Double(None)],
                )],
            ),
        )
        .unwrap();
        let ungrouped = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            ungrouped_request,
            ungrouped_result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        let ProjectedQueryValue::Reduction { rows, .. } = ungrouped.value() else {
            panic!("expected ungrouped reduction")
        };
        assert!(rows[0].group().is_none());
        assert_eq!(ungrouped.resource_measure().cells(), 2);

        let group = BindingId::new(1);
        let grouped_request = validate_match_request(
            &registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![
                        binding(
                            &registry,
                            0,
                            "employee",
                            ThingKind::Entity,
                            MatchMode::Exact,
                        ),
                        binding(
                            &registry,
                            1,
                            "employee",
                            ThingKind::Entity,
                            MatchMode::Exact,
                        ),
                    ],
                    predicate: None,
                    allowed_cross_joins: BTreeSet::from([BindingPair::new(root, group)]),
                },
                MatchOperation::ReduceBy {
                    root,
                    group: Some(group),
                    reducers: vec![ReduceTerm {
                        reduction: Reduction::Count,
                        input: None,
                    }],
                },
            ),
        )
        .unwrap();
        let grouped_result = validate_provider_result(
            &registry,
            &grouped_request,
            ProviderResultEvidence::reduction(
                grouped_request.request_token(),
                grouped_request.shape_id().clone(),
                root,
                Some(group),
                vec![ReductionRow::new(
                    Some(employee(&registry, "0x9", "employee", "group", 9)),
                    vec![ReducedValue::Count(4)],
                )],
            ),
        )
        .unwrap();
        let grouped = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            grouped_request,
            grouped_result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        let ProjectedQueryValue::Reduction { rows, .. } = grouped.value() else {
            panic!("expected grouped reduction")
        };
        assert!(matches!(
            rows[0].group(),
            Some(ProjectedReductionGroup::Thing(thing)) if thing.iid() == "0x9"
        ));
        assert_eq!(grouped.resource_measure().cells(), 2);

        let double = -0.0_f64;
        let request = reduction_request(&registry, |root, score| MatchOperation::ReduceByField {
            root,
            group: score.clone(),
            reducers: vec![
                ReduceTerm {
                    reduction: Reduction::Count,
                    input: None,
                },
                ReduceTerm {
                    reduction: Reduction::Mean,
                    input: Some(score),
                },
            ],
        });
        let MatchOperation::ReduceByField { group, .. } = &request.request().operation else {
            unreachable!()
        };
        let result = validate_provider_result(
            &registry,
            &request,
            ProviderResultEvidence::field_reduction(
                request.request_token(),
                request.shape_id().clone(),
                root,
                group.clone(),
                vec![ReductionRow::new_field(
                    AttributeValue::Long(7),
                    vec![ReducedValue::Count(2), ReducedValue::Double(Some(double))],
                )],
            ),
        )
        .unwrap();
        let projected = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        let ProjectedQueryValue::FieldReduction { rows, .. } = projected.value() else {
            panic!("expected field reduction")
        };
        assert!(
            matches!(rows[0].group(), Some(ProjectedReductionGroup::Field(value)) if value.value() == &CanonicalValue::Long(7))
        );
        assert!(
            matches!(rows[0].values(), [ProjectedReducedValue::Count(2), ProjectedReducedValue::Double(Some(value))] if value.bits() == double.to_bits())
        );
        assert_eq!(projected.resource_measure().cells(), 3);

        let owner = descriptor(&registry, "employee");
        let name = BoundFieldId::new(root, registry.field_id(&owner, "name").unwrap());
        let score = BoundFieldId::new(root, registry.field_id(&owner, "score").unwrap());
        let tuple_request = validate_match_request(
            &registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![binding(
                        &registry,
                        0,
                        "employee",
                        ThingKind::Entity,
                        MatchMode::Exact,
                    )],
                    predicate: None,
                    allowed_cross_joins: BTreeSet::new(),
                },
                MatchOperation::ReduceByFields {
                    root,
                    groups: vec![score.clone(), name.clone()],
                    reducers: vec![ReduceTerm {
                        reduction: Reduction::Count,
                        input: None,
                    }],
                },
            ),
        )
        .unwrap();
        let tuple_result = validate_provider_result(
            &registry,
            &tuple_request,
            ProviderResultEvidence::field_tuple_reduction(
                tuple_request.request_token(),
                tuple_request.shape_id().clone(),
                root,
                vec![score, name],
                vec![ReductionRow::new_fields(
                    vec![
                        AttributeValue::Long(7),
                        AttributeValue::String("Ada".to_owned()),
                    ],
                    vec![ReducedValue::Count(1)],
                )],
            ),
        )
        .unwrap();
        let tuple = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            tuple_request,
            tuple_result,
            ProjectedQueryMaterializationLimits::default(),
        )
        .unwrap();
        assert_eq!(tuple.resource_measure().cells(), 3);
        assert_eq!(tuple.resource_measure().attribute_values(), 2);
        let exact = tuple.resource_measure();

        let owner = descriptor(&registry, "employee");
        let score = BoundFieldId::new(root, registry.field_id(&owner, "score").unwrap());
        let name = BoundFieldId::new(root, registry.field_id(&owner, "name").unwrap());
        let tuple_request = validate_match_request(
            &registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![binding(
                        &registry,
                        0,
                        "employee",
                        ThingKind::Entity,
                        MatchMode::Exact,
                    )],
                    predicate: None,
                    allowed_cross_joins: BTreeSet::new(),
                },
                MatchOperation::ReduceByFields {
                    root,
                    groups: vec![score.clone(), name.clone()],
                    reducers: vec![ReduceTerm {
                        reduction: Reduction::Count,
                        input: None,
                    }],
                },
            ),
        )
        .unwrap();
        let tuple_result = validate_provider_result(
            &registry,
            &tuple_request,
            ProviderResultEvidence::field_tuple_reduction(
                tuple_request.request_token(),
                tuple_request.shape_id().clone(),
                root,
                vec![score, name],
                vec![ReductionRow::new_fields(
                    vec![
                        AttributeValue::Long(7),
                        AttributeValue::String("Ada".to_owned()),
                    ],
                    vec![ReducedValue::Count(1)],
                )],
            ),
        )
        .unwrap();
        let error = materialize_projected_query_result(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            tuple_request,
            tuple_result,
            ProjectedQueryMaterializationLimits::tightened(
                exact.rows(),
                exact.cells() - 1,
                exact.things(),
                exact.attribute_values(),
                exact.bytes(),
            ),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "projected_query_cell_limit");
        assert_eq!(detail_count(&error, "actual"), exact.cells() as u64);
    }

    #[test]
    fn projected_query_cancellation_fails_before_any_result_is_published() {
        let (installed, registry) = fixture();
        let make = || {
            let request = one_rows_request(&registry);
            let solutions = vec![solution(vec![(
                0,
                employee(&registry, "0xcancel", "person", "cancelled", 1),
            )])];
            validate_rows(&registry, request, solutions)
        };
        let (request, result) = make();
        let cancellation = AnswerCancellation::default();
        cancellation.cancel();

        let error = materialize_projected_query_result_with_cancellation(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            result,
            ProjectedQueryMaterializationLimits::default(),
            &cancellation,
        )
        .expect_err("pre-cancelled native materialization must be all-or-nothing");

        assert_eq!(error.category(), SdkDiagnosticCategory::Cancelled);
        assert_eq!(error.code().as_str(), "provider_cancelled");
        assert_eq!(
            error.path(),
            &[SdkDiagnosticPathSegment::Query(
                SdkQueryDiagnosticPathKind::ProviderEvidence,
            )]
        );

        let (request, result) = make();
        let error = materialize_projected_query_result_with_budget(
            &installed,
            &registry,
            ProjectedQueryOrigin::remote_unbound(),
            request,
            result,
            ProjectedQueryMaterializationLimits::default(),
            &AnswerCancellation::default(),
            Some(QueryExecutionDeadline::from_timeout_milliseconds(0)),
        )
        .expect_err("an expired invocation cannot publish a materialized result");
        assert_eq!(error.category(), SdkDiagnosticCategory::ResourceLimit);
        assert_eq!(error.code().as_str(), "transaction_deadline_exceeded");
        assert_eq!(
            error.path(),
            &[SdkDiagnosticPathSegment::Query(
                SdkQueryDiagnosticPathKind::ProviderEvidence,
            )]
        );
    }
}
