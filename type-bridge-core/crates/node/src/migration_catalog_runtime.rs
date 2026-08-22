//! Immutable generated migration-catalog inspection for Node.

use napi::bindgen_prelude::{Buffer, Error, Result};
use napi_derive::napi;
use type_bridge_schema::{ManagedDeltaContext, SafetyClass, decode_schema_authority};
use type_bridge_schema_migration::{MigrationCatalog, migration_runtime_capability_vocabulary};

#[napi(object)]
pub struct NodeMigrationIdentity {
    pub app_label: String,
    pub name: String,
}

#[napi(object)]
pub struct NodeMigrationHistoryEntry {
    pub id: NodeMigrationIdentity,
    pub parents: Vec<NodeMigrationIdentity>,
    pub manifest_digest: String,
    pub step_count: u32,
    pub safety: String,
    pub reversible: bool,
}

/// Generated-package-owned, source-free migration catalog.
#[napi]
pub struct NodeMigrationCatalog {
    inner: MigrationCatalog,
}

#[napi]
impl NodeMigrationCatalog {
    #[napi]
    pub fn fingerprint_json(&self) -> Result<String> {
        serde_json::to_string(self.inner.fingerprint())
            .map_err(|_| catalog_error("migration_catalog_encoding_failed"))
    }

    #[napi(getter)]
    pub fn length(&self) -> u32 {
        u32::try_from(self.inner.entries().len()).expect("bundle entry limit fits u32")
    }

    #[napi]
    pub fn is_empty(&self) -> bool {
        self.inner.entries().is_empty()
    }

    #[napi]
    pub fn heads(&self) -> Vec<NodeMigrationIdentity> {
        self.inner.heads().iter().map(migration_identity).collect()
    }

    #[napi]
    pub fn entry(&self, index: u32) -> Option<NodeMigrationHistoryEntry> {
        self.inner.entries().get(index as usize).map(|entry| {
            let manifest = entry.manifest();
            NodeMigrationHistoryEntry {
                id: migration_identity(manifest.id()),
                parents: manifest.parents().iter().map(migration_identity).collect(),
                manifest_digest: entry.manifest_digest().to_hex(),
                step_count: u32::try_from(manifest.steps().len())
                    .expect("manifest step limit fits u32"),
                safety: safety_name(manifest.safety()).to_owned(),
                reversible: manifest.reversible(),
            }
        })
    }
}

/// Open canonical generated history bytes under their generated schema authority.
#[napi]
pub fn open_migration_catalog(
    schema_authority: Buffer,
    history_bundle: Buffer,
) -> Result<NodeMigrationCatalog> {
    let capabilities = migration_runtime_capability_vocabulary()
        .map_err(|error| catalog_error(error.code().as_str()))?;
    let authority = decode_schema_authority(schema_authority.as_ref(), &capabilities)
        .map_err(|_| catalog_error("generated_schema_authority_invalid"))?;
    let context = ManagedDeltaContext::new(
        authority.managed_scope().id().clone(),
        authority.semantic_profile().id().clone(),
        capabilities,
    );
    MigrationCatalog::open(history_bundle.as_ref(), &context)
        .map(|inner| NodeMigrationCatalog { inner })
        .map_err(|error| catalog_error(error.code().as_str()))
}

fn migration_identity(id: &type_bridge_contract::migration::MigrationId) -> NodeMigrationIdentity {
    NodeMigrationIdentity {
        app_label: id.app_label().as_str().to_owned(),
        name: id.name().as_str().to_owned(),
    }
}

fn safety_name(safety: SafetyClass) -> &'static str {
    match safety {
        SafetyClass::FormalOnly => "formal_only",
        SafetyClass::SchemaMetadata => "schema_metadata",
        SafetyClass::Additive => "additive",
        SafetyClass::Conditional => "conditional",
        SafetyClass::BackfillRequired => "backfill_required",
        SafetyClass::Destructive => "destructive",
        SafetyClass::Opaque => "opaque",
        SafetyClass::Unsupported => "unsupported",
    }
}

fn catalog_error(code: &str) -> Error {
    Error::from_reason(format!("migration catalog rejected [{code}]"))
}

#[cfg(test)]
mod tests {
    use type_bridge_schema_migration::{MigrationHistoryGraph, VerifiedMigrationHistoryBundle};

    use super::NodeMigrationCatalog;

    #[test]
    fn empty_catalog_has_bounded_immutable_node_shape() {
        let graph = MigrationHistoryGraph::from_verified(std::iter::empty()).unwrap();
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
        let catalog = NodeMigrationCatalog {
            inner: type_bridge_schema_migration::MigrationCatalog::from_verified_bundle(bundle)
                .unwrap(),
        };

        assert_eq!(catalog.length(), 0);
        assert!(catalog.is_empty());
        assert!(catalog.heads().is_empty());
        assert!(catalog.entry(0).is_none());
        assert!(catalog.fingerprint_json().unwrap().contains("sha256"));
    }
}
