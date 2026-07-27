//! Context-free canonical syntax for migration assertions.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Serialize, Serializer};

use crate::capability::{CapabilityId, CapabilitySet};
use crate::codec::{FormatVersion, to_canonical_json};
use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use crate::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use crate::id::{AttributeId, RoleId, TypeId};
use crate::limits::{MAX_BINDINGS, MAX_OUTPUT_NAME_BYTES, StructuralLimits};
use crate::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use crate::value::CanonicalValue;

/// Fingerprint domain for canonical migration assertions.
pub const MIGRATION_ASSERTION_FINGERPRINT_DOMAIN: &str = "typebridge.query.migration-assertion";
/// Canonicalization identifier for canonical migration assertions.
pub const MIGRATION_ASSERTION_CANONICALIZATION: &str = "typebridge.migration-assertion/v1";

const CAP_ASSERTION: &str = "query.migration-assertion";
const CAP_ISA: &str = "query.pattern.isa";
const CAP_ISA_SUBTYPES: &str = "query.pattern.isa-subtypes";
const CAP_HAS: &str = "query.pattern.has";
const CAP_LINKS: &str = "query.pattern.links";
const CAP_VALUE: &str = "query.pattern.value";
const CAP_NEGATION: &str = "query.pattern.negation";

/// Return the complete capability vocabulary used by canonical migration assertions.
pub fn migration_assertion_capability_vocabulary() -> CapabilitySet {
    let mut capabilities = CapabilitySet::new();
    for capability in [
        CAP_ASSERTION,
        CAP_ISA,
        CAP_ISA_SUBTYPES,
        CAP_HAS,
        CAP_LINKS,
        CAP_VALUE,
        CAP_NEGATION,
    ] {
        insert_capability(&mut capabilities, capability)
            .expect("static migration assertion capability ID is canonical");
    }
    capabilities
}

/// Dense zero-based identity of one binding in a typed assertion plan.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct BindingId(u16);

impl BindingId {
    /// Construct a binding ordinal within the canonical binding ceiling.
    pub fn new(value: u16) -> Result<Self, Diagnostic> {
        if usize::from(value) < MAX_BINDINGS {
            Ok(Self(value))
        } else {
            Err(assertion_failure(
                DiagnosticCategory::ResourceLimit,
                "migration_assertion_binding_id_out_of_range",
                "binding ID exceeds the canonical structural ceiling",
            ))
        }
    }

    /// Return the zero-based ordinal.
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Validated canonical query-variable spelling without a `$` sigil.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct QueryVariable(String);

impl QueryVariable {
    /// Validate a bounded lowercase variable spelling.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        let mut bytes = value.bytes();
        let valid = !value.is_empty()
            && value.len() <= MAX_OUTPUT_NAME_BYTES
            && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
            && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        if valid {
            Ok(Self(value))
        } else {
            Err(assertion_failure(
                DiagnosticCategory::InvalidContract,
                "migration_assertion_invalid_variable",
                "query variable must be a bounded lowercase identifier",
            ))
        }
    }

    /// Return the canonical spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for QueryVariable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for QueryVariable {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// One explicit dense binding declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AssertionBinding {
    id: BindingId,
    variable: QueryVariable,
}

impl AssertionBinding {
    /// Construct one typed binding declaration.
    pub const fn new(id: BindingId, variable: QueryVariable) -> Self {
        Self { id, variable }
    }

    /// Return the dense binding identity.
    pub const fn id(&self) -> BindingId {
        self.id
    }

    /// Return the user-facing query variable.
    pub const fn variable(&self) -> &QueryVariable {
        &self.variable
    }
}

/// One role-qualified relation player.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AssertionRolePlayer {
    player: BindingId,
    role: RoleId,
}

impl AssertionRolePlayer {
    /// Construct a typed role player.
    pub const fn new(role: RoleId, player: BindingId) -> Self {
        Self { player, role }
    }

    /// Return the role identity.
    pub const fn role(&self) -> &RoleId {
        &self.role
    }

    /// Return the player binding.
    pub const fn player(&self) -> BindingId {
        self.player
    }
}

/// Closed comparator vocabulary for typed canonical values.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueComparator {
    /// Equal values.
    Equal,
    /// Unequal values.
    NotEqual,
    /// Strictly less.
    Less,
    /// Less or equal.
    LessOrEqual,
    /// Strictly greater.
    Greater,
    /// Greater or equal.
    GreaterOrEqual,
}

