//! Canonical implementation-independent codec ceilings.

use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticDetailValue};

/// Maximum canonical artifact size: 16 MiB.
pub const MAX_CANONICAL_BYTES: usize = 16 * 1024 * 1024;
/// Maximum remote request/response envelope size: 32 MiB.
///
/// Remote envelopes embed a canonical plan plus invocation rows and framing,
/// so their owning-format budget is deliberately larger than the plan codec's
/// 16 MiB ceiling while remaining independently bounded.
pub const MAX_REMOTE_ENVELOPE_BYTES: usize = 32 * 1024 * 1024;
/// Maximum JSON nesting depth, counting the root as one.
pub const MAX_CANONICAL_DEPTH: usize = 64;
/// Maximum direct members in one array or object.
pub const MAX_CANONICAL_COLLECTION_LEN: usize = 65_536;
/// Maximum UTF-8 byte length of one string or object key: 1 MiB.
pub const MAX_CANONICAL_STRING_BYTES: usize = 1024 * 1024;

/// The complete canonical JSON structural limit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecLimits {
    /// Maximum encoded or decoded bytes.
    pub max_bytes: usize,
    /// Maximum nested value depth.
    pub max_depth: usize,
    /// Maximum direct collection members.
    pub max_collection_len: usize,
    /// Maximum bytes in one string or key.
    pub max_string_bytes: usize,
}

impl CodecLimits {
    /// Canonical Phase 1 codec limits.
    pub const CANONICAL: Self = Self {
        max_bytes: MAX_CANONICAL_BYTES,
        max_depth: MAX_CANONICAL_DEPTH,
        max_collection_len: MAX_CANONICAL_COLLECTION_LEN,
        max_string_bytes: MAX_CANONICAL_STRING_BYTES,
    };
}

impl Default for CodecLimits {
    fn default() -> Self {
        Self::CANONICAL
    }
}

/// Canonical limits used by every contract codec consumer.
pub const CANONICAL_CODEC_LIMITS: CodecLimits = CodecLimits::CANONICAL;

/// Owning-format limits for V1 remote replies and capability advertisements.
pub const REMOTE_ENVELOPE_CODEC_LIMITS: CodecLimits = CodecLimits {
    max_bytes: MAX_REMOTE_ENVELOPE_BYTES,
    max_depth: MAX_CANONICAL_DEPTH,
    max_collection_len: MAX_CANONICAL_COLLECTION_LEN,
    max_string_bytes: MAX_CANONICAL_STRING_BYTES,
};

/// Owning-format limits for V1 remote query request envelopes.
///
/// A request embeds an independently canonical plan beneath exactly one
/// framing object. The extra level admits a standalone plan at the canonical
/// depth boundary without relaxing that plan's own revalidation or the limits
/// for replies and capability advertisements.
pub const REMOTE_REQUEST_CODEC_LIMITS: CodecLimits = CodecLimits {
    max_bytes: MAX_REMOTE_ENVELOPE_BYTES,
    max_depth: MAX_CANONICAL_DEPTH + 1,
    max_collection_len: MAX_CANONICAL_COLLECTION_LEN,
    max_string_bytes: MAX_CANONICAL_STRING_BYTES,
};

/// Maximum number of selected output slots in one typed plan.
pub const MAX_SELECTED_SLOTS: usize = 16;
/// Maximum number of bindings in one typed plan.
pub const MAX_BINDINGS: usize = 256;
/// Maximum number of nodes in one predicate tree.
pub const MAX_PREDICATE_NODES: usize = 4_096;
/// Maximum predicate nesting depth, counting a root as depth one.
pub const MAX_PREDICATE_DEPTH: usize = 64;
/// Maximum number of children in one boolean expression.
pub const MAX_BOOLEAN_TERMS: usize = 256;
/// Maximum number of explicitly allowed cross-join binding pairs.
pub const MAX_ALLOWED_CROSS_JOINS: usize = 1_024;
/// Maximum number of public row/root ordering terms.
pub const MAX_ORDER_TERMS: usize = 64;
/// Maximum number of ordering terms for one collected output slot.
pub const MAX_COLLECTION_ORDER_TERMS: usize = 64;
/// Maximum number of input rows in one typed invocation.
pub const MAX_INPUT_ROWS: usize = 4_096;
/// Maximum encoded bytes across one invocation's input-row batch: 4 MiB.
pub const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
/// Maximum canonical bytes in one complete plan-bound query invocation.
///
/// The owning wire object adds 234 fixed bytes around the independently
/// bounded input-row batch: the `inputs` key, the longest operation spelling
/// (`exists`), and the fixed-domain query-plan fingerprint. Contract tests
/// construct an invocation whose input batch is exactly [`MAX_INPUT_BYTES`]
/// and prove that this ceiling is both attainable and one-byte tight.
pub const MAX_QUERY_INVOCATION_BYTES: usize = MAX_INPUT_BYTES + 234;
/// Maximum UTF-8 bytes in one output or query-variable name.
pub const MAX_OUTPUT_NAME_BYTES: usize = 128;
/// Maximum UTF-8 bytes in one serialized semantic identity.
pub const MAX_SEMANTIC_ID_BYTES: usize = 512;
/// Maximum bytes in one canonical serialized diagnostic.
pub const MAX_DIAGNOSTIC_BYTES: usize = 65_536;

