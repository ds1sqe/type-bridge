//! Database session handles and connection options.

use std::fmt;
use std::sync::Arc;

#[allow(unused_imports)]
use crate::error::{Error, Result};
use crate::schema::{Schema, SchemaPackage, Unbound};
use type_bridge_orm::_registry::DescriptorRegistry;

/// Normalized outcome of creating the database bound to a generated client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseCreateOutcome {
    /// This operation created the database.
    Created,
    /// The database already existed or a concurrent creator won the race.
    AlreadyExists,
}

/// Normalized outcome of deleting the database bound to a generated client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseDeleteOutcome {
    /// This operation observed the database present and established its absence.
    Deleted,
    /// The database was already absent before destructive dispatch.
    AlreadyAbsent,
}

/// Read-only state of one exact managed database and reserved journal pair.
#[cfg(feature = "typedb")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedDatabasePairState {
    /// Neither member exists.
    Absent,
    /// Only a standalone managed database without TypeBridge control state exists.
    StandaloneManaged,
    /// Both members exist and the journal owns this exact database and scope.
    OwnedPair,
    /// Only the exact owner-verified journal remains.
    OwnedJournalOrphan,
}

/// Outcome of executing one pair-aware database deletion plan.
#[cfg(feature = "typedb")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedDatabaseDeleteOutcome {
    /// Neither member existed when execution began.
    AlreadyAbsent,
    /// A standalone managed database was deleted.
    DeletedStandaloneManaged,
    /// The owner-verified journal was deleted before its managed database.
    DeletedOwnedPair,
    /// An owner-verified orphan journal was deleted.
    DeletedOwnedJournalOrphan,
}

/// Owned, single-use destructive admission for one inspected database pair.
#[cfg(feature = "typedb")]
pub struct ManagedDatabaseDeletionPlan {
    inner: type_bridge_schema_migration_typedb::ManagedDatabasePairDeletionPlan,
}

#[cfg(feature = "typedb")]
impl ManagedDatabaseDeletionPlan {
    /// Return the exact pair state admitted by this plan.
    #[must_use]
    pub fn inspected_state(&self) -> ManagedDatabasePairState {
        map_pair_state(self.inner.inspected_state())
    }

    /// Revalidate and execute journal-first deletion.
    pub async fn execute(self) -> Result<ManagedDatabaseDeleteOutcome> {
        self.inner
            .execute()
            .await
            .map(map_delete_outcome)
            .map_err(administration_error)
    }

    /// Revalidate interruptibly, then execute without masking provider outcomes.
    pub async fn execute_controlled(
        self,
        control: &crate::MigrationExecutionControl,
    ) -> Result<ManagedDatabaseDeleteOutcome> {
        self.inner
            .execute_controlled(control)
            .await
            .map(map_delete_outcome)
            .map_err(administration_error)
    }
}

/// Connection options for TypeDB servers.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionOptions {
    address: String,
    database: String,
    username: Option<String>,
    password: Option<String>,
    http_port: u16,
    tls: bool,
}

impl fmt::Debug for ConnectionOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionOptions")
            .field("address", &self.address)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("http_port", &self.http_port)
            .field("tls", &self.tls)
            .finish()
    }
}

impl ConnectionOptions {
    /// Create connection options targeting a database server.
    #[must_use]
    pub fn new(address: impl Into<String>, database: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            database: database.into(),
            username: None,
            password: None,
            http_port: 8000,
            tls: false,
        }
    }

    /// Set authentication credentials.
    #[must_use]
    pub fn credentials(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }

    /// Set the HTTP probe port.
    #[must_use]
    pub fn http_port(mut self, port: u16) -> Self {
        self.http_port = port;
        self
    }

    /// Enable or disable TLS.
    #[must_use]
    pub fn tls(mut self, enabled: bool) -> Self {
        self.tls = enabled;
        self
    }

    /// Return the server address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Return the target database name.
    #[must_use]
    pub fn database(&self) -> &str {
        &self.database
    }

    /// Return the HTTP probe port.
    #[must_use]
    pub fn get_http_port(&self) -> u16 {
        self.http_port
    }

    /// Return whether TLS is enabled.
    #[must_use]
    pub fn is_tls(&self) -> bool {
        self.tls
    }
}

