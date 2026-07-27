//! Trusted, reversible schema transitions over one durable managed scope.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::capability::{CapabilityId, CapabilitySet};
use crate::codec::{FormatVersion, ensure_format_version, to_canonical_json};
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::limits::MAX_CANONICAL_COLLECTION_LEN;
use crate::managed_scope::ManagedScopeBinding;
use crate::schema::{DeclaredIdentityFingerprint, SchemaFact, SchemaFactId};
use crate::schema_fingerprint::{
    ManagedDeclaredIdentityFingerprint, ManagedSemanticSchemaFingerprint,
};

/// Transition capability required only when a patch uses provider-native redefinition.
pub const SCHEMA_REDEFINE_CAPABILITY: &str = "schema.redefine";

/// The format version of a canonical schema patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct PatchFormatVersion(u16);

impl PatchFormatVersion {
    /// The first canonical schema-patch format.
    pub const V1: Self = Self(1);

    /// Return the raw version number.
    pub const fn get(self) -> u16 {
        self.0
    }

    pub(crate) fn from_wire(value: u16) -> Result<Self, Diagnostic> {
        if value == Self::V1.get() {
            Ok(Self::V1)
        } else {
            Err(delta_diagnostic(
                DiagnosticCategory::InvalidContract,
                "unsupported_patch_format_version",
                "schema patch format version is not supported",
            )
            .with_detail("actual", i64::from(value))
            .with_detail("supported", i64::from(Self::V1.get())))
        }
    }
}

/// Deterministically ordered identities owned by one managed schema scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ManagedFactSelection(BTreeSet<SchemaFactId>);

impl ManagedFactSelection {
    /// Build a bounded selection, rejecting duplicate fact identities.
    pub fn new(fact_ids: impl IntoIterator<Item = SchemaFactId>) -> Result<Self, Diagnostic> {
        let mut selection = BTreeSet::new();
        for fact_id in fact_ids {
            if !selection.insert(fact_id) {
                return Err(delta_diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "duplicate_managed_fact_id",
                    "managed fact selection contains a duplicate identity",
                ));
            }
            if selection.len() > MAX_CANONICAL_COLLECTION_LEN {
                return Err(delta_diagnostic(
                    DiagnosticCategory::ResourceLimit,
                    "too_many_managed_fact_ids",
                    "managed fact selection exceeds the canonical collection limit",
                ));
            }
        }
        Ok(Self(selection))
    }

    /// Return an empty managed selection.
    #[must_use]
    pub fn empty() -> Self {
        Self(BTreeSet::new())
    }

    /// Return whether this selection contains one fact identity.
    pub fn contains(&self, fact_id: &SchemaFactId) -> bool {
        self.0.contains(fact_id)
    }

    /// Return the number of selected fact identities.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Return whether no facts are selected.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate in stable fact-identity order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &SchemaFactId> {
        self.0.iter()
    }
}

/// Fingerprint-bound state of one explicitly selected managed schema scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManagedSchemaState {
    declared_identity: DeclaredIdentityFingerprint,
    format: FormatVersion,
    managed_declared_identity: ManagedDeclaredIdentityFingerprint,
    managed_semantic_schema: ManagedSemanticSchemaFingerprint,
    required_capabilities: CapabilitySet,
    scope: ManagedScopeBinding,
    selection: ManagedFactSelection,
}

impl ManagedSchemaState {
    /// Bind one exact declaration, selection, and its managed fingerprint views.
    pub fn new(
        format: FormatVersion,
        required_capabilities: CapabilitySet,
        scope: ManagedScopeBinding,
        selection: ManagedFactSelection,
        declared_identity: DeclaredIdentityFingerprint,
        managed_declared_identity: ManagedDeclaredIdentityFingerprint,
        managed_semantic_schema: ManagedSemanticSchemaFingerprint,
    ) -> Result<Self, Diagnostic> {
        ensure_format_version(format, FormatVersion::V1)?;
        Ok(Self {
            declared_identity,
            format,
            managed_declared_identity,
            managed_semantic_schema,
            required_capabilities,
            scope,
            selection,
        })
    }