/// Shared implementation-independent typed-plan structural ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructuralLimits {
    /// Selected output slots.
    pub selected_slots: usize,
    /// Plan bindings.
    pub bindings: usize,
    /// Total predicate nodes.
    pub predicate_nodes: usize,
    /// Predicate-tree depth.
    pub predicate_depth: usize,
    /// Children in one boolean expression.
    pub boolean_terms: usize,
    /// Explicitly allowed cross joins.
    pub allowed_cross_joins: usize,
    /// Public row/root ordering terms.
    pub order_terms: usize,
    /// Per-collection ordering terms.
    pub collection_order_terms: usize,
    /// Input rows in one invocation.
    pub input_rows: usize,
    /// Encoded bytes across one invocation's input rows.
    pub input_bytes: usize,
    /// Named output-slot UTF-8 bytes.
    pub output_name_bytes: usize,
    /// Semantic identity UTF-8 bytes.
    pub semantic_id_bytes: usize,
    /// Canonical diagnostic bytes.
    pub diagnostic_bytes: usize,
}

impl StructuralLimits {
    /// Canonical cross-language protocol limits.
    pub const CANONICAL: Self = Self {
        selected_slots: MAX_SELECTED_SLOTS,
        bindings: MAX_BINDINGS,
        predicate_nodes: MAX_PREDICATE_NODES,
        predicate_depth: MAX_PREDICATE_DEPTH,
        boolean_terms: MAX_BOOLEAN_TERMS,
        allowed_cross_joins: MAX_ALLOWED_CROSS_JOINS,
        order_terms: MAX_ORDER_TERMS,
        collection_order_terms: MAX_COLLECTION_ORDER_TERMS,
        input_rows: MAX_INPUT_ROWS,
        input_bytes: MAX_INPUT_BYTES,
        output_name_bytes: MAX_OUTPUT_NAME_BYTES,
        semantic_id_bytes: MAX_SEMANTIC_ID_BYTES,
        diagnostic_bytes: MAX_DIAGNOSTIC_BYTES,
    };

    /// Return whether `actual` fits the selected-slot ceiling.
    pub const fn allows_selected_slots(self, actual: usize) -> bool {
        actual <= self.selected_slots
    }

    /// Return whether `actual` fits the binding ceiling.
    pub const fn allows_bindings(self, actual: usize) -> bool {
        actual <= self.bindings
    }

    /// Return whether `actual` fits the invocation input-row ceiling.
    pub const fn allows_input_rows(self, actual: usize) -> bool {
        actual <= self.input_rows
    }

    /// Return whether `actual` fits the invocation input-byte ceiling.
    pub const fn allows_input_bytes(self, actual: usize) -> bool {
        actual <= self.input_bytes
    }

    /// Return whether `actual` fits the public ordering-term ceiling.
    pub const fn allows_order_terms(self, actual: usize) -> bool {
        actual <= self.order_terms
    }

    /// Return whether `actual` fits the predicate-node ceiling.
    pub const fn allows_predicate_nodes(self, actual: usize) -> bool {
        actual <= self.predicate_nodes
    }

    /// Return whether `actual` fits the predicate-depth ceiling.
    pub const fn allows_predicate_depth(self, actual: usize) -> bool {
        actual <= self.predicate_depth
    }

    /// Return whether `actual` fits the diagnostic-byte ceiling.
    pub const fn allows_diagnostic_bytes(self, actual: usize) -> bool {
        actual <= self.diagnostic_bytes
    }
}

impl StructuralLimits {
    /// [`QueryLimits::annotate`] for a stage that holds only the structural
    /// limits.
    #[must_use]
    pub fn annotate(self, diagnostic: Diagnostic) -> Diagnostic {
        QueryLimits {
            structural: self,
            ..QueryLimits::CANONICAL
        }
        .annotate(diagnostic)
    }
}