/// One typed operand in a value comparison.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValueOperand {
    /// Read the scalar value of an attribute binding.
    Binding {
        /// The attribute binding whose value is read.
        binding: BindingId,
    },
    /// Compare with an exact canonical literal.
    Literal {
        /// The exact canonical scalar to compare against.
        value: CanonicalValue,
    },
}

impl ValueOperand {
    /// Construct a binding operand.
    pub const fn binding(binding: BindingId) -> Self {
        Self::Binding { binding }
    }

    /// Construct a literal operand.
    pub const fn literal(value: CanonicalValue) -> Self {
        Self::Literal { value }
    }
}

/// Minimal closed typed pattern algebra used by migration assertions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssertionPattern {
    /// Constrain a binding to one schema type, optionally including subtypes.
    Isa {
        /// The binding the type constraint applies to.
        binding: BindingId,
        /// Whether subtypes of the named type also satisfy the constraint.
        include_subtypes: bool,
        /// The schema type the binding must instantiate.
        type_id: TypeId,
    },
    /// Bind an owned attribute through an effective ownership.
    Has {
        /// The binding that receives the owned attribute instance.
        attribute: BindingId,
        /// The schema attribute type being read.
        attribute_id: AttributeId,
        /// The binding that owns the attribute.
        owner: BindingId,
    },
    /// Bind a relation and role-qualified players.
    Links {
        /// The role-qualified players the relation must link.
        players: Vec<AssertionRolePlayer>,
        /// The binding that receives the relation instance.
        relation: BindingId,
        /// The schema relation type being matched.
        relation_id: TypeId,
    },
    /// Compare two exact typed scalar operands.
    Value {
        /// The comparison operator.
        comparator: ValueComparator,
        /// The left operand.
        left: ValueOperand,
        /// The right operand.
        right: ValueOperand,
    },
    /// Negate a closed nested conjunction.
    Not {
        /// The conjunction that must not match.
        patterns: Vec<AssertionPattern>,
    },
}

/// The only expectation admitted by the first migration assertion revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionExpectation {
    /// The validated query must produce no rows.
    NoRows,
}

/// A context-free, canonical typed migration assertion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MigrationAssertionPlan {
    bindings: Vec<AssertionBinding>,
    expectation: AssertionExpectation,
    format: FormatVersion,
    managed_semantics: ManagedSemanticSchemaFingerprint,
    outputs: Vec<BindingId>,
    patterns: Vec<AssertionPattern>,
    required_capabilities: CapabilitySet,
    witnesses: Vec<BindingId>,
}

impl MigrationAssertionPlan {
    /// Validate and construct one canonical plan under fixed protocol limits.
    pub fn new(
        bindings: Vec<AssertionBinding>,
        patterns: Vec<AssertionPattern>,
        outputs: Vec<BindingId>,
        witnesses: Vec<BindingId>,
        managed_semantics: ManagedSemanticSchemaFingerprint,
        expectation: AssertionExpectation,
    ) -> Result<Self, Diagnostic> {
        Self::new_with_limits(
            bindings,
            patterns,
            outputs,
            witnesses,
            managed_semantics,
            expectation,
            StructuralLimits::CANONICAL,
        )
    }

