//! Rust-owned migration runtime resources prepared for the additive C ABI.

// The complete owner graph is intentionally assembled before any symbol is
// exported: ABI 1.4's exact export ledger must remain frozen until the whole
// ABI 1.5 migration surface can be activated atomically.
#![allow(dead_code)]

use std::sync::Arc;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_schema::{ManagedDeltaContext, SafetyClass, VerifiedSchemaAuthority};
use type_bridge_schema_migration::{MigrationCatalog, migration_runtime_capability_vocabulary};

use crate::diagnostic::stable;

/// Immutable catalog state shared by C catalog and entry handles.
#[derive(Debug)]
pub(crate) struct MigrationCatalogState {
    catalog: MigrationCatalog,
    fingerprint_json: Vec<u8>,
}

impl MigrationCatalogState {
    /// Open exact generated bundle bytes under their verified package authority.
    pub(crate) fn open(
        authority: &VerifiedSchemaAuthority,
        history_bundle: &[u8],
    ) -> Result<Arc<Self>, Diagnostic> {
        let capabilities = migration_runtime_capability_vocabulary()?;
        let context = ManagedDeltaContext::new(
            authority.managed_scope().id().clone(),
            authority.semantic_profile().id().clone(),
            capabilities,
        );
        let catalog = MigrationCatalog::open(history_bundle, &context)?;
        let fingerprint_json = serde_json::to_vec(catalog.fingerprint()).map_err(|_| {
            stable(
                DiagnosticCategory::Integrity,
                "c_migration_catalog_fingerprint_encoding_failed",
                "The verified migration catalog fingerprint could not be encoded",
            )
        })?;
        Ok(Arc::new(Self {
            catalog,
            fingerprint_json,
        }))
    }

    /// Return the canonical fingerprint bytes retained by this owner.
    pub(crate) fn fingerprint_json(&self) -> &[u8] {
        &self.fingerprint_json
    }

    /// Return the bounded number of verified entries.
    pub(crate) fn len(&self) -> usize {
        self.catalog.entries().len()
    }

    /// Create an independently owned snapshot for one topological ordinal.
    pub(crate) fn entry(self: &Arc<Self>, index: usize) -> Option<MigrationHistoryEntryState> {
        self.catalog
            .entries()
            .get(index)
            .map(|_| MigrationHistoryEntryState {
                catalog: Arc::clone(self),
                index,
            })
    }

    /// Create independently owned snapshots for canonical graph heads.
    pub(crate) fn heads(&self) -> Vec<MigrationIdentitySnapshot> {
        self.catalog
            .heads()
            .iter()
            .map(MigrationIdentitySnapshot::from_id)
            .collect()
    }
}

/// Copied compound migration identity with no delimiter-joined representation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MigrationIdentitySnapshot {
    pub(crate) app_label: Vec<u8>,
    pub(crate) name: Vec<u8>,
}

impl MigrationIdentitySnapshot {
    fn from_id(id: &type_bridge_contract::migration::MigrationId) -> Self {
        Self {
            app_label: id.app_label().as_str().as_bytes().to_vec(),
            name: id.name().as_str().as_bytes().to_vec(),
        }
    }
}

/// Independently owned immutable entry handle retaining its catalog owner.
#[derive(Clone, Debug)]
pub(crate) struct MigrationHistoryEntryState {
    catalog: Arc<MigrationCatalogState>,
    index: usize,
}

impl MigrationHistoryEntryState {
    fn entry(&self) -> &type_bridge_schema_migration::VerifiedMigrationHistoryBundleEntry {
        &self.catalog.catalog.entries()[self.index]
    }

    pub(crate) fn identity(&self) -> MigrationIdentitySnapshot {
        MigrationIdentitySnapshot::from_id(self.entry().manifest().id())
    }

    pub(crate) fn parents(&self) -> Vec<MigrationIdentitySnapshot> {
        self.entry()
            .manifest()
            .parents()
            .iter()
            .map(MigrationIdentitySnapshot::from_id)
            .collect()
    }

    pub(crate) fn manifest_digest(&self) -> [u8; 32] {
        self.entry().manifest_digest().bytes()
    }

    pub(crate) fn step_count(&self) -> usize {
        self.entry().manifest().steps().len()
    }

    pub(crate) fn safety(&self) -> SafetyClass {
        self.entry().manifest().safety()
    }

    pub(crate) fn reversible(&self) -> bool {
        self.entry().manifest().reversible()
    }
}

#[cfg(test)]
mod tests {
    use type_bridge_schema_migration::{MigrationHistoryGraph, VerifiedMigrationHistoryBundle};

    use super::MigrationCatalogState;

    #[test]
    fn empty_catalog_owner_has_bounded_independent_shape() {
        let graph = MigrationHistoryGraph::from_verified(std::iter::empty()).unwrap();
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
        let catalog =
            type_bridge_schema_migration::MigrationCatalog::from_verified_bundle(bundle).unwrap();
        let fingerprint_json = serde_json::to_vec(catalog.fingerprint()).unwrap();
        let owner = std::sync::Arc::new(MigrationCatalogState {
            catalog,
            fingerprint_json,
        });

        assert_eq!(owner.len(), 0);
        assert!(owner.heads().is_empty());
        assert!(owner.entry(0).is_none());
        assert!(owner.fingerprint_json().starts_with(b"{"));
    }
}
