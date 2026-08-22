use std::collections::BTreeMap;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_schema_migration::{
    VerifiedMigrationHistoryBundle, encode_verified_migration_history_bundle,
};

use crate::invalid;

/// Fixed language-neutral generated resource containing canonical migration history.
pub const MIGRATION_HISTORY_BUNDLE_RESOURCE: &str = "typebridge/migration-history.json";

/// An ordered in-memory package with normalized relative paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedPackage {
    files: BTreeMap<String, Vec<u8>>,
}

impl GeneratedPackage {
    pub(crate) fn try_new(
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, Diagnostic> {
        let mut ordered = BTreeMap::new();
        for (path, bytes) in files {
            if path.is_empty()
                || path.starts_with('/')
                || path.contains('\\')
                || path.contains('\0')
                || path
                    .split('/')
                    .any(|part| part.is_empty() || matches!(part, "." | ".."))
            {
                return Err(invalid(
                    "invalid_generated_package_path",
                    "generated package paths must be normalized relative paths",
                ));
            }
            if ordered.insert(path, bytes).is_some() {
                return Err(invalid(
                    "duplicate_generated_package_path",
                    "a generated package path may be emitted only once",
                ));
            }
        }
        Ok(Self { files: ordered })
    }

    /// Return files in bytewise path order.
    #[must_use]
    pub const fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }

    /// Borrow one generated file.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    /// Embed one replay-verified canonical migration-history bundle.
    ///
    /// The fixed path and canonical encoder prevent emitters or callers from
    /// substituting binding-specific history bytes or an arbitrary resource.
    pub fn with_migration_history_bundle(
        mut self,
        bundle: &VerifiedMigrationHistoryBundle,
    ) -> Result<Self, Diagnostic> {
        let bytes = encode_verified_migration_history_bundle(bundle)?;
        if self
            .files
            .insert(MIGRATION_HISTORY_BUNDLE_RESOURCE.to_owned(), bytes)
            .is_some()
        {
            return Err(invalid(
                "duplicate_generated_migration_history_resource",
                "an emitter attempted to replace the canonical migration-history resource",
            ));
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_schema_migration::MigrationHistoryGraph;

    #[test]
    fn verified_history_is_embedded_at_one_fixed_canonical_path() {
        let graph = MigrationHistoryGraph::from_verified(std::iter::empty::<
            type_bridge_schema_migration::VerifiedSchemaMigrationManifest,
        >())
        .expect("empty verified graph");
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).expect("empty bundle");
        let expected = encode_verified_migration_history_bundle(&bundle).expect("canonical bytes");
        let package = GeneratedPackage::try_new([("src/lib.rs".to_owned(), Vec::new())])
            .expect("fixture package")
            .with_migration_history_bundle(&bundle)
            .expect("embed bundle");
        assert_eq!(
            package.get(MIGRATION_HISTORY_BUNDLE_RESOURCE),
            Some(expected.as_slice())
        );
    }

    #[test]
    fn emitter_cannot_preoccupy_the_history_authority_path() {
        let graph = MigrationHistoryGraph::from_verified(std::iter::empty::<
            type_bridge_schema_migration::VerifiedSchemaMigrationManifest,
        >())
        .expect("empty verified graph");
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).expect("empty bundle");
        let package = GeneratedPackage::try_new([(
            MIGRATION_HISTORY_BUNDLE_RESOURCE.to_owned(),
            b"foreign".to_vec(),
        )])
        .expect("fixture package");
        let error = package
            .with_migration_history_bundle(&bundle)
            .expect_err("fixed history path cannot be replaced");
        assert_eq!(
            error.code().as_str(),
            "duplicate_generated_migration_history_resource"
        );
    }
}
