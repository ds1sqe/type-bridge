//! Offline desired-state migration generation.
//!
//! Generation is the authoring half of the migration engine: the committed
//! head's verified target schema is the source, the desired schema compiled
//! from schema sources is the target, and the minimal fact delta between them
//! becomes the next canonical manifest. Everything derives from the inputs —
//! no timestamps, no environment — so a clean checkout regenerates identical
//! bytes.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::migration::{
    MigrationId, MigrationStep, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::AssertionExpectation;
use type_bridge_contract::schema::{DeclaredSchema, SchemaDelta};
use type_bridge_query::{MigrationAssertionValidationContext, lower_condition_to_plan};
use type_bridge_schema::{
    ManagedDeltaContext, SafetyClass, SafetyDerivationProfile, apply_delta,
    derive_safety_conditions, diff_managed, inverse_delta, managed_schema_state, resolve,
};

use crate::history::MigrationHistoryGraph;
use crate::lowering::{
    SchemaFactCatalog, SchemaLoweringBinding, SchemaLoweringDiagnostic,
    lower_schema_delta_with_verified_assertions,
};
use crate::manifest::{
    SchemaMigrationDraft, VerifiedSchemaMigrationManifest, build_verified_manifest,
    delta_diagnostic, encode_verified_manifest, verify_assertion_coverage,
};
use crate::profile::schema_lowering_profile_binding;
use type_bridge_contract::managed_scope::SemanticProfileBinding;

/// Manifest build rejections that only indict the claimed reverse program.
///
/// A structural inverse always exists, but it is recorded only when the
/// verifier accepts it as a real semantic rollback; these codes downgrade the
/// draft to an irreversible manifest instead of failing generation.
const REVERSE_REJECTION_CODES: [&str; 4] = [
    "migration_manifest_reverse_unresolved_safety",
    "migration_manifest_reverse_requires_assertions",
    "migration_manifest_inverse_replay_mismatch",
    "migration_manifest_inverse_plan_invalid",
];

/// Inputs authoring the next migration for one application lineage.
#[derive(Clone, Copy, Debug)]
pub struct MigrationGenerationRequest<'a> {
    /// Durable application label binding the migration lineage.
    pub app_label: &'a str,
    /// Author-supplied descriptive name; the ordinal prefix is allocated.
    pub base_name: &'a str,
    /// Exact declared source when history is empty.
    pub genesis_source: &'a DeclaredSchema,
    /// Desired schema compiled from the current schema sources.
    pub desired: &'a DeclaredSchema,
    /// Managed scope, semantic profile, and available capabilities.
    pub context: &'a ManagedDeltaContext,
}

/// Result of one offline generation attempt.
#[derive(Clone, Debug)]
pub enum MigrationGenerationOutcome {
    /// The head target already equals the desired schema.
    UpToDate,
    /// A new verified manifest was authored.
    Generated(Box<GeneratedMigration>),
}

/// A freshly authored manifest with its exact canonical persistence bytes.
#[derive(Clone, Debug)]
pub struct GeneratedMigration {
    manifest: VerifiedSchemaMigrationManifest,
    canonical_bytes: Vec<u8>,
}

impl GeneratedMigration {
    /// Return the verified manifest.
    pub const fn manifest(&self) -> &VerifiedSchemaMigrationManifest {
        &self.manifest
    }

    /// Return the exact canonical manifest bytes to persist.
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    /// Return the stem-bound canonical file name for this manifest.
    pub fn file_name(&self) -> String {
        format!("{}.tbmigration.json", self.manifest.id().name().as_str())
    }

    /// Return the review-only TypeQL preview file name for this manifest.
    pub fn preview_file_name(&self) -> String {
        format!("{}.typeql", self.manifest.id().name().as_str())
    }
}

/// Failure rendering a review-only TypeQL preview for a generated manifest.
#[derive(Clone, Debug)]
pub enum MigrationPreviewError {
    /// Verification or replay of the manifest steps failed.
    Diagnostic(Diagnostic),
    /// Provider lowering rejected a statement unit.
    Lowering(SchemaLoweringDiagnostic),
}

impl fmt::Display for MigrationPreviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostic(diagnostic) => diagnostic.fmt(formatter),
            Self::Lowering(diagnostic) => diagnostic.fmt(formatter),
        }
    }
}

impl Error for MigrationPreviewError {}

