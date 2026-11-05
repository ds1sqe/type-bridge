//! Canonical generated manager filters over projected models.

use std::marker::PhantomData;

use type_bridge_orm::session::context::TransactionContext;
use type_bridge_orm::{
    AnswerCancellation, InstalledRuntimeProjection, ProjectedManagerComparison,
    ProjectedManagerFilter, ProjectedManagerFilterExecutor,
    ProjectedManagerFilterInvocationControl, QueryExecutionResourceLimits, QueryTarget,
    QueryTargetError,
};

use crate::__codegen::{CompleteModel, EntityModel, FieldToken, Model, QueryValued, RelationModel};
use crate::__codegen::{EncodedCreate, IntoEncodedCreate};
use crate::entity_codec::resolve_entity_authority;
use crate::error::{Error, ModelValidationPhase};
use crate::hooks::{CrudOperation, HookRunner, HookState, ModelKind};
use crate::manager_target::ManagerTarget;
use crate::projected_codec::{
    manager_filter_error, materialize_projected, project_manager_predicate,
};
use crate::relation_codec::resolve_relation_authority;
use crate::schema::Schema;
use crate::{Iid, Result, WriteResult};

struct ProjectedFilterCore<'target> {
    installed: &'target InstalledRuntimeProjection,
    target: QueryTarget<'target>,
    filter: ProjectedManagerFilter,
    hooks: HookRunner,
}

impl<'target> ProjectedFilterCore<'target> {
    fn control(&self) -> ProjectedManagerFilterInvocationControl {
        manager_read_control(self.target)
    }

    fn try_new(
        installed: &'target InstalledRuntimeProjection,
        target: QueryTarget<'target>,
        model: type_bridge_contract::id::TypeId,
    ) -> Result<Self> {
        let filter =
            ProjectedManagerFilter::try_new(installed, model).map_err(manager_filter_error)?;
        Ok(Self {
            installed,
            target,
            filter,
            hooks: HookRunner::default(),
        })
    }

    fn try_and<S, M, V>(
        &self,
        field: FieldToken<M, V>,
        operator: ProjectedManagerComparison,
        value: &V,
    ) -> Result<Self>
    where
        S: Schema,
        M: Model<Schema = S>,
        V: Model<Schema = S> + QueryValued,
    {
        let (field, value) = project_manager_predicate::<S, M, V>(
            self.installed,
            self.filter.model(),
            field,
            value,
        )?;
        let filter = self
            .filter
            .try_and(self.installed, &field, operator, &value)
            .map_err(manager_filter_error)?;
        Ok(Self {
            installed: self.installed,
            target: self.target,
            hooks: self.hooks.clone(),
            filter,
        })
    }

    fn try_and_iid(&self, iid: &Iid) -> Result<Self> {
        Ok(Self {
            installed: self.installed,
            target: self.target,
            hooks: self.hooks.clone(),
            filter: self
                .filter
                .try_and_iid(self.installed, iid)
                .map_err(manager_filter_error)?,
        })
    }

    fn try_and_iids(&self, iids: &[Iid]) -> Result<Self> {
        Ok(Self {
            installed: self.installed,
            target: self.target,
            hooks: self.hooks.clone(),
            filter: self
                .filter
                .try_and_iids(self.installed, iids)
                .map_err(manager_filter_error)?,
        })
    }

    async fn pre_hooks<M: CompleteModel>(
        &self,
        kind: ModelKind,
        operation: CrudOperation,
        input: Option<&EncodedCreate>,
    ) -> Result<Vec<(Option<&str>, HookState)>> {
        if !self.hooks.has_hooks() || self.filter.matches_none() {
            return Ok(Vec::new());
        }
        let mut states = Vec::new();
        if let Some(iids) = self.filter.exact_iids() {
            for iid in iids {
                let state = self
                    .hooks
                    .run_pre(M::TYPE_ID_JSON, kind, operation, Some(iid.as_str()), input)
                    .await?;
                states.push((Some(iid.as_str()), state));
            }
        } else {
            states.push((
                None,
                self.hooks
                    .run_pre(M::TYPE_ID_JSON, kind, operation, None, input)
                    .await?,
            ));
        }
        Ok(states)
    }