    fn new_with_limits(
        bindings: Vec<AssertionBinding>,
        patterns: Vec<AssertionPattern>,
        mut outputs: Vec<BindingId>,
        mut witnesses: Vec<BindingId>,
        managed_semantics: ManagedSemanticSchemaFingerprint,
        expectation: AssertionExpectation,
        limits: StructuralLimits,
    ) -> Result<Self, Diagnostic> {
        if bindings.is_empty() || !limits.allows_bindings(bindings.len()) {
            return Err(assertion_failure(
                DiagnosticCategory::ResourceLimit,
                "migration_assertion_binding_limit",
                "assertion binding count is empty or exceeds the structural ceiling",
            ));
        }
        let mut variables = BTreeSet::new();
        for (index, binding) in bindings.iter().enumerate() {
            if usize::from(binding.id.get()) != index {
                return Err(assertion_failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_assertion_bindings_not_dense",
                    "assertion binding IDs must be ordered dense zero-based ordinals",
                ));
            }
            if !variables.insert(binding.variable.clone()) {
                return Err(assertion_failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_assertion_duplicate_variable",
                    "assertion query variables must be unique",
                ));
            }
        }
        canonical_binding_set(&mut outputs, bindings.len(), "output")?;
        canonical_binding_set(&mut witnesses, bindings.len(), "witness")?;
        if !limits.allows_selected_slots(outputs.len()) {
            return Err(assertion_failure(
                DiagnosticCategory::ResourceLimit,
                "migration_assertion_output_limit",
                "assertion output count exceeds the structural ceiling",
            ));
        }
        if outputs.iter().any(|id| witnesses.binary_search(id).is_ok()) {
            return Err(assertion_failure(
                DiagnosticCategory::InvalidContract,
                "migration_assertion_witness_is_output",
                "a hidden witness cannot also be an output binding",
            ));
        }
        if patterns.is_empty() || patterns.len() > limits.boolean_terms {
            return Err(assertion_failure(
                DiagnosticCategory::ResourceLimit,
                "migration_assertion_pattern_limit",
                "assertion root conjunction is empty or exceeds the term ceiling",
            ));
        }
        let mut stats = PatternStats::default();
        for pattern in &patterns {
            inspect_pattern(pattern, 1, bindings.len(), limits, &mut stats)?;
        }
        let required_capabilities = derive_capabilities(&patterns)?;
        Ok(Self {
            bindings,
            expectation,
            format: FormatVersion::V1,
            managed_semantics,
            outputs,
            patterns,
            required_capabilities,
            witnesses,
        })
    }

    /// Return the owning format version.
    pub const fn format(&self) -> FormatVersion {
        self.format
    }

    /// Return the exact managed semantic schema this plan was validated against.
    pub const fn managed_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.managed_semantics
    }

    /// Return dense binding declarations.
    pub fn bindings(&self) -> &[AssertionBinding] {
        &self.bindings
    }

    /// Look up one dense binding declaration.
    pub fn binding(&self, id: BindingId) -> Option<&AssertionBinding> {
        self.bindings.get(usize::from(id.get()))
    }

    /// Return the ordered positive/negative pattern conjunction.
    pub fn patterns(&self) -> &[AssertionPattern] {
        &self.patterns
    }

    /// Return canonical-sorted output bindings.
    pub fn outputs(&self) -> &[BindingId] {
        &self.outputs
    }

    /// Return canonical-sorted hidden witness bindings.
    pub fn witnesses(&self) -> &[BindingId] {
        &self.witnesses
    }

    /// Return the closed assertion expectation.
    pub const fn expectation(&self) -> AssertionExpectation {
        self.expectation
    }

    /// Return open capabilities derived from syntax.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Encode exact canonical bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        encode_migration_assertion_plan(self)
    }

    /// Compute a canonical domain-separated plan fingerprint.
    pub fn fingerprint(&self) -> Result<MigrationAssertionPlanFingerprint, Diagnostic> {
        MigrationAssertionPlanFingerprint::compute(self)
    }
}

/// Fingerprint of exact canonical migration assertion bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct MigrationAssertionPlanFingerprint(Fingerprint);

impl MigrationAssertionPlanFingerprint {
    /// Compute the fixed-domain fingerprint of a trusted plan.
    pub fn compute(plan: &MigrationAssertionPlan) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(MIGRATION_ASSERTION_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(MIGRATION_ASSERTION_CANONICALIZATION)?,
            None,
            &plan.canonical_bytes()?,
        )))
    }

    /// Return the generic fingerprint.
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}

/// Encode a trusted plan as bounded canonical JSON.
pub fn encode_migration_assertion_plan(
    plan: &MigrationAssertionPlan,
) -> Result<Vec<u8>, Diagnostic> {
    to_canonical_json(plan)
}

/// Decode canonical bytes through private constructor-rebuilding wire types.
pub fn decode_migration_assertion_plan(bytes: &[u8]) -> Result<MigrationAssertionPlan, Diagnostic> {
    crate::migration_assertion_wire::decode_migration_assertion_plan(bytes)
}

#[derive(Default)]
struct PatternStats {
    nodes: usize,
}