impl From<(&str, &str)> for ConnectionOptions {
    fn from((address, database): (&str, &str)) -> Self {
        Self::new(address, database)
    }
}

/// Primary database session handle type-branded by schema `S`.
pub struct Database<S: Schema = Unbound> {
    inner: type_bridge_orm::Database,
    installed_schema: Option<Arc<type_bridge_orm::InstalledRuntimeProjection>>,
    match_registry: Option<Arc<DescriptorRegistry>>,
    #[cfg_attr(not(feature = "typedb"), allow(dead_code))]
    managed_scope_id: Option<type_bridge_contract::managed_scope::ManagedScopeId>,
    marker: std::marker::PhantomData<fn() -> S>,
}

pub(crate) fn build_match_registry(
    installed: &type_bridge_orm::InstalledRuntimeProjection,
) -> Result<Arc<DescriptorRegistry>> {
    installed
        .match_registry()
        .map(Arc::new)
        .map_err(Error::from_orm)
}

impl Database<Unbound> {
    /// Connect to a TypeDB server returning an unbound database handle.
    #[cfg(feature = "typedb")]
    pub async fn connect(options: impl Into<ConnectionOptions>) -> Result<Database<Unbound>> {
        let opts = options.into();
        let username = opts.username.as_deref().unwrap_or("admin");
        let password = opts.password.as_deref().unwrap_or("password");
        let orm_opts = type_bridge_orm::ConnectOptions {
            http_port: opts.http_port,
            tls: opts.tls,
            ..type_bridge_orm::ConnectOptions::default()
        };

        let inner = type_bridge_orm::Database::connect_with_options(
            &opts.address,
            &opts.database,
            username,
            password,
            orm_opts,
        )
        .await
        .map_err(Error::from_orm)?;

        Ok(Database {
            inner,
            installed_schema: None,
            match_registry: None,
            managed_scope_id: None,
            marker: std::marker::PhantomData,
        })
    }

    /// Construct a Database session wrapping an existing ORM Database (crate-internal).
    #[allow(dead_code)]
    pub(crate) fn from_orm_database(inner: type_bridge_orm::Database) -> Self {
        Self {
            inner,
            installed_schema: None,
            match_registry: None,
            managed_scope_id: None,
            marker: std::marker::PhantomData,
        }
    }

    /// Bind and verify a generated schema package, transitioning to `Database<S>`.
    pub fn with_schema<S: Schema>(self, schema: SchemaPackage<S>) -> Result<Database<S>> {
        let (installed, authority) = schema.verify_and_install_with_authority()?;
        let match_registry = build_match_registry(&installed)?;
        Ok(Database::from_bound_parts(
            self.inner,
            installed,
            match_registry,
            authority.map(|authority| authority.managed_scope().id().clone()),
        ))
    }
}

