//! Immutable exact-model filters and bounded generated-manager terminals.

use std::cmp::Ordering;
use std::sync::Arc;

use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::ProjectedTokenIdentity;
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticMessage, SdkDiagnosticName,
    SdkDiagnosticPathSegment, SdkExecutionDiagnostic, SdkProviderOperation,
    SdkQueryDiagnosticPathKind,
};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

use crate::execution_diagnostic::{lower_execution_error, lower_match_error};
use crate::match_request::selected_result_executor::ManagerRootSelection;
use crate::match_request::{
    BindingId, ComparisonOp, DescriptorId, MatchExpr, MatchMode, MatchOperation, SessionHandle,
    ValidatedMatchRequest,
};
use crate::projected_model::{ProjectedAttributeValue, ProjectedThing, ProjectionBrand};
use crate::projected_query::{
    ProjectedQueryOrigin, materialize_projected_manager_things_with_budget,
};
use crate::query_execution_limits::{
    MAX_QUERY_ATTRIBUTE_VALUES, MAX_QUERY_BYTES, QueryExecutionDeadline,
    QueryExecutionResourceLimits,
};
use crate::runtime_projection::InstalledRuntimeProjection;
use crate::session::backend::AnswerCancellation;
use crate::session::{Database, TransactionContext};

const MAX_MANAGER_FILTER_PREDICATES: usize = crate::match_request::MAX_BOOLEAN_TERMS;

/// One closed scalar comparison admitted by an exact generated-manager filter.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProjectedManagerComparison {
    /// Canonical scalar equality.
    Eq,
    /// Canonical scalar inequality.
    Ne,
    /// Canonical scalar less-than ordering.
    Lt,
    /// Canonical scalar less-than-or-equal ordering.
    Lte,
    /// Canonical scalar greater-than ordering.
    Gt,
    /// Canonical scalar greater-than-or-equal ordering.
    Gte,
}

impl ProjectedManagerComparison {
    const fn match_operator(self) -> ComparisonOp {
        match self {
            Self::Eq => ComparisonOp::Equal,
            Self::Ne => ComparisonOp::NotEqual,
            Self::Lt => ComparisonOp::LessThan,
            Self::Lte => ComparisonOp::LessThanOrEqual,
            Self::Gt => ComparisonOp::GreaterThan,
            Self::Gte => ComparisonOp::GreaterThanOrEqual,
        }
    }

    fn admits(self, domain: ValueTypeTag) -> bool {
        domain != ValueTypeTag::Boolean || matches!(self, Self::Eq | Self::Ne)
    }
}

/// Allocation-free resource measure for one immutable manager filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectedManagerFilterResourceMeasure {
    items: usize,
    bytes: usize,
    attribute_values: usize,
}

impl ProjectedManagerFilterResourceMeasure {
    /// Return the authored predicate count.
    #[must_use]
    pub const fn items(self) -> usize {
        self.items
    }

    /// Return the exact cached canonical input-byte charge.
    #[must_use]
    pub const fn bytes(self) -> usize {
        self.bytes
    }

    /// Return the exact projected-literal count.
    #[must_use]
    pub const fn attribute_values(self) -> usize {
        self.attribute_values
    }
}

#[derive(Clone, Debug)]
struct ProjectedManagerPredicate {
    field: OwnsFactId,
    comparison: ProjectedManagerComparison,
    value: ProjectedAttributeValue,
}

/// One persistent, exact-model, generated-field filter.
///
/// Every transition returns a new value. Field-token provenance is nominally
/// fenced by generated facades before they pass the resolved
/// [`ProjectedTokenIdentity`]; this common seam independently rechecks exact
/// projection membership, effective ownership, scalar domain, and constraints.
#[derive(Clone, Debug)]
pub struct ProjectedManagerFilter {
    brand: ProjectionBrand,
    model: TypeId,
    predicates: Arc<[ProjectedManagerPredicate]>,
    measure: ProjectedManagerFilterResourceMeasure,
}

impl ProjectedManagerFilter {
    /// Start an empty conjunction for one exact projected entity or relation.
    pub fn try_new(
        installed: &InstalledRuntimeProjection,
        model: TypeId,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        require_manager_model(installed, &model)?;
        Ok(Self {
            brand: ProjectionBrand::from_installed(installed),
            measure: ProjectedManagerFilterResourceMeasure {
                items: 0,
                bytes: type_id_bytes(&model),
                attribute_values: 0,
            },
            model,
            predicates: Arc::from([]),
        })
    }