impl Default for StructuralLimits {
    fn default() -> Self {
        Self::CANONICAL
    }
}

/// Canonical structural limits shared by V1 and V2 typed plans.
pub const CANONICAL_STRUCTURAL_LIMITS: StructuralLimits = StructuralLimits::CANONICAL;

/// Deepest canonical JSON nesting the codec can decode.
///
/// The JSON parser refuses input nested 128 levels or deeper and reports it as
/// malformed rather than as a resource limit, so no configured depth may ask
/// for more. A remote request frames the plan in one extra level, which is
/// why configured depth stops one short of the parser's own bound.
pub const MAX_SUPPORTED_CANONICAL_DEPTH: usize = 126;

/// Widest selection the generated typed tuples of every binding can spell.
///
/// Generated query, grouping, and aggregate typing is sealed at sixteen
/// positions, so this limit can be lowered but not raised.
pub const MAX_SUPPORTED_SELECTED_SLOTS: usize = MAX_SELECTED_SLOTS;

/// Largest binding count addressable by a 16-bit binding identity.
pub const MAX_SUPPORTED_BINDINGS: usize = 1 << 16;

/// The complete set of ceilings applied to one query session's plans.
///
/// Every query-path limit is a field here; nothing on that path reads a
/// constant. [`Self::CANONICAL`] reproduces the released fixed ceilings. A
/// value from a session, an advertisement, or an embedding host is untrusted
/// until [`Self::validate`] accepts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryLimits {
    /// Canonical JSON ceilings for one plan or invocation artifact.
    pub codec: CodecLimits,
    /// Typed-plan structural ceilings.
    pub structural: StructuralLimits,
    /// Maximum bytes in one remote request envelope.
    ///
    /// Reply size is not a query limit: it belongs to the caller's answer
    /// budget and the executor's answer ceilings.
    pub remote_request_bytes: usize,
}

/// Why a [`QueryLimits`] value was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryLimitsError {
    /// No limit has this name.
    Unknown {
        /// The unrecognized name.
        name: String,
    },
    /// A limit is zero or beyond what the implementation supports.
    OutOfRange {
        /// The stable name of the offending limit.
        limit: &'static str,
        /// The refused value.
        value: u64,
        /// The smallest admissible value.
        minimum: u64,
        /// The largest admissible value.
        maximum: u64,
    },
}

impl QueryLimitsError {
    /// Return the name of the limit this error is about.
    #[must_use]
    pub fn limit(&self) -> &str {
        match self {
            Self::Unknown { name } => name,
            Self::OutOfRange { limit, .. } => limit,
        }
    }
}

impl std::fmt::Display for QueryLimitsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown { name } => write!(formatter, "`{name}` is not a known query limit"),
            Self::OutOfRange {
                limit,
                value,
                minimum,
                maximum,
            } => write!(
                formatter,
                "query limit `{limit}` is {value}; it must be between {minimum} and {maximum}"
            ),
        }
    }
}

impl std::error::Error for QueryLimitsError {}

macro_rules! query_limit_fields {
    ($(($name:literal, $($path:ident).+, $maximum:expr)),+ $(,)?) => {
        impl QueryLimits {
            /// Stable names of every limit, in declaration order.
            pub const FIELDS: &'static [&'static str] = &[$($name),+];

            /// Return the limit with this stable name.
            #[must_use]
            pub fn get(&self, name: &str) -> Option<usize> {
                match name {
                    $($name => Some(self.$($path).+),)+
                    _ => None,
                }
            }

            /// Replace the limit with this stable name.
            ///
            /// Returns `false` and changes nothing for an unknown name. The
            /// result is unvalidated until [`Self::validate`] accepts it.
            pub fn set(&mut self, name: &str, value: usize) -> bool {
                match name {
                    $($name => self.$($path).+ = value,)+
                    _ => return false,
                }
                true
            }

            /// Return the largest value the implementation supports for
            /// the limit with this stable name.
            #[must_use]
            pub fn supported_maximum(name: &str) -> Option<usize> {
                match name {
                    $($name => Some($maximum),)+
                    _ => None,
                }
            }
        }
    };
}

