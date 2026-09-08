//! Database session handles and connection options.

use std::fmt;
use std::sync::Arc;

#[allow(unused_imports)]
use crate::error::{Error, Result};
use crate::schema::{Schema, SchemaPackage, Unbound};
use type_bridge_orm::_registry::DescriptorRegistry;

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
    marker: std::marker::PhantomData<fn() -> S>,
}

fn build_match_registry(
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
            marker: std::marker::PhantomData,
        }
    }

    /// Bind and verify a generated schema package, transitioning to `Database<S>`.
    pub fn with_schema<S: Schema>(self, schema: SchemaPackage<S>) -> Result<Database<S>> {
        let installed = schema.verify_and_install()?;
        let match_registry = build_match_registry(&installed)?;
        Ok(Database {
            inner: self.inner,
            installed_schema: Some(installed),
            match_registry: Some(match_registry),
            marker: std::marker::PhantomData,
        })
    }
}

impl<S: Schema> Database<S> {
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
            marker: std::marker::PhantomData,
        }
    }
    #[cfg(test)]
    pub(crate) fn from_test_unbound_parts(inner: type_bridge_orm::Database) -> Self {
        Self {
            inner,
            installed_schema: None,
            match_registry: None,
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