    /// Append one exact generated-field comparison without mutating this filter.
    pub fn try_and(
        &self,
        installed: &InstalledRuntimeProjection,
        field: &ProjectedTokenIdentity,
        comparison: ProjectedManagerComparison,
        value: &ProjectedAttributeValue,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        self.validate_for(installed)?;
        let index = self.predicates.len();
        if index >= MAX_MANAGER_FILTER_PREDICATES {
            return Err(resource_limit(
                "manager_filter_predicate_limit",
                "The manager filter exceeds the canonical predicate ceiling",
                predicate_collection_path(&self.model),
                u64::try_from(index.saturating_add(1)).unwrap_or(u64::MAX),
                u64::try_from(MAX_MANAGER_FILTER_PREDICATES).unwrap_or(u64::MAX),
                false,
            ));
        }
        let (owner, field_id) = match field {
            ProjectedTokenIdentity::Field { owner, field } => (owner, field),
            ProjectedTokenIdentity::Model(_)
            | ProjectedTokenIdentity::Role { .. }
            | ProjectedTokenIdentity::Function(_)
            | _ => {
                return Err(invalid_input(
                    "manager_filter_requires_field_token",
                    "A manager predicate requires one generated field token",
                    indexed_predicate_path(&self.model, index),
                ));
            }
        };
        let path = indexed_field_path(&self.model, index, field_id);
        if owner != &self.model || field_id.owner() != owner {
            return Err(integrity(
                "field_owner_mismatch",
                "The generated field token belongs to a different exact model owner",
                path,
            ));
        }
        if installed
            .projection()
            .projected_token_ordinal(field)
            .is_none()
        {
            return Err(integrity(
                "field_not_projected",
                "The generated field token is absent from the installed projection",
                path,
            ));
        }
        let model = installed
            .projection()
            .models()
            .get(&self.model)
            .expect("the filter model was revalidated");
        let field_projection = model.query_tokens().fields().get(field_id).ok_or_else(|| {
            integrity(
                "field_not_projected",
                "The generated field token is absent from the selected exact model",
                indexed_field_path(&self.model, index, field_id),
            )
        })?;

        if value.semantic_fingerprint() != installed.projection().semantic_fingerprint()
            || value.binding_target() != installed.projection().target()
            || value.projection_fingerprint() != installed.projection().projection_fingerprint()
        {
            return Err(at_path(
                SdkExecutionDiagnostic::generated_token_package_mismatch(),
                indexed_field_path(&self.model, index, field_id),
            ));
        }
        let expected_attribute = TypeId::new(
            TypeKind::Attribute,
            field_projection.id().attribute().label().as_str(),
        )
        .map_err(|_| SdkExecutionDiagnostic::internal_failure())?;
        if value.attribute_type() != &expected_attribute {
            return Err(invalid_input(
                "wrong_scalar_domain",
                "The projected filter literal belongs to a different field domain",
                indexed_field_path(&self.model, index, field_id),
            ));
        }
        if !comparison.admits(value.value().value_type()) {
            return Err(invalid_input(
                "invalid_operator_for_type",
                "The manager comparison operator is invalid for this scalar domain",
                indexed_field_path(&self.model, index, field_id),
            ));
        }
        installed.validate_canonical_field_value(&self.model, field_id, value.value())?;

        let next_bytes = self
            .measure
            .bytes
            .checked_add(owns_id_bytes(field_id))
            .and_then(|bytes| bytes.checked_add(1))
            .and_then(|bytes| bytes.checked_add(value.resource_measure().bytes()))
            .ok_or_else(|| {
                resource_limit(
                    "manager_filter_byte_limit",
                    "The manager filter byte counter overflowed",
                    predicate_collection_path(&self.model),
                    u64::MAX,
                    MAX_QUERY_BYTES,
                    true,
                )
            })?;
        if u64::try_from(next_bytes).unwrap_or(u64::MAX) > MAX_QUERY_BYTES {
            return Err(resource_limit(
                "manager_filter_byte_limit",
                "The manager filter exceeds the canonical byte ceiling",
                predicate_collection_path(&self.model),
                u64::try_from(next_bytes).unwrap_or(u64::MAX),
                MAX_QUERY_BYTES,
                true,
            ));
        }
        let next_attribute_values = self.measure.attribute_values.saturating_add(1);
        if u64::try_from(next_attribute_values).unwrap_or(u64::MAX) > MAX_QUERY_ATTRIBUTE_VALUES {
            return Err(resource_limit(
                "manager_filter_attribute_value_limit",
                "The manager filter exceeds the canonical literal ceiling",
                predicate_collection_path(&self.model),
                u64::try_from(next_attribute_values).unwrap_or(u64::MAX),
                MAX_QUERY_ATTRIBUTE_VALUES,
                false,
            ));
        }
        let mut predicates = Vec::new();
        predicates
            .try_reserve_exact(index.saturating_add(1))
            .map_err(|_| {
                resource_exhausted(
                    "manager_filter_allocation_exhausted",
                    "The manager filter could not reserve bounded predicate storage",
                    predicate_collection_path(&self.model),
                )
            })?;
        predicates.extend(self.predicates.iter().cloned());
        predicates.push(ProjectedManagerPredicate {
            field: field_id.clone(),
            comparison,
            value: value.clone(),
        });
        Ok(Self {
            brand: self.brand.clone(),
            model: self.model.clone(),
            predicates: Arc::from(predicates),
            measure: ProjectedManagerFilterResourceMeasure {
                items: index + 1,
                bytes: next_bytes,
                attribute_values: next_attribute_values,
            },
        })
    }

    /// Return the exact model selected by this filter.
    #[must_use]
    pub const fn model(&self) -> &TypeId {
        &self.model
    }

