use std::collections::BTreeMap;

use type_bridge_contract::diagnostic::Diagnostic;

use crate::invalid;

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
                || path.split('/').any(|part| part.is_empty() || matches!(part, "." | ".."))
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
    pub const fn files(&self) -> &BTreeMap<String, Vec<u8>> { &self.files }

    /// Borrow one generated file.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }
}
