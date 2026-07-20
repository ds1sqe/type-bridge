//! Offline migration authoring and planning over the source workspace.
//!
//! These are the library equivalents of `type-bridge migration make` and
//! `type-bridge migration plan`: both run without any provider connection.
//! The desired schema is always the workspace's compiled schema sources, the
//! source is the committed migration head, and workspace policy is carried by
//! the validated config — never inferred.

use std::collections::BTreeSet;
use std::path::PathBuf;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_schema::SafetyClass;
use type_bridge_schema_compat::{ADOPTED_GENESIS_FILE_NAME, parse_adopted_genesis};
use type_bridge_schema_migration::{
    GeneratedMigration, MigrationGenerationOutcome, MigrationGenerationRequest,
    MigrationHistoryGraph, MigrationPreviewError, discover_verified_migration_chain,
    generate_next_migration, render_migration_preview, write_generated_migration,
};

use crate::{TypeBridgeWorkspace, TypeBridgeWorkspaceError};

/// One dependency-ordered entry of an offline apply plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationPlanEntry {
    id: MigrationId,
    safety: SafetyClass,
    reversible: bool,
}

impl MigrationPlanEntry {
    /// Return the compound migration identity.
    pub const fn id(&self) -> &MigrationId {
        &self.id
    }

    /// Return the verified safety classification the policy will meet.
    pub const fn safety(&self) -> SafetyClass {
        self.safety
    }

    /// Return whether the manifest carries a verified reverse program.
    pub const fn reversible(&self) -> bool {
        self.reversible
    }
}

impl TypeBridgeWorkspace {
    /// Return the canonical migration directory resolved under the root.
    #[must_use]
    pub fn migration_directory_absolute_path(&self) -> PathBuf {
        self.config()
            .workspace_root()
            .as_path()
            .join(self.config().migration_v2_directory().as_path())
    }

    /// Return the adopted-genesis artifact path under the migration directory.
    #[must_use]
    pub fn adopted_genesis_absolute_path(&self) -> PathBuf {
        self.migration_directory_absolute_path()
            .join(ADOPTED_GENESIS_FILE_NAME)
    }

    /// Discover and replay-verify the workspace's canonical migration chain.
    pub fn discover_migrations(&self) -> Result<MigrationHistoryGraph, TypeBridgeWorkspaceError> {
        Ok(discover_verified_migration_chain(
            &self.migration_directory_absolute_path(),
            &self.migration_genesis()?,
            self.delta_context(),
        )?)
    }

    /// Author the next canonical migration toward the compiled schema sources.
    ///
    /// Offline by default: the source is the committed head's verified target
    /// (or the genesis for an empty history) and the target is the workspace's
    /// desired declared schema. Nothing is written; pair the outcome with
    /// [`Self::write_generated_migration`].
    pub fn migration_make(
        &self,
        base_name: &str,
    ) -> Result<MigrationGenerationOutcome, TypeBridgeWorkspaceError> {
        let graph = self.discover_migrations()?;
        let genesis = self.migration_genesis()?;
        let request = MigrationGenerationRequest {
            app_label: self.config().app_label().as_str(),
            base_name,
            genesis_source: &genesis,
            desired: self.declared_schema(),
            context: self.delta_context(),
        };
        Ok(generate_next_migration(&graph, &request)?)
    }

    /// Persist one generated manifest and its review-only TypeQL preview.
    pub fn write_generated_migration(
        &self,
        generated: &GeneratedMigration,
    ) -> Result<PathBuf, TypeBridgeWorkspaceError> {
        let preview = render_migration_preview(generated.manifest(), self.delta_context())
            .map_err(preview_diagnostic)?;
        Ok(write_generated_migration(
            &self.migration_directory_absolute_path(),
            generated,
            &preview,
        )?)
    }

    /// Plan the dependency-ordered chain from an explicit applied basis.
    ///
    /// The plan is offline: it names what would apply and each manifest's
    /// verified safety classification, so an operator can see which entries
    /// the workspace policy will gate before connecting to any database.
    pub fn migration_plan(
        &self,
        applied: &BTreeSet<MigrationId>,
    ) -> Result<Vec<MigrationPlanEntry>, TypeBridgeWorkspaceError> {
        let graph = self.discover_migrations()?;
        let order = graph.plan_apply_to_default_head(applied)?;
        order
            .into_iter()
            .map(|id| {
                let manifest = graph.manifest(&id).ok_or_else(|| {
                    TypeBridgeWorkspaceError::Contract(Diagnostic::new(
                        type_bridge_contract::diagnostic::DiagnosticCategory::Integrity,
                        type_bridge_contract::diagnostic::DiagnosticCode::new(
                            "workspace_migration_plan_missing_manifest",
                        )
                        .expect("static workspace diagnostic code"),
                        "planned identity has no verified manifest",
                    ))
                })?;
                Ok(MigrationPlanEntry {
                    id,
                    safety: manifest.safety(),
                    reversible: manifest.reversible(),
                })
            })
            .collect()
    }

    /// Return the genesis source every parentless manifest verifies against.
    ///
    /// A workspace that never adopted a legacy (v1) database starts managed
    /// history from the empty schema. An adopted workspace carries the
    /// `adopted-genesis.typeql` artifact beside its canonical manifests; the
    /// reconstructed legacy head parsed from those bytes is the genesis on
    /// every offline and connected operation, so the legacy-frontier bridge
    /// and everything chained onto it replay-verify against the exact head
    /// the adoption recorded.
    pub fn migration_genesis(&self) -> Result<DeclaredSchema, TypeBridgeWorkspaceError> {
        let path = self.adopted_genesis_absolute_path();
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(DeclaredSchema::from_facts(
                    self.declared_schema().format(),
                    CapabilitySet::new(),
                    std::iter::empty(),
                )?);
            }
            Err(_) => {
                return Err(TypeBridgeWorkspaceError::Contract(Diagnostic::new(
                    type_bridge_contract::diagnostic::DiagnosticCategory::Integrity,
                    type_bridge_contract::diagnostic::DiagnosticCode::new(
                        "workspace_adopted_genesis_unreadable",
                    )
                    .expect("static workspace diagnostic code"),
                    "adopted-genesis artifact exists but cannot be read",
                )));
            }
        };
        let document = DocumentId::new(ADOPTED_GENESIS_FILE_NAME)?;
        parse_adopted_genesis(document, &source).map_err(TypeBridgeWorkspaceError::Contract)
    }
}

fn preview_diagnostic(error: MigrationPreviewError) -> TypeBridgeWorkspaceError {
    match error {
        MigrationPreviewError::Diagnostic(diagnostic) => {
            TypeBridgeWorkspaceError::Contract(diagnostic)
        }
        MigrationPreviewError::Lowering(lowering) => {
            TypeBridgeWorkspaceError::Contract(Diagnostic::new(
                type_bridge_contract::diagnostic::DiagnosticCategory::InvalidContract,
                type_bridge_contract::diagnostic::DiagnosticCode::new(lowering.code())
                    .expect("lowering diagnostic codes are canonical"),
                "migration preview lowering failed",
            ))
        }
    }
}
