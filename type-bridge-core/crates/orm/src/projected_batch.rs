//! Immutable binding-neutral projected mutation batches and provider-free policy checks.

use std::cmp::Ordering;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use type_bridge_contract::id::{TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::projection::ModelProjection;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticMessage, SdkDiagnosticName,
    SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
};
use type_bridge_contract::value::CanonicalValue;

use crate::projected_model::{ProjectedCreate, ProjectionBrand};
use crate::query_execution_limits::{
    MAX_QUERY_ATTRIBUTE_VALUES, MAX_QUERY_BYTES, MAX_QUERY_COLLECTION_MEMBERS,
    MAX_QUERY_GRAPH_NODES, MAX_QUERY_ITEMS, MAX_QUERY_ROLE_PLAYERS, MAX_QUERY_STATEMENTS,
    QueryExecutionDeadline, QueryExecutionResourceLimits,
};
use crate::runtime_projection::InstalledRuntimeProjection;
use crate::session::backend::AnswerCancellation;

/// One closed projected mutation operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectedBatchOperation {
    /// Insert every complete create input.
    Insert,
    /// Put every complete keyed create input.
    Put,
    /// Replace every exact IID with a complete create input.
    Update,
    /// Delete every exact IID.
    Delete,
}

/// One caller-controlled row in a projected mutation batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectedBatchRow {
    /// A complete insert or put input.
    Create(ProjectedCreate),
    /// An exact update target and complete replacement input.
    Update {
        /// Canonical TypeDB thing IID.
        iid: String,
        /// Complete exact-model replacement input.
        replacement: ProjectedCreate,
    },
    /// An exact delete target.
    Delete {
        /// Canonical TypeDB thing IID.
        iid: String,
    },
}

/// Allocation-free cached resource measure for one projected batch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProjectedBatchResourceMeasure {
    items: u64,
    bytes: u64,
    graph_nodes: u64,
    attribute_values: u64,
    collection_members: u64,
    role_players: u64,
    statements: u32,
}

impl ProjectedBatchResourceMeasure {
    /// Return the number of input rows.
    #[must_use]
    pub const fn items(self) -> u64 {
        self.items
    }
    /// Return recursively charged projected and IID bytes.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.bytes
    }
    /// Return target things plus embedded role-player references.
    #[must_use]
    pub const fn graph_nodes(self) -> u64 {
        self.graph_nodes
    }
    /// Return owner scalars plus embedded reference-key scalars.
    #[must_use]
    pub const fn attribute_values(self) -> u64 {
        self.attribute_values
    }
    /// Return nested field scalars, role references, and reference-key scalars.
    #[must_use]
    pub const fn collection_members(self) -> u64 {
        self.collection_members
    }
    /// Return embedded role-player references.
    #[must_use]
    pub const fn role_players(self) -> u64 {
        self.role_players
    }
    /// Return the whole-batch provider-call forecast.
    #[must_use]
    pub const fn statements(self) -> u32 {
        self.statements
    }
}

#[derive(Debug)]
struct NormalizedProjectedBatchRow {
    ordinal: usize,
    row: ProjectedBatchRow,
}

/// One deterministic owned-vector reservation site.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectedBatchReservationSite {
    /// Normalized row storage.
    Rows,
    /// Exact-IID duplicate index storage.
    Targets,
    /// Effective-key duplicate index storage.
    Keys,
}

/// Deterministic reservation-failure probe for provider-free tests.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProjectedBatchReservationProbe {
    fail_at: Option<ProjectedBatchReservationSite>,
}

impl ProjectedBatchReservationProbe {
    /// Fail the selected reservation site before the real allocator is called.
    #[must_use]
    #[cfg(test)]
    const fn failing_at(site: ProjectedBatchReservationSite) -> Self {
        Self {
            fail_at: Some(site),
        }
    }

    fn check(self, site: ProjectedBatchReservationSite) -> Result<(), SdkExecutionDiagnostic> {
        if self.fail_at == Some(site) {
            Err(allocation_failure(site))
        } else {
            Ok(())
        }
    }
}

/// Immutable exact-model projected mutation batch.
#[derive(Debug)]
pub struct ProjectedBatch {
    brand: ProjectionBrand,
    model: TypeId,
    operation: ProjectedBatchOperation,
    rows: Vec<NormalizedProjectedBatchRow>,
    measure: ProjectedBatchResourceMeasure,
}

impl ProjectedBatch {
    /// Construct the shared redacted diagnostic for binding-owned batch row
    /// storage that cannot be reserved.
    #[doc(hidden)]
    #[must_use]
    pub fn binding_allocation_failure() -> SdkExecutionDiagnostic {
        append_path(
            SdkExecutionDiagnostic::resource_limit(
                sdk_code("projected_batch_allocation_exhausted"),
                sdk_message("The projected batch binding could not reserve bounded row storage"),
            ),
            [SdkDiagnosticPathSegment::Argument(name("rows"))],
        )
    }

    /// Validate a binding-owned row count against the common hard item
    /// ceiling before the binding duplicates or projects caller inputs.
    #[doc(hidden)]
    pub fn validate_binding_row_count(row_count: usize) -> Result<(), SdkExecutionDiagnostic> {
        validate_row_count(row_count, MAX_QUERY_ITEMS).map(|_| ())
    }

    /// Validate and normalize one homogeneous projected batch.
    pub fn try_new(
        installed: &InstalledRuntimeProjection,
        model: TypeId,
        operation: ProjectedBatchOperation,
        rows: Vec<ProjectedBatchRow>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_internal(
            installed,
            model,
            operation,
            rows,
            None,
            ProjectedBatchReservationProbe::default(),
        )
    }

    /// Validate and normalize a terminal batch against an already captured control.
    #[doc(hidden)]
    pub fn try_new_for_invocation(
        installed: &InstalledRuntimeProjection,
        model: TypeId,
        operation: ProjectedBatchOperation,
        rows: Vec<ProjectedBatchRow>,
        control: &ProjectedBatchInvocationControl,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_internal(
            installed,
            model,
            operation,
            rows,
            Some(control),
            ProjectedBatchReservationProbe::default(),
        )
    }

