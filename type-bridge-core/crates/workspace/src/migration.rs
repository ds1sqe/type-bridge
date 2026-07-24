//! Offline migration authoring and planning over the source workspace.
//!
//! These are the library equivalents of `type-bridge migration make` and
//! `type-bridge migration plan`: both run without any provider connection.
//! The desired schema is always the workspace's compiled schema sources, the
//! source is the committed migration head, and workspace policy is carried by
//! the validated config — never inferred.

use std::collections::BTreeSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_schema::SafetyClass;
use type_bridge_schema_compat::{ADOPTED_GENESIS_FILE_NAME, parse_adopted_genesis};
use type_bridge_schema_migration::{
    GeneratedMigration, MigrationDirectory, MigrationGenerationOutcome, MigrationGenerationRequest,
    MigrationHistoryGraph, MigrationPreviewError,
    canonical_history_declared_legacy_bridge_count_in, discover_verified_migration_chain_in,
    generate_next_migration, render_migration_preview, require_adoption_authority_pair,
    require_adoption_authority_pair_state, try_acquire_migration_authoring_lock,
    write_generated_migration_under_lock,
};

use crate::{
    TypeBridgeWorkspace, TypeBridgeWorkspaceError, WorkspaceConfigError, WorkspaceConfigErrorCode,
};

fn migration_directory_escape() -> TypeBridgeWorkspaceError {
    TypeBridgeWorkspaceError::Config(
        WorkspaceConfigError::new(
            WorkspaceConfigErrorCode::PathNotConfined,
            "the migration directory must remain beneath the workspace without symbolic links",
        )
        .with_detail("migration_v2_directory"),
    )
}

/// An open, workspace-bound migration-directory authority.
///
/// The directory descriptor remains live for the authority's lifetime. Its
/// operational path resolves through that descriptor rather than through the
/// mutable workspace pathname, so replacing a checked component cannot
/// redirect discovery or publication.
///
#[derive(Debug)]
pub struct MigrationDirectoryAuthority {
    directory: MigrationDirectory,
    configured_path: PathBuf,
    owner: Arc<()>,
}

impl MigrationDirectoryAuthority {
    /// Return the retained capability used for filesystem operations.
    #[must_use]
    pub const fn directory(&self) -> &MigrationDirectory {
        &self.directory
    }