    /// Return the authored predicate count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.predicates.len()
    }

    /// Return whether this filter is the empty conjunction.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.predicates.is_empty()
    }

    /// Return the cached exact input-resource measure.
    #[must_use]
    pub const fn resource_measure(&self) -> ProjectedManagerFilterResourceMeasure {
        self.measure
    }

    /// Revalidate this filter against one installed runtime projection.
    pub fn validate_for(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.brand.validate(installed, type_path(&self.model))?;
        require_manager_model(installed, &self.model)?;
        for (index, predicate) in self.predicates.iter().enumerate() {
            let identity = ProjectedTokenIdentity::Field {
                owner: self.model.clone(),
                field: predicate.field.clone(),
            };
            if installed
                .projection()
                .projected_token_ordinal(&identity)
                .is_none()
            {
                return Err(integrity(
                    "field_not_projected",
                    "The manager filter contains a field absent from the installed projection",
                    indexed_field_path(&self.model, index, &predicate.field),
                ));
            }
            if predicate.field.owner() != &self.model {
                return Err(integrity(
                    "field_owner_mismatch",
                    "The manager filter contains a field owned by a different exact model",
                    indexed_field_path(&self.model, index, &predicate.field),
                ));
            }
            predicate.value.validate_for(installed)?;
            if !predicate
                .comparison
                .admits(predicate.value.value().value_type())
            {
                return Err(invalid_input(
                    "invalid_operator_for_type",
                    "The manager comparison operator is invalid for this scalar domain",
                    indexed_field_path(&self.model, index, &predicate.field),
                ));
            }
            installed.validate_canonical_field_value(
                &self.model,
                &predicate.field,
                predicate.value.value(),
            )?;
        }
        Ok(())
    }

    /// Reconstruct the strict manager subset from one canonically validated
    /// exact single-model `CountBy` query.
    #[doc(hidden)]
    pub fn try_from_validated_exact_query(
        installed: &InstalledRuntimeProjection,
        validated: &ValidatedMatchRequest,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let request = validated.request();
        let [binding] = request.plan.bindings.as_slice() else {
            return Err(manager_query_shape_invalid());
        };
        let MatchOperation::CountBy { root } = request.operation else {
            return Err(manager_query_shape_invalid());
        };
        if root != binding.id
            || binding.match_mode != MatchMode::Exact
            || !request.plan.allowed_cross_joins.is_empty()
        {
            return Err(manager_query_shape_invalid());
        }
        let model = descriptor_type_id(&binding.descriptor, binding.thing_kind)
            .ok_or_else(manager_query_shape_invalid)?;
        let mut filter = Self::try_new(installed, model.clone())?;
        if let Some(expression) = request.plan.predicate.as_ref() {
            append_validated_manager_expression(
                installed,
                &model,
                &binding.descriptor,
                root,
                expression,
                &mut filter,
            )?;
        }
        Ok(filter)
    }

    fn require_first_identity(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<(), SdkExecutionDiagnostic> {
        let model = installed
            .projection()
            .models()
            .get(&self.model)
            .expect("the filter model was validated");
        let keys = model.reference_read().key_fields();
        if keys.is_empty() {
            return Err(invalid_input(
                "manager_first_requires_identity",
                "Manager first requires equality evidence for every reference-key field",
                type_path(&self.model),
            ));
        }
        for key in keys {
            let mut selected: Option<(&CanonicalValue, usize)> = None;
            for (index, predicate) in self.predicates.iter().enumerate() {
                if &predicate.field != key || predicate.comparison != ProjectedManagerComparison::Eq
                {
                    continue;
                }
                if let Some((value, _)) = selected {
                    if !manager_values_semantically_equal(value, predicate.value.value()) {
                        return Err(invalid_input(
                            "manager_first_requires_identity",
                            "Manager first has conflicting equality values for one reference key",
                            indexed_field_path(&self.model, index, key),
                        ));
                    }
                } else {
                    selected = Some((predicate.value.value(), index));
                }
            }
            if selected.is_none() {
                return Err(invalid_input(
                    "manager_first_requires_identity",
                    "Manager first is missing equality evidence for one reference key",
                    vec![
                        SdkDiagnosticPathSegment::Type(self.model.clone()),
                        SdkDiagnosticPathSegment::Field(key.clone()),
                    ],
                ));
            }
        }
        Ok(())
    }

    fn check_terminal_resources(
        &self,
        limits: QueryExecutionResourceLimits,
    ) -> Result<(), SdkExecutionDiagnostic> {
        let limits = limits.effective();
        let items = u64::try_from(self.measure.items).unwrap_or(u64::MAX);
        if items > limits.items {
            return Err(resource_limit(
                "manager_filter_predicate_limit",
                "The manager filter exceeds the captured item ceiling",
                predicate_collection_path(&self.model),
                items,
                limits.items,
                false,
            ));
        }
        let bytes = u64::try_from(self.measure.bytes).unwrap_or(u64::MAX);
        if bytes > limits.bytes {
            return Err(resource_limit(
                "manager_filter_byte_limit",
                "The manager filter exceeds the captured byte ceiling",
                predicate_collection_path(&self.model),
                bytes,
                limits.bytes,
                true,
            ));
        }
        let values = u64::try_from(self.measure.attribute_values).unwrap_or(u64::MAX);
        if values > limits.attribute_values {
            return Err(resource_limit(
                "manager_filter_attribute_value_limit",
                "The manager filter exceeds the captured attribute-value ceiling",
                predicate_collection_path(&self.model),
                values,
                limits.attribute_values,
                false,
            ));
        }
        Ok(())
    }
}