query_limit_fields![
    ("canonical_bytes", codec.max_bytes, usize::MAX),
    (
        "canonical_depth",
        codec.max_depth,
        MAX_SUPPORTED_CANONICAL_DEPTH
    ),
    (
        "canonical_collection_len",
        codec.max_collection_len,
        usize::MAX
    ),
    ("canonical_string_bytes", codec.max_string_bytes, usize::MAX),
    (
        "selected_slots",
        structural.selected_slots,
        MAX_SUPPORTED_SELECTED_SLOTS
    ),
    ("bindings", structural.bindings, MAX_SUPPORTED_BINDINGS),
    ("predicate_nodes", structural.predicate_nodes, usize::MAX),
    (
        "predicate_depth",
        structural.predicate_depth,
        MAX_SUPPORTED_CANONICAL_DEPTH
    ),
    ("boolean_terms", structural.boolean_terms, usize::MAX),
    (
        "allowed_cross_joins",
        structural.allowed_cross_joins,
        usize::MAX
    ),
    ("order_terms", structural.order_terms, usize::MAX),
    (
        "collection_order_terms",
        structural.collection_order_terms,
        usize::MAX
    ),
    ("input_rows", structural.input_rows, usize::MAX),
    ("input_bytes", structural.input_bytes, usize::MAX),
    (
        "output_name_bytes",
        structural.output_name_bytes,
        usize::MAX
    ),
    (
        "semantic_id_bytes",
        structural.semantic_id_bytes,
        usize::MAX
    ),
    ("diagnostic_bytes", structural.diagnostic_bytes, usize::MAX),
    ("remote_request_bytes", remote_request_bytes, usize::MAX),
];

impl QueryLimits {
    /// The released fixed ceilings.
    pub const CANONICAL: Self = Self {
        codec: CodecLimits::CANONICAL,
        structural: StructuralLimits::CANONICAL,
        remote_request_bytes: MAX_REMOTE_ENVELOPE_BYTES,
    };

    /// Admit one untrusted limits value.
    ///
    /// Every limit must be at least one and no larger than the
    /// implementation supports. Bounds the implementation does not have are
    /// not invented here: byte and count ceilings are otherwise free.
    pub fn validate(self) -> Result<Self, QueryLimitsError> {
        for name in Self::FIELDS {
            let value = self.get(name).expect("listed limit exists");
            let maximum = Self::supported_maximum(name).expect("listed limit exists");
            if value == 0 || value > maximum {
                return Err(QueryLimitsError::OutOfRange {
                    limit: name,
                    value: u64::try_from(value).unwrap_or(u64::MAX),
                    minimum: 1,
                    maximum: u64::try_from(maximum).unwrap_or(u64::MAX),
                });
            }
        }
        Ok(self)
    }

    /// Build validated limits from named overrides of the canonical
    /// defaults.
    ///
    /// This is the one entry point for limits that arrive as untrusted
    /// name/value pairs: a host configuration, a language binding, or a
    /// server file. A limit that is not named keeps its default.
    pub fn from_overrides<'name>(
        overrides: impl IntoIterator<Item = (&'name str, u64)>,
    ) -> Result<Self, QueryLimitsError> {
        let mut limits = Self::CANONICAL;
        for (name, value) in overrides {
            let Some(listed) = Self::FIELDS.iter().find(|listed| **listed == name) else {
                return Err(QueryLimitsError::Unknown {
                    name: name.to_owned(),
                });
            };
            let Ok(value) = usize::try_from(value) else {
                return Err(QueryLimitsError::OutOfRange {
                    limit: listed,
                    value,
                    minimum: 1,
                    maximum: u64::try_from(
                        Self::supported_maximum(listed).expect("listed limit exists"),
                    )
                    .unwrap_or(u64::MAX),
                });
            };
            limits.set(listed, value);
        }
        limits.validate()
    }

    /// Return the stricter of two limits, field by field.
    #[must_use]
    pub fn min(mut self, other: Self) -> Self {
        for name in Self::FIELDS {
            let stricter = self
                .get(name)
                .expect("listed limit exists")
                .min(other.get(name).expect("listed limit exists"));
            self.set(name, stricter);
        }
        self
    }

    /// Codec limits for one remote request envelope.
    ///
    /// A request embeds an independently canonical plan beneath exactly one
    /// framing object, hence the extra level.
    #[must_use]
    pub const fn remote_request_codec(self) -> CodecLimits {
        CodecLimits {
            max_bytes: self.remote_request_bytes,
            max_depth: self.codec.max_depth + 1,
            ..self.codec
        }
    }

    /// Maximum canonical bytes in one complete plan-bound invocation.
    ///
    /// The owning wire object adds 234 fixed bytes around the input-row
    /// batch; see [`MAX_QUERY_INVOCATION_BYTES`].
    #[must_use]
    pub const fn invocation_bytes(self) -> usize {
        self.structural
            .input_bytes
            .saturating_add(MAX_QUERY_INVOCATION_BYTES - MAX_INPUT_BYTES)
    }
}