    /// Return the configured path for diagnostics and operator output only.
    #[must_use]
    pub fn display_path(&self) -> &Path {
        &self.configured_path
    }
}

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
    /// Open the existing migration directory as retained authority.
    pub fn open_migration_directory(
        &self,
    ) -> Result<MigrationDirectoryAuthority, TypeBridgeWorkspaceError> {
        self.open_migration_directory_impl(false)
    }

    /// Create missing real directory components and retain the result as authority.
    ///
    /// Creation is component-relative to already opened parent descriptors;
    /// symbolic links and non-directories are rejected at every step.
    pub fn ensure_migration_directory(
        &self,
    ) -> Result<MigrationDirectoryAuthority, TypeBridgeWorkspaceError> {
        self.open_migration_directory_impl(true)
    }

    fn open_migration_directory_impl(
        &self,
        create: bool,
    ) -> Result<MigrationDirectoryAuthority, TypeBridgeWorkspaceError> {
        let root = self.config().workspace_root().as_path();
        let directory = MigrationDirectory::open_beneath_directory(
            self.root_directory(),
            self.config().migration_v2_directory().as_path(),
            create,
        )
        .map_err(|_| migration_directory_escape())?;
        Ok(MigrationDirectoryAuthority {
            directory,
            configured_path: root.join(self.config().migration_v2_directory().as_path()),
            owner: Arc::clone(self.authority_owner()),
        })
    }

    fn require_migration_directory(
        &self,
        directory: &MigrationDirectoryAuthority,
    ) -> Result<(), TypeBridgeWorkspaceError> {
        let expected = self
            .config()
            .workspace_root()
            .as_path()
            .join(self.config().migration_v2_directory().as_path());
        if !Arc::ptr_eq(&directory.owner, self.authority_owner())
            || directory.configured_path != expected
        {
            return Err(migration_directory_escape());
        }
        Ok(())
    }

    /// Discover and replay-verify the workspace's canonical migration chain.
    pub fn discover_migrations(&self) -> Result<MigrationHistoryGraph, TypeBridgeWorkspaceError> {
        let directory = self.open_migration_directory()?;
        self.discover_migrations_in(&directory)
    }

    /// Discover from one retained directory authority.
    pub fn discover_migrations_in(
        &self,
        directory: &MigrationDirectoryAuthority,
    ) -> Result<MigrationHistoryGraph, TypeBridgeWorkspaceError> {
        self.discover_migrations_with_genesis_in(directory)
            .map(|(graph, _)| graph)
    }

    fn discover_migrations_with_genesis_in(
        &self,
        directory: &MigrationDirectoryAuthority,
    ) -> Result<(MigrationHistoryGraph, DeclaredSchema), TypeBridgeWorkspaceError> {
        self.require_migration_directory(directory)?;
        let adopted_source = read_adopted_genesis_bounded(directory.directory())?;
        let adopted_genesis_present = adopted_source.is_some();
        let declared_bridge_count =
            canonical_history_declared_legacy_bridge_count_in(directory.directory())?;
        require_adoption_authority_pair_state(adopted_genesis_present, declared_bridge_count)?;
        let genesis = self.parse_migration_genesis(adopted_source)?;
        let graph = discover_verified_migration_chain_in(
            directory.directory(),
            &genesis,
            self.delta_context(),
        )?;
        require_adoption_authority_pair(&graph, adopted_genesis_present)?;
        Ok((graph, genesis))
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
        let directory = self.open_migration_directory()?;
        self.migration_make_in(&directory, base_name)
    }

    /// Author against one retained directory authority.
    pub fn migration_make_in(
        &self,
        directory: &MigrationDirectoryAuthority,
        base_name: &str,
    ) -> Result<MigrationGenerationOutcome, TypeBridgeWorkspaceError> {
        let (graph, genesis) = self.discover_migrations_with_genesis_in(directory)?;
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
        let directory = self.open_migration_directory()?;
        self.write_generated_migration_in(&directory, generated)?;
        Ok(directory.display_path().join(generated.file_name()))
    }

    /// Persist through one retained directory authority.
    pub fn write_generated_migration_in(
        &self,
        directory: &MigrationDirectoryAuthority,
        generated: &GeneratedMigration,
    ) -> Result<PathBuf, TypeBridgeWorkspaceError> {
        self.require_migration_directory(directory)?;
        // Discovery, deterministic regeneration, and publication are one
        // serialized authority operation. Locking only the final filenames
        // permits two differently named candidates derived from one stale
        // head to publish as ambiguous siblings.
        let authoring_lock = try_acquire_migration_authoring_lock(directory.directory())?;
        let base_name = generated
            .manifest()
            .id()
            .name()
            .as_str()
            .split_once('_')
            .map(|(_, base_name)| base_name)
            .filter(|base_name| !base_name.is_empty())
            .ok_or_else(generated_migration_stale)?;
        let regenerated = self.migration_make_in(directory, base_name)?;
        let MigrationGenerationOutcome::Generated(regenerated) = regenerated else {
            return Err(generated_migration_stale());
        };
        if regenerated.canonical_bytes() != generated.canonical_bytes() {
            return Err(generated_migration_stale());
        }
        let preview = render_migration_preview(generated.manifest(), self.delta_context())
            .map_err(preview_diagnostic)?;
        Ok(write_generated_migration_under_lock(
            &authoring_lock,
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
        let directory = self.open_migration_directory()?;
        self.migration_plan_in(&directory, applied)
    }

    /// Plan from one retained directory authority.
    pub fn migration_plan_in(
        &self,
        directory: &MigrationDirectoryAuthority,
        applied: &BTreeSet<MigrationId>,
    ) -> Result<Vec<MigrationPlanEntry>, TypeBridgeWorkspaceError> {
        let graph = self.discover_migrations_in(directory)?;
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
        let directory = self.open_migration_directory()?;
        self.migration_genesis_in(&directory)
    }

    /// Read the genesis from one retained directory authority.
    pub fn migration_genesis_in(
        &self,
        directory: &MigrationDirectoryAuthority,
    ) -> Result<DeclaredSchema, TypeBridgeWorkspaceError> {
        self.require_migration_directory(directory)?;
        self.parse_migration_genesis(read_adopted_genesis_bounded(directory.directory())?)
    }

    fn parse_migration_genesis(
        &self,
        source: Option<String>,
    ) -> Result<DeclaredSchema, TypeBridgeWorkspaceError> {
        let Some(source) = source else {
            return Ok(DeclaredSchema::from_facts(
                self.declared_schema().format(),
                CapabilitySet::new(),
                std::iter::empty(),
            )?);
        };
        let document = DocumentId::new(ADOPTED_GENESIS_FILE_NAME)?;
        parse_adopted_genesis(document, &source).map_err(TypeBridgeWorkspaceError::Contract)
    }
}

fn read_adopted_genesis_bounded(
    directory: &MigrationDirectory,
) -> Result<Option<String>, TypeBridgeWorkspaceError> {
    let limit = type_bridge_schema_compat::MAX_TYPEQL_SCHEMA_BYTES;
    let file = match directory.open_regular_readonly(ADOPTED_GENESIS_FILE_NAME.as_ref()) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(adopted_genesis_read_error(
                type_bridge_contract::diagnostic::DiagnosticCategory::Integrity,
                "workspace_adopted_genesis_unreadable",
                "adopted-genesis artifact exists but cannot be read as a regular file",
            ));
        }
    };
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(adopted_genesis_read_error(
            type_bridge_contract::diagnostic::DiagnosticCategory::Integrity,
            "workspace_adopted_genesis_not_regular",
            "adopted-genesis artifact must be a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| {
            adopted_genesis_read_error(
                type_bridge_contract::diagnostic::DiagnosticCategory::Integrity,
                "workspace_adopted_genesis_unreadable",
                "adopted-genesis artifact exists but cannot be read",
            )
        })?;
    if bytes.len() > limit {
        return Err(adopted_genesis_read_error(
            type_bridge_contract::diagnostic::DiagnosticCategory::ResourceLimit,
            "workspace_adopted_genesis_oversized",
            "adopted-genesis artifact exceeds the schema byte ceiling",
        ));
    }
    String::from_utf8(bytes).map(Some).map_err(|_| {
        adopted_genesis_read_error(
            type_bridge_contract::diagnostic::DiagnosticCategory::InvalidContract,
            "workspace_adopted_genesis_not_utf8",
            "adopted-genesis artifact is not valid UTF-8",
        )
    })
}

fn generated_migration_stale() -> TypeBridgeWorkspaceError {
    TypeBridgeWorkspaceError::Contract(Diagnostic::new(
        type_bridge_contract::diagnostic::DiagnosticCategory::Integrity,
        type_bridge_contract::diagnostic::DiagnosticCode::new(
            "workspace_generated_migration_stale",
        )
        .expect("static workspace diagnostic code"),
        "generated migration no longer matches this workspace directory authority",
    ))
}

fn adopted_genesis_read_error(
    category: type_bridge_contract::diagnostic::DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> TypeBridgeWorkspaceError {
    TypeBridgeWorkspaceError::Contract(Diagnostic::new(
        category,
        type_bridge_contract::diagnostic::DiagnosticCode::new(code)
            .expect("static workspace diagnostic code"),
        message,
    ))
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