    async fn delete<M: CompleteModel>(&self, kind: ModelKind) -> Result<()> {
        let states = self
            .pre_hooks::<M>(kind, CrudOperation::Delete, None)
            .await?;
        ProjectedManagerFilterExecutor::new(self.installed)
            .delete_on_target(self.target, &self.filter, self.control())
            .await
            .map_err(crate::projected_batch::map_target_execution_error)?;
        for (iid, state) in states {
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    kind,
                    CrudOperation::Delete,
                    iid,
                    None,
                    state,
                )
                .await;
        }
        Ok(())
    }

    async fn update<M: CompleteModel>(
        &self,
        kind: ModelKind,
        input: M::Create,
    ) -> Result<WriteResult<M>> {
        let encoded = if self.hooks.has_hooks() && !self.filter.matches_none() {
            Some(input.clone().into_encoded_create().map_err(|error| {
                crate::entity_codec::map_validation_error(error, ModelValidationPhase::Input)
            })?)
        } else {
            None
        };
        let states = self
            .pre_hooks::<M>(kind, CrudOperation::Update, encoded.as_ref())
            .await?;
        let replacement =
            crate::projected_codec::project_create(input, self.filter.model(), self.installed)?;
        let binding_error = std::sync::Mutex::new(None);
        let result = ProjectedManagerFilterExecutor::new(self.installed)
            .update_on_target_mapped(
                self.target, &self.filter, replacement, self.control(), |things| {
                    crate::projected_codec::materialize_write_result::<M>(things, self.installed)
                        .map_err(|error| {
                            *binding_error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                            type_bridge_orm::ProjectedBatchExecutor::binding_materialization_failure(0)
                        })
                },
            ).await;
        let original = binding_error
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let output = result.map_err(|error| match (error, original) {
            (QueryTargetError::Commit(error), Some(original)) => {
                Error::from(error.with_binding_source(Box::new(original)))
            }
            (_, Some(original)) => original,
            (error, None) => crate::projected_batch::map_target_execution_error(error),
        })?;
        for (iid, state) in states {
            let returned_iid = iid.and_then(|iid| {
                output
                    .items()
                    .iter()
                    .find(|item| item.iid().eq_ignore_ascii_case(iid))
                    .map(|item| item.iid())
            });
            self.hooks
                .run_post(
                    M::TYPE_ID_JSON,
                    kind,
                    CrudOperation::Update,
                    returned_iid,
                    encoded.as_ref(),
                    state,
                )
                .await;
        }
        Ok(output)
    }

    async fn all<M: CompleteModel>(&self) -> Result<Vec<M>> {
        let values = ProjectedManagerFilterExecutor::new(self.installed)
            .all_on_target(self.target, &self.filter, self.control())
            .await
            .map_err(filter_target_error)?;
        values
            .into_iter()
            .map(|value| materialize_projected::<M>(value, self.installed))
            .collect()
    }

    async fn first<M: CompleteModel>(&self) -> Result<Option<M>> {
        let value = ProjectedManagerFilterExecutor::new(self.installed)
            .first_on_target(self.target, &self.filter, self.control())
            .await
            .map_err(filter_target_error)?;
        value
            .map(|value| materialize_projected::<M>(value, self.installed))
            .transpose()
    }

    async fn count(&self) -> Result<u64> {
        ProjectedManagerFilterExecutor::new(self.installed)
            .count_on_target(self.target, &self.filter, self.control())
            .await
            .map_err(filter_target_error)
    }

    async fn exists(&self) -> Result<bool> {
        ProjectedManagerFilterExecutor::new(self.installed)
            .exists_on_target(self.target, &self.filter, self.control())
            .await
            .map_err(filter_target_error)
    }
}

/// Immutable canonical filter for one exact generated entity model.
///
/// Every call to [`Self::where_`] returns a sibling filter and leaves this
/// handle reusable. [`Self::first`] is identity-strict; it never implements
/// the released arbitrary-first compatibility behavior of other bindings.
pub struct ProjectedEntityFilter<'target, S: Schema, M: EntityModel<Schema = S>> {
    core: ProjectedFilterCore<'target>,
    marker: PhantomData<fn() -> (S, M)>,
}

