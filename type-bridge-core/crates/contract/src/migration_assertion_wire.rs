//! Private fail-closed wire DTOs for migration assertions.

use serde::{Deserialize, Serialize};

use crate::capability::CapabilitySet;
use crate::codec::{FormatVersion, ensure_format_version, from_canonical_json};
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintAlgorithm, FingerprintDigest,
    FingerprintDomain, SemanticProfileId,
};
use crate::id::{AttributeId, RoleId, TypeId, TypeKind};
use crate::migration_assertion::{
    AssertionBinding, AssertionExpectation, AssertionPattern, AssertionRolePlayer,
    BindingId, MigrationAssertionPlan, QueryVariable, ValueComparator, ValueOperand,
    assertion_failure, encode_migration_assertion_plan,
};
use crate::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use crate::value::CanonicalValue;

pub(crate) fn decode_migration_assertion_plan(
    bytes: &[u8],
) -> Result<MigrationAssertionPlan, Diagnostic> {
    let wire = from_canonical_json::<MigrationAssertionPlanWire>(bytes)?;
    let trusted = wire.rebuild()?;
    if encode_migration_assertion_plan(&trusted)? != bytes {
        return Err(assertion_failure(
            DiagnosticCategory::Integrity,
            "migration_assertion_wire_mismatch",
            "assertion bytes normalize after trusted reconstruction",
        ));
    }
    Ok(trusted)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MigrationAssertionPlanWire {
    bindings: Vec<AssertionBindingWire>,
    expectation: String,
    format: u16,
    managed_semantics: FingerprintWire,
    outputs: Vec<u16>,
    patterns: Vec<AssertionPatternWire>,
    required_capabilities: CapabilitySet,
    witnesses: Vec<u16>,
}

impl MigrationAssertionPlanWire {
    fn rebuild(self) -> Result<MigrationAssertionPlan, Diagnostic> {
        ensure_format_version(FormatVersion::from_raw(self.format), FormatVersion::V1)?;
        if self.expectation != "no_rows" {
            return Err(assertion_failure(
                DiagnosticCategory::InvalidContract,
                "migration_assertion_unknown_expectation",
                "assertion expectation is not supported",
            ));
        }
        let trusted = MigrationAssertionPlan::new(
            self.bindings
                .into_iter()
                .map(AssertionBindingWire::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.patterns
                .into_iter()
                .map(AssertionPatternWire::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.outputs
                .into_iter()
                .map(|id| raw_binding(id))
                .collect::<Result<Vec<_>, _>>()?,
            self.witnesses
                .into_iter()
                .map(|id| raw_binding(id))
                .collect::<Result<Vec<_>, _>>()?,
            ManagedSemanticSchemaFingerprint::from_wire(
                self.managed_semantics.rebuild()?,
            )?,
            AssertionExpectation::NoRows,
        )?;
        if self.required_capabilities != *trusted.required_capabilities() {
            return Err(assertion_failure(
                DiagnosticCategory::Integrity,
                "migration_assertion_capability_mismatch",
                "assertion required capabilities are not syntax-derived",
            ));
        }
        Ok(trusted)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FingerprintWire {
    domain: FingerprintDomain,
    algorithm: FingerprintAlgorithm,
    canonicalization: CanonicalizationVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    semantic_profile: Option<SemanticProfileId>,
    digest: FingerprintDigest,
}

impl FingerprintWire {
    pub(crate) fn rebuild(self) -> Result<Fingerprint, Diagnostic> {
        serde_json::from_value(serde_json::to_value(self).map_err(|_| {
            assertion_failure(
                DiagnosticCategory::InvalidContract,
                "invalid_migration_assertion_fingerprint",
                "migration assertion fingerprint cannot be represented as JSON",
            )
        })?)
        .map_err(|_| {
            assertion_failure(
                DiagnosticCategory::Integrity,
                "invalid_migration_assertion_fingerprint",
                "migration assertion fingerprint metadata is malformed",
            )
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AssertionBindingWire {
    id: u16,
    variable: String,
}

impl AssertionBindingWire {
    fn rebuild(self) -> Result<AssertionBinding, Diagnostic> {
        Ok(AssertionBinding::new(
            BindingId::new(self.id)?,
            QueryVariable::new(self.variable)?,
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum AssertionPatternWire {
    Isa {
        binding: u16,
        include_subtypes: bool,
        type_id: TypeIdWire,
    },
    Has {
        attribute: u16,
        attribute_id: String,
        owner: u16,
    },
    Links {
        players: Vec<AssertionRolePlayerWire>,
        relation: u16,
        relation_id: TypeIdWire,
    },
    Value {
        comparator: ValueComparatorWire,
        left: ValueOperandWire,
        right: ValueOperandWire,
    },
    Not { patterns: Vec<AssertionPatternWire> },
}

impl AssertionPatternWire {
    fn rebuild(self) -> Result<AssertionPattern, Diagnostic> {
        Ok(match self {
            Self::Isa {
                binding,
                include_subtypes,
                type_id,
            } => AssertionPattern::Isa {
                binding: raw_binding(binding)?,
                include_subtypes,
                type_id: type_id.rebuild()?,
            },
            Self::Has {
                attribute,
                attribute_id,
                owner,
            } => AssertionPattern::Has {
                attribute: raw_binding(attribute)?,
                attribute_id: AttributeId::new(attribute_id)?,
                owner: raw_binding(owner)?,
            },
            Self::Links {
                players,
                relation,
                relation_id,
            } => AssertionPattern::Links {
                players: players
                    .into_iter()
                    .map(AssertionRolePlayerWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
                relation: raw_binding(relation)?,
                relation_id: relation_id.rebuild()?,
            },
            Self::Value {
                comparator,
                left,
                right,
            } => AssertionPattern::Value {
                comparator: comparator.rebuild(),
                left: left.rebuild()?,
                right: right.rebuild()?,
            },
            Self::Not { patterns } => AssertionPattern::Not {
                patterns: patterns
                    .into_iter()
                    .map(AssertionPatternWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            },
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssertionRolePlayerWire {
    player: u16,
    role: RoleIdWire,
}

impl AssertionRolePlayerWire {
    pub(crate) fn rebuild(self) -> Result<AssertionRolePlayer, Diagnostic> {
        Ok(AssertionRolePlayer::new(
            self.role.rebuild()?,
            raw_binding(self.player)?,
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ValueComparatorWire {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

impl ValueComparatorWire {
    pub(crate) fn rebuild(self) -> ValueComparator {
        match self {
            Self::Equal => ValueComparator::Equal,
            Self::NotEqual => ValueComparator::NotEqual,
            Self::Less => ValueComparator::Less,
            Self::LessOrEqual => ValueComparator::LessOrEqual,
            Self::Greater => ValueComparator::Greater,
            Self::GreaterOrEqual => ValueComparator::GreaterOrEqual,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ValueOperandWire {
    Binding { binding: u16 },
    Literal { value: CanonicalValue },
}

impl ValueOperandWire {
    fn rebuild(self) -> Result<ValueOperand, Diagnostic> {
        Ok(match self {
            Self::Binding { binding } => ValueOperand::binding(raw_binding(binding)?),
            Self::Literal { value } => ValueOperand::literal(value),
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TypeIdWire {
    kind: TypeKind,
    label: String,
}

impl TypeIdWire {
    pub(crate) fn rebuild(self) -> Result<TypeId, Diagnostic> {
        TypeId::new(self.kind, self.label)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RoleIdWire {
    declaring_relation: String,
    label: String,
}

impl RoleIdWire {
    pub(crate) fn rebuild(self) -> Result<RoleId, Diagnostic> {
        RoleId::new(self.declaring_relation, self.label)
    }
}

fn raw_binding(value: u16) -> Result<BindingId, Diagnostic> {
    BindingId::new(value)
}