/// One dimension of the answer policy: how much a query may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnswerLimitField {
    /// Stable public name.
    pub name: &'static str,
    /// The value in force when nothing was configured.
    pub default: u64,
    /// The largest value a peer may configure or advertise.
    pub maximum: u64,
}

/// The answer policy dimensions, in declaration order.
///
/// This table is the one statement of their names, defaults, and supported
/// maxima. Only the timeout and the statement count have a maximum below the
/// type's: a day keeps deadline arithmetic exact, and no terminal issues
/// more than three provider statements.
pub const ANSWER_LIMIT_FIELDS: [AnswerLimitField; 8] = [
    AnswerLimitField {
        name: "timeout_milliseconds",
        default: 30_000,
        maximum: 24 * 60 * 60 * 1000,
    },
    AnswerLimitField {
        name: "items",
        default: MAX_CANONICAL_COLLECTION_LEN as u64,
        maximum: u64::MAX,
    },
    AnswerLimitField {
        name: "bytes",
        default: MAX_REMOTE_ENVELOPE_BYTES as u64,
        maximum: u64::MAX,
    },
    AnswerLimitField {
        name: "graph_nodes",
        default: MAX_CANONICAL_COLLECTION_LEN as u64,
        maximum: u64::MAX,
    },
    AnswerLimitField {
        name: "attribute_values",
        default: MAX_CANONICAL_COLLECTION_LEN as u64,
        maximum: u64::MAX,
    },
    AnswerLimitField {
        name: "collection_members",
        default: MAX_CANONICAL_COLLECTION_LEN as u64,
        maximum: u64::MAX,
    },
    AnswerLimitField {
        name: "role_players",
        default: MAX_CANONICAL_COLLECTION_LEN as u64,
        maximum: u64::MAX,
    },
    AnswerLimitField {
        name: "statements",
        default: 3,
        maximum: 3,
    },
];

/// Look one answer policy dimension up by its stable name.
#[must_use]
pub fn answer_limit_field(name: &str) -> Option<AnswerLimitField> {
    ANSWER_LIMIT_FIELDS
        .iter()
        .copied()
        .find(|field| field.name == name)
}

/// `source` detail of a limit left at its released default.
pub const LIMIT_SOURCE_DEFAULT: &str = "default";
/// `source` detail of a limit a session or host configured.
pub const LIMIT_SOURCE_CONFIGURED: &str = "configured";
/// `source` detail of a limit a remote executor advertised.
pub const LIMIT_SOURCE_ADVERTISED: &str = "advertised";

/// The limit each query resource-limit diagnostic code reports against.
///
/// One table keeps the attribution in step with the codes: a diagnostic is
/// annotated where the effective limits are known instead of at each of the
/// sites that can raise it.
const LIMIT_BY_DIAGNOSTIC_CODE: &[(&str, &str)] = &[
    ("canonical_json_too_deep", "canonical_depth"),
    ("canonical_json_too_large", "canonical_bytes"),
    ("canonical_collection_too_large", "canonical_collection_len"),
    ("canonical_string_too_large", "canonical_string_bytes"),
    ("query_builder_authored_binding_limit", "bindings"),
    ("query_invocation_input_byte_limit", "input_bytes"),
    ("query_invocation_row_limit", "input_rows"),
    ("query_plan_binding_limit", "bindings"),
    ("query_plan_disjunction_term_limit", "boolean_terms"),
    ("query_plan_function_argument_limit", "boolean_terms"),
    ("query_plan_input_limit", "bindings"),
    ("query_plan_local_function_limit", "bindings"),
    ("query_plan_name_limit", "output_name_bytes"),
    ("query_plan_negation_term_limit", "boolean_terms"),
    ("query_plan_output_limit", "selected_slots"),
    ("query_plan_pattern_depth_limit", "predicate_depth"),
    ("query_plan_pattern_limit", "boolean_terms"),
    ("query_plan_pattern_node_limit", "predicate_nodes"),
    ("query_plan_reachable_depth", "predicate_depth"),
    ("query_plan_reachable_expansion_limit", "predicate_nodes"),
    ("query_plan_reduce_term_limit", "boolean_terms"),
    ("query_plan_role_player_limit", "boolean_terms"),
    ("query_plan_sort_term_limit", "order_terms"),
    ("query_plan_try_term_limit", "boolean_terms"),
    ("query_plan_v2_boolean_term_limit", "boolean_terms"),
    ("query_plan_v2_compatibility_value_limit", "canonical_bytes"),
    ("query_plan_v2_cross_join_limit", "allowed_cross_joins"),
    ("query_plan_v2_order_limit", "order_terms"),
    ("query_plan_v2_output_limit", "selected_slots"),
    ("query_plan_v2_pattern_depth_limit", "predicate_depth"),
    ("query_plan_v2_pattern_node_limit", "predicate_nodes"),
    ("query_plan_v2_reachable_depth", "predicate_depth"),
    ("query_plan_v2_reachable_expansion_limit", "predicate_nodes"),
    ("query_plan_v2_reducer_limit", "selected_slots"),
    ("query_plan_v2_reduction_group_limit", "selected_slots"),
    ("query_remote_v2_envelope_too_large", "remote_request_bytes"),
    ("query_v2_adapter_literal_byte_limit", "canonical_bytes"),
    ("query_v2_lowered_typeql_byte_limit", "canonical_bytes"),
    ("query_v2_reachable_expansion_limit", "predicate_nodes"),
];