impl<S: Schema> Database<S> {
    pub(crate) fn from_bound_parts(
        inner: type_bridge_orm::Database,
        installed: Arc<type_bridge_orm::InstalledRuntimeProjection>,
        match_registry: Arc<DescriptorRegistry>,
        managed_scope_id: Option<type_bridge_contract::managed_scope::ManagedScopeId>,
    ) -> Self {
        Self {
            inner,
            installed_schema: Some(installed),
            match_registry: Some(match_registry),
            managed_scope_id,
            marker: std::marker::PhantomData,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test_parts(
        inner: type_bridge_orm::Database,
        installed: type_bridge_orm::InstalledRuntimeProjection,
    ) -> Self {
        let installed = Arc::new(installed);
        let match_registry =
            build_match_registry(&installed).expect("test projection descriptors register");
        Self {
            inner,
            installed_schema: Some(installed),
            match_registry: Some(match_registry),
            managed_scope_id: None,
            marker: std::marker::PhantomData,
        }
    }
    #[cfg(test)]
    pub(crate) fn from_test_unbound_parts(inner: type_bridge_orm::Database) -> Self {
        Self {
            inner,
            installed_schema: None,
            match_registry: None,
            managed_scope_id: None,
            marker: std::marker::PhantomData,
        }
    }
    /// Create a lightweight client-owned exact entity manager.
    pub fn entities<M>(&self) -> crate::entity_manager::EntityManager<'_, S, M>
    where
        M: crate::__codegen::EntityModel<Schema = S>,
    {
        crate::entity_manager::EntityManager::new(self)
    }
    /// Create a lightweight client-owned exact relation manager.
    pub fn relations<M>(&self) -> crate::relation_manager::RelationManager<'_, S, M>
    where
        M: crate::__codegen::RelationModel<Schema = S>,
    {
        crate::relation_manager::RelationManager::new(self)
    }
    /// Open one client-owned write transaction over this schema-bound
    /// database. Operations on its borrowed managers never auto-commit;
    /// the caller terminally commits or rolls back, and dropping the open
    /// transaction releases the context without commit.
    pub async fn write(&self) -> Result<crate::transaction::WriteTransaction<'_, S>> {
        crate::transaction::WriteTransaction::open(self).await
    }
    /// Open one reusable client-owned read transaction. Query terminals
    /// borrow and reuse its retained context until explicit close or drop.
    pub async fn read(&self) -> Result<crate::transaction::ReadTransaction<'_, S>> {
        crate::transaction::ReadTransaction::open(self).await
    }
    /// Return the target database name.
    #[must_use]
    pub fn database_name(&self) -> &str {
        self.inner.database_name()
    }

    /// Return whether the one configured database exists.
    pub async fn database_exists(&self) -> Result<bool> {
        self.inner.database_exists().await.map_err(Error::from_orm)
    }

    /// Return whether the managed database exists, honoring execution controls.
    #[cfg(feature = "typedb")]
    pub async fn database_exists_controlled(
        &self,
        control: &crate::MigrationExecutionControl,
    ) -> Result<bool> {
        if let Some(scope) = &self.managed_scope_id {
            return self
                .pair_administrator(scope.clone())?
                .database_exists_controlled(control)
                .await
                .map_err(administration_error);
        }
        control.check().map_err(administration_error)?;
        self.database_exists().await
    }

    /// Create the one configured database and return its normalized outcome.
    pub async fn create_database(&self) -> Result<DatabaseCreateOutcome> {
        #[cfg(feature = "typedb")]
        if let Some(scope) = &self.managed_scope_id {
            return self
                .pair_administrator(scope.clone())?
                .create_database_outcome()
                .await
                .map(|outcome| match outcome {
                    type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome::Created => DatabaseCreateOutcome::Created,
                    type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome::AlreadyExists => DatabaseCreateOutcome::AlreadyExists,
                })
                .map_err(administration_error);
        }
        self.inner
            .create_database_outcome()
            .await
            .map(|outcome| match outcome {
                type_bridge_orm::session::DatabaseCreateOutcome::Created => {
                    DatabaseCreateOutcome::Created
                }
                type_bridge_orm::session::DatabaseCreateOutcome::AlreadyExists => {
                    DatabaseCreateOutcome::AlreadyExists
                }
            })
            .map_err(Error::from_orm)
    }