    /// Return the owning schema format.
    pub const fn format(&self) -> FormatVersion {
        self.format
    }

    /// Return capabilities required by the selected schema state.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Return the durable managed-scope binding.
    pub const fn scope(&self) -> &ManagedScopeBinding {
        &self.scope
    }

    /// Return the exact selected fact identities.
    pub const fn selection(&self) -> &ManagedFactSelection {
        &self.selection
    }

    /// Return the full declared-schema identity used by resolution.
    pub const fn declared_identity(&self) -> &DeclaredIdentityFingerprint {
        &self.declared_identity
    }

    /// Return the managed declared-identity fingerprint.
    pub const fn managed_declared_identity(&self) -> &ManagedDeclaredIdentityFingerprint {
        &self.managed_declared_identity
    }

    /// Return the managed semantic-schema fingerprint.
    pub const fn managed_semantic_schema(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.managed_semantic_schema
    }
}

/// Public discriminator for an opaque schema operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaOperationKind {
    /// Define one non-empty, identity-sorted fact group.
    Define,
    /// Replace one fact with an unequal payload under the same identity.
    Redefine,
    /// Remove one exact fact.
    Undefine,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SchemaOperationData {
    Define {
        facts: Vec<SchemaFact>,
    },
    Redefine {
        expected: Box<SchemaFact>,
        replacement: Box<SchemaFact>,
    },
    Undefine {
        fact: SchemaFact,
    },
}

/// One validated schema transition with an exact offline inverse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SchemaOperation(SchemaOperationData);

impl SchemaOperation {
    /// Define a non-empty fact group, canonicalized into fact-identity order.
    pub fn define(mut facts: Vec<SchemaFact>) -> Result<Self, Diagnostic> {
        if facts.is_empty() {
            return Err(delta_diagnostic(
                DiagnosticCategory::InvalidContract,
                "empty_schema_define",
                "schema define operation requires at least one fact",
            ));
        }
        if facts.len() > MAX_CANONICAL_COLLECTION_LEN {
            return Err(delta_diagnostic(
                DiagnosticCategory::ResourceLimit,
                "too_many_schema_define_facts",
                "schema define operation exceeds the canonical collection limit",
            ));
        }
        facts.sort_by_key(SchemaFact::id);
        if facts.windows(2).any(|pair| pair[0].id() == pair[1].id()) {
            return Err(delta_diagnostic(
                DiagnosticCategory::InvalidContract,
                "duplicate_schema_operation_fact_id",
                "schema define operation contains duplicate fact identities",
            ));
        }
        Ok(Self(SchemaOperationData::Define { facts }))
    }

    /// Redefine an existing fact without changing its stable identity.
    pub fn redefine(expected: SchemaFact, replacement: SchemaFact) -> Result<Self, Diagnostic> {
        if expected.id() != replacement.id() {
            return Err(delta_diagnostic(
                DiagnosticCategory::InvalidContract,
                "schema_redefine_identity_mismatch",
                "schema redefinition must preserve the fact identity",
            ));
        }
        if expected == replacement {
            return Err(delta_diagnostic(
                DiagnosticCategory::InvalidContract,
                "schema_redefine_noop",
                "schema redefinition requires an unequal replacement payload",
            ));
        }
        Ok(Self(SchemaOperationData::Redefine {
            expected: Box::new(expected),
            replacement: Box::new(replacement),
        }))
    }

    /// Undefine one exact fact.
    #[must_use]
    pub const fn undefine(fact: SchemaFact) -> Self {
        Self(SchemaOperationData::Undefine { fact })
    }

    /// Return the operation discriminator.
    #[must_use]
    pub const fn kind(&self) -> SchemaOperationKind {
        match &self.0 {
            SchemaOperationData::Define { .. } => SchemaOperationKind::Define,
            SchemaOperationData::Redefine { .. } => SchemaOperationKind::Redefine,
            SchemaOperationData::Undefine { .. } => SchemaOperationKind::Undefine,
        }
    }