    /// Validate with a deterministic reservation-failure probe.
    #[doc(hidden)]
    #[cfg(test)]
    pub(crate) fn try_new_with_reservation_probe(
        installed: &InstalledRuntimeProjection,
        model: TypeId,
        operation: ProjectedBatchOperation,
        rows: Vec<ProjectedBatchRow>,
        probe: ProjectedBatchReservationProbe,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_internal(installed, model, operation, rows, None, probe)
    }

    #[cfg(test)]
    fn try_new_for_invocation_with_reservation_probe(
        installed: &InstalledRuntimeProjection,
        model: TypeId,
        operation: ProjectedBatchOperation,
        rows: Vec<ProjectedBatchRow>,
        control: &ProjectedBatchInvocationControl,
        probe: ProjectedBatchReservationProbe,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_internal(installed, model, operation, rows, Some(control), probe)
    }

    fn try_new_internal(
        installed: &InstalledRuntimeProjection,
        model: TypeId,
        operation: ProjectedBatchOperation,
        rows: Vec<ProjectedBatchRow>,
        control: Option<&ProjectedBatchInvocationControl>,
        probe: ProjectedBatchReservationProbe,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let projected_model = require_batch_model(installed, &model, operation)?;
        let row_count = rows.len();
        if row_count == 0 {
            return Ok(Self {
                brand: ProjectionBrand::from_installed(installed),
                model,
                operation,
                rows: Vec::new(),
                measure: ProjectedBatchResourceMeasure::default(),
            });
        }
        if let Some(control) = control {
            control.check()?;
        }
        let item_ceiling = control.map_or(MAX_QUERY_ITEMS, |control| control.limits.items);
        let item_count = validate_row_count(row_count, item_ceiling)?;
        if let Some(control) = control {
            control.check()?;
        }
        let mut normalized = reserved(row_count, ProjectedBatchReservationSite::Rows, probe)?;
        let target_capacity = if matches!(
            operation,
            ProjectedBatchOperation::Update | ProjectedBatchOperation::Delete
        ) {
            row_count
        } else {
            0
        };
        if let Some(control) = control {
            control.check()?;
        }
        let mut target_rows = reserved_nonempty(
            target_capacity,
            ProjectedBatchReservationSite::Targets,
            probe,
        )?;
        let has_effective_keys = !projected_model.reference_read().key_fields().is_empty();
        let key_capacity = if has_effective_keys
            && matches!(
                operation,
                ProjectedBatchOperation::Insert
                    | ProjectedBatchOperation::Put
                    | ProjectedBatchOperation::Update
            ) {
            row_count
        } else {
            0
        };
        if let Some(control) = control {
            control.check()?;
        }
        let mut key_rows =
            reserved_nonempty(key_capacity, ProjectedBatchReservationSite::Keys, probe)?;
        let mut measure = ProjectedBatchResourceMeasure::default();

        for (ordinal, row) in rows.into_iter().enumerate() {
            if let Some(control) = control {
                control.check()?;
            }
            validate_row(installed, &model, operation, ordinal, &row)?;
            charge_row(projected_model, &mut measure, &row, ordinal, control)?;
            if matches!(
                row,
                ProjectedBatchRow::Update { .. } | ProjectedBatchRow::Delete { .. }
            ) {
                target_rows.push(ordinal);
            }
            if has_effective_keys && row_create(&row).is_some() {
                key_rows.push(ordinal);
            }
            normalized.push(NormalizedProjectedBatchRow { ordinal, row });
        }
        if let Some(control) = control {
            control.check()?;
        }
        reject_duplicate_targets(&normalized, &mut target_rows)?;
        if let Some(control) = control {
            control.check()?;
        }
        reject_duplicate_keys(projected_model, &normalized, &mut key_rows)?;
        measure.items = item_count;
        measure.statements = statement_forecast(operation, measure.role_players > 0, row_count);
        if let Some(control) = control {
            control.check()?;
        }
        validate_limits(
            measure,
            control.map_or(QueryExecutionResourceLimits::default(), |control| {
                control.limits
            }),
        )?;

        Ok(Self {
            brand: ProjectionBrand::from_installed(installed),
            model,
            operation,
            rows: normalized,
            measure,
        })
    }

    /// Return the exact projected model identity.
    #[must_use]
    pub const fn model(&self) -> &TypeId {
        &self.model
    }
    /// Return the closed operation kind.
    #[must_use]
    pub const fn operation(&self) -> ProjectedBatchOperation {
        self.operation
    }
    /// Return the number of ordered input rows.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.rows.len()
    }
    /// Report whether the batch has no rows.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    /// Return one normalized row and its retained input ordinal.
    #[doc(hidden)]
    #[must_use]
    pub fn row_at(&self, index: usize) -> Option<(usize, &ProjectedBatchRow)> {
        self.rows.get(index).map(|row| (row.ordinal, &row.row))
    }
    /// Return the cached resource measure without allocation.
    #[must_use]
    pub const fn resource_measure(&self) -> ProjectedBatchResourceMeasure {
        self.measure
    }
    /// Return the semantic-schema brand.
    #[must_use]
    pub const fn semantic_fingerprint(
        &self,
    ) -> &type_bridge_contract::schema_fingerprint::SemanticSchemaFingerprint {
        self.brand.semantic()
    }
    /// Return the exact binding target brand.
    #[must_use]
    pub const fn binding_target(&self) -> type_bridge_contract::projection::BindingTarget {
        self.brand.target()
    }
    /// Return the binding-projection brand.
    #[must_use]
    pub const fn projection_fingerprint(
        &self,
    ) -> &type_bridge_contract::projection::BindingProjectionFingerprint {
        self.brand.projection()
    }

    /// Verify the retained package brand and model authority for reuse.
    pub fn validate_for(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.brand.validate(installed, type_path(&self.model))?;
        require_batch_model(installed, &self.model, self.operation).map(|_| ())
    }
}

/// One invocation policy captured before batch construction or allocation.
#[doc(hidden)]
#[derive(Debug)]
pub struct ProjectedBatchInvocationControl {
    limits: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
    #[cfg(test)]
    reference_checks_until_cancel: AtomicUsize,
}