    /// Create the configured database with cancellation and deadline control.
    #[cfg(feature = "typedb")]
    pub async fn create_database_controlled(
        &self,
        control: &crate::MigrationExecutionControl,
    ) -> Result<DatabaseCreateOutcome> {
        if let Some(scope) = &self.managed_scope_id {
            return self
                .pair_administrator(scope.clone())?
                .create_database_outcome_controlled(control)
                .await
                .map(|outcome| match outcome {
                    type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome::Created => DatabaseCreateOutcome::Created,
                    type_bridge_schema_migration_typedb::ManagedDatabasePairCreateOutcome::AlreadyExists => DatabaseCreateOutcome::AlreadyExists,
                })
                .map_err(administration_error);
        }
        control.check().map_err(administration_error)?;
        self.create_database().await
    }

    /// Delete the one configured database and return its normalized outcome.
    pub async fn delete_database(&self) -> Result<DatabaseDeleteOutcome> {
        #[cfg(feature = "typedb")]
        if self.managed_scope_id.is_some() {
            return Err(Error::Database {
                message: "managed database deletion requires plan_database_delete() and explicit plan execution"
                    .to_owned(),
                source: None,
            });
        }
        self.inner
            .delete_database_outcome()
            .await
            .map(|outcome| match outcome {
                type_bridge_orm::session::DatabaseDeleteOutcome::Deleted => {
                    DatabaseDeleteOutcome::Deleted
                }
                type_bridge_orm::session::DatabaseDeleteOutcome::AlreadyAbsent => {
                    DatabaseDeleteOutcome::AlreadyAbsent
                }
            })
            .map_err(Error::from_orm)
    }

    /// Inspect the exact managed database and reserved journal pair.
    #[cfg(feature = "typedb")]
    pub async fn inspect_database_pair(&self) -> Result<ManagedDatabasePairState> {
        let scope = self
            .managed_scope_id
            .clone()
            .ok_or_else(|| Error::Database {
                message:
                    "managed database administration requires verified generated schema authority"
                        .to_owned(),
                source: None,
            })?;
        self.pair_administrator(scope)?
            .inspect()
            .await
            .map(map_pair_state)
            .map_err(administration_error)
    }

    /// Inspect the managed pair while honoring cancellation and a deadline.
    #[cfg(feature = "typedb")]
    pub async fn inspect_database_pair_controlled(
        &self,
        control: &crate::MigrationExecutionControl,
    ) -> Result<ManagedDatabasePairState> {
        let scope = self
            .managed_scope_id
            .clone()
            .ok_or_else(|| Error::Database {
                message:
                    "managed database administration requires verified generated schema authority"
                        .to_owned(),
                source: None,
            })?;
        self.pair_administrator(scope)?
            .inspect_controlled(control)
            .await
            .map(map_pair_state)
            .map_err(administration_error)
    }

    /// Inspect and retain an explicit pair-aware destructive deletion plan.
    #[cfg(feature = "typedb")]
    pub async fn plan_database_delete(&self) -> Result<ManagedDatabaseDeletionPlan> {
        let scope = self
            .managed_scope_id
            .clone()
            .ok_or_else(|| Error::Database {
                message: "managed database deletion requires verified generated schema authority"
                    .to_owned(),
                source: None,
            })?;
        self.pair_administrator(scope)?
            .plan_delete()
            .await
            .map(|inner| ManagedDatabaseDeletionPlan { inner })
            .map_err(administration_error)
    }

    /// Inspect and retain a deletion plan while honoring execution controls.
    #[cfg(feature = "typedb")]
    pub async fn plan_database_delete_controlled(
        &self,
        control: &crate::MigrationExecutionControl,
    ) -> Result<ManagedDatabaseDeletionPlan> {
        let scope = self
            .managed_scope_id
            .clone()
            .ok_or_else(|| Error::Database {
                message: "managed database deletion requires verified generated schema authority"
                    .to_owned(),
                source: None,
            })?;
        self.pair_administrator(scope)?
            .plan_delete_controlled(control)
            .await
            .map(|inner| ManagedDatabaseDeletionPlan { inner })
            .map_err(administration_error)
    }

