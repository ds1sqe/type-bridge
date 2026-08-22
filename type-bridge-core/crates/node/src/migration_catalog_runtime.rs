//! Immutable generated migration-catalog inspection for Node.

use std::collections::BTreeSet;

use napi::bindgen_prelude::{Buffer, Error, Result};
use napi_derive::napi;
use type_bridge_schema::{ManagedDeltaContext, SafetyClass, decode_schema_authority};
use type_bridge_schema_migration::{
    MigrationApplyPlanError, MigrationApplyTarget, MigrationCatalog, VerifiedMigrationApplyPlan,
    VerifiedMigrationRollbackPlan, migration_runtime_capability_vocabulary,
};

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

    #[napi]
    pub fn preview_apply(
        &self,
        applied: Vec<NodeMigrationIdentity>,
        targets: Option<Vec<NodeMigrationIdentity>>,
    ) -> Result<NodeMigrationPreview> {
        let target = match targets {
            None => MigrationApplyTarget::DefaultHead,
            Some(targets) => MigrationApplyTarget::Explicit(migration_set(targets)?),
        };
        self.inner
            .preview_apply(&migration_set(applied)?, &target)
            .map(|plan| NodeMigrationPreview {
                inner: NodeMigrationPreviewInner::Apply(plan),
            })
            .map_err(plan_error)
    }

    #[napi]
    pub fn preview_rollback(
        &self,
        applied: Vec<NodeMigrationIdentity>,
        removals: Vec<NodeMigrationIdentity>,
    ) -> Result<NodeMigrationPreview> {
        self.inner
            .preview_rollback(&migration_set(applied)?, &migration_set(removals)?)
            .map(|plan| NodeMigrationPreview {
                inner: NodeMigrationPreviewInner::Rollback(plan),
            })
            .map_err(plan_error)
    }
}

enum NodeMigrationPreviewInner {
    Apply(VerifiedMigrationApplyPlan),
    Rollback(VerifiedMigrationRollbackPlan),
}

/// Immutable provider-free forward or rollback preview.
#[napi]
pub struct NodeMigrationPreview {
    inner: NodeMigrationPreviewInner,
}

#[napi]
impl NodeMigrationPreview {
    #[napi(getter)]
    pub fn direction(&self) -> &'static str {
        match self.inner {
            NodeMigrationPreviewInner::Apply(_) => "apply",
            NodeMigrationPreviewInner::Rollback(_) => "rollback",
        }
    }

    #[napi(getter)]
    pub fn execution_authorized(&self) -> bool {
        match &self.inner {
            NodeMigrationPreviewInner::Apply(plan) => plan.execution_authorized(),
            NodeMigrationPreviewInner::Rollback(plan) => plan.execution_authorized(),
        }
    }

    #[napi]
    pub fn migration_count(&self) -> u32 {
        bounded_u32(match &self.inner {
            NodeMigrationPreviewInner::Apply(plan) => plan.migrations().len(),
            NodeMigrationPreviewInner::Rollback(plan) => plan.rollbacks().len(),
        })
    }

    #[napi]
    pub fn migration(&self, index: u32) -> Option<NodeMigrationPreviewEntry> {
        match &self.inner {
            NodeMigrationPreviewInner::Apply(plan) => {
                plan.migrations()
                    .get(index as usize)
                    .map(|entry| NodeMigrationPreviewEntry {
                        id: migration_identity(entry.manifest().id()),
                        safety: safety_name(entry.manifest().safety()).to_owned(),
                        step_count: bounded_u32(entry.steps().len()),
                        transaction_group_count: bounded_u32(entry.transaction_groups().len()),
                        backfill_count: bounded_u32(entry.backfill_step_indices().len()),
                        reversible: entry.manifest().reversible(),
                    })
            }
            NodeMigrationPreviewInner::Rollback(plan) => {
                plan.rollbacks()
                    .get(index as usize)
                    .map(|entry| NodeMigrationPreviewEntry {
                        id: migration_identity(entry.manifest().id()),
                        safety: safety_name(entry.rollback_safety()).to_owned(),
                        step_count: bounded_u32(entry.operations().len()),
                        transaction_group_count: bounded_u32(entry.steps().len()),
                        backfill_count: bounded_u32(entry.backfills().len()),
                        reversible: true,
                    })
            }
        }
    }
}

#[napi(object)]
pub struct NodeMigrationPreviewEntry {
    pub id: NodeMigrationIdentity,
    pub safety: String,
    pub step_count: u32,
    pub transaction_group_count: u32,
    pub backfill_count: u32,
    pub reversible: bool,
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

fn migration_set(
    identities: Vec<NodeMigrationIdentity>,
) -> Result<BTreeSet<type_bridge_contract::migration::MigrationId>> {
    identities
        .into_iter()
        .map(|id| {
            type_bridge_contract::migration::MigrationId::new(id.app_label, id.name)
                .map_err(|error| catalog_error(error.code().as_str()))
        })
        .collect()
}

fn plan_error(error: MigrationApplyPlanError) -> Error {
    match error {
        MigrationApplyPlanError::Contract(error) => catalog_error(error.code().as_str()),
        MigrationApplyPlanError::Schema(_) => {
            catalog_error("migration_preview_schema_replay_failed")
        }
        MigrationApplyPlanError::Lowering(error) => catalog_error(error.code()),
    }
}

fn bounded_u32(value: usize) -> u32 {
    u32::try_from(value).expect("canonical migration limit fits u32")
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