impl ProjectedBatchInvocationControl {
    /// Capture effective limits, one absolute deadline, and cancellation owner.
    #[must_use]
    pub fn capture(limits: QueryExecutionResourceLimits, cancellation: AnswerCancellation) -> Self {
        let limits = limits.effective();
        Self {
            limits,
            deadline: QueryExecutionDeadline::for_limits(limits),
            cancellation,
            #[cfg(test)]
            reference_checks_until_cancel: AtomicUsize::new(usize::MAX),
        }
    }

    /// Return the absolute deadline captured at terminal entry.
    #[must_use]
    pub const fn deadline(&self) -> QueryExecutionDeadline {
        self.deadline
    }

    fn check(&self) -> Result<(), SdkExecutionDiagnostic> {
        if self.cancellation.is_cancelled() {
            return Err(SdkExecutionDiagnostic::data_operation_cancelled());
        }
        if self.deadline.is_expired() {
            return Err(SdkExecutionDiagnostic::data_operation_deadline_exceeded());
        }
        Ok(())
    }

    #[cfg(test)]
    fn cancel_after_reference_checks(&self, checks: usize) {
        self.reference_checks_until_cancel
            .store(checks, AtomicOrdering::Relaxed);
    }

    fn check_reference(&self) -> Result<(), SdkExecutionDiagnostic> {
        #[cfg(test)]
        {
            let remaining = self
                .reference_checks_until_cancel
                .load(AtomicOrdering::Relaxed);
            if remaining != usize::MAX {
                if remaining == 0 {
                    self.cancellation.cancel();
                } else {
                    self.reference_checks_until_cancel
                        .fetch_sub(1, AtomicOrdering::Relaxed);
                }
            }
        }
        self.check()
    }
}

/// A provider-free, control-checked invocation over one immutable batch.
#[doc(hidden)]
#[derive(Debug)]
pub struct PreparedProjectedBatchInvocation<'batch> {
    batch: &'batch ProjectedBatch,
    limits: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
}

impl<'batch> PreparedProjectedBatchInvocation<'batch> {
    /// Validate cancellation, one absolute deadline, and all resource dimensions.
    pub fn try_new(
        installed: &InstalledRuntimeProjection,
        batch: &'batch ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        batch.validate_for(installed)?;
        let ProjectedBatchInvocationControl {
            limits,
            deadline,
            cancellation,
            #[cfg(test)]
                reference_checks_until_cancel: _,
        } = control;
        let prepared = Self {
            batch,
            limits,
            deadline,
            cancellation,
        };
        if !batch.is_empty() {
            prepared.check_control()?;
            validate_limits(batch.measure, limits)?;
        }
        Ok(prepared)
    }

    /// Return the immutable batch.
    #[must_use]
    pub const fn batch(&self) -> &'batch ProjectedBatch {
        self.batch
    }
    /// Return the exact effective resource limits.
    #[must_use]
    pub const fn limits(&self) -> QueryExecutionResourceLimits {
        self.limits
    }
    /// Return the single captured absolute deadline.
    #[must_use]
    pub const fn deadline(&self) -> QueryExecutionDeadline {
        self.deadline
    }
    /// Return the shared cancellation owner.
    #[must_use]
    pub const fn cancellation(&self) -> &AnswerCancellation {
        &self.cancellation
    }

    /// Recheck the captured cancellation owner and absolute deadline.
    pub fn check_control(&self) -> Result<(), SdkExecutionDiagnostic> {
        if self.batch.is_empty() {
            return Ok(());
        }
        if self.cancellation.is_cancelled() {
            return Err(SdkExecutionDiagnostic::data_operation_cancelled());
        }
        if self.deadline.is_expired() {
            return Err(SdkExecutionDiagnostic::data_operation_deadline_exceeded());
        }
        Ok(())
    }
}

fn require_batch_model<'a>(
    installed: &'a InstalledRuntimeProjection,
    type_id: &TypeId,
    operation: ProjectedBatchOperation,
) -> Result<&'a ModelProjection, SdkExecutionDiagnostic> {
    if !matches!(type_id.kind(), TypeKind::Entity | TypeKind::Relation) {
        return Err(invalid(
            "wrong_model_kind",
            "Projected batches require an entity or relation model",
            type_path(type_id),
        ));
    }
    let model = installed
        .projection()
        .models()
        .get(type_id)
        .ok_or_else(|| {
            invalid(
                "model_not_projected",
                "The batch model is absent from the installed runtime projection",
                type_path(type_id),
            )
        })?;
    if model.declaration().is_abstract() || !model.declaration().is_constructible() {
        return Err(invalid(
            "model_not_constructible",
            "Projected batches require a concrete constructible model",
            type_path(type_id),
        ));
    }
    if !matches!(operation, ProjectedBatchOperation::Delete) && !model.create().enabled() {
        return Err(invalid(
            "model_not_creatable",
            "The projected batch operation requires an enabled create facet",
            type_path(type_id),
        ));
    }
    if matches!(operation, ProjectedBatchOperation::Put)
        && model.reference_read().key_fields().is_empty()
    {
        return Err(invalid(
            "put_requires_projected_key",
            "Projected put requires an effective reference key",
            type_path(type_id),
        ));
    }
    Ok(model)
}

fn validate_row(
    installed: &InstalledRuntimeProjection,
    model: &TypeId,
    operation: ProjectedBatchOperation,
    ordinal: usize,
    row: &ProjectedBatchRow,
) -> Result<(), SdkExecutionDiagnostic> {
    let form_matches = matches!(
        (operation, row),
        (
            ProjectedBatchOperation::Insert | ProjectedBatchOperation::Put,
            ProjectedBatchRow::Create(_)
        ) | (
            ProjectedBatchOperation::Update,
            ProjectedBatchRow::Update { .. }
        ) | (
            ProjectedBatchOperation::Delete,
            ProjectedBatchRow::Delete { .. }
        )
    );
    if !form_matches {
        return Err(invalid(
            "batch_row_operation_mismatch",
            "The batch row form does not match its operation",
            row_path(ordinal),
        ));
    }
    if let Some(create) = row_create(row) {
        create
            .validate_for(installed)
            .map_err(|error| prefix_row(error, ordinal))?;
        if create.type_id() != model {
            return Err(invalid(
                "batch_row_model_mismatch",
                "The batch row belongs to a different exact model",
                {
                    let mut path = row_path(ordinal);
                    path.push(SdkDiagnosticPathSegment::Type(create.type_id().clone()));
                    path
                },
            ));
        }
        if model.kind() == TypeKind::Relation
            && !matches!(operation, ProjectedBatchOperation::Delete)
            && create.roles().values().all(Vec::is_empty)
        {
            let mut path = row_path(ordinal);
            path.push(SdkDiagnosticPathSegment::Type(model.clone()));
            return Err(invalid(
                "relation_requires_role_player",
                "Projected relation writes require at least one final role player",
                path,
            ));
        }
    }
    if let Some(iid) = row_iid(row)
        && !is_canonical_thing_iid(iid)
    {
        return Err(invalid(
            "noncanonical_iid",
            "The batch target IID is not canonical TypeDB identity text",
            iid_path(ordinal),
        ));
    }
    Ok(())
}