    #[cfg(feature = "typedb")]
    fn pair_administrator(
        &self,
        scope: type_bridge_contract::managed_scope::ManagedScopeId,
    ) -> Result<type_bridge_schema_migration_typedb::ManagedDatabasePairAdministrator> {
        type_bridge_schema_migration_typedb::ManagedDatabasePairAdministrator::from_managed_database(
            Arc::new(self.inner.clone()),
            scope,
        )
        .map_err(administration_error)
    }

    /// Explicitly close this database's provider connection.
    ///
    /// Closing is idempotent. Once closed, the database cannot admit new
    /// provider work, while repeated calls remain harmless.
    pub fn close(&self) -> Result<()> {
        self.inner.close().map_err(Error::from_orm)
    }

    /// Return whether this database handle is bound to a verified schema.
    #[must_use]
    pub fn is_schema_bound(&self) -> bool {
        self.installed_schema.is_some()
    }

    /// Return the internal ORM handle for engine mechanics (crate-internal).
    #[allow(dead_code)]
    pub(crate) fn inner_orm(&self) -> &type_bridge_orm::Database {
        &self.inner
    }

    pub(crate) fn operation_limits(
        &self,
        requested: type_bridge_orm::QueryExecutionResourceLimits,
    ) -> type_bridge_orm::QueryExecutionResourceLimits {
        self.inner.answer_limits().map_or_else(
            || requested.effective(),
            |ceiling| requested.constrained_by(ceiling),
        )
    }

    /// Return the installed projection if schema-bound (crate-internal).
    #[allow(dead_code)]
    pub(crate) fn installed_schema(
        &self,
    ) -> Option<&Arc<type_bridge_orm::InstalledRuntimeProjection>> {
        self.installed_schema.as_ref()
    }

    /// Return the match descriptor registry if schema-bound (crate-internal).
    #[allow(dead_code)]
    pub(crate) fn match_registry(&self) -> Option<&Arc<DescriptorRegistry>> {
        self.match_registry.as_ref()
    }
}

#[cfg(feature = "typedb")]
fn map_pair_state(
    state: type_bridge_schema_migration_typedb::ManagedDatabasePairState,
) -> ManagedDatabasePairState {
    match state {
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::Absent => {
            ManagedDatabasePairState::Absent
        }
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::StandaloneManaged => {
            ManagedDatabasePairState::StandaloneManaged
        }
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::OwnedPair => {
            ManagedDatabasePairState::OwnedPair
        }
        type_bridge_schema_migration_typedb::ManagedDatabasePairState::OwnedJournalOrphan => {
            ManagedDatabasePairState::OwnedJournalOrphan
        }
    }
}

#[cfg(feature = "typedb")]
fn map_delete_outcome(
    outcome: type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome,
) -> ManagedDatabaseDeleteOutcome {
    match outcome {
        type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::AlreadyAbsent => {
            ManagedDatabaseDeleteOutcome::AlreadyAbsent
        }
        type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedStandaloneManaged => ManagedDatabaseDeleteOutcome::DeletedStandaloneManaged,
        type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedOwnedPair => {
            ManagedDatabaseDeleteOutcome::DeletedOwnedPair
        }
        type_bridge_schema_migration_typedb::ManagedDatabasePairDeleteOutcome::DeletedOwnedJournalOrphan => ManagedDatabaseDeleteOutcome::DeletedOwnedJournalOrphan,
    }
}