impl QueryLimits {
    /// Return whether the named limit is still at its released default.
    ///
    /// The answer is the `source` detail of a diagnostic raised against
    /// that limit. A caller that merged in advertised limits overrides it
    /// with [`LIMIT_SOURCE_ADVERTISED`] where the advertisement is binding.
    #[must_use]
    pub fn source_of(&self, name: &str) -> &'static str {
        if self.get(name) == Self::CANONICAL.get(name) {
            LIMIT_SOURCE_DEFAULT
        } else {
            LIMIT_SOURCE_CONFIGURED
        }
    }

    /// Name the limit a resource-limit diagnostic was raised against.
    ///
    /// Adds `limit`, `maximum`, and `source` details to a query
    /// resource-limit diagnostic that does not carry them yet. Any other
    /// diagnostic is returned unchanged.
    #[must_use]
    pub fn annotate(self, diagnostic: Diagnostic) -> Diagnostic {
        let Some((_, limit)) = LIMIT_BY_DIAGNOSTIC_CODE
            .iter()
            .find(|(code, _)| *code == diagnostic.code().as_str())
        else {
            return diagnostic;
        };
        self.annotate_limit(diagnostic, limit)
    }

    /// Annotate a refusal raised by a stage that holds only the one ceiling
    /// it checked, such as a binding bounding a foreign sequence before it
    /// materializes it.
    ///
    /// `maximum` is the ceiling that stage enforced. `source` is derived by
    /// comparing it with the canonical default of the limit the code names.
    #[must_use]
    pub fn annotate_refusal(diagnostic: Diagnostic, maximum: usize) -> Diagnostic {
        let Some((_, limit)) = LIMIT_BY_DIAGNOSTIC_CODE
            .iter()
            .find(|(code, _)| *code == diagnostic.code().as_str())
        else {
            return diagnostic;
        };
        let mut limits = Self::CANONICAL;
        limits.set(limit, maximum);
        limits.annotate_limit(diagnostic, limit)
    }

    /// [`Self::annotate`] for limits merged from a caller's own and an
    /// executor's advertisement.
    ///
    /// `self` is the merged value. Where the advertisement is the stricter
    /// side for the refused limit, `source` says so: raising the caller's
    /// own setting would not help.
    #[must_use]
    pub fn annotate_negotiated(self, configured: Self, diagnostic: Diagnostic) -> Diagnostic {
        self.mark_advertised(configured, self.annotate(diagnostic))
    }

    /// Set `source` to advertised on an annotated refusal whose limit the
    /// advertisement tightened below the caller's own value.
    #[must_use]
    pub fn mark_advertised(self, configured: Self, diagnostic: Diagnostic) -> Diagnostic {
        let Some(DiagnosticDetailValue::Text(limit)) = diagnostic.details().get("limit") else {
            return diagnostic;
        };
        if self.get(limit) < configured.get(limit) {
            diagnostic.with_detail("source", LIMIT_SOURCE_ADVERTISED)
        } else {
            diagnostic
        }
    }

    /// [`Self::annotate`] for a diagnostic whose limit the caller knows
    /// better than the code table, such as a codec failure of an envelope.
    #[must_use]
    pub fn annotate_limit(self, diagnostic: Diagnostic, limit: &'static str) -> Diagnostic {
        if diagnostic.category() != DiagnosticCategory::ResourceLimit
            || diagnostic.details().contains_key("limit")
        {
            return diagnostic;
        }
        let Some(maximum) = self.get(limit) else {
            return diagnostic;
        };
        diagnostic
            .with_detail("limit", limit)
            .with_detail("maximum", i64::try_from(maximum).unwrap_or(i64::MAX))
            .with_detail("source", self.source_of(limit))
    }
}