fn charge_row(
    model: &ModelProjection,
    measure: &mut ProjectedBatchResourceMeasure,
    row: &ProjectedBatchRow,
    ordinal: usize,
    control: Option<&ProjectedBatchInvocationControl>,
) -> Result<(), SdkExecutionDiagnostic> {
    measure.graph_nodes = add(measure.graph_nodes, 1, "graph_nodes", ordinal)?;
    if let Some(create) = row_create(row) {
        measure.bytes = add(
            measure.bytes,
            usize_u64(create.resource_measure().bytes(), "bytes", ordinal)?,
            "bytes",
            ordinal,
        )?;
        for field in model.create().fields() {
            if let Some(control) = control {
                control.check()?;
            }
            let values = &create.fields()[field.token()];
            let count = usize_u64(values.len(), "attribute_values", ordinal)?;
            measure.attribute_values =
                add(measure.attribute_values, count, "attribute_values", ordinal)?;
            measure.collection_members = add(
                measure.collection_members,
                count,
                "collection_members",
                ordinal,
            )?;
        }
        for role_id in model.create().roles().keys() {
            if let Some(control) = control {
                control.check()?;
            }
            let references = &create.roles()[role_id];
            let count = usize_u64(references.len(), "role_players", ordinal)?;
            measure.graph_nodes = add(measure.graph_nodes, count, "graph_nodes", ordinal)?;
            measure.role_players = add(measure.role_players, count, "role_players", ordinal)?;
            measure.collection_members = add(
                measure.collection_members,
                count,
                "collection_members",
                ordinal,
            )?;
            for reference in references {
                if let Some(control) = control {
                    control.check_reference()?;
                }
                let key_count = usize_u64(reference.keys().len(), "attribute_values", ordinal)?;
                measure.attribute_values = add(
                    measure.attribute_values,
                    key_count,
                    "attribute_values",
                    ordinal,
                )?;
                measure.collection_members = add(
                    measure.collection_members,
                    key_count,
                    "collection_members",
                    ordinal,
                )?;
            }
        }
    }
    if let Some(iid) = row_iid(row) {
        measure.bytes = add(
            measure.bytes,
            usize_u64(iid.len(), "bytes", ordinal)?,
            "bytes",
            ordinal,
        )?;
    }
    Ok(())
}

fn reject_duplicate_targets(
    rows: &[NormalizedProjectedBatchRow],
    indices: &mut [usize],
) -> Result<(), SdkExecutionDiagnostic> {
    indices.sort_unstable_by(|&left, &right| {
        iid_cmp(
            row_iid(&rows[left].row).expect("target index refers to IID row"),
            row_iid(&rows[right].row).expect("target index refers to IID row"),
        )
        .then_with(|| left.cmp(&right))
    });
    let mut duplicate = None;
    for pair in indices.windows(2) {
        let [left, right] = pair else { continue };
        if row_iid(&rows[*left].row)
            .expect("target index refers to IID row")
            .eq_ignore_ascii_case(
                row_iid(&rows[*right].row).expect("target index refers to IID row"),
            )
        {
            retain_earliest_conflict(&mut duplicate, (*left, *right));
        }
    }
    duplicate.map_or(Ok(()), |(first, second)| {
        Err(conflict(
            "duplicate_batch_target",
            "The same exact IID appears more than once in the batch",
            iid_path(second),
            first,
        ))
    })
}

fn iid_cmp(left: &str, right: &str) -> Ordering {
    left.bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(right.bytes().map(|byte| byte.to_ascii_lowercase()))
}

fn reject_duplicate_keys(
    model: &ModelProjection,
    rows: &[NormalizedProjectedBatchRow],
    indices: &mut [usize],
) -> Result<(), SdkExecutionDiagnostic> {
    let key_fields = model.reference_read().key_fields();
    if key_fields.is_empty() {
        return Ok(());
    }
    indices.sort_unstable_by(|&left, &right| {
        effective_key_cmp(
            key_fields,
            row_create(&rows[left].row).expect("key index refers to create row"),
            row_create(&rows[right].row).expect("key index refers to create row"),
        )
        .then_with(|| left.cmp(&right))
    });
    let mut duplicate = None;
    for pair in indices.windows(2) {
        let [left, right] = pair else { continue };
        let left_create = row_create(&rows[*left].row).expect("key index refers to create row");
        let right_create = row_create(&rows[*right].row).expect("key index refers to create row");
        if effective_keys_equal(key_fields, left_create, right_create) {
            retain_earliest_conflict(&mut duplicate, (*left, *right));
        }
    }
    duplicate.map_or(Ok(()), |(first, second)| {
        let mut path = row_path(second);
        path.push(SdkDiagnosticPathSegment::Field(key_fields[0].clone()));
        Err(conflict(
            "duplicate_batch_key",
            "The same semantic effective key appears more than once in the batch",
            path,
            first,
        ))
    })
}

fn retain_earliest_conflict(candidate: &mut Option<(usize, usize)>, conflict: (usize, usize)) {
    let conflict = (conflict.0.min(conflict.1), conflict.0.max(conflict.1));
    if candidate.is_none_or(|current| (conflict.1, conflict.0) < (current.1, current.0)) {
        *candidate = Some(conflict);
    }
}