impl From<Diagnostic> for MigrationPreviewError {
    fn from(value: Diagnostic) -> Self {
        Self::Diagnostic(value)
    }
}

impl From<SchemaLoweringDiagnostic> for MigrationPreviewError {
    fn from(value: SchemaLoweringDiagnostic) -> Self {
        Self::Lowering(value)
    }
}

/// Author the next verified migration from the discovered history graph.
///
/// The source schema is the sole head's verified target (genesis when the
/// graph is empty); a multi-head history is refused until an explicit merge
/// migration joins it. Conditional operations receive verifier-derived
/// assertion steps; the structural inverse is recorded only when the
/// verifier accepts it as a real rollback program.
pub fn generate_next_migration(
    graph: &MigrationHistoryGraph,
    request: &MigrationGenerationRequest<'_>,
) -> Result<MigrationGenerationOutcome, Diagnostic> {
    for (id, _) in graph.manifests() {
        if id.app_label().as_str() != request.app_label {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_generation_foreign_app_label",
                "history contains a migration from a different application lineage",
            ));
        }
    }

    let (source, parents) = match graph.default_head()? {
        None => (request.genesis_source, Vec::new()),
        Some(head) => (
            graph
                .manifest(head)
                .expect("a graph head is always a graph member")
                .target_schema(),
            vec![head.clone()],
        ),
    };

    let source_state = managed_schema_state(source, request.context).map_err(delta_diagnostic)?;
    let desired_state =
        managed_schema_state(request.desired, request.context).map_err(delta_diagnostic)?;
    if source_state == desired_state {
        return Ok(MigrationGenerationOutcome::UpToDate);
    }
    let delta = diff_managed(source, request.desired, request.context).map_err(delta_diagnostic)?;

    let id = MigrationId::new(request.app_label, allocate_name(graph, request.base_name))?;
    if graph.manifest(&id).is_some() {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_generation_duplicate_name",
            "allocated migration identity already exists in verified history",
        ));
    }

    let steps = author_steps(&delta, source, request.desired, request.context, true)?;
    let draft = SchemaMigrationDraft::new(id.clone(), parents.clone(), steps)?;
    let manifest = match build_verified_manifest(draft, (source, request.context)) {
        Ok(manifest) => manifest,
        Err(diagnostic) if REVERSE_REJECTION_CODES.contains(&diagnostic.code().as_str()) => {
            let steps = author_steps(&delta, source, request.desired, request.context, false)?;
            let draft = SchemaMigrationDraft::new(id, parents, steps)?;
            build_verified_manifest(draft, (source, request.context))?
        }
        Err(diagnostic) => return Err(diagnostic),
    };
    let canonical_bytes = encode_verified_manifest(&manifest)?;
    Ok(MigrationGenerationOutcome::Generated(Box::new(
        GeneratedMigration {
            manifest,
            canonical_bytes,
        },
    )))
}

/// Render the review-only TypeQL preview of a verified manifest.
///
/// The preview replays the manifest steps and lowers every schema delta under
/// the context's capabilities. Destructive units render without an approval —
/// the preview is exactly what an operator inspects before granting one —
/// while classes no approval can execute (backfill, opaque) surface their
/// gate diagnostic instead of producing a misleading partial preview.
pub fn render_migration_preview(
    manifest: &VerifiedSchemaMigrationManifest,
    context: &ManagedDeltaContext,
) -> Result<String, MigrationPreviewError> {
    let binding = SchemaLoweringBinding::current(context.available_capabilities().clone())?;
    let safety_profile = SafetyDerivationProfile::new(
        manifest.semantic_profile().clone(),
        manifest.lowering_profile().clone(),
    )?;
    let mut current = manifest.source_schema().clone();
    let mut pending = Vec::new();
    let mut queries = Vec::new();
    for step in manifest.steps() {
        let Some(schema_step) = step.as_schema_delta() else {
            pending.push(step);
            continue;
        };
        let target =
            apply_delta(&current, schema_step.delta(), context).map_err(delta_diagnostic)?;
        let coverage = verify_assertion_coverage(
            &pending,
            schema_step.delta(),
            &current,
            &target,
            &safety_profile,
        )?;
        pending.clear();
        let source_catalog = SchemaFactCatalog::new(current.facts().cloned())?;
        let target_catalog = SchemaFactCatalog::new(target.facts().cloned())?;
        let lowering = lower_schema_delta_with_verified_assertions(
            schema_step.delta(),
            &source_catalog,
            &target_catalog,
            &binding,
            coverage.conditional_operation_indices(),
            true,
        )?;
        for unit in lowering.units() {
            for statement in unit.statements() {
                queries.push(statement.query().to_owned());
            }
        }
        current = target;
    }
    Ok(format!("{}\n", queries.join("\n\n")))
}