impl<'target, S, M> ProjectedEntityFilter<'target, S, M>
where
    S: Schema,
    M: EntityModel<Schema = S> + CompleteModel,
{
    pub(crate) fn for_target(context: ManagerTarget<'target>, hooks: HookRunner) -> Result<Self> {
        let ManagerTarget { installed, target } = context;
        let (model, _) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            false,
        )?;
        let mut filter = Self::new(installed, target, model)?;
        filter.core.hooks = hooks;
        Ok(filter)
    }

    fn new(
        installed: &'target InstalledRuntimeProjection,
        target: QueryTarget<'target>,
        model: type_bridge_contract::id::TypeId,
    ) -> Result<Self> {
        Ok(Self {
            core: ProjectedFilterCore::try_new(installed, target, model)?,
            marker: PhantomData,
        })
    }

    /// Return a new sibling with one exact generated field comparison appended.
    pub fn where_<V>(
        &self,
        field: FieldToken<M, V>,
        operator: ProjectedManagerComparison,
        value: &V,
    ) -> Result<Self>
    where
        V: Model<Schema = S> + QueryValued,
    {
        Ok(Self {
            core: self.core.try_and::<S, M, V>(field, operator, value)?,
            marker: PhantomData,
        })
    }

    /// Match one typed IID; combine with `where_` to constrain it further.
    pub fn iid(&self, iid: &Iid) -> Result<Self> {
        Ok(Self {
            core: self.core.try_and_iid(iid)?,
            marker: PhantomData,
        })
    }

    /// Match any IID in this set; an empty set matches nothing.
    pub fn iids(&self, iids: &[Iid]) -> Result<Self> {
        Ok(Self {
            core: self.core.try_and_iids(iids)?,
            marker: PhantomData,
        })
    }

    /// Delete every matching exact model. Field filters emit one hook event with no IID.
    pub async fn delete(&self) -> Result<()> {
        self.core.delete::<M>(ModelKind::Entity).await
    }

    /// Replace every match and return its freshly hydrated model.
    /// Field filters emit one hook event with no IID; IID filters emit one per input.
    pub async fn update(&self, input: M::Create) -> Result<WriteResult<M>> {
        self.core.update::<M>(ModelKind::Entity, input).await
    }

    /// Hydrate every matching exact entity, deduplicated by IID.
    pub async fn all(&self) -> Result<Vec<M>> {
        self.core.all::<M>().await
    }

    /// Hydrate the optional exact entity selected by an IID or complete key equality.
    pub async fn first(&self) -> Result<Option<M>> {
        self.core.first::<M>().await
    }

    /// Count matching exact entity IIDs.
    pub async fn count(&self) -> Result<u64> {
        self.core.count().await
    }

    /// Return whether at least one exact entity IID matches.
    pub async fn exists(&self) -> Result<bool> {
        self.core.exists().await
    }
}

/// Immutable canonical filter for one exact generated relation model.
pub struct ProjectedRelationFilter<'target, S: Schema, M: RelationModel<Schema = S>> {
    core: ProjectedFilterCore<'target>,
    marker: PhantomData<fn() -> (S, M)>,
}

impl<'target, S, M> ProjectedRelationFilter<'target, S, M>
where
    S: Schema,
    M: RelationModel<Schema = S> + CompleteModel,
{
    pub(crate) fn for_target(context: ManagerTarget<'target>, hooks: HookRunner) -> Result<Self> {
        let ManagerTarget { installed, target } = context;
        let (model, _) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            false,
        )?;
        let mut filter = Self::new(installed, target, model)?;
        filter.core.hooks = hooks;
        Ok(filter)
    }

    fn new(
        installed: &'target InstalledRuntimeProjection,
        target: QueryTarget<'target>,
        model: type_bridge_contract::id::TypeId,
    ) -> Result<Self> {
        Ok(Self {
            core: ProjectedFilterCore::try_new(installed, target, model)?,
            marker: PhantomData,
        })
    }

    /// Return a new sibling with one exact generated field comparison appended.
    pub fn where_<V>(
        &self,
        field: FieldToken<M, V>,
        operator: ProjectedManagerComparison,
        value: &V,
    ) -> Result<Self>
    where
        V: Model<Schema = S> + QueryValued,
    {
        Ok(Self {
            core: self.core.try_and::<S, M, V>(field, operator, value)?,
            marker: PhantomData,
        })
    }

    /// Match one typed IID; combine with `where_` to constrain it further.
    pub fn iid(&self, iid: &Iid) -> Result<Self> {
        Ok(Self {
            core: self.core.try_and_iid(iid)?,
            marker: PhantomData,
        })
    }

    /// Match any IID in this set; an empty set matches nothing.
    pub fn iids(&self, iids: &[Iid]) -> Result<Self> {
        Ok(Self {
            core: self.core.try_and_iids(iids)?,
            marker: PhantomData,
        })
    }

    /// Delete every matching exact model. Field filters emit one hook event with no IID.
    pub async fn delete(&self) -> Result<()> {
        self.core.delete::<M>(ModelKind::Relation).await
    }

    /// Replace every match and return its freshly hydrated model.
    /// Field filters emit one hook event with no IID; IID filters emit one per input.
    pub async fn update(&self, input: M::Create) -> Result<WriteResult<M>> {
        self.core.update::<M>(ModelKind::Relation, input).await
    }

    /// Hydrate every matching exact relation, deduplicated by IID.
    pub async fn all(&self) -> Result<Vec<M>> {
        self.core.all::<M>().await
    }

    /// Hydrate the optional exact relation selected by an IID or complete key equality.
    pub async fn first(&self) -> Result<Option<M>> {
        self.core.first::<M>().await
    }

    /// Count matching exact relation IIDs.
    pub async fn count(&self) -> Result<u64> {
        self.core.count().await
    }

    /// Return whether at least one exact relation IID matches.
    pub async fn exists(&self) -> Result<bool> {
        self.core.exists().await
    }
}