    /// Return defined facts when this is a define operation.
    pub fn defined_facts(&self) -> Option<&[SchemaFact]> {
        match &self.0 {
            SchemaOperationData::Define { facts } => Some(facts),
            _ => None,
        }
    }

    /// Return the expected current fact when this is a redefinition.
    pub fn expected_fact(&self) -> Option<&SchemaFact> {
        match &self.0 {
            SchemaOperationData::Redefine { expected, .. } => Some(expected),
            _ => None,
        }
    }

    /// Return the replacement fact when this is a redefinition.
    pub fn replacement_fact(&self) -> Option<&SchemaFact> {
        match &self.0 {
            SchemaOperationData::Redefine { replacement, .. } => Some(replacement),
            _ => None,
        }
    }

    /// Return the exact removed fact when this is an undefinition.
    pub const fn undefined_fact(&self) -> Option<&SchemaFact> {
        match &self.0 {
            SchemaOperationData::Undefine { fact } => Some(fact),
            _ => None,
        }
    }

    /// Return all affected identities in canonical order for this operation.
    #[must_use]
    pub fn affected_ids(&self) -> Vec<SchemaFactId> {
        match &self.0 {
            SchemaOperationData::Define { facts } => facts.iter().map(SchemaFact::id).collect(),
            SchemaOperationData::Redefine { expected, .. } => vec![expected.id()],
            SchemaOperationData::Undefine { fact } => vec![fact.id()],
        }
    }

    /// Derive exact inverse operations; grouped definitions invert in reverse order.
    #[must_use]
    pub fn inverse(&self) -> Vec<Self> {
        match &self.0 {
            SchemaOperationData::Define { facts } => {
                facts.iter().rev().cloned().map(Self::undefine).collect()
            }
            SchemaOperationData::Redefine {
                expected,
                replacement,
            } => vec![
                Self::redefine(replacement.as_ref().clone(), expected.as_ref().clone())
                    .expect("an inverse redefinition preserves a validated unequal identity"),
            ],
            SchemaOperationData::Undefine { fact } => vec![
                Self::define(vec![fact.clone()])
                    .expect("a singleton inverse definition is always non-empty and unique"),
            ],
        }
    }
}

/// One immutable ordered patch between two exact managed schema states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SchemaDelta {
    format: PatchFormatVersion,
    operations: Vec<SchemaOperation>,
    required_capabilities: CapabilitySet,
    source: ManagedSchemaState,
    target: ManagedSchemaState,
}

