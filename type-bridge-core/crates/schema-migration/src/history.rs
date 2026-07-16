//! Canonical V2 discovery and pure migration-history graph planning.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use type_bridge_contract::codec::from_canonical_json;
use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::migration::{MIGRATION_FORMAT_V1, MigrationId};
use type_bridge_contract::schema::DeclaredSchema;
use type_bridge_schema::ManagedDeltaContext;

use crate::manifest::peek_manifest_identity;
use crate::{
    VerifiedSchemaMigrationManifest, decode_verified_manifest,
    encode_verified_manifest,
};

const CANONICAL_MIGRATION_SUFFIX: &str = ".tbmigration.json";

/// A validated DAG whose only node authority is a verified canonical manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationHistoryGraph {
    children: BTreeMap<MigrationId, BTreeSet<MigrationId>>,
    heads: Vec<MigrationId>,
    manifests: BTreeMap<MigrationId, VerifiedSchemaMigrationManifest>,
    parents: BTreeMap<MigrationId, BTreeSet<MigrationId>>,
    topological: Vec<MigrationId>,
}

impl MigrationHistoryGraph {
    /// Build and validate a complete history graph from verified manifests only.
    pub fn from_verified(
        manifests: impl IntoIterator<Item = VerifiedSchemaMigrationManifest>,
    ) -> Result<Self, Diagnostic> {
        let mut by_id = BTreeMap::new();
        for manifest in manifests {
            let id = manifest.id().clone();
            if by_id.insert(id, manifest).is_some() {
                return Err(graph_failure(
                    "migration_history_duplicate_id",
                    "migration history contains a duplicate compound identity",
                ));
            }
        }

        let mut parents = BTreeMap::new();
        let mut children = by_id
            .keys()
            .cloned()
            .map(|id| (id, BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        for (id, manifest) in &by_id {
            let direct = manifest.parents().iter().cloned().collect::<BTreeSet<_>>();
            if direct.contains(id) {
                return Err(graph_failure(
                    "migration_history_self_parent",
                    "migration history contains a self-parent edge",
                ));
            }
            for parent in &direct {
                if !by_id.contains_key(parent) {
                    return Err(graph_failure(
                        "migration_history_missing_parent",
                        "migration history references a missing parent",
                    ));
                }
                children
                    .get_mut(parent)
                    .expect("validated parent has a child set")
                    .insert(id.clone());
            }
            parents.insert(id.clone(), direct);
        }

        let topological = topological_order(&parents, &children)?;
        let heads = children
            .iter()
            .filter(|(_, children)| children.is_empty())
            .map(|(id, _)| id.clone())
            .collect();
        Ok(Self {
            children,
            heads,
            manifests: by_id,
            parents,
            topological,
        })
    }

    /// Return the number of verified history nodes.
    pub fn len(&self) -> usize {
        self.manifests.len()
    }

    /// Return whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }

    /// Return one verified manifest by compound identity.
    pub fn manifest(&self, id: &MigrationId) -> Option<&VerifiedSchemaMigrationManifest> {
        self.manifests.get(id)
    }

    /// Iterate verified manifests in compound-identity order.
    pub fn manifests(
        &self,
    ) -> impl ExactSizeIterator<Item = (&MigrationId, &VerifiedSchemaMigrationManifest)> {
        self.manifests.iter()
    }

    /// Return deterministic topological order with compound-ID ready-node ties.
    pub fn topological_order(&self) -> &[MigrationId] {
        &self.topological
    }

    /// Return every graph head in compound-identity order.
    pub fn heads(&self) -> &[MigrationId] {
        &self.heads
    }

    /// Return the implicit default head, rejecting ambiguous multi-head history.
    pub fn default_head(&self) -> Result<Option<&MigrationId>, Diagnostic> {
        match self.heads.as_slice() {
            [] => Ok(None),
            [head] => Ok(Some(head)),
            _ => Err(graph_failure(
                "migration_history_ambiguous_default_head",
                "implicit default head is ambiguous in a multi-head history",
            )),
        }
    }

    /// Require an applied set to contain every ancestor of every applied node.
    pub fn validate_applied(
        &self,
        applied: &BTreeSet<MigrationId>,
    ) -> Result<(), Diagnostic> {
        for id in applied {
            let direct = self.parents.get(id).ok_or_else(|| {
                graph_failure(
                    "migration_history_unknown_applied_id",
                    "applied set contains an identity outside verified history",
                )
            })?;
            if direct.iter().any(|parent| !applied.contains(parent)) {
                return Err(graph_failure(
                    "migration_history_applied_not_downward_closed",
                    "applied set omits an ancestor of an applied migration",
                ));
            }
        }
        Ok(())
    }

    /// Return maximal applied nodes, which form the applied reachability frontier.
    pub fn applied_frontier(
        &self,
        applied: &BTreeSet<MigrationId>,
    ) -> Result<Vec<MigrationId>, Diagnostic> {
        self.validate_applied(applied)?;
        Ok(applied
            .iter()
            .filter(|id| {
                self.children[*id]
                    .iter()
                    .all(|child| !applied.contains(child))
            })
            .cloned()
            .collect())
    }

    /// Plan the missing ancestor closure for explicit target nodes.
    pub fn plan_apply(
        &self,
        applied: &BTreeSet<MigrationId>,
        targets: &BTreeSet<MigrationId>,
    ) -> Result<Vec<MigrationId>, Diagnostic> {
        self.validate_applied(applied)?;
        let mut closure = BTreeSet::new();
        let mut pending = targets.iter().cloned().collect::<Vec<_>>();
        while let Some(id) = pending.pop() {
            let direct = self.parents.get(&id).ok_or_else(|| {
                graph_failure(
                    "migration_history_unknown_apply_target",
                    "apply target is outside verified history",
                )
            })?;
            if closure.insert(id) {
                pending.extend(direct.iter().cloned());
            }
        }
        let required = closure
            .difference(applied)
            .cloned()
            .collect::<BTreeSet<_>>();
        order_apply_subset(&required, applied, &self.parents, &self.children)
    }

    /// Plan application to the sole implicit head.
    pub fn plan_apply_to_default_head(
        &self,
        applied: &BTreeSet<MigrationId>,
    ) -> Result<Vec<MigrationId>, Diagnostic> {
        let Some(head) = self.default_head()? else {
            self.validate_applied(applied)?;
            return Ok(Vec::new());
        };
        self.plan_apply(applied, &BTreeSet::from([head.clone()]))
    }

    /// Reverse-topologically order an explicit rollback set.
    ///
    /// A node cannot be rolled back while any applied descendant remains.
    pub fn plan_rollback(
        &self,
        applied: &BTreeSet<MigrationId>,
        removals: &BTreeSet<MigrationId>,
    ) -> Result<Vec<MigrationId>, Diagnostic> {
        self.validate_applied(applied)?;
        for id in removals {
            if !self.manifests.contains_key(id) {
                return Err(graph_failure(
                    "migration_history_unknown_rollback_target",
                    "rollback target is outside verified history",
                ));
            }
            if !applied.contains(id) {
                return Err(graph_failure(
                    "migration_history_rollback_not_applied",
                    "rollback target is not in the applied set",
                ));
            }
            if self.children[id]
                .iter()
                .any(|child| applied.contains(child) && !removals.contains(child))
            {
                return Err(graph_failure(
                    "migration_history_remaining_applied_descendant",
                    "rollback would leave an applied descendant without its ancestor",
                ));
            }
        }
        order_rollback_subset(removals, &self.parents, &self.children)
    }
}

/// Discover only direct canonical V2 children and verify each before graph use.
///
/// The callback is the context provider: it must call `decode_verified_manifest`
/// with the honest source/context for the candidate bytes. Discovery additionally
/// requires the returned verified artifact to re-encode byte-identically.
pub fn discover_verified_migrations<F>(
    directory: &Path,
    mut verify: F,
) -> Result<MigrationHistoryGraph, Diagnostic>
where
    F: FnMut(&Path, &[u8]) -> Result<VerifiedSchemaMigrationManifest, Diagnostic>,
{
    let mut verified = Vec::new();
    for candidate in collect_canonical_candidates(directory)? {
        let manifest = verify(&candidate.path, &candidate.bytes)?;
        if encode_verified_manifest(&manifest)? != candidate.bytes {
            return Err(discovery_failure(
                "migration_discovery_verifier_bytes_mismatch",
                "injected verifier returned an artifact for different bytes",
            ));
        }
        require_stem_binding(&manifest, &candidate.stem)?;
        verified.push(manifest);
    }
    MigrationHistoryGraph::from_verified(verified)
}

/// Discover, order, and replay-verify one complete canonical migration chain.
///
/// Each manifest verifies against its authoring source schema: `genesis_source`
/// for parentless manifests, otherwise its parents' verified target. Decoding
/// therefore runs in dependency order regardless of filename order. A manifest
/// with several parents is accepted only when every parent reached the same
/// verified target state; divergent-branch merge sources are rejected until the
/// merge-generation contract defines their recorded source. Every candidate
/// must re-encode byte-identically through `decode_verified_manifest`.
pub fn discover_verified_migration_chain(
    directory: &Path,
    genesis_source: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> Result<MigrationHistoryGraph, Diagnostic> {
    let candidates = collect_canonical_candidates(directory)?;
    let mut headers = Vec::with_capacity(candidates.len());
    let mut index_by_id = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let (id, parents) = peek_manifest_identity(&candidate.bytes)?;
        if index_by_id.insert(id.clone(), index).is_some() {
            return Err(discovery_failure(
                "migration_discovery_duplicate_id",
                "two canonical migration files claim the same migration identity",
            ));
        }
        headers.push((id, parents));
    }
    for (_, parents) in &headers {
        for parent in parents {
            if !index_by_id.contains_key(parent) {
                return Err(discovery_failure(
                    "migration_discovery_unknown_parent",
                    "canonical migration references a parent absent from the directory",
                ));
            }
        }
    }
    let order = order_candidate_headers(&headers, &index_by_id)?;

    let mut verified_by_id: BTreeMap<MigrationId, VerifiedSchemaMigrationManifest> =
        BTreeMap::new();
    for id in order {
        let index = index_by_id[&id];
        let candidate = &candidates[index];
        let parents = &headers[index].1;
        let source = match parents.split_first() {
            None => genesis_source,
            Some((first, rest)) => {
                let first_parent = &verified_by_id[first];
                for parent in rest {
                    if verified_by_id[parent].target_state()
                        != first_parent.target_state()
                    {
                        return Err(discovery_failure(
                            "migration_discovery_divergent_merge_sources",
                            "merge manifest parents reached different verified target states",
                        ));
                    }
                }
                first_parent.target_schema()
            }
        };
        let manifest = decode_verified_manifest(&candidate.bytes, (source, context))?;
        require_stem_binding(&manifest, &candidate.stem)?;
        verified_by_id.insert(id, manifest);
    }
    MigrationHistoryGraph::from_verified(
        verified_by_id.into_values().collect::<Vec<_>>(),
    )
}

fn require_stem_binding(
    manifest: &VerifiedSchemaMigrationManifest,
    stem: &str,
) -> Result<(), Diagnostic> {
    if manifest.id().name().as_str() != stem {
        return Err(discovery_failure(
            "migration_discovery_filename_manifest_mismatch",
            "filename stem does not equal the verified manifest name",
        ));
    }
    Ok(())
}

fn order_candidate_headers(
    headers: &[(MigrationId, Vec<MigrationId>)],
    index_by_id: &BTreeMap<MigrationId, usize>,
) -> Result<Vec<MigrationId>, Diagnostic> {
    let mut parents = BTreeMap::new();
    let mut children: BTreeMap<MigrationId, BTreeSet<MigrationId>> = index_by_id
        .keys()
        .map(|id| (id.clone(), BTreeSet::new()))
        .collect();
    for (id, parent_ids) in headers {
        let parent_set = parent_ids.iter().cloned().collect::<BTreeSet<_>>();
        for parent in &parent_set {
            children
                .get_mut(parent)
                .expect("unknown parents are rejected before ordering")
                .insert(id.clone());
        }
        parents.insert(id.clone(), parent_set);
    }
    let all = parents.keys().cloned().collect::<BTreeSet<_>>();
    order_apply_subset(&all, &BTreeSet::new(), &parents, &children)
}

struct CanonicalCandidate {
    path: PathBuf,
    stem: String,
    bytes: Vec<u8>,
}

fn collect_canonical_candidates(
    directory: &Path,
) -> Result<Vec<CanonicalCandidate>, Diagnostic> {
    let read_dir = fs::read_dir(directory).map_err(|_| {
        discovery_failure(
            "migration_discovery_directory_unreadable",
            "canonical migration directory cannot be read",
        )
    })?;
    let mut entries = read_dir
        .map(|entry| {
            entry.map_err(|_| {
                discovery_failure(
                    "migration_discovery_entry_unreadable",
                    "canonical migration directory entry cannot be read",
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    let mut candidates = Vec::new();
    for entry in entries {
        let file_type = entry.file_type().map_err(|_| {
            discovery_failure(
                "migration_discovery_entry_unreadable",
                "canonical migration entry type cannot be read",
            )
        })?;
        if file_type.is_dir() {
            return Err(discovery_failure(
                "migration_discovery_nested_authority",
                "nested directories cannot contain canonical migration authority",
            ));
        }
        if !file_type.is_file() {
            return Err(discovery_failure(
                "migration_discovery_non_regular_entry",
                "canonical migration directory contains a non-regular entry",
            ));
        }
        let file_name = entry.file_name().into_string().map_err(|_| {
            discovery_failure(
                "migration_discovery_non_utf8_filename",
                "canonical migration filename is not valid UTF-8",
            )
        })?;
        let Some(stem) = file_name.strip_suffix(CANONICAL_MIGRATION_SUFFIX) else {
            continue;
        };
        if stem.is_empty() {
            return Err(discovery_failure(
                "migration_discovery_empty_stem",
                "canonical migration filename has an empty manifest-name stem",
            ));
        }
        let path = entry.path();
        let bytes = fs::read(&path).map_err(|_| {
            discovery_failure(
                "migration_discovery_file_unreadable",
                "canonical migration file cannot be read",
            )
        })?;
        sniff_v1_format(&bytes)?;
        candidates.push(CanonicalCandidate {
            path,
            stem: stem.to_owned(),
            bytes,
        });
    }
    Ok(candidates)
}

fn sniff_v1_format(bytes: &[u8]) -> Result<(), Diagnostic> {
    let value = from_canonical_json::<Value>(bytes)?;
    let format = value
        .as_object()
        .and_then(|object| object.get("format"))
        .and_then(Value::as_str);
    if format != Some(MIGRATION_FORMAT_V1) {
        return Err(discovery_failure(
            "migration_discovery_unknown_format",
            "canonical migration format is absent or unsupported",
        ));
    }
    Ok(())
}

fn topological_order(
    parents: &BTreeMap<MigrationId, BTreeSet<MigrationId>>,
    children: &BTreeMap<MigrationId, BTreeSet<MigrationId>>,
) -> Result<Vec<MigrationId>, Diagnostic> {
    let all = parents.keys().cloned().collect::<BTreeSet<_>>();
    let applied = BTreeSet::new();
    let order = order_apply_subset(&all, &applied, parents, children)?;
    if order.len() != parents.len() {
        return Err(graph_failure(
            "migration_history_cycle",
            "migration history contains a parent cycle",
        ));
    }
    Ok(order)
}

fn order_apply_subset(
    required: &BTreeSet<MigrationId>,
    applied: &BTreeSet<MigrationId>,
    parents: &BTreeMap<MigrationId, BTreeSet<MigrationId>>,
    children: &BTreeMap<MigrationId, BTreeSet<MigrationId>>,
) -> Result<Vec<MigrationId>, Diagnostic> {
    let mut remaining = required
        .iter()
        .map(|id| {
            let count = parents[id]
                .iter()
                .filter(|parent| required.contains(*parent) && !applied.contains(*parent))
                .count();
            (id.clone(), count)
        })
        .collect::<BTreeMap<_, _>>();
    let mut ready = remaining
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(required.len());
    while let Some(id) = ready.iter().next().cloned() {
        ready.remove(&id);
        order.push(id.clone());
        for child in &children[&id] {
            if let Some(count) = remaining.get_mut(child) {
                *count -= 1;
                if *count == 0 {
                    ready.insert(child.clone());
                }
            }
        }
        remaining.remove(&id);
    }
    if !remaining.is_empty() {
        return Err(graph_failure(
            "migration_history_cycle",
            "migration history contains a parent cycle",
        ));
    }
    Ok(order)
}

fn order_rollback_subset(
    removals: &BTreeSet<MigrationId>,
    parents: &BTreeMap<MigrationId, BTreeSet<MigrationId>>,
    children: &BTreeMap<MigrationId, BTreeSet<MigrationId>>,
) -> Result<Vec<MigrationId>, Diagnostic> {
    let mut remaining = removals
        .iter()
        .map(|id| {
            (
                id.clone(),
                children[id]
                    .iter()
                    .filter(|child| removals.contains(*child))
                    .count(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut ready = remaining
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(removals.len());
    while let Some(id) = ready.iter().next().cloned() {
        ready.remove(&id);
        order.push(id.clone());
        for parent in &parents[&id] {
            if let Some(count) = remaining.get_mut(parent) {
                *count -= 1;
                if *count == 0 {
                    ready.insert(parent.clone());
                }
            }
        }
        remaining.remove(&id);
    }
    if !remaining.is_empty() {
        return Err(graph_failure(
            "migration_history_cycle",
            "rollback subset contains a parent cycle",
        ));
    }
    Ok(order)
}

fn graph_failure(code: &'static str, message: &'static str) -> Diagnostic {
    failure(DiagnosticCategory::Integrity, code, message)
}

fn discovery_failure(code: &'static str, message: &'static str) -> Diagnostic {
    failure(DiagnosticCategory::InvalidContract, code, message)
}

fn failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static history diagnostic code is canonical"),
        message,
    )
}