/// Immutable read filter over a generated root and its concrete descendants.
/// Results preserve the generated subtype association; writes require an exact manager.
pub struct ProjectedSubtypeFilter<
    'target,
    S: Schema,
    M: crate::__codegen::SubtypeRootModel<Schema = S>,
> {
    core: ProjectedFilterCore<'target>,
    marker: PhantomData<fn() -> (S, M)>,
}

impl<'target, S, M> ProjectedSubtypeFilter<'target, S, M>
where
    S: Schema,
    M: crate::__codegen::SubtypeRootModel<Schema = S>,
{
    pub(crate) fn for_target(context: ManagerTarget<'target>) -> Result<Self> {
        let ManagerTarget { installed, target } = context;
        let model = match M::thing_kind() {
            crate::__codegen::ThingKind::Entity => {
                resolve_entity_authority(
                    M::TYPE_ID_JSON,
                    installed,
                    ModelValidationPhase::Input,
                    false,
                )?
                .0
            }
            crate::__codegen::ThingKind::Relation => {
                resolve_relation_authority(
                    M::TYPE_ID_JSON,
                    installed,
                    ModelValidationPhase::Input,
                    false,
                )?
                .0
            }
        };
        let mut core = ProjectedFilterCore::try_new(installed, target, model)?;
        core.filter = core.filter.including_subtypes();
        Ok(Self {
            core,
            marker: PhantomData,
        })
    }

    /// Append one generated root-field comparison to a reusable sibling filter.
    pub fn where_<V>(
        &self,
        field: FieldToken<M, V>,
        operator: ProjectedManagerComparison,
        value: &V,
    ) -> Result<Self>
    where
        V: Model<Schema = S> + QueryValued,
    {
        Ok(Self {
            core: self.core.try_and::<S, M, V>(field, operator, value)?,
            marker: PhantomData,
        })
    }

    /// Match one typed IID across the root and its concrete descendants.
    pub fn iid(&self, iid: &Iid) -> Result<Self> {
        Ok(Self {
            core: self.core.try_and_iid(iid)?,
            marker: PhantomData,
        })
    }

    /// Match any supplied IID; an empty set matches nothing.
    pub fn iids(&self, iids: &[Iid]) -> Result<Self> {
        Ok(Self {
            core: self.core.try_and_iids(iids)?,
            marker: PhantomData,
        })
    }

    fn materialize(&self, thing: &type_bridge_orm::ProjectedThing) -> Result<M::Subtypes> {
        let row = crate::projected_codec::projected_to_hydrated_row(thing, self.core.installed)?;
        M::__tb_dispatch_subtype(&row, &crate::__codegen::HydrationCapability::new()).map_err(
            |error| {
                crate::entity_codec::map_validation_error(error, ModelValidationPhase::Hydration)
            },
        )
    }

    /// Return every matching root or concrete descendant in application result order.
    pub async fn all(&self) -> Result<Vec<M::Subtypes>> {
        ProjectedManagerFilterExecutor::new(self.core.installed)
            .all_on_target(self.core.target, &self.core.filter, self.core.control())
            .await
            .map_err(filter_target_error)?
            .iter()
            .map(|thing| self.materialize(thing))
            .collect()
    }

    /// Return an identity-proven optional root or concrete descendant.
    pub async fn first(&self) -> Result<Option<M::Subtypes>> {
        ProjectedManagerFilterExecutor::new(self.core.installed)
            .first_on_target(self.core.target, &self.core.filter, self.core.control())
            .await
            .map_err(filter_target_error)?
            .map(|thing| self.materialize(&thing))
            .transpose()
    }

    /// Count distinct matching IIDs, including concrete descendants.
    pub async fn count(&self) -> Result<u64> {
        self.core.count().await
    }

    /// Return whether any root or concrete descendant matches.
    pub async fn exists(&self) -> Result<bool> {
        self.core.exists().await
    }
}

