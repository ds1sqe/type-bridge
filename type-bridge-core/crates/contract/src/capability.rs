//! Open capability identifiers and deterministic capability sets.

use std::collections::BTreeSet;
use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticDetailValue};

/// Maximum ASCII byte length of one namespaced capability identifier.
pub const MAX_CAPABILITY_ID_BYTES: usize = 255;

/// An open validated namespaced capability identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Validate an identifier such as `query.given-multi-row`.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        let segments = value.split('.').collect::<Vec<_>>();
        let valid_segment = |segment: &str| {
            let mut bytes = segment.bytes();
            bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
                && bytes.all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_')
                })
        };
        if value.len() <= MAX_CAPABILITY_ID_BYTES
            && segments.len() >= 2
            && segments.iter().all(|s| valid_segment(s))
        {
            Ok(Self(value))
        } else {
            Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "malformed_capability_id",
                "capability ID must be a bounded lowercase namespaced identifier",
            ))
        }
    }
    /// Return the canonical identifier spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
impl Serialize for CapabilityId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for CapabilityId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// A deterministically ordered open capability set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet(BTreeSet<CapabilityId>);

impl CapabilitySet {
    /// Construct an empty set.
    pub const fn new() -> Self {
        Self(BTreeSet::new())
    }
    /// Insert one capability.
    pub fn insert(&mut self, capability: CapabilityId) -> bool {
        self.0.insert(capability)
    }
    /// Return whether one capability is present.
    pub fn contains(&self, capability: &CapabilityId) -> bool {
        self.0.contains(capability)
    }
    /// Return the number of capabilities.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Return whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Iterate in deterministic lexical order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &CapabilityId> {
        self.0.iter()
    }
    /// Return capabilities in this required set that are absent from `available`.
    pub fn missing_from(&self, available: &Self) -> Self {
        Self(self.0.difference(&available.0).cloned().collect())
    }
    /// Reject missing required capabilities before provider I/O.
    pub fn ensure_supported_by(&self, available: &Self) -> Result<(), Diagnostic> {
        let missing = self.missing_from(available);
        if missing.is_empty() {
            return Ok(());
        }
        Err(Diagnostic::stable(
            DiagnosticCategory::UnsupportedCapability,
            "unsupported_required_capability",
            "one or more required capabilities are not advertised",
        )
        .with_detail(
            "missing",
            DiagnosticDetailValue::TextList(
                missing.iter().map(|id| id.as_str().to_owned()).collect(),
            ),
        ))
    }
}

impl FromIterator<CapabilityId> for CapabilitySet {
    fn from_iter<T: IntoIterator<Item = CapabilityId>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}
impl IntoIterator for CapabilitySet {
    type Item = CapabilityId;
    type IntoIter = std::collections::btree_set::IntoIter<CapabilityId>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> CapabilityId {
        CapabilityId::new(value).unwrap()
    }

    #[test]
    fn unknown_open_ids_round_trip_in_deterministic_order() {
        let set = CapabilitySet::from_iter([id("schema.annotations"), id("query.future-feature")]);
        let bytes = serde_json::to_vec(&set).unwrap();
        assert_eq!(bytes, br#"["query.future-feature","schema.annotations"]"#);
        assert_eq!(
            serde_json::from_slice::<CapabilitySet>(&bytes).unwrap(),
            set
        );
    }

    #[test]
    fn malformed_capability_ids_fail_closed() {
        for value in [
            "",
            "query",
            "Query.feature",
            "query..feature",
            "query.feature!",
        ] {
            assert_eq!(
                CapabilityId::new(value).unwrap_err().code().as_str(),
                "malformed_capability_id",
            );
        }
        assert!(serde_json::from_str::<CapabilityId>(r#""Query.feature""#).is_err());
    }

    #[test]
    fn missing_required_capability_has_a_stable_diagnostic() {
        let required = CapabilitySet::from_iter([id("query.given-multi-row")]);
        let error = required
            .ensure_supported_by(&CapabilitySet::new())
            .unwrap_err();
        assert_eq!(error.code().as_str(), "unsupported_required_capability");
    }
}