fn append_validated_manager_expression(
    installed: &InstalledRuntimeProjection,
    model: &TypeId,
    binding_descriptor: &DescriptorId,
    root: BindingId,
    expression: &MatchExpr,
    filter: &mut ProjectedManagerFilter,
) -> Result<(), SdkExecutionDiagnostic> {
    match expression {
        MatchExpr::And { expressions } => {
            for expression in expressions {
                append_validated_manager_expression(
                    installed,
                    model,
                    binding_descriptor,
                    root,
                    expression,
                    filter,
                )?;
            }
            Ok(())
        }
        MatchExpr::FieldValue {
            field,
            operator,
            value,
        } => {
            if field.binding != root || &field.field.owner != binding_descriptor {
                return Err(manager_query_shape_invalid());
            }
            let model_projection = installed
                .projection()
                .models()
                .get(model)
                .ok_or_else(manager_query_shape_invalid)?;
            let field_projection = model_projection
                .query_tokens()
                .fields()
                .values()
                .find(|candidate| candidate.target_name().as_str() == field.field.name)
                .ok_or_else(manager_query_shape_invalid)?;
            let comparison =
                comparison_from_match(*operator).ok_or_else(manager_query_shape_invalid)?;
            let attribute_type = TypeId::new(
                TypeKind::Attribute,
                field_projection.id().attribute().label().as_str(),
            )
            .map_err(|_| SdkExecutionDiagnostic::internal_failure())?;
            let projected = ProjectedAttributeValue::try_from_attribute_value(
                installed,
                attribute_type,
                value,
            )?;
            *filter = filter.try_and(
                installed,
                &ProjectedTokenIdentity::Field {
                    owner: model.clone(),
                    field: field_projection.id().clone(),
                },
                comparison,
                &projected,
            )?;
            Ok(())
        }
        _ => Err(manager_query_shape_invalid()),
    }
}

/// One manager-terminal policy captured before semantic terminal construction.
#[doc(hidden)]
pub struct ProjectedManagerFilterInvocationControl {
    limits: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
}

impl ProjectedManagerFilterInvocationControl {
    /// Capture effective limits, one absolute deadline, and cancellation owner.
    #[must_use]
    pub fn capture(limits: QueryExecutionResourceLimits, cancellation: AnswerCancellation) -> Self {
        let limits = limits.effective();
        Self {
            limits,
            deadline: QueryExecutionDeadline::for_limits(limits),
            cancellation,
        }
    }

    /// Preserve an absolute deadline captured by an enclosing binding
    /// terminal while clamping its effective common resource policy.
    #[doc(hidden)]
    #[must_use]
    pub fn from_parts(
        limits: QueryExecutionResourceLimits,
        deadline: QueryExecutionDeadline,
        cancellation: AnswerCancellation,
    ) -> Self {
        Self {
            limits: limits.effective(),
            deadline,
            cancellation,
        }
    }

    /// Recheck cancellation and deadline at one exact model path.
    pub fn checkpoint(&self, model: &TypeId) -> Result<(), SdkExecutionDiagnostic> {
        self.deadline
            .check(&self.cancellation)
            .map_err(|diagnostic| at_path(diagnostic, type_path(model)))
    }

    fn constrained_by(mut self, ceiling: Option<QueryExecutionResourceLimits>) -> Self {
        if let Some(ceiling) = ceiling {
            self.limits = self.limits.constrained_by(ceiling);
        }
        self
    }
}

/// Binding-neutral executor for exact generated-manager filters.
pub struct ProjectedManagerFilterExecutor<'projection> {
    installed: &'projection InstalledRuntimeProjection,
}

impl<'projection> ProjectedManagerFilterExecutor<'projection> {
    /// Create an executor over one verified installed runtime projection.
    #[must_use]
    pub const fn new(installed: &'projection InstalledRuntimeProjection) -> Self {
        Self { installed }
    }