fn inspect_pattern(
    pattern: &AssertionPattern,
    depth: usize,
    binding_count: usize,
    limits: StructuralLimits,
    stats: &mut PatternStats,
) -> Result<(), Diagnostic> {
    stats.nodes += 1;
    if !limits.allows_predicate_nodes(stats.nodes) {
        return Err(assertion_failure(
            DiagnosticCategory::ResourceLimit,
            "migration_assertion_pattern_node_limit",
            "assertion pattern count exceeds the structural ceiling",
        ));
    }
    if !limits.allows_predicate_depth(depth) {
        return Err(assertion_failure(
            DiagnosticCategory::ResourceLimit,
            "migration_assertion_pattern_depth_limit",
            "assertion pattern depth exceeds the structural ceiling",
        ));
    }
    match pattern {
        AssertionPattern::Isa { binding, .. } => check_binding(*binding, binding_count),
        AssertionPattern::Has {
            owner, attribute, ..
        } => {
            check_binding(*owner, binding_count)?;
            check_binding(*attribute, binding_count)
        }
        AssertionPattern::Links {
            relation, players, ..
        } => {
            check_binding(*relation, binding_count)?;
            if players.is_empty() || players.len() > limits.boolean_terms {
                return Err(assertion_failure(
                    DiagnosticCategory::ResourceLimit,
                    "migration_assertion_role_player_limit",
                    "links pattern has no players or exceeds the term ceiling",
                ));
            }
            for player in players {
                check_binding(player.player(), binding_count)?;
            }
            Ok(())
        }
        AssertionPattern::Value { left, right, .. } => {
            check_operand(left, binding_count)?;
            check_operand(right, binding_count)
        }
        AssertionPattern::Not { patterns } => {
            if patterns.is_empty() || patterns.len() > limits.boolean_terms {
                return Err(assertion_failure(
                    DiagnosticCategory::ResourceLimit,
                    "migration_assertion_negation_term_limit",
                    "negation is empty or exceeds the boolean-term ceiling",
                ));
            }
            for child in patterns {
                inspect_pattern(child, depth + 1, binding_count, limits, stats)?;
            }
            Ok(())
        }
    }
}

fn check_operand(operand: &ValueOperand, binding_count: usize) -> Result<(), Diagnostic> {
    match operand {
        ValueOperand::Binding { binding } => check_binding(*binding, binding_count),
        ValueOperand::Literal { .. } => Ok(()),
    }
}

fn check_binding(binding: BindingId, binding_count: usize) -> Result<(), Diagnostic> {
    if usize::from(binding.get()) < binding_count {
        Ok(())
    } else {
        Err(assertion_failure(
            DiagnosticCategory::InvalidContract,
            "migration_assertion_unknown_binding",
            "assertion pattern references an undeclared binding",
        ))
    }
}

fn canonical_binding_set(
    bindings: &mut [BindingId],
    binding_count: usize,
    kind: &'static str,
) -> Result<(), Diagnostic> {
    bindings.sort();
    if bindings.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(assertion_failure(
            DiagnosticCategory::InvalidContract,
            "migration_assertion_duplicate_binding_set_member",
            kind,
        ));
    }
    for binding in bindings.iter().copied() {
        check_binding(binding, binding_count)?;
    }
    Ok(())
}

fn derive_capabilities(patterns: &[AssertionPattern]) -> Result<CapabilitySet, Diagnostic> {
    let mut capabilities = CapabilitySet::new();
    insert_capability(&mut capabilities, CAP_ASSERTION)?;
    for pattern in patterns {
        collect_capabilities(pattern, &mut capabilities)?;
    }
    Ok(capabilities)
}

fn collect_capabilities(
    pattern: &AssertionPattern,
    capabilities: &mut CapabilitySet,
) -> Result<(), Diagnostic> {
    match pattern {
        AssertionPattern::Isa {
            include_subtypes, ..
        } => {
            insert_capability(capabilities, CAP_ISA)?;
            if *include_subtypes {
                insert_capability(capabilities, CAP_ISA_SUBTYPES)?;
            }
        }
        AssertionPattern::Has { .. } => insert_capability(capabilities, CAP_HAS)?,
        AssertionPattern::Links { .. } => insert_capability(capabilities, CAP_LINKS)?,
        AssertionPattern::Value { .. } => insert_capability(capabilities, CAP_VALUE)?,
        AssertionPattern::Not { patterns } => {
            insert_capability(capabilities, CAP_NEGATION)?;
            for child in patterns {
                collect_capabilities(child, capabilities)?;
            }
        }
    }
    Ok(())
}

fn insert_capability(
    capabilities: &mut CapabilitySet,
    value: &'static str,
) -> Result<(), Diagnostic> {
    capabilities.insert(CapabilityId::new(value)?);
    Ok(())
}

pub(crate) fn assertion_failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static assertion diagnostic code is canonical"),
        message,
    )
}