impl Default for QueryLimits {
    fn default() -> Self {
        Self::CANONICAL
    }
}

#[cfg(test)]
mod query_limits_tests {
    use super::*;
    use crate::codec::{from_canonical_json_with_limits, to_canonical_json_with_limits};

    fn nested(depth: usize) -> serde_json::Value {
        let mut value = serde_json::Value::Array(Vec::new());
        for _ in 1..depth {
            value = serde_json::Value::Array(vec![value]);
        }
        value
    }

    #[test]
    fn canonical_limits_are_the_released_constants_and_validate() {
        let limits = QueryLimits::CANONICAL.validate().expect("canonical limits");
        assert_eq!(limits.remote_request_codec(), REMOTE_REQUEST_CODEC_LIMITS);
        assert_eq!(limits.invocation_bytes(), MAX_QUERY_INVOCATION_BYTES);
        assert_eq!(QueryLimits::default(), limits);
    }

    #[test]
    fn every_limit_is_named_once_and_round_trips() {
        let mut limits = QueryLimits::CANONICAL;
        let mut seen = std::collections::BTreeSet::new();
        for (index, name) in QueryLimits::FIELDS.iter().enumerate() {
            assert!(seen.insert(*name), "duplicate limit name {name}");
            assert!(limits.set(name, index + 7));
        }
        for (index, name) in QueryLimits::FIELDS.iter().enumerate() {
            assert_eq!(limits.get(name), Some(index + 7));
        }
        assert!(!limits.set("unknown", 1));
        assert_eq!(limits.get("unknown"), None);
        // Every field of the three structs is reachable by name: a value
        // with every named limit changed shares no field with the default.
        let codec = limits.codec;
        let structural = limits.structural;
        assert_eq!(QueryLimits::FIELDS.len(), 18);
        assert_ne!(codec.max_bytes, CodecLimits::CANONICAL.max_bytes);
        assert_ne!(structural.diagnostic_bytes, MAX_DIAGNOSTIC_BYTES);
    }

    #[test]
    fn validation_refuses_zero_and_unsupported_values_by_name() {
        let mut zero = QueryLimits::CANONICAL;
        zero.structural.boolean_terms = 0;
        assert_eq!(zero.validate().unwrap_err().limit(), "boolean_terms");

        let mut deep = QueryLimits::CANONICAL;
        deep.codec.max_depth = MAX_SUPPORTED_CANONICAL_DEPTH + 1;
        let error = deep.validate().unwrap_err();
        assert_eq!(error.limit(), "canonical_depth");
        assert!(error.to_string().contains("between 1 and 126"));

        let mut wide = QueryLimits::CANONICAL;
        wide.structural.bindings = MAX_SUPPORTED_BINDINGS + 1;
        assert_eq!(wide.validate().unwrap_err().limit(), "bindings");

        let mut slots = QueryLimits::CANONICAL;
        slots.structural.selected_slots = MAX_SUPPORTED_SELECTED_SLOTS + 1;
        assert_eq!(slots.validate().unwrap_err().limit(), "selected_slots");

        let mut raised = QueryLimits::CANONICAL;
        raised.codec.max_bytes = 64 * 1024 * 1024;
        raised.remote_request_bytes = 128 * 1024 * 1024;
        raised.codec.max_depth = MAX_SUPPORTED_CANONICAL_DEPTH;
        raised
            .validate()
            .expect("limits above canonical are admissible");
    }

    #[test]
    fn every_attributed_code_names_a_real_limit() {
        for (code, limit) in LIMIT_BY_DIAGNOSTIC_CODE {
            assert!(
                QueryLimits::CANONICAL.get(limit).is_some(),
                "{code} is attributed to unknown limit {limit}"
            );
        }
    }