    /// Return all distinct exact-model matches in an owned read transaction.
    pub async fn all(
        &self,
        database: &Database,
        filter: &ProjectedManagerFilter,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Vec<Arc<ProjectedThing>>, SdkExecutionDiagnostic> {
        self.all_with_control(
            database,
            filter,
            ProjectedManagerFilterInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Return all matches using one pre-captured terminal policy.
    #[doc(hidden)]
    pub async fn all_with_control(
        &self,
        database: &Database,
        filter: &ProjectedManagerFilter,
        control: ProjectedManagerFilterInvocationControl,
    ) -> Result<Vec<Arc<ProjectedThing>>, SdkExecutionDiagnostic> {
        let control = control.constrained_by(database.answer_limits());
        self.preflight(filter, &control, false)?;
        let (registry, request) = self.build_request(filter, false)?;
        let hydrated = database
            .execute_manager_roots_with_limits(
                &registry,
                &request,
                ManagerRootSelection::All,
                control
                    .limits
                    .direct_with_deadline(control.cancellation.clone(), control.deadline),
            )
            .await
            .map_err(|error| lower_manager_execution(error, filter.model()))?;
        materialize_projected_manager_things_with_budget(
            self.installed,
            &registry,
            ProjectedQueryOrigin::for_database(database),
            filter.model(),
            &hydrated.into_things(),
            control.limits.projected(),
            &control.cancellation,
            Some(control.deadline),
        )
    }

    /// Return all matches without consuming one caller-owned read transaction.
    pub async fn all_in_read_transaction(
        &self,
        transaction: &TransactionContext,
        filter: &ProjectedManagerFilter,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Vec<Arc<ProjectedThing>>, SdkExecutionDiagnostic> {
        self.all_in_read_transaction_with_control(
            transaction,
            filter,
            ProjectedManagerFilterInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Borrowed counterpart of [`Self::all_with_control`].
    #[doc(hidden)]
    pub async fn all_in_read_transaction_with_control(
        &self,
        transaction: &TransactionContext,
        filter: &ProjectedManagerFilter,
        control: ProjectedManagerFilterInvocationControl,
    ) -> Result<Vec<Arc<ProjectedThing>>, SdkExecutionDiagnostic> {
        let control = control.constrained_by(transaction.answer_limits());
        self.preflight(filter, &control, false)?;
        let (registry, request) = self.build_request(filter, false)?;
        let hydrated = transaction
            .execute_manager_roots_with_limits(
                &registry,
                &request,
                ManagerRootSelection::All,
                control
                    .limits
                    .direct_with_deadline(control.cancellation.clone(), control.deadline),
            )
            .await
            .map_err(|error| lower_manager_execution(error, filter.model()))?;
        materialize_projected_manager_things_with_budget(
            self.installed,
            &registry,
            ProjectedQueryOrigin::for_transaction(transaction)?,
            filter.model(),
            &hydrated.into_things(),
            control.limits.projected(),
            &control.cancellation,
            Some(control.deadline),
        )
    }

    /// Return the optional identity-proven first match in an owned read transaction.
    pub async fn first(
        &self,
        database: &Database,
        filter: &ProjectedManagerFilter,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Option<Arc<ProjectedThing>>, SdkExecutionDiagnostic> {
        self.first_with_control(
            database,
            filter,
            ProjectedManagerFilterInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Return first using one pre-captured terminal policy.
    #[doc(hidden)]
    pub async fn first_with_control(
        &self,
        database: &Database,
        filter: &ProjectedManagerFilter,
        control: ProjectedManagerFilterInvocationControl,
    ) -> Result<Option<Arc<ProjectedThing>>, SdkExecutionDiagnostic> {
        let control = control.constrained_by(database.answer_limits());
        self.preflight(filter, &control, true)?;
        let (registry, request) = self.build_request(filter, false)?;
        let hydrated = database
            .execute_manager_roots_with_limits(
                &registry,
                &request,
                ManagerRootSelection::First,
                control
                    .limits
                    .direct_with_deadline(control.cancellation.clone(), control.deadline),
            )
            .await
            .map_err(|error| lower_manager_execution(error, filter.model()))?;
        let projected = materialize_projected_manager_things_with_budget(
            self.installed,
            &registry,
            ProjectedQueryOrigin::for_database(database),
            filter.model(),
            &hydrated.into_things(),
            control.limits.projected(),
            &control.cancellation,
            Some(control.deadline),
        )?;
        optional_manager_first(projected)
    }

    /// Return first without consuming one caller-owned read transaction.
    pub async fn first_in_read_transaction(
        &self,
        transaction: &TransactionContext,
        filter: &ProjectedManagerFilter,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<Option<Arc<ProjectedThing>>, SdkExecutionDiagnostic> {
        self.first_in_read_transaction_with_control(
            transaction,
            filter,
            ProjectedManagerFilterInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Borrowed counterpart of [`Self::first_with_control`].
    #[doc(hidden)]
    pub async fn first_in_read_transaction_with_control(
        &self,
        transaction: &TransactionContext,
        filter: &ProjectedManagerFilter,
        control: ProjectedManagerFilterInvocationControl,
    ) -> Result<Option<Arc<ProjectedThing>>, SdkExecutionDiagnostic> {
        let control = control.constrained_by(transaction.answer_limits());
        self.preflight(filter, &control, true)?;
        let (registry, request) = self.build_request(filter, false)?;
        let hydrated = transaction
            .execute_manager_roots_with_limits(
                &registry,
                &request,
                ManagerRootSelection::First,
                control
                    .limits
                    .direct_with_deadline(control.cancellation.clone(), control.deadline),
            )
            .await
            .map_err(|error| lower_manager_execution(error, filter.model()))?;
        let projected = materialize_projected_manager_things_with_budget(
            self.installed,
            &registry,
            ProjectedQueryOrigin::for_transaction(transaction)?,
            filter.model(),
            &hydrated.into_things(),
            control.limits.projected(),
            &control.cancellation,
            Some(control.deadline),
        )?;
        optional_manager_first(projected)
    }

    /// Count all distinct exact-model matches in an owned read transaction.
    pub async fn count(
        &self,
        database: &Database,
        filter: &ProjectedManagerFilter,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        self.count_with_control(
            database,
            filter,
            ProjectedManagerFilterInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Count using one pre-captured terminal policy.
    #[doc(hidden)]
    pub async fn count_with_control(
        &self,
        database: &Database,
        filter: &ProjectedManagerFilter,
        control: ProjectedManagerFilterInvocationControl,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        let control = control.constrained_by(database.answer_limits());
        self.preflight(filter, &control, false)?;
        let (registry, request) = self.build_request(filter, false)?;
        let result = database
            .execute_match_with_limits(
                &registry,
                &request,
                control
                    .limits
                    .direct_with_deadline(control.cancellation.clone(), control.deadline),
            )
            .await
            .map_err(|error| lower_execution_error(error, SdkProviderOperation::Read))?;
        let canonical = result
            .for_request(&request)
            .map_err(|error| lower_match_error(&error))?;
        let crate::match_request::MatchResult::Count { value, .. } = canonical else {
            return Err(SdkExecutionDiagnostic::internal_failure());
        };
        Ok(*value)
    }

    /// Count without consuming one caller-owned read transaction.
    pub async fn count_in_read_transaction(
        &self,
        transaction: &TransactionContext,
        filter: &ProjectedManagerFilter,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        self.count_in_read_transaction_with_control(
            transaction,
            filter,
            ProjectedManagerFilterInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Borrowed counterpart of [`Self::count_with_control`].
    #[doc(hidden)]
    pub async fn count_in_read_transaction_with_control(
        &self,
        transaction: &TransactionContext,
        filter: &ProjectedManagerFilter,
        control: ProjectedManagerFilterInvocationControl,
    ) -> Result<u64, SdkExecutionDiagnostic> {
        let control = control.constrained_by(transaction.answer_limits());
        self.preflight(filter, &control, false)?;
        let (registry, request) = self.build_request(filter, false)?;
        let result = transaction
            .execute_match_with_limits(
                &registry,
                &request,
                control
                    .limits
                    .direct_with_deadline(control.cancellation.clone(), control.deadline),
            )
            .await
            .map_err(|error| lower_execution_error(error, SdkProviderOperation::Read))?;
        let canonical = result
            .for_request(&request)
            .map_err(|error| lower_match_error(&error))?;
        let crate::match_request::MatchResult::Count { value, .. } = canonical else {
            return Err(SdkExecutionDiagnostic::internal_failure());
        };
        Ok(*value)
    }

    /// Test whether any exact-model match exists in an owned read transaction.
    pub async fn exists(
        &self,
        database: &Database,
        filter: &ProjectedManagerFilter,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<bool, SdkExecutionDiagnostic> {
        self.exists_with_control(
            database,
            filter,
            ProjectedManagerFilterInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Test existence using one pre-captured terminal policy.
    #[doc(hidden)]
    pub async fn exists_with_control(
        &self,
        database: &Database,
        filter: &ProjectedManagerFilter,
        control: ProjectedManagerFilterInvocationControl,
    ) -> Result<bool, SdkExecutionDiagnostic> {
        let control = control.constrained_by(database.answer_limits());
        self.preflight(filter, &control, false)?;
        let (registry, request) = self.build_request(filter, true)?;
        let result = database
            .execute_match_with_limits(
                &registry,
                &request,
                control
                    .limits
                    .direct_with_deadline(control.cancellation.clone(), control.deadline),
            )
            .await
            .map_err(|error| lower_execution_error(error, SdkProviderOperation::Read))?;
        let canonical = result
            .for_request(&request)
            .map_err(|error| lower_match_error(&error))?;
        let crate::match_request::MatchResult::Exists { value, .. } = canonical else {
            return Err(SdkExecutionDiagnostic::internal_failure());
        };
        Ok(*value)
    }

    /// Test existence without consuming one caller-owned read transaction.
    pub async fn exists_in_read_transaction(
        &self,
        transaction: &TransactionContext,
        filter: &ProjectedManagerFilter,
        limits: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<bool, SdkExecutionDiagnostic> {
        self.exists_in_read_transaction_with_control(
            transaction,
            filter,
            ProjectedManagerFilterInvocationControl::capture(limits, cancellation),
        )
        .await
    }

    /// Borrowed counterpart of [`Self::exists_with_control`].
    #[doc(hidden)]
    pub async fn exists_in_read_transaction_with_control(
        &self,
        transaction: &TransactionContext,
        filter: &ProjectedManagerFilter,
        control: ProjectedManagerFilterInvocationControl,
    ) -> Result<bool, SdkExecutionDiagnostic> {
        let control = control.constrained_by(transaction.answer_limits());
        self.preflight(filter, &control, false)?;
        let (registry, request) = self.build_request(filter, true)?;
        let result = transaction
            .execute_match_with_limits(
                &registry,
                &request,
                control
                    .limits
                    .direct_with_deadline(control.cancellation.clone(), control.deadline),
            )
            .await
            .map_err(|error| lower_execution_error(error, SdkProviderOperation::Read))?;
        let canonical = result
            .for_request(&request)
            .map_err(|error| lower_match_error(&error))?;
        let crate::match_request::MatchResult::Exists { value, .. } = canonical else {
            return Err(SdkExecutionDiagnostic::internal_failure());
        };
        Ok(*value)
    }

    fn preflight(
        &self,
        filter: &ProjectedManagerFilter,
        control: &ProjectedManagerFilterInvocationControl,
        first: bool,
    ) -> Result<(), SdkExecutionDiagnostic> {
        control.checkpoint(filter.model())?;
        filter.validate_for(self.installed)?;
        filter.check_terminal_resources(control.limits)?;
        if first {
            filter.require_first_identity(self.installed)?;
        }
        control.checkpoint(filter.model())
    }

    fn build_request(
        &self,
        filter: &ProjectedManagerFilter,
        exists: bool,
    ) -> Result<
        (
            Arc<crate::_registry::DescriptorRegistry>,
            ValidatedMatchRequest,
        ),
        SdkExecutionDiagnostic,
    > {
        let registry = Arc::new(
            self.installed
                .match_registry()
                .map_err(|error| lower_execution_error(error, SdkProviderOperation::Read))?,
        );
        let session = SessionHandle::new(Arc::clone(&registry));
        let root = session
            .exact(filter.model.label().as_str())
            .map_err(|error| lower_execution_error(error, SdkProviderOperation::Read))?;
        let shape = session
            .positional([root.one()])
            .map_err(|error| lower_execution_error(error, SdkProviderOperation::Read))?;
        let query = session
            .query(shape)
            .map_err(|error| lower_execution_error(error, SdkProviderOperation::Read))?;
        let query = query
            .where_predicates_in_order(self.build_predicates(filter, &root)?)
            .map_err(|error| lower_execution_error(error, SdkProviderOperation::Read))?;
        let request = if exists {
            query.validate_exists_by(&root)
        } else {
            query.validate_count_by(&root)
        }
        .map_err(|error| lower_execution_error(error, SdkProviderOperation::Read))?;
        Ok((registry, request))
    }

    fn build_predicates(
        &self,
        filter: &ProjectedManagerFilter,
        root: &crate::match_request::BindingHandle,
    ) -> Result<Vec<crate::match_request::PredicateHandle>, SdkExecutionDiagnostic> {
        let model = self
            .installed
            .projection()
            .models()
            .get(filter.model())
            .expect("manager preflight validated the model");
        let mut predicates = Vec::new();
        predicates
            .try_reserve_exact(filter.predicates.len())
            .map_err(|_| {
                resource_exhausted(
                    "manager_filter_allocation_exhausted",
                    "The manager terminal could not reserve bounded predicate storage",
                    predicate_collection_path(filter.model()),
                )
            })?;
        for predicate in filter.predicates.iter() {
            let projected = model
                .query_tokens()
                .fields()
                .get(&predicate.field)
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            let field = root
                .field(projected.target_name().as_str())
                .map_err(|error| lower_execution_error(error, SdkProviderOperation::Read))?;
            predicates.push(field.compare_value(
                predicate.comparison.match_operator(),
                predicate.value.to_attribute_value(),
            ));
        }
        Ok(predicates)
    }
}

fn optional_manager_first(
    mut values: Vec<Arc<ProjectedThing>>,
) -> Result<Option<Arc<ProjectedThing>>, SdkExecutionDiagnostic> {
    match values.len() {
        0 => Ok(None),
        1 => Ok(values.pop()),
        _ => Err(SdkExecutionDiagnostic::internal_failure()),
    }
}

fn lower_manager_execution(error: crate::OrmError, model: &TypeId) -> SdkExecutionDiagnostic {
    if let crate::OrmError::Match(match_error) = &error
        && match_error.code().as_str() == "manager_first_identity_not_unique"
    {
        return integrity(
            "manager_first_identity_not_unique",
            "Identity-proven manager first matched multiple exact-model IIDs",
            vec![
                SdkDiagnosticPathSegment::Type(model.clone()),
                SdkDiagnosticPathSegment::Query(SdkQueryDiagnosticPathKind::ProviderEvidence),
            ],
        )
        .try_with_detail(
            sdk_name("identity_count"),
            SdkDiagnosticDetailValue::Count(2),
        )
        .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure());
    }
    lower_execution_error(error, SdkProviderOperation::Read)
}

fn require_manager_model(
    installed: &InstalledRuntimeProjection,
    model: &TypeId,
) -> Result<(), SdkExecutionDiagnostic> {
    if !matches!(model.kind(), TypeKind::Entity | TypeKind::Relation) {
        return Err(invalid_input(
            "wrong_model_kind",
            "Generated managers require an entity or relation model",
            type_path(model),
        ));
    }
    if !installed.projection().models().contains_key(model) {
        return Err(invalid_input(
            "model_not_projected",
            "The manager model is absent from the installed runtime projection",
            type_path(model),
        ));
    }
    Ok(())
}

fn comparison_from_match(operator: ComparisonOp) -> Option<ProjectedManagerComparison> {
    match operator {
        ComparisonOp::Equal => Some(ProjectedManagerComparison::Eq),
        ComparisonOp::NotEqual => Some(ProjectedManagerComparison::Ne),
        ComparisonOp::LessThan => Some(ProjectedManagerComparison::Lt),
        ComparisonOp::LessThanOrEqual => Some(ProjectedManagerComparison::Lte),
        ComparisonOp::GreaterThan => Some(ProjectedManagerComparison::Gt),
        ComparisonOp::GreaterThanOrEqual => Some(ProjectedManagerComparison::Gte),
        ComparisonOp::Contains
        | ComparisonOp::StartsWith
        | ComparisonOp::EndsWith
        | ComparisonOp::Regex => None,
    }
}

fn manager_values_semantically_equal(left: &CanonicalValue, right: &CanonicalValue) -> bool {
    match (left, right) {
        (CanonicalValue::Duration(left), CanonicalValue::Duration(right)) => left == right,
        _ => left.semantic_cmp_same_domain(right) == Some(Ordering::Equal),
    }
}

fn descriptor_type_id(
    descriptor: &crate::match_request::DescriptorId,
    kind: crate::match_request::ThingKind,
) -> Option<TypeId> {
    let (prefix, label) = descriptor.as_str().split_once(':')?;
    let expected = match kind {
        crate::match_request::ThingKind::Entity => ("entity", TypeKind::Entity),
        crate::match_request::ThingKind::Relation => ("relation", TypeKind::Relation),
    };
    (prefix == expected.0)
        .then(|| TypeId::new(expected.1, label).ok())
        .flatten()
}

fn type_id_bytes(value: &TypeId) -> usize {
    value.label().as_str().len().saturating_add(1)
}

fn owns_id_bytes(value: &OwnsFactId) -> usize {
    type_id_bytes(value.owner())
        .saturating_add(value.attribute().label().as_str().len())
        .saturating_add(1)
}

fn type_path(model: &TypeId) -> Vec<SdkDiagnosticPathSegment> {
    vec![SdkDiagnosticPathSegment::Type(model.clone())]
}

fn predicate_collection_path(model: &TypeId) -> Vec<SdkDiagnosticPathSegment> {
    vec![
        SdkDiagnosticPathSegment::Type(model.clone()),
        SdkDiagnosticPathSegment::Argument(sdk_name("predicates")),
    ]
}

fn indexed_predicate_path(model: &TypeId, index: usize) -> Vec<SdkDiagnosticPathSegment> {
    vec![
        SdkDiagnosticPathSegment::Type(model.clone()),
        SdkDiagnosticPathSegment::Argument(sdk_name("predicates")),
        SdkDiagnosticPathSegment::Index(u64::try_from(index).unwrap_or(u64::MAX)),
    ]
}

fn indexed_field_path(
    model: &TypeId,
    index: usize,
    field: &OwnsFactId,
) -> Vec<SdkDiagnosticPathSegment> {
    let mut path = indexed_predicate_path(model, index);
    path.push(SdkDiagnosticPathSegment::Field(field.clone()));
    path
}

fn manager_query_shape_invalid() -> SdkExecutionDiagnostic {
    invalid_input(
        "manager_filter_query_shape_invalid",
        "Only one exact selected model and an authored conjunction of field literals can become a manager filter",
        vec![SdkDiagnosticPathSegment::Query(
            SdkQueryDiagnosticPathKind::Plan,
        )],
    )
}

fn invalid_input(
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    at_path(
        SdkExecutionDiagnostic::invalid_input(sdk_code(code), sdk_message(message)),
        path,
    )
}

fn integrity(
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    at_path(
        SdkExecutionDiagnostic::integrity(sdk_code(code), sdk_message(message)),
        path,
    )
}

fn resource_exhausted(
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    at_path(
        SdkExecutionDiagnostic::resource_limit(sdk_code(code), sdk_message(message)),
        path,
    )
}

fn resource_limit(
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
    actual: u64,
    maximum: u64,
    bytes: bool,
) -> SdkExecutionDiagnostic {
    let diagnostic = resource_exhausted(code, message, path);
    let (actual_name, maximum_name, actual, maximum) = if bytes {
        (
            "actual_bytes",
            "maximum_bytes",
            SdkDiagnosticDetailValue::ByteCount(actual),
            SdkDiagnosticDetailValue::ByteCount(maximum),
        )
    } else {
        (
            "actual",
            "maximum",
            SdkDiagnosticDetailValue::Count(actual),
            SdkDiagnosticDetailValue::Count(maximum),
        )
    };
    diagnostic
        .try_with_detail(sdk_name(actual_name), actual)
        .and_then(|diagnostic| diagnostic.try_with_detail(sdk_name(maximum_name), maximum))
        .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
}

fn at_path(
    mut diagnostic: SdkExecutionDiagnostic,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    for segment in path {
        diagnostic = match diagnostic.try_at(segment) {
            Ok(diagnostic) => diagnostic,
            Err(_) => return SdkExecutionDiagnostic::internal_failure(),
        };
    }
    diagnostic
}

fn sdk_code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static manager-filter code is canonical")
}

fn sdk_message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static manager-filter message is canonical")
}

fn sdk_name(value: &'static str) -> SdkDiagnosticName {
    SdkDiagnosticName::new(value).expect("static manager-filter name is canonical")
}
