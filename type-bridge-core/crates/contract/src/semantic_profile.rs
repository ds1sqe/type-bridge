//! Versioned defaults that affect schema semantics without changing declarations.

use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::SemanticProfileId;
use crate::value::Cardinality;

/// Interface family whose omitted cardinality is profile-defined.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum InterfaceKind {
    /// Attribute ownership.
    Owns,
    /// Relation role declaration.
    Relates,
    /// Role playing declaration.
    Plays,
}

/// Closed server-semantic defaults selected by a versioned profile identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticProfile {
    id: SemanticProfileId,
    owns_cardinality: Cardinality,
    relates_cardinality: Cardinality,
    plays_cardinality: Cardinality,
    key_owns_cardinality: Cardinality,
}

impl SemanticProfile {
    /// Resolve a supported semantic profile, rejecting unknown default tables.
    pub fn resolve(id: &SemanticProfileId) -> Result<Self, Diagnostic> {
        match id.as_str() {
            "typedb-3.11.5/v1" | "typedb-3.12.1/v1" => {
                let bounded_to_one = Cardinality::new(0, Some(1))
                    .expect("the frozen zero-to-one cardinality is valid");
                let unconstrained = Cardinality::new(0, None)
                    .expect("the frozen unconstrained cardinality is valid");
                let exactly_one = Cardinality::new(1, Some(1))
                    .expect("the frozen exactly-one cardinality is valid");
                Ok(Self {
                    id: id.clone(),
                    owns_cardinality: bounded_to_one,
                    relates_cardinality: bounded_to_one,
                    plays_cardinality: unconstrained,
                    key_owns_cardinality: exactly_one,
                })
            }
            _ => Err(Diagnostic::stable(
                DiagnosticCategory::UnsupportedCapability,
                "unsupported_semantic_profile",
                "semantic profile has no frozen schema-default table",
            )),
        }
    }

    /// Return the versioned profile identifier.
    #[must_use]
    pub const fn id(&self) -> &SemanticProfileId {
        &self.id
    }

    /// Return the materialized cardinality for an omitted interface annotation.
    #[must_use]
    pub const fn default_cardinality(&self, kind: InterfaceKind) -> Cardinality {
        match kind {
            InterfaceKind::Owns => self.owns_cardinality,
            InterfaceKind::Relates => self.relates_cardinality,
            InterfaceKind::Plays => self.plays_cardinality,
        }
    }

    /// Materialize effective interface cardinality, including `@key`'s exact-one contract.
    #[must_use]
    pub const fn effective_cardinality(
        &self,
        kind: InterfaceKind,
        explicit: Option<Cardinality>,
        key: bool,
    ) -> Cardinality {
        match (kind, key, explicit) {
            (InterfaceKind::Owns, true, _) => self.key_owns_cardinality,
            (_, _, Some(cardinality)) => cardinality,
            _ => self.default_cardinality(kind),
        }
    }
}
