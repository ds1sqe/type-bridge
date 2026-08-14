//! Canonical generated manager filters over exact projected models.

use std::marker::PhantomData;

use type_bridge_orm::session::context::TransactionContext;
use type_bridge_orm::{
    AnswerCancellation, InstalledRuntimeProjection, ProjectedManagerComparison,
    ProjectedManagerFilter, ProjectedManagerFilterExecutor, QueryExecutionResourceLimits,
};

use crate::__codegen::{CompleteModel, EntityModel, FieldToken, Model, QueryValued, RelationModel};
use crate::entity_codec::resolve_entity_authority;
use crate::error::{Error, ModelValidationPhase};
use crate::projected_codec::{
    manager_filter_error, materialize_projected, project_manager_predicate,
};
use crate::relation_codec::resolve_relation_authority;
use crate::schema::Schema;
use crate::{Database, Result};

#[derive(Clone, Copy)]
enum FilterTarget<'target> {
    Database(&'target type_bridge_orm::Database),
    ReadTransaction(&'target TransactionContext),
}

struct ProjectedFilterCore<'target> {
    installed: &'target InstalledRuntimeProjection,
    target: FilterTarget<'target>,
    filter: ProjectedManagerFilter,
}

impl<'target> ProjectedFilterCore<'target> {
    fn try_new(
        installed: &'target InstalledRuntimeProjection,
        target: FilterTarget<'target>,
        model: type_bridge_contract::id::TypeId,
    ) -> Result<Self> {
        let filter =
            ProjectedManagerFilter::try_new(installed, model).map_err(manager_filter_error)?;
        Ok(Self {
            installed,
            target,
            filter,
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
            filter,
        })
    }

    async fn all<M: CompleteModel>(&self) -> Result<Vec<M>> {
        let executor = ProjectedManagerFilterExecutor::new(self.installed);
        let values = match self.target {
            FilterTarget::Database(database) => {
                executor
                    .all(
                        database,
                        &self.filter,
                        QueryExecutionResourceLimits::default(),
                        AnswerCancellation::default(),
                    )
                    .await
            }
            FilterTarget::ReadTransaction(transaction) => {
                executor
                    .all_in_read_transaction(
                        transaction,
                        &self.filter,
                        QueryExecutionResourceLimits::default(),
                        AnswerCancellation::default(),
                    )
                    .await
            }
        }
        .map_err(manager_filter_error)?;
        values
            .into_iter()
            .map(|value| materialize_projected::<M>((*value).clone(), self.installed))
            .collect()
    }

    async fn first<M: CompleteModel>(&self) -> Result<Option<M>> {
        let executor = ProjectedManagerFilterExecutor::new(self.installed);
        let value = match self.target {
            FilterTarget::Database(database) => {
                executor
                    .first(
                        database,
                        &self.filter,
                        QueryExecutionResourceLimits::default(),
                        AnswerCancellation::default(),
                    )
                    .await
            }
            FilterTarget::ReadTransaction(transaction) => {
                executor
                    .first_in_read_transaction(
                        transaction,
                        &self.filter,
                        QueryExecutionResourceLimits::default(),
                        AnswerCancellation::default(),
                    )
                    .await
            }
        }
        .map_err(manager_filter_error)?;
        value
            .map(|value| materialize_projected::<M>((*value).clone(), self.installed))
            .transpose()
    }

    async fn count(&self) -> Result<u64> {
        let executor = ProjectedManagerFilterExecutor::new(self.installed);
        match self.target {
            FilterTarget::Database(database) => {
                executor
                    .count(
                        database,
                        &self.filter,
                        QueryExecutionResourceLimits::default(),
                        AnswerCancellation::default(),
                    )
                    .await
            }
            FilterTarget::ReadTransaction(transaction) => {
                executor
                    .count_in_read_transaction(
                        transaction,
                        &self.filter,
                        QueryExecutionResourceLimits::default(),
                        AnswerCancellation::default(),
                    )
                    .await
            }
        }
        .map_err(manager_filter_error)
    }

    async fn exists(&self) -> Result<bool> {
        let executor = ProjectedManagerFilterExecutor::new(self.installed);
        match self.target {
            FilterTarget::Database(database) => {
                executor
                    .exists(
                        database,
                        &self.filter,
                        QueryExecutionResourceLimits::default(),
                        AnswerCancellation::default(),
                    )
                    .await
            }
            FilterTarget::ReadTransaction(transaction) => {
                executor
                    .exists_in_read_transaction(
                        transaction,
                        &self.filter,
                        QueryExecutionResourceLimits::default(),
                        AnswerCancellation::default(),
                    )
                    .await
            }
        }
        .map_err(manager_filter_error)
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
    pub(crate) fn for_database(database: &'target Database<S>) -> Result<Self> {
        let installed = database.installed_schema().ok_or_else(schema_not_bound)?;
        let (model, _) = resolve_entity_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            false,
        )?;
        Self::new(
            installed,
            FilterTarget::Database(database.inner_orm()),
            model,
        )
    }

    fn new(
        installed: &'target InstalledRuntimeProjection,
        target: FilterTarget<'target>,
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

    /// Hydrate every matching exact entity, deduplicated by IID.
    pub async fn all(&self) -> Result<Vec<M>> {
        self.core.all::<M>().await
    }

    /// Hydrate the optional exact entity selected by a complete key equality.
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
    pub(crate) fn for_database(database: &'target Database<S>) -> Result<Self> {
        let installed = database.installed_schema().ok_or_else(schema_not_bound)?;
        let (model, _) = resolve_relation_authority(
            M::TYPE_ID_JSON,
            installed,
            ModelValidationPhase::Input,
            false,
        )?;
        Self::new(
            installed,
            FilterTarget::Database(database.inner_orm()),
            model,
        )
    }

    fn new(
        installed: &'target InstalledRuntimeProjection,
        target: FilterTarget<'target>,
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

    /// Hydrate every matching exact relation, deduplicated by IID.
    pub async fn all(&self) -> Result<Vec<M>> {
        self.core.all::<M>().await
    }

    /// Hydrate the optional exact relation selected by a complete key equality.
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
            FilterTarget::ReadTransaction(self.transaction),
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
            FilterTarget::ReadTransaction(self.transaction),
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

fn schema_not_bound() -> Error {
    Error::model_validation(
        ModelValidationPhase::Input,
        "schema_not_bound",
        vec![],
        "database is not schema-bound",
        None,
    )
}