impl SchemaDelta {
    /// Validate a complete managed-state transition and derive its capabilities.
    pub fn new(
        format: PatchFormatVersion,
        source: ManagedSchemaState,
        target: ManagedSchemaState,
        operations: Vec<SchemaOperation>,
    ) -> Result<Self, Diagnostic> {
        PatchFormatVersion::from_wire(format.get())?;
        if source.format() != target.format() {
            return Err(delta_diagnostic(
                DiagnosticCategory::InvalidContract,
                "schema_delta_format_transition",
                "schema delta source and target formats must match exactly",
            ));
        }
        if source.scope() != target.scope() {
            return Err(delta_diagnostic(
                DiagnosticCategory::InvalidContract,
                "schema_delta_scope_transition",
                "schema delta cannot change its durable managed-scope binding",
            ));
        }
        if source
            .managed_semantic_schema()
            .as_fingerprint()
            .semantic_profile()
            != target
                .managed_semantic_schema()
                .as_fingerprint()
                .semantic_profile()
        {
            return Err(delta_diagnostic(
                DiagnosticCategory::InvalidContract,
                "schema_delta_semantic_profile_transition",
                "schema delta cannot cross semantic-profile identities",
            ));
        }
        if source == target {
            return Err(delta_diagnostic(
                DiagnosticCategory::InvalidContract,
                "schema_delta_noop",
                "schema delta source and target states must differ",
            ));
        }
        if operations.is_empty()
            && (source.selection() != target.selection()
                || source.required_capabilities() == target.required_capabilities()
                || source.managed_declared_identity() == target.managed_declared_identity()
                || source.managed_semantic_schema() == target.managed_semantic_schema())
        {
            return Err(delta_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_schema_delta_capability_transition",
                "operation-free schema deltas require an unchanged selection plus distinct capabilities and managed fingerprints",
            ));
        }
        if operations.len() > MAX_CANONICAL_COLLECTION_LEN {
            return Err(delta_diagnostic(
                DiagnosticCategory::ResourceLimit,
                "too_many_schema_operations",
                "schema delta exceeds the canonical operation limit",
            ));
        }
        let mut affected = BTreeSet::new();
        let mut transitioned = source.selection.0.clone();
        for operation in &operations {
            for fact_id in operation.affected_ids() {
                if !affected.insert(fact_id) {
                    return Err(delta_diagnostic(
                        DiagnosticCategory::InvalidContract,
                        "duplicate_schema_delta_fact_id",
                        "schema delta operations affect one fact identity more than once",
                    ));
                }
            }
            match &operation.0 {
                SchemaOperationData::Define { facts } => {
                    for fact in facts {
                        if !transitioned.insert(fact.id()) {
                            return Err(delta_diagnostic(
                                DiagnosticCategory::InvalidContract,
                                "schema_delta_define_existing_fact",
                                "schema delta defines an identity already present in its source selection",
                            ));
                        }
                    }
                }
                SchemaOperationData::Redefine { expected, .. } => {
                    if !transitioned.contains(&expected.id()) {
                        return Err(delta_diagnostic(
                            DiagnosticCategory::InvalidContract,
                            "schema_delta_redefine_missing_fact",
                            "schema delta redefines an identity absent from its source selection",
                        ));
                    }
                }
                SchemaOperationData::Undefine { fact } => {
                    if !transitioned.remove(&fact.id()) {
                        return Err(delta_diagnostic(
                            DiagnosticCategory::InvalidContract,
                            "schema_delta_undefine_missing_fact",
                            "schema delta undefines an identity absent from its source selection",
                        ));
                    }
                }
            }
        }
        if transitioned != target.selection.0 {
            return Err(delta_diagnostic(
                DiagnosticCategory::InvalidContract,
                "schema_delta_selection_mismatch",
                "schema operations do not produce the declared target managed selection",
            ));
        }

        let mut required_capabilities = source
            .required_capabilities()
            .iter()
            .chain(target.required_capabilities().iter())
            .cloned()
            .collect::<CapabilitySet>();
        if operations
            .iter()
            .any(|operation| operation.kind() == SchemaOperationKind::Redefine)
        {
            required_capabilities.insert(CapabilityId::new(SCHEMA_REDEFINE_CAPABILITY)?);
        }

        Ok(Self {
            format,
            operations,
            required_capabilities,
            source,
            target,
        })
    }

    /// Return the schema-patch format.
    pub const fn format(&self) -> PatchFormatVersion {
        self.format
    }

    /// Return the exact source-state precondition.
    pub const fn source(&self) -> &ManagedSchemaState {
        &self.source
    }

    /// Return the exact target state.
    pub const fn target(&self) -> &ManagedSchemaState {
        &self.target
    }

    /// Return operations in caller-preserved transition order.
    pub fn operations(&self) -> &[SchemaOperation] {
        &self.operations
    }

    /// Return capabilities derived from both states and the transition table.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Encode this trusted delta as exact canonical JSON.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        encode_schema_delta(self)
    }
}

/// Encode only a previously validated trusted schema delta.
pub fn encode_schema_delta(delta: &SchemaDelta) -> Result<Vec<u8>, Diagnostic> {
    to_canonical_json(delta)
}

/// Decode canonical bytes through private wire DTOs and every trusted constructor.
pub fn decode_schema_delta(bytes: &[u8]) -> Result<SchemaDelta, Diagnostic> {
    crate::schema_delta_wire::decode_schema_delta(bytes)
}

fn delta_diagnostic(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::stable(category, code, message)
}
