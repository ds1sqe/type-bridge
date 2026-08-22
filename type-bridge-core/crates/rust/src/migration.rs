//! Generated-package-owned immutable migration catalog inspection.

use std::marker::PhantomData;

use type_bridge_contract::fingerprint::Fingerprint;
use type_bridge_contract::migration::{MigrationId, MigrationStep};
use type_bridge_schema::{ManagedDeltaContext, SafetyClass};

use crate::error::{Error, Result};
use crate::schema::{Schema, SchemaPackage};

/// Immutable replay-verified migration catalog branded by one generated schema.
#[derive(Clone, Debug)]
pub struct MigrationCatalog<S: Schema> {
    inner: type_bridge_schema_migration::MigrationCatalog,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> MigrationCatalog<S> {
    /// Return the canonical history-bundle fingerprint.
    #[must_use]
    pub const fn fingerprint(&self) -> &Fingerprint {
        self.inner.fingerprint()
    }

    /// Return the number of replay-verified history entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.entries().len()
    }

    /// Report whether the catalog contains no committed migrations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.entries().is_empty()
    }

    /// Return graph heads in canonical compound-identity order.
    #[must_use]
    pub fn heads(&self) -> &[MigrationId] {
        self.inner.heads()
    }

    /// Inspect one history entry by deterministic topological ordinal.
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<MigrationHistoryEntry<'_>> {
        self.inner
            .entries()
            .get(index)
            .map(|entry| MigrationHistoryEntry { inner: entry })
    }
}

/// Borrowed immutable view of one catalog history entry.
#[derive(Clone, Copy, Debug)]
pub struct MigrationHistoryEntry<'a> {
    inner: &'a type_bridge_schema_migration::VerifiedMigrationHistoryBundleEntry,
}

impl MigrationHistoryEntry<'_> {
    /// Return the canonical compound migration identity.
    #[must_use]
    pub const fn id(&self) -> &MigrationId {
        self.inner.manifest().id()
    }

    /// Return canonical parent identities.
    #[must_use]
    pub fn parents(&self) -> &[MigrationId] {
        self.inner.manifest().parents()
    }

    /// Return the exact canonical manifest digest.
    #[must_use]
    pub const fn manifest_digest(
        &self,
    ) -> type_bridge_contract::migration::MigrationManifestDigest {
        self.inner.manifest_digest()
    }

    /// Return the number of ordered schema, assertion, and backfill steps.
    #[must_use]
    pub fn step_count(&self) -> usize {
        self.inner.manifest().steps().len()
    }

    /// Return one ordered step for typed inspection.
    #[must_use]
    pub fn step(&self, index: usize) -> Option<&MigrationStep> {
        self.inner.manifest().steps().get(index)
    }

    /// Return the closed safety classification.
    #[must_use]
    pub const fn safety(&self) -> SafetyClass {
        self.inner.manifest().safety()
    }

    /// Report whether the complete migration has a verified reverse.
    #[must_use]
    pub const fn is_reversible(&self) -> bool {
        self.inner.manifest().reversible()
    }
}

impl<S: Schema> SchemaPackage<S> {
    /// Open the generated package's canonical migration-history resource.
    ///
    /// Package authority and every bundled schema/manifest are verified
    /// offline before the catalog is returned. This operation performs no
    /// connection, database, transaction, journal, or provider I/O.
    pub fn open_migration_catalog(&self, bytes: &[u8]) -> Result<MigrationCatalog<S>> {
        let (_projection, authority) = self.verify_and_install_with_authority()?;
        let authority = authority.ok_or_else(|| Error::SchemaVerification {
            message: "migration catalogs require generated schema authority".to_owned(),
            source: None,
        })?;
        let context = ManagedDeltaContext::new(
            authority.managed_scope().id().clone(),
            authority.semantic_profile().id().clone(),
            type_bridge_schema_migration::migration_runtime_capability_vocabulary().map_err(
                |error| Error::SchemaVerification {
                    message: format!(
                        "migration runtime capability vocabulary is invalid [{}]",
                        error.code().as_str()
                    ),
                    source: Some(Box::new(error)),
                },
            )?,
        );
        let inner = type_bridge_schema_migration::MigrationCatalog::open(bytes, &context).map_err(
            |error| Error::SchemaVerification {
                message: format!(
                    "generated migration catalog was rejected [{}]",
                    error.code().as_str()
                ),
                source: Some(Box::new(error)),
            },
        )?;
        Ok(MigrationCatalog {
            inner,
            marker: PhantomData,
        })
    }
}