fn effective_key_cmp(
    key_fields: &[type_bridge_contract::schema::OwnsFactId],
    left: &ProjectedCreate,
    right: &ProjectedCreate,
) -> Ordering {
    for field in key_fields {
        let left = left.fields()[field][0].value();
        let right = right.fields()[field][0].value();
        let ordering = left
            .semantic_cmp_same_domain(right)
            .unwrap_or_else(|| left.cmp(right));
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

fn effective_keys_equal(
    key_fields: &[type_bridge_contract::schema::OwnsFactId],
    left: &ProjectedCreate,
    right: &ProjectedCreate,
) -> bool {
    key_fields.iter().all(|field| {
        let left = left
            .fields()
            .get(field)
            .and_then(|values| values.first())
            .map(|value| value.value());
        let right = right
            .fields()
            .get(field)
            .and_then(|values| values.first())
            .map(|value| value.value());
        matches!((left, right), (Some(left), Some(right)) if semantic_equal(left, right))
    })
}

fn semantic_equal(left: &CanonicalValue, right: &CanonicalValue) -> bool {
    left == right || left.semantic_cmp_same_domain(right) == Some(Ordering::Equal)
}

fn validate_limits(
    measure: ProjectedBatchResourceMeasure,
    limits: QueryExecutionResourceLimits,
) -> Result<(), SdkExecutionDiagnostic> {
    for (name, code, actual, ceiling) in [
        ("items", "batch_item_limit", measure.items, limits.items),
        ("bytes", "batch_byte_limit", measure.bytes, limits.bytes),
        (
            "graph_nodes",
            "batch_graph_node_limit",
            measure.graph_nodes,
            limits.graph_nodes,
        ),
        (
            "attribute_values",
            "batch_attribute_value_limit",
            measure.attribute_values,
            limits.attribute_values,
        ),
        (
            "collection_members",
            "batch_collection_member_limit",
            measure.collection_members,
            limits.collection_members,
        ),
        (
            "role_players",
            "batch_role_player_limit",
            measure.role_players,
            limits.role_players,
        ),
        (
            "statements",
            "batch_statement_limit",
            u64::from(measure.statements),
            u64::from(limits.statements),
        ),
    ] {
        if actual > ceiling {
            return Err(limit_failure(code, name, actual, ceiling));
        }
    }
    Ok(())
}

fn statement_forecast(
    operation: ProjectedBatchOperation,
    requires_reference_resolution: bool,
    rows: usize,
) -> u32 {
    if rows == 0 {
        return 0;
    }
    match operation {
        ProjectedBatchOperation::Delete => 1,
        ProjectedBatchOperation::Put => 3,
        ProjectedBatchOperation::Insert | ProjectedBatchOperation::Update
            if requires_reference_resolution =>
        {
            3
        }
        ProjectedBatchOperation::Insert | ProjectedBatchOperation::Update => 2,
    }
}

fn reserved<T>(
    capacity: usize,
    site: ProjectedBatchReservationSite,
    probe: ProjectedBatchReservationProbe,
) -> Result<Vec<T>, SdkExecutionDiagnostic> {
    probe.check(site)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| allocation_failure(site))?;
    Ok(values)
}

fn reserved_nonempty<T>(
    capacity: usize,
    site: ProjectedBatchReservationSite,
    probe: ProjectedBatchReservationProbe,
) -> Result<Vec<T>, SdkExecutionDiagnostic> {
    if capacity == 0 {
        Ok(Vec::new())
    } else {
        reserved(capacity, site, probe)
    }
}

fn row_create(row: &ProjectedBatchRow) -> Option<&ProjectedCreate> {
    match row {
        ProjectedBatchRow::Create(create) => Some(create),
        ProjectedBatchRow::Update { replacement, .. } => Some(replacement),
        ProjectedBatchRow::Delete { .. } => None,
    }
}

fn row_iid(row: &ProjectedBatchRow) -> Option<&str> {
    match row {
        ProjectedBatchRow::Update { iid, .. } | ProjectedBatchRow::Delete { iid } => Some(iid),
        ProjectedBatchRow::Create(_) => None,
    }
}

fn add(
    current: u64,
    value: u64,
    dimension: &'static str,
    ordinal: usize,
) -> Result<u64, SdkExecutionDiagnostic> {
    current.checked_add(value).ok_or_else(|| {
        limit_failure_at_row(
            limit_code(dimension),
            dimension,
            u64::MAX,
            hard_ceiling(dimension),
            ordinal,
        )
    })
}

fn usize_u64(
    value: usize,
    dimension: &'static str,
    ordinal: usize,
) -> Result<u64, SdkExecutionDiagnostic> {
    u64::try_from(value).map_err(|_| {
        limit_failure_at_row(
            limit_code(dimension),
            dimension,
            u64::MAX,
            hard_ceiling(dimension),
            ordinal,
        )
    })
}

fn prefix_row(error: SdkExecutionDiagnostic, ordinal: usize) -> SdkExecutionDiagnostic {
    error
        .try_with_path_prefix(row_path(ordinal))
        .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
}

fn row_path(index: usize) -> Vec<SdkDiagnosticPathSegment> {
    vec![
        SdkDiagnosticPathSegment::Argument(name("rows")),
        SdkDiagnosticPathSegment::Index(
            u64::try_from(index).expect("batch row count is bounded by MAX_QUERY_ITEMS"),
        ),
    ]
}

fn iid_path(index: usize) -> Vec<SdkDiagnosticPathSegment> {
    let mut path = row_path(index);
    path.push(SdkDiagnosticPathSegment::Argument(name("iid")));
    path
}

fn type_path(type_id: &TypeId) -> Vec<SdkDiagnosticPathSegment> {
    vec![SdkDiagnosticPathSegment::Type(type_id.clone())]
}

fn invalid(
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    append_path(
        SdkExecutionDiagnostic::invalid_input(sdk_code(code), sdk_message(message)),
        path,
    )
}

fn conflict(
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
    first: usize,
) -> SdkExecutionDiagnostic {
    append_path(
        SdkExecutionDiagnostic::invalid_input(sdk_code(code), sdk_message(message)),
        path,
    )
    .try_with_detail(
        name("first_conflicting_index"),
        SdkDiagnosticDetailValue::Count(
            u64::try_from(first).expect("batch row count is bounded by MAX_QUERY_ITEMS"),
        ),
    )
    .expect("batch conflict detail is bounded")
}

fn limit_failure(
    code: &'static str,
    dimension: &'static str,
    actual: u64,
    ceiling: u64,
) -> SdkExecutionDiagnostic {
    let path = vec![
        SdkDiagnosticPathSegment::Argument(name("limits")),
        SdkDiagnosticPathSegment::Argument(name(dimension)),
    ];
    let diagnostic = append_path(
        SdkExecutionDiagnostic::resource_limit(
            sdk_code(code),
            sdk_message("The projected batch exceeds a binding neutral execution limit"),
        ),
        path,
    );
    with_limit_details(diagnostic, dimension, actual, ceiling)
}

fn with_limit_details(
    diagnostic: SdkExecutionDiagnostic,
    dimension: &'static str,
    actual: u64,
    ceiling: u64,
) -> SdkExecutionDiagnostic {
    if dimension == "bytes" {
        diagnostic
            .try_with_detail(
                name("actual_bytes"),
                SdkDiagnosticDetailValue::ByteCount(actual),
            )
            .and_then(|diagnostic| {
                diagnostic.try_with_detail(
                    name("maximum_bytes"),
                    SdkDiagnosticDetailValue::ByteCount(ceiling),
                )
            })
            .expect("batch byte-limit details are bounded")
    } else {
        diagnostic
            .try_with_detail(name("actual"), SdkDiagnosticDetailValue::Count(actual))
            .and_then(|diagnostic| {
                diagnostic
                    .try_with_detail(name("maximum"), SdkDiagnosticDetailValue::Count(ceiling))
            })
            .expect("batch count-limit details are bounded")
    }
}

fn limit_failure_at_row(
    code: &'static str,
    dimension: &'static str,
    actual: u64,
    ceiling: u64,
    ordinal: usize,
) -> SdkExecutionDiagnostic {
    saturated_limit_failure(with_limit_details(
        append_path(
            SdkExecutionDiagnostic::resource_limit(
                sdk_code(code),
                sdk_message("The projected batch exceeds a binding neutral execution limit"),
            ),
            row_path(ordinal),
        ),
        dimension,
        actual,
        ceiling,
    ))
}

fn saturated_limit_failure(diagnostic: SdkExecutionDiagnostic) -> SdkExecutionDiagnostic {
    diagnostic
        .try_with_detail(
            name("actual_saturated"),
            SdkDiagnosticDetailValue::Boolean(true),
        )
        .expect("batch overflow detail is bounded")
}

fn limit_code(dimension: &'static str) -> &'static str {
    match dimension {
        "items" => "batch_item_limit",
        "bytes" => "batch_byte_limit",
        "graph_nodes" => "batch_graph_node_limit",
        "attribute_values" => "batch_attribute_value_limit",
        "collection_members" => "batch_collection_member_limit",
        "role_players" => "batch_role_player_limit",
        "statements" => "batch_statement_limit",
        _ => "batch_resource_limit",
    }
}

fn validate_row_count(row_count: usize, item_ceiling: u64) -> Result<u64, SdkExecutionDiagnostic> {
    let item_count = match u64::try_from(row_count) {
        Ok(item_count) => item_count,
        Err(_) => {
            return Err(saturated_limit_failure(limit_failure(
                "batch_item_limit",
                "items",
                u64::MAX,
                item_ceiling,
            )));
        }
    };
    if item_count > item_ceiling {
        return Err(limit_failure(
            "batch_item_limit",
            "items",
            item_count,
            item_ceiling,
        ));
    }
    Ok(item_count)
}

fn hard_ceiling(dimension: &'static str) -> u64 {
    match dimension {
        "items" => MAX_QUERY_ITEMS,
        "bytes" => MAX_QUERY_BYTES,
        "graph_nodes" => MAX_QUERY_GRAPH_NODES,
        "attribute_values" => MAX_QUERY_ATTRIBUTE_VALUES,
        "collection_members" => MAX_QUERY_COLLECTION_MEMBERS,
        "role_players" => MAX_QUERY_ROLE_PLAYERS,
        "statements" => u64::from(MAX_QUERY_STATEMENTS),
        _ => u64::MAX,
    }
}

fn allocation_failure(_site: ProjectedBatchReservationSite) -> SdkExecutionDiagnostic {
    append_path(
        SdkExecutionDiagnostic::resource_limit(
            sdk_code("projected_batch_allocation_exhausted"),
            sdk_message("The projected batch could not reserve bounded normalization storage"),
        ),
        [SdkDiagnosticPathSegment::Argument(name("rows"))],
    )
}

fn append_path(
    mut diagnostic: SdkExecutionDiagnostic,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    for segment in path {
        diagnostic = diagnostic
            .try_at(segment)
            .expect("batch diagnostic paths are bounded");
    }
    diagnostic
}

fn sdk_code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static batch code is canonical")
}

fn sdk_message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static batch message is valid")
}