#[cfg(feature = "typedb")]
fn administration_error(error: type_bridge_contract::diagnostic::Diagnostic) -> Error {
    Error::from_contract_diagnostic(error)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use type_bridge_orm::error::OrmError;
    use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, TransactionOps, TxType};

    use super::Database;
    use crate::schema::Unbound;

    struct CloseBackend {
        closed: Arc<AtomicBool>,
        close_calls: Arc<AtomicUsize>,
    }

    struct AdministrationBackend {
        databases: Arc<Mutex<BTreeSet<String>>>,
    }

    impl DriverBackend for AdministrationBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async { Err(OrmError::Connection("unexpected transaction".into())) })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn database_exists(
            &self,
            database: &str,
        ) -> BoxFuture<'_, std::result::Result<bool, OrmError>> {
            let exists = self.databases.lock().unwrap().contains(database);
            Box::pin(async move { Ok(exists) })
        }

        fn create_database(
            &self,
            database: &str,
        ) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
            self.databases.lock().unwrap().insert(database.to_owned());
            Box::pin(async { Ok(()) })
        }

        fn delete_database(
            &self,
            database: &str,
        ) -> BoxFuture<'_, std::result::Result<(), OrmError>> {
            self.databases.lock().unwrap().remove(database);
            Box::pin(async { Ok(()) })
        }

        fn schema_text(
            &self,
            _database: &str,
        ) -> BoxFuture<'_, std::result::Result<String, OrmError>> {
            Box::pin(async { Ok(String::new()) })
        }
    }

    impl DriverBackend for CloseBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async {
                Err(OrmError::Connection(
                    "closed test backend cannot open transactions".into(),
                ))
            })
        }

        fn is_open(&self) -> bool {
            !self.closed.load(Ordering::SeqCst)
        }

        fn close_connection(&self) -> std::result::Result<(), OrmError> {
            self.close_calls.fetch_add(1, Ordering::SeqCst);
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn explicit_database_close_is_idempotent() {
        let closed = Arc::new(AtomicBool::new(false));
        let close_calls = Arc::new(AtomicUsize::new(0));
        let inner = type_bridge_orm::Database::with_backend(
            Box::new(CloseBackend {
                closed: Arc::clone(&closed),
                close_calls: Arc::clone(&close_calls),
            }),
            "app",
        );
        let database: Database<Unbound> = Database::from_test_unbound_parts(inner);

        database.close().unwrap();
        database.close().unwrap();

        assert!(closed.load(Ordering::SeqCst));
        assert_eq!(close_calls.load(Ordering::SeqCst), 2);
    }

    #[cfg(feature = "typedb")]
    #[tokio::test]
    async fn generated_database_administration_is_pair_aware_and_plan_owned() {
        let databases = Arc::new(Mutex::new(BTreeSet::new()));
        let inner = type_bridge_orm::Database::with_backend(
            Box::new(AdministrationBackend {
                databases: Arc::clone(&databases),
            }),
            "app",
        );
        let database: Database<Unbound> = Database {
            inner,
            installed_schema: None,
            match_registry: None,
            managed_scope_id: Some(
                type_bridge_contract::managed_scope::ManagedScopeId::new("generated-scope")
                    .unwrap(),
            ),
            marker: std::marker::PhantomData,
        };

        let cancellation = type_bridge_schema_migration::MigrationCancellation::default();
        cancellation.cancel();
        let control = type_bridge_schema_migration::MigrationExecutionControl::new(
            cancellation,
            None,
            type_bridge_schema_migration::MigrationExecutionResourceLimits::default(),
        );
        let cancelled = database
            .database_exists_controlled(&control)
            .await
            .expect_err("pre-cancelled administration rejects before an effect");
        assert_eq!(cancelled.code(), Some("migration_execution_cancelled"));
        assert_eq!(cancelled.category(), crate::ErrorCategory::Cancelled);

        assert_eq!(
            database.create_database().await.unwrap(),
            super::DatabaseCreateOutcome::Created
        );
        assert_eq!(
            database.inspect_database_pair().await.unwrap(),
            super::ManagedDatabasePairState::StandaloneManaged
        );
        assert!(database.delete_database().await.is_err());
        let plan = database.plan_database_delete().await.unwrap();
        drop(database);
        assert_eq!(
            plan.execute().await.unwrap(),
            super::ManagedDatabaseDeleteOutcome::DeletedStandaloneManaged
        );
        assert!(databases.lock().unwrap().is_empty());
    }
}