/// Borrowed read-only manager for one exact generated entity model.
pub struct ReadEntityManager<'transaction, S: Schema, M: EntityModel<Schema = S>> {
    installed: &'transaction InstalledRuntimeProjection,
    transaction: &'transaction TransactionContext,
    marker: PhantomData<fn() -> (S, M)>,
}

impl<'transaction, S, M> ReadEntityManager<'transaction, S, M>
where
    S: Schema,
    M: EntityModel<Schema = S> + CompleteModel,
{
    pub(crate) fn new(
        installed: &'transaction InstalledRuntimeProjection,
        transaction: &'transaction TransactionContext,
    ) -> Self {
        Self {
            installed,
            transaction,
            marker: PhantomData,
        }
    }

    /// Start one empty immutable exact-model filter on this borrowed context.
    pub fn filter(&self) -> Result<ProjectedEntityFilter<'transaction, S, M>> {
        let (model, _) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            self.installed,
            ModelValidationPhase::Input,
            false,
        )?;
        ProjectedEntityFilter::new(
            self.installed,
            QueryTarget::Transaction(self.transaction),
            model,
        )
    }

    /// Start a canonical filter with one exact generated field comparison.
    pub fn where_<V>(
        &self,
        field: FieldToken<M, V>,
        operator: ProjectedManagerComparison,
        value: &V,
    ) -> Result<ProjectedEntityFilter<'transaction, S, M>>
    where
        V: Model<Schema = S> + QueryValued,
    {
        self.filter()?.where_(field, operator, value)
    }
}

/// Borrowed read-only manager for one exact generated relation model.
pub struct ReadRelationManager<'transaction, S: Schema, M: RelationModel<Schema = S>> {
    installed: &'transaction InstalledRuntimeProjection,
    transaction: &'transaction TransactionContext,
    marker: PhantomData<fn() -> (S, M)>,
}

impl<'transaction, S, M> ReadRelationManager<'transaction, S, M>
where
    S: Schema,
    M: RelationModel<Schema = S> + CompleteModel,
{
    pub(crate) fn new(
        installed: &'transaction InstalledRuntimeProjection,
        transaction: &'transaction TransactionContext,
    ) -> Self {
        Self {
            installed,
            transaction,
            marker: PhantomData,
        }
    }

    /// Start one empty immutable exact-model filter on this borrowed context.
    pub fn filter(&self) -> Result<ProjectedRelationFilter<'transaction, S, M>> {
        let (model, _) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            self.installed,
            ModelValidationPhase::Input,
            false,
        )?;
        ProjectedRelationFilter::new(
            self.installed,
            QueryTarget::Transaction(self.transaction),
            model,
        )
    }

    /// Start a canonical filter with one exact generated field comparison.
    pub fn where_<V>(
        &self,
        field: FieldToken<M, V>,
        operator: ProjectedManagerComparison,
        value: &V,
    ) -> Result<ProjectedRelationFilter<'transaction, S, M>>
    where
        V: Model<Schema = S> + QueryValued,
    {
        self.filter()?.where_(field, operator, value)
    }
}

fn filter_target_error(error: QueryTargetError) -> Error {
    match error {
        QueryTargetError::Callback(_) => crate::remote::target_error(error),
        other => manager_filter_error(
            other.into_diagnostic(type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read),
        ),
    }
}

/// Preserve the manager item ceiling while capturing the ordinary read budget.
pub(crate) fn manager_read_control(
    target: QueryTarget<'_>,
) -> ProjectedManagerFilterInvocationControl {
    let mut limits = target.answer_limits().unwrap_or_default();
    limits.items = target
        .manager_item_limit()
        .unwrap_or_else(|| QueryExecutionResourceLimits::default().items);
    ProjectedManagerFilterInvocationControl::capture(limits, AnswerCancellation::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_filter_target_keeps_application_error_and_source() {
        let error = filter_target_error(QueryTargetError::Callback(Box::new(Error::remote(
            "application_transport_down",
            "application transport failed",
            Some(Box::new(std::io::Error::other("application cause"))),
        ))));
        assert_eq!(error.category(), crate::ErrorCategory::Remote);
        assert_eq!(error.code(), Some("application_transport_down"));
        assert_eq!(
            std::error::Error::source(&error).unwrap().to_string(),
            "application cause"
        );
    }
}