fn name(value: &'static str) -> SdkDiagnosticName {
    SdkDiagnosticName::new(value).expect("static batch name is canonical")
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, RoleId};
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
    use type_bridge_contract::schema::{DocumentId, OwnsFactId};
    use type_bridge_contract::value::{CanonicalDouble, CanonicalString};
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

    use crate::projected_model::{ProjectedAttributeValue, ProjectedReference};

    fn installed() -> InstalledRuntimeProjection {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("batch-reservation.yaml").unwrap(),
            r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  note: { value: string }
entities:
  keyed:
    owns:
      identifier: { key: true }
  unkeyed:
    owns:
      note: { card: 1 }
relations:
  membership:
    owns:
      identifier: { key: true }
    relates:
      member: { card: { min: 0, max: 2 } }
plays:
  keyed:
    membership: [member]
"#,
        )])
        .unwrap();
        let resolved = resolve(
            &normalize_documents(&documents).unwrap(),
            &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        )
        .unwrap();
        InstalledRuntimeProjection::try_new(
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
    }

    fn create(
        installed: &InstalledRuntimeProjection,
        model_label: &str,
        attribute: &str,
    ) -> ProjectedCreate {
        let model = TypeId::new(TypeKind::Entity, model_label).unwrap();
        let attribute_id = TypeId::new(TypeKind::Attribute, attribute).unwrap();
        ProjectedCreate::try_new(
            installed,
            model.clone(),
            vec![(
                OwnsFactId::new(model, AttributeId::new(attribute).unwrap()).unwrap(),
                vec![
                    ProjectedAttributeValue::try_new(
                        installed,
                        attribute_id,
                        CanonicalValue::String(CanonicalString::new("value").unwrap()),
                    )
                    .unwrap(),
                ],
            )],
            vec![],
        )
        .unwrap()
    }

    fn relation_create(installed: &InstalledRuntimeProjection) -> ProjectedCreate {
        let keyed = TypeId::new(TypeKind::Entity, "keyed").unwrap();
        let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
        let key = OwnsFactId::new(keyed.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let references = ["0x10", "0x11"]
            .into_iter()
            .map(|iid| {
                ProjectedReference::try_new(
                    installed,
                    keyed.clone(),
                    Some(iid.into()),
                    vec![(key.clone(), {
                        ProjectedAttributeValue::try_new(
                            installed,
                            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
                            CanonicalValue::String(CanonicalString::new(iid).unwrap()),
                        )
                        .unwrap()
                    })],
                )
                .unwrap()
            })
            .collect();
        ProjectedCreate::try_new(
            installed,
            membership.clone(),
            vec![(
                OwnsFactId::new(membership, AttributeId::new("identifier").unwrap()).unwrap(),
                vec![
                    ProjectedAttributeValue::try_new(
                        installed,
                        TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
                        CanonicalValue::String(CanonicalString::new("membership").unwrap()),
                    )
                    .unwrap(),
                ],
            )],
            vec![(RoleId::new("membership", "member").unwrap(), references)],
        )
        .unwrap()
    }

    #[test]
    fn semantic_key_comparison_equates_signed_double_zero() {
        let negative = CanonicalValue::Double(CanonicalDouble::new(-0.0).unwrap());
        let positive = CanonicalValue::Double(CanonicalDouble::new(0.0).unwrap());
        assert_ne!(negative, positive);
        assert!(semantic_equal(&negative, &positive));

        let values = [
            positive,
            CanonicalValue::Double(CanonicalDouble::new(1.0).unwrap()),
            negative,
        ];
        let mut indices = [0, 1, 2];
        indices.sort_unstable_by(|left, right| {
            values[*left]
                .semantic_cmp_same_domain(&values[*right])
                .unwrap()
                .then_with(|| left.cmp(right))
        });
        assert!(
            indices.windows(2).any(|pair| {
                semantic_equal(&values[pair[0]], &values[pair[1]]) && pair == [0, 2]
            })
        );
    }

    #[test]
    fn statement_forecast_covers_every_operation_and_never_exceeds_hard_cap() {
        for operation in [
            ProjectedBatchOperation::Insert,
            ProjectedBatchOperation::Put,
            ProjectedBatchOperation::Update,
            ProjectedBatchOperation::Delete,
        ] {
            for requires_references in [false, true] {
                assert_eq!(statement_forecast(operation, requires_references, 0), 0);
                let expected = match operation {
                    ProjectedBatchOperation::Delete => 1,
                    ProjectedBatchOperation::Put => 3,
                    ProjectedBatchOperation::Insert | ProjectedBatchOperation::Update
                        if requires_references =>
                    {
                        3
                    }
                    ProjectedBatchOperation::Insert | ProjectedBatchOperation::Update => 2,
                };
                let actual = statement_forecast(operation, requires_references, 1);
                assert_eq!(actual, expected);
                assert!(actual <= MAX_QUERY_STATEMENTS);
            }
        }
    }

    #[test]
    fn allocation_sites_are_private_and_publicly_indistinguishable() {
        let diagnostics = [
            ProjectedBatchReservationSite::Rows,
            ProjectedBatchReservationSite::Targets,
            ProjectedBatchReservationSite::Keys,
        ]
        .map(allocation_failure);
        assert!(diagnostics.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(
            diagnostics[0].path(),
            [SdkDiagnosticPathSegment::Argument(name("rows"))]
        );
        assert!(diagnostics[0].details().is_empty());
    }

    #[test]
    fn reservation_probe_fails_each_real_site_and_skips_zero_capacity() {
        let installed = installed();
        let keyed = TypeId::new(TypeKind::Entity, "keyed").unwrap();
        let unkeyed = TypeId::new(TypeKind::Entity, "unkeyed").unwrap();
        let cases = [
            (
                ProjectedBatchReservationSite::Rows,
                keyed.clone(),
                ProjectedBatchOperation::Insert,
                vec![ProjectedBatchRow::Create(create(
                    &installed,
                    "keyed",
                    "identifier",
                ))],
            ),
            (
                ProjectedBatchReservationSite::Keys,
                keyed.clone(),
                ProjectedBatchOperation::Insert,
                vec![ProjectedBatchRow::Create(create(
                    &installed,
                    "keyed",
                    "identifier",
                ))],
            ),
            (
                ProjectedBatchReservationSite::Targets,
                keyed.clone(),
                ProjectedBatchOperation::Delete,
                vec![ProjectedBatchRow::Delete { iid: "0x10".into() }],
            ),
        ];
        for (site, model, operation, rows) in cases {
            let error = ProjectedBatch::try_new_with_reservation_probe(
                &installed,
                model,
                operation,
                rows,
                ProjectedBatchReservationProbe::failing_at(site),
            )
            .unwrap_err();
            assert_eq!(
                error.code().as_str(),
                "projected_batch_allocation_exhausted"
            );
            assert_eq!(
                error.path(),
                [SdkDiagnosticPathSegment::Argument(name("rows"))]
            );
            assert!(error.details().is_empty());
        }

        ProjectedBatch::try_new_with_reservation_probe(
            &installed,
            keyed.clone(),
            ProjectedBatchOperation::Insert,
            vec![ProjectedBatchRow::Create(create(
                &installed,
                "keyed",
                "identifier",
            ))],
            ProjectedBatchReservationProbe::failing_at(ProjectedBatchReservationSite::Targets),
        )
        .unwrap();
        ProjectedBatch::try_new_with_reservation_probe(
            &installed,
            unkeyed,
            ProjectedBatchOperation::Insert,
            vec![ProjectedBatchRow::Create(create(
                &installed, "unkeyed", "note",
            ))],
            ProjectedBatchReservationProbe::failing_at(ProjectedBatchReservationSite::Keys),
        )
        .unwrap();
        for site in [
            ProjectedBatchReservationSite::Rows,
            ProjectedBatchReservationSite::Targets,
            ProjectedBatchReservationSite::Keys,
        ] {
            ProjectedBatch::try_new_with_reservation_probe(
                &installed,
                keyed.clone(),
                ProjectedBatchOperation::Delete,
                vec![],
                ProjectedBatchReservationProbe::failing_at(site),
            )
            .unwrap();
        }
    }

    #[test]
    fn controlled_construction_checks_policy_before_reservation() {
        let installed = installed();
        let keyed = TypeId::new(TypeKind::Entity, "keyed").unwrap();
        let row = || {
            vec![ProjectedBatchRow::Create(create(
                &installed,
                "keyed",
                "identifier",
            ))]
        };
        let cancelled = AnswerCancellation::default();
        cancelled.cancel();
        let cancelled_control = ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::default(),
            cancelled,
        );
        let error = ProjectedBatch::try_new_for_invocation_with_reservation_probe(
            &installed,
            keyed.clone(),
            ProjectedBatchOperation::Insert,
            row(),
            &cancelled_control,
            ProjectedBatchReservationProbe::failing_at(ProjectedBatchReservationSite::Rows),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "provider_cancelled");

        let authority = ProjectedBatch::try_new_for_invocation_with_reservation_probe(
            &installed,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            ProjectedBatchOperation::Delete,
            vec![ProjectedBatchRow::Delete { iid: "0x10".into() }],
            &cancelled_control,
            ProjectedBatchReservationProbe::failing_at(ProjectedBatchReservationSite::Rows),
        )
        .unwrap_err();
        assert_eq!(authority.code().as_str(), "wrong_model_kind");

        let expired = ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::tightened(
                0,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                MAX_QUERY_STATEMENTS,
            ),
            AnswerCancellation::default(),
        );
        let error = ProjectedBatch::try_new_for_invocation_with_reservation_probe(
            &installed,
            keyed.clone(),
            ProjectedBatchOperation::Insert,
            row(),
            &expired,
            ProjectedBatchReservationProbe::failing_at(ProjectedBatchReservationSite::Rows),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "transaction_deadline_exceeded");

        let zero_items = ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::tightened(
                30_000,
                0,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                MAX_QUERY_STATEMENTS,
            ),
            AnswerCancellation::default(),
        );
        let error = ProjectedBatch::try_new_for_invocation_with_reservation_probe(
            &installed,
            keyed.clone(),
            ProjectedBatchOperation::Insert,
            row(),
            &zero_items,
            ProjectedBatchReservationProbe::failing_at(ProjectedBatchReservationSite::Rows),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "batch_item_limit");

        let cancelled = AnswerCancellation::default();
        cancelled.cancel();
        let empty_control = ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::tightened(0, 0, 0, 0, 0, 0, 0, 0),
            cancelled,
        );
        ProjectedBatch::try_new_for_invocation_with_reservation_probe(
            &installed,
            keyed,
            ProjectedBatchOperation::Insert,
            vec![],
            &empty_control,
            ProjectedBatchReservationProbe::failing_at(ProjectedBatchReservationSite::Rows),
        )
        .unwrap();
    }

    #[test]
    fn controlled_construction_checks_each_reference_inside_one_row() {
        let installed = installed();
        let control = ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        );
        control.cancel_after_reference_checks(1);
        let error = ProjectedBatch::try_new_for_invocation(
            &installed,
            TypeId::new(TypeKind::Relation, "membership").unwrap(),
            ProjectedBatchOperation::Insert,
            vec![ProjectedBatchRow::Create(relation_create(&installed))],
            &control,
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "provider_cancelled");
    }

    #[test]
    fn hard_item_cap_precedes_rows_reservation_probe() {
        let installed = installed();
        let rows = vec![
            ProjectedBatchRow::Delete { iid: "0x10".into() };
            usize::try_from(MAX_QUERY_ITEMS).unwrap() + 1
        ];
        let error = ProjectedBatch::try_new_with_reservation_probe(
            &installed,
            TypeId::new(TypeKind::Entity, "keyed").unwrap(),
            ProjectedBatchOperation::Delete,
            rows,
            ProjectedBatchReservationProbe::failing_at(ProjectedBatchReservationSite::Rows),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "batch_item_limit");
    }

    #[test]
    fn checked_resource_overflow_is_row_attributed_and_truthful() {
        let error = add(u64::MAX, 1, "graph_nodes", 2).unwrap_err();
        assert_eq!(error.code().as_str(), "batch_graph_node_limit");
        assert_eq!(error.path(), row_path(2));
        assert_eq!(
            error.details().get(&name("actual")),
            Some(&SdkDiagnosticDetailValue::Count(u64::MAX))
        );
        assert_eq!(
            error.details().get(&name("maximum")),
            Some(&SdkDiagnosticDetailValue::Count(MAX_QUERY_GRAPH_NODES))
        );
        assert_eq!(
            error.details().get(&name("actual_saturated")),
            Some(&SdkDiagnosticDetailValue::Boolean(true))
        );
        assert_eq!(error.details().len(), 3);

        let items = saturated_limit_failure(limit_failure(
            "batch_item_limit",
            "items",
            u64::MAX,
            MAX_QUERY_ITEMS,
        ));
        assert_eq!(
            items.path(),
            [
                SdkDiagnosticPathSegment::Argument(name("limits")),
                SdkDiagnosticPathSegment::Argument(name("items")),
            ]
        );
        assert_eq!(
            items.details().get(&name("actual")),
            Some(&SdkDiagnosticDetailValue::Count(u64::MAX))
        );
        assert_eq!(
            items.details().get(&name("maximum")),
            Some(&SdkDiagnosticDetailValue::Count(MAX_QUERY_ITEMS))
        );
        assert_eq!(
            items.details().get(&name("actual_saturated")),
            Some(&SdkDiagnosticDetailValue::Boolean(true))
        );
        assert_eq!(items.details().len(), 3);
    }
}