    #[test]
    fn annotation_names_the_limit_its_value_and_its_source() {
        use crate::diagnostic::{DiagnosticCode, DiagnosticDetailValue};
        let raise = |code: &str, category| {
            Diagnostic::new(
                category,
                DiagnosticCode::new(code).expect("code"),
                "refused",
            )
        };
        let mut limits = QueryLimits::CANONICAL;
        limits.structural.boolean_terms = 8;

        let configured = limits.annotate(raise(
            "query_plan_v2_boolean_term_limit",
            DiagnosticCategory::ResourceLimit,
        ));
        assert_eq!(
            configured.details().get("limit"),
            Some(&DiagnosticDetailValue::Text("boolean_terms".into()))
        );
        assert_eq!(
            configured.details().get("maximum"),
            Some(&DiagnosticDetailValue::Long(8))
        );
        assert_eq!(
            configured.details().get("source"),
            Some(&DiagnosticDetailValue::Text("configured".into()))
        );

        let default = limits.annotate(raise(
            "canonical_json_too_deep",
            DiagnosticCategory::ResourceLimit,
        ));
        assert_eq!(
            default.details().get("source"),
            Some(&DiagnosticDetailValue::Text("default".into()))
        );
        assert_eq!(
            default.details().get("maximum"),
            Some(&DiagnosticDetailValue::Long(64))
        );

        // Not a resource limit, and not an attributed code: both unchanged.
        let invalid = limits.annotate(raise(
            "canonical_json_too_deep",
            DiagnosticCategory::InvalidContract,
        ));
        assert!(invalid.details().is_empty());
        let unknown = limits.annotate(raise("some_other_limit", DiagnosticCategory::ResourceLimit));
        assert!(unknown.details().is_empty());
    }

    #[test]
    fn named_overrides_build_validated_limits() {
        let limits = QueryLimits::from_overrides([("canonical_bytes", 64 << 20), ("bindings", 8)])
            .expect("named overrides");
        assert_eq!(limits.codec.max_bytes, 64 << 20);
        assert_eq!(limits.structural.bindings, 8);
        assert_eq!(limits.structural.boolean_terms, MAX_BOOLEAN_TERMS);
        assert_eq!(
            QueryLimits::from_overrides([]).expect("no overrides"),
            QueryLimits::CANONICAL
        );
        assert_eq!(
            QueryLimits::from_overrides([("made_up", 1)]).unwrap_err(),
            QueryLimitsError::Unknown {
                name: "made_up".into()
            }
        );
        assert_eq!(
            QueryLimits::from_overrides([("canonical_depth", 1_000)])
                .unwrap_err()
                .limit(),
            "canonical_depth"
        );
    }

    #[test]
    fn a_refusal_is_annotated_from_the_ceiling_its_stage_enforced() {
        use crate::diagnostic::{DiagnosticCode, DiagnosticDetailValue};
        let refusal = || {
            Diagnostic::new(
                DiagnosticCategory::ResourceLimit,
                DiagnosticCode::new("query_plan_sort_term_limit").expect("code"),
                "refused",
            )
        };
        let default = QueryLimits::annotate_refusal(refusal(), MAX_ORDER_TERMS);
        assert_eq!(
            default.details().get("source"),
            Some(&DiagnosticDetailValue::Text("default".into()))
        );
        let configured = QueryLimits::annotate_refusal(refusal(), 4);
        assert_eq!(
            configured.details().get("maximum"),
            Some(&DiagnosticDetailValue::Long(4))
        );
        assert_eq!(
            configured.details().get("source"),
            Some(&DiagnosticDetailValue::Text("configured".into()))
        );
    }

    #[test]
    fn min_takes_the_stricter_value_of_each_limit() {
        let mut left = QueryLimits::CANONICAL;
        left.codec.max_depth = 100;
        left.structural.bindings = 8;
        let mut right = QueryLimits::CANONICAL;
        right.structural.bindings = 512;
        right.remote_request_bytes = 1024;
        let merged = left.min(right);
        assert_eq!(merged.codec.max_depth, 64);
        assert_eq!(merged.structural.bindings, 8);
        assert_eq!(merged.remote_request_bytes, 1024);
    }

    #[test]
    fn the_supported_depth_is_what_the_parser_decodes_inside_a_request_frame() {
        let mut limits = QueryLimits::CANONICAL;
        limits.codec.max_depth = MAX_SUPPORTED_CANONICAL_DEPTH;
        let request = limits
            .validate()
            .expect("supported depth")
            .remote_request_codec();
        let deepest = nested(MAX_SUPPORTED_CANONICAL_DEPTH + 1);
        let bytes = to_canonical_json_with_limits(&deepest, request).expect("encodes");
        from_canonical_json_with_limits::<serde_json::Value>(&bytes, request)
            .expect("the deepest admissible request frame decodes");

        // One level further is beyond the parser whatever the limits say,
        // which is why validation stops where it does.
        let unbounded = CodecLimits {
            max_depth: usize::MAX,
            ..request
        };
        let beyond = to_canonical_json_with_limits(&nested(128), unbounded).expect("encodes");
        assert!(from_canonical_json_with_limits::<serde_json::Value>(&beyond, unbounded).is_err());
    }
}
