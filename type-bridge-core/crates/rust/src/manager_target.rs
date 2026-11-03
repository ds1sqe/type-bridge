//! Connection-selected execution context shared by generated CRUD managers.

use type_bridge_orm::{InstalledRuntimeProjection, QueryTarget};

use crate::__codegen::CompleteModel;
use crate::error::{Error, ModelValidationPhase};
use crate::hooks::{CrudOperation, ModelKind};
use crate::{Result, WriteResult};

#[derive(Clone, Copy)]
pub(crate) struct ManagerTarget<'target> {
    pub(crate) installed: &'target InstalledRuntimeProjection,
    pub(crate) target: QueryTarget<'target>,
}

impl ManagerTarget<'_> {
    pub(crate) async fn write<M: CompleteModel>(
        self,
        create: type_bridge_orm::ProjectedCreate,
        operation: CrudOperation,
        iid: Option<&str>,
        kind: ModelKind,
    ) -> Result<WriteResult<M>> {
        if let QueryTarget::Database(database) = self.target {
            if kind == ModelKind::Relation {
                type_bridge_orm::ProjectedCrudExecutor::new(self.installed)
                    .preflight_relation_create_for_database_with_compatibility(database, &create)
                    .map_err(|error| Error::from_projected_crud(error, kind, Some(operation)))?;
            }
            let tx = database
                .transaction_context(type_bridge_orm::session::backend::TxType::Write)
                .await
                .map_err(Error::from_orm)?;
            let value = match crate::projected_batch::execute_transaction_single(
                &tx,
                self.installed,
                create,
                operation,
                iid,
                kind,
            )
            .await
            {
                Ok(value) => value,
                Err(error) => {
                    let _ = tx.rollback().await;
                    return Err(error);
                }
            };
            tx.commit_classified()
                .await
                .map_err(|error| Error::from_orm(error.into_orm_error()))?;
            return Ok(value);
        }
        crate::projected_batch::execute_target_single(
            self.target,
            self.installed,
            create,
            operation,
            iid,
        )
        .await
    }
}

pub(crate) fn schema_not_bound() -> Error {
    Error::model_validation(
        ModelValidationPhase::Input,
        "schema_not_bound",
        vec![],
        "database is not schema-bound",
        None,
    )
}