/// Persist a generated manifest and its TypeQL preview into a directory.
///
/// Both files are created exclusively; an existing manifest or preview file
/// is a conflict, never an overwrite. Returns the manifest path.
pub fn write_generated_migration(
    directory: &Path,
    generated: &GeneratedMigration,
    preview: &str,
) -> Result<PathBuf, Diagnostic> {
    let manifest_path = directory.join(generated.file_name());
    let preview_path = directory.join(generated.preview_file_name());
    create_exclusive(&manifest_path, generated.canonical_bytes())?;
    if let Err(diagnostic) = create_exclusive(&preview_path, preview.as_bytes()) {
        let _ = fs::remove_file(&manifest_path);
        return Err(diagnostic);
    }
    Ok(manifest_path)
}

fn create_exclusive(path: &Path, bytes: &[u8]) -> Result<(), Diagnostic> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_generation_write_conflict",
                    "generated migration file already exists in the target directory",
                )
            } else {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_generation_write_failed",
                    "generated migration file could not be created",
                )
            }
        })?;
    file.write_all(bytes).map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "migration_generation_write_failed",
            "generated migration file could not be written completely",
        )
    })
}

/// Allocate the next ordinal-prefixed name in the lineage's local convention.
///
/// Ordinal prefixes are display/allocation conventions, never execution
/// order; names without a numeric prefix simply do not participate.
fn allocate_name(graph: &MigrationHistoryGraph, base_name: &str) -> String {
    let next = graph
        .manifests()
        .filter_map(|(id, _)| leading_ordinal(id.name().as_str()))
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    format!("{next:04}_{base_name}")
}

fn leading_ordinal(name: &str) -> Option<u64> {
    let (digits, _) = name.split_once('_')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Author verifier-derived assertion steps followed by the single delta step.
fn author_steps(
    delta: &SchemaDelta,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
    with_reverse: bool,
) -> Result<Vec<MigrationStep>, Diagnostic> {
    let safety_profile = SafetyDerivationProfile::new(
        SemanticProfileBinding::resolve(context.semantic_profile().clone())?,
        schema_lowering_profile_binding()?,
    )?;
    let resolved = resolve(source, context.semantic_profile()).map_err(|diagnostics| {
        diagnostics
            .iter()
            .next()
            .map(|diagnostic| diagnostic.diagnostic().clone())
            .unwrap_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_generation_resolution_failed",
                    "assertion source resolution failed without a diagnostic",
                )
            })
    })?;
    let source_state = managed_schema_state(source, context).map_err(delta_diagnostic)?;
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &source_state);

    let mut steps = Vec::new();
    for (ordinal, operation) in delta.operations().iter().enumerate() {
        let derived =
            derive_safety_conditions(ordinal, operation, source, target, &safety_profile)?;
        // Only conditional requirements become assertions. Destructive guard
        // conditions are deliberately omitted: an approved destructive
        // migration means the data loss is intended, and a NoRows guard would
        // convert it into refuse-if-populated.
        let required = derived
            .conditions()
            .iter()
            .filter(|condition| condition.policy() == SafetyClass::Conditional);
        for (index, condition) in required.enumerate() {
            let validated = lower_condition_to_plan(
                condition,
                &validation_context,
                StructuralLimits::CANONICAL,
            )?;
            steps.push(MigrationStep::assertion(
                MigrationStepId::new(format!("assert-{ordinal}-{index}"))?,
                validated.plan().clone(),
                AssertionExpectation::NoRows,
            )?);
        }
    }
    let reverse = if with_reverse {
        Some(inverse_delta(delta).map_err(delta_diagnostic)?)
    } else {
        None
    };
    steps.push(MigrationStep::from(SchemaDeltaStep::new(
        MigrationStepId::new("schema-delta")?,
        delta.clone(),
        reverse,
    )?));
    Ok(steps)
}

fn failure(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static generation diagnostic code is canonical"),
        message,
    )
}
