//! Binding-neutral generated-query execution and materialization limits.

use std::time::{Duration, Instant};

use std::collections::BTreeMap;

use type_bridge_contract::limits::{
    ANSWER_LIMIT_FIELDS, LIMIT_SOURCE_CONFIGURED, LIMIT_SOURCE_DEFAULT, answer_limit_field,
};
use type_bridge_contract::query_remote_v2::RemoteLimitsV2;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
    SdkQueryDiagnosticCategory, SdkQueryDiagnosticPathKind,
};

use crate::match_request::MatchExecutionLimits;
use crate::projected_query::ProjectedQueryMaterializationLimits;
use crate::session::backend::AnswerCancellation;

/// Direct-session default execution duration in milliseconds.
pub const DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS: u64 = ANSWER_LIMIT_FIELDS[0].direct_default;
/// Direct-session default provider/result item count.
pub const DEFAULT_DIRECT_QUERY_ITEMS: u64 = ANSWER_LIMIT_FIELDS[1].direct_default;
/// Direct-session default provider, reply, and projected payload bytes.
pub const DEFAULT_DIRECT_QUERY_BYTES: u64 = ANSWER_LIMIT_FIELDS[2].direct_default;
/// Direct-session default hydrated/projected graph-node count.
pub const DEFAULT_DIRECT_QUERY_GRAPH_NODES: u64 = ANSWER_LIMIT_FIELDS[3].direct_default;
/// Direct-session default hydrated/projected attribute-value count.
pub const DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES: u64 = ANSWER_LIMIT_FIELDS[4].direct_default;
/// Direct-session default collection members, including reduction cells.
pub const DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS: u64 = ANSWER_LIMIT_FIELDS[5].direct_default;
/// Direct-session default relation-role-player references.
pub const DEFAULT_DIRECT_QUERY_ROLE_PLAYERS: u64 = ANSWER_LIMIT_FIELDS[6].direct_default;
/// Direct-session default provider statements per terminal.
pub const DEFAULT_DIRECT_QUERY_STATEMENTS: u32 = ANSWER_LIMIT_FIELDS[7].direct_default as u32;

/// Proxy-connection default execution duration in milliseconds.
pub const DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS: u64 = ANSWER_LIMIT_FIELDS[0].proxy_default;
/// Proxy-connection default provider/result item count.
pub const DEFAULT_PROXY_QUERY_ITEMS: u64 = ANSWER_LIMIT_FIELDS[1].proxy_default;
/// Proxy-connection default provider, reply, and projected payload bytes.
pub const DEFAULT_PROXY_QUERY_BYTES: u64 = ANSWER_LIMIT_FIELDS[2].proxy_default;
/// Proxy-connection default hydrated/projected graph-node count.
pub const DEFAULT_PROXY_QUERY_GRAPH_NODES: u64 = ANSWER_LIMIT_FIELDS[3].proxy_default;
/// Proxy-connection default hydrated/projected attribute-value count.
pub const DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES: u64 = ANSWER_LIMIT_FIELDS[4].proxy_default;
/// Proxy-connection default collection members, including reduction cells.
pub const DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS: u64 = ANSWER_LIMIT_FIELDS[5].proxy_default;
/// Proxy-connection default relation-role-player references.
pub const DEFAULT_PROXY_QUERY_ROLE_PLAYERS: u64 = ANSWER_LIMIT_FIELDS[6].proxy_default;
/// Proxy-connection default provider statements per terminal.
pub const DEFAULT_PROXY_QUERY_STATEMENTS: u32 = ANSWER_LIMIT_FIELDS[7].proxy_default as u32;

/// Longest execution duration a policy may ask for, in milliseconds.
pub const MAX_QUERY_TIMEOUT_MILLISECONDS: u64 = ANSWER_LIMIT_FIELDS[0].maximum;
/// Most provider statements a terminal can need. No terminal issues more,
/// so a larger value would be accepted and ignored.
pub const MAX_QUERY_STATEMENTS: u32 = ANSWER_LIMIT_FIELDS[7].maximum as u32;

/// The 2.2 name of [`DEFAULT_PROXY_QUERY_ITEMS`], with the same value.
#[deprecated(
    since = "2.3.0",
    note = "use DEFAULT_PROXY_QUERY_ITEMS; removed in 3.0"
)]
pub const MAX_QUERY_ITEMS: u64 = DEFAULT_PROXY_QUERY_ITEMS;
/// The 2.2 name of [`DEFAULT_PROXY_QUERY_BYTES`], with the same value.
#[deprecated(
    since = "2.3.0",
    note = "use DEFAULT_PROXY_QUERY_BYTES; removed in 3.0"
)]
pub const MAX_QUERY_BYTES: u64 = DEFAULT_PROXY_QUERY_BYTES;
/// The 2.2 name of [`DEFAULT_PROXY_QUERY_GRAPH_NODES`], with the same value.
#[deprecated(
    since = "2.3.0",
    note = "use DEFAULT_PROXY_QUERY_GRAPH_NODES; removed in 3.0"
)]
pub const MAX_QUERY_GRAPH_NODES: u64 = DEFAULT_PROXY_QUERY_GRAPH_NODES;
/// The 2.2 name of [`DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES`], with the same value.
#[deprecated(
    since = "2.3.0",
    note = "use DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES; removed in 3.0"
)]
pub const MAX_QUERY_ATTRIBUTE_VALUES: u64 = DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES;
/// The 2.2 name of [`DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS`], with the same value.
#[deprecated(
    since = "2.3.0",
    note = "use DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS; removed in 3.0"
)]
pub const MAX_QUERY_COLLECTION_MEMBERS: u64 = DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS;
/// The 2.2 name of [`DEFAULT_PROXY_QUERY_ROLE_PLAYERS`], with the same value.
#[deprecated(
    since = "2.3.0",
    note = "use DEFAULT_PROXY_QUERY_ROLE_PLAYERS; removed in 3.0"
)]
pub const MAX_QUERY_ROLE_PLAYERS: u64 = DEFAULT_PROXY_QUERY_ROLE_PLAYERS;

/// One absolute monotonic deadline shared by every stage of one query terminal.
///
/// Capture this value at the public terminal entry, before semantic
/// construction or allocation. Passing the same token through direct or
/// remote execution and projected materialization prevents any stage from
/// restarting the caller's timeout.
#[derive(Clone, Copy, Debug)]
pub struct QueryExecutionDeadline {
    deadline: Instant,
}

impl QueryExecutionDeadline {
    /// Capture one absolute deadline from a timeout, clamped to the longest
    /// supported execution duration.
    #[must_use]
    pub fn from_timeout_milliseconds(timeout_milliseconds: u64) -> Self {
        let timeout = timeout_milliseconds.min(MAX_QUERY_TIMEOUT_MILLISECONDS);
        let now = Instant::now();
        Self {
            deadline: now
                .checked_add(Duration::from_millis(timeout))
                .unwrap_or(now),
        }
    }

    /// Capture the deadline prescribed by one common resource policy.
    #[must_use]
    pub fn for_limits(limits: QueryExecutionResourceLimits) -> Self {
        Self::from_timeout_milliseconds(limits.effective().timeout_milliseconds)
    }

    /// Reuse an already established monotonic deadline at an internal seam.
    #[doc(hidden)]
    #[must_use]
    pub(crate) const fn from_instant(deadline: Instant) -> Self {
        Self { deadline }
    }

    /// The earlier of this deadline and `other`.
    #[must_use]
    pub fn earliest(self, other: Self) -> Self {
        Self {
            deadline: self.deadline.min(other.deadline),
        }
    }

    /// Return whether the absolute deadline has elapsed.
    #[must_use]
    pub fn is_expired(self) -> bool {
        Instant::now() >= self.deadline
    }

    /// Return the exact monotonic deadline for native await combinators.
    #[doc(hidden)]
    #[must_use]
    pub const fn instant(self) -> Instant {
        self.deadline
    }

    /// Return the remaining whole-millisecond remote request lifetime.
    ///
    /// A positive sub-millisecond remainder rounds up to one millisecond; an
    /// expired budget returns zero and must be rejected before request
    /// construction.
    #[doc(hidden)]
    #[must_use]
    pub fn remaining_milliseconds(self) -> u64 {
        let now = Instant::now();
        if now >= self.deadline {
            return 0;
        }
        let remaining = self.deadline.duration_since(now);
        u64::try_from(remaining.as_millis())
            .unwrap_or(u64::MAX)
            .max(1)
    }

    /// Check the shared cancellation owner and this invocation deadline,
    /// returning the canonical redacted SDK diagnostic on interruption.
    #[doc(hidden)]
    pub fn check(self, cancellation: &AnswerCancellation) -> Result<(), SdkExecutionDiagnostic> {
        let (category, code) = if cancellation.is_cancelled() {
            (SdkQueryDiagnosticCategory::Cancelled, "provider_cancelled")
        } else if self.is_expired() {
            (
                SdkQueryDiagnosticCategory::ResourceLimit,
                "transaction_deadline_exceeded",
            )
        } else {
            return Ok(());
        };
        let diagnostic = SdkExecutionDiagnostic::query_failure(
            category,
            SdkDiagnosticCode::new(code).expect("static query interruption code is canonical"),
        )
        .try_at(SdkDiagnosticPathSegment::Query(
            SdkQueryDiagnosticPathKind::ProviderEvidence,
        ))
        .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure());
        Err(diagnostic)
    }
}

/// Released resource-limit codes and the policy dimension each one reports.
const DIMENSION_BY_REFUSAL_CODE: [(&str, &str); 9] = [
    ("transaction_deadline_exceeded", "timeout_milliseconds"),
    ("processed_item_limit", "items"),
    ("solution_scan_limit", "items"),
    ("response_byte_limit", "bytes"),
    ("hydrated_thing_limit", "graph_nodes"),
    ("hydrated_attribute_value_limit", "attribute_values"),
    ("collected_concept_limit", "collection_members"),
    ("hydrated_role_player_limit", "role_players"),
    ("statement_count_limit", "statements"),
];

/// One answer-policy value above its supported maximum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryExecutionLimitOutOfRange {
    /// Stable name of the dimension.
    pub limit: &'static str,
    /// The refused value.
    pub value: u64,
    /// The largest supported value.
    pub maximum: u64,
}

impl std::fmt::Display for QueryExecutionLimitOutOfRange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "answer limit `{}` is {}, above the supported maximum {}",
            self.limit, self.value, self.maximum
        )
    }
}

impl std::error::Error for QueryExecutionLimitOutOfRange {}

/// Why a set of named answer-policy values was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryExecutionLimitsError {
    /// The name is not an answer-policy dimension.
    Unknown {
        /// The refused name.
        name: String,
    },
    /// The value is above the dimension's supported maximum.
    OutOfRange(QueryExecutionLimitOutOfRange),
}

impl std::fmt::Display for QueryExecutionLimitsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown { name } => write!(formatter, "`{name}` is not a known answer limit"),
            Self::OutOfRange(refusal) => refusal.fmt(formatter),
        }
    }
}

impl std::error::Error for QueryExecutionLimitsError {}

/// The transport an answer policy runs on, which decides its defaults.
///
/// A direct session keeps the 2.2 in-process defaults. A proxy connection
/// defaults to the wire ceilings, so an unconfigured remote answer fits one
/// envelope and a default-configured server advertises no override.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum AnswerTransport {
    /// An in-process session against the database.
    Direct,
    /// A connection to a TypeBridge query server.
    Proxy,
}

/// The answer policy of one query terminal, the same on every transport.
///
/// `items` bounds provider evidence and public rows, `bytes` bounds
/// provider/reply bytes and projected payload bytes, `graph_nodes` includes
/// top-level things and nested role players, `attribute_values` includes
/// hydrated and projected scalars, and `collection_members` includes collected
/// identities plus group keys and reducer cells. `role_players` counts
/// role-player references independently of their graph nodes. `statements` is
/// enforced before each provider call. No field is accepted and ignored on
/// either lane.
///
/// [`Self::default_for`] is the policy a session starts from on its
/// transport, and [`Default`] is the direct one. A caller may tighten
/// any dimension, and may raise one by configuring it explicitly; only the
/// timeout and the statement count have a supported maximum. A session never
/// widens the policy of the connection it runs on
/// ([`Self::constrained_by`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryExecutionResourceLimits {
    /// Execution timeout in milliseconds.
    pub timeout_milliseconds: u64,
    /// Provider/result item ceiling.
    pub items: u64,
    /// Provider/reply/projected byte ceiling.
    pub bytes: u64,
    /// Hydrated/projected graph-node ceiling.
    pub graph_nodes: u64,
    /// Hydrated/projected attribute-value ceiling.
    pub attribute_values: u64,
    /// Collection-member and reduction-cell ceiling.
    pub collection_members: u64,
    /// Role-player reference ceiling.
    pub role_players: u64,
    /// Provider statement ceiling.
    pub statements: u32,
}

impl QueryExecutionResourceLimits {
    /// The policy dimensions by their stable public names, in declaration
    /// order.
    pub const NAMES: [&'static str; 8] = [
        ANSWER_LIMIT_FIELDS[0].name,
        ANSWER_LIMIT_FIELDS[1].name,
        ANSWER_LIMIT_FIELDS[2].name,
        ANSWER_LIMIT_FIELDS[3].name,
        ANSWER_LIMIT_FIELDS[4].name,
        ANSWER_LIMIT_FIELDS[5].name,
        ANSWER_LIMIT_FIELDS[6].name,
        ANSWER_LIMIT_FIELDS[7].name,
    ];

    /// The policy in force on `transport` when nothing was configured.
    ///
    /// This is the one place an unset policy becomes a default.
    #[must_use]
    pub const fn default_for(transport: AnswerTransport) -> Self {
        match transport {
            AnswerTransport::Direct => Self {
                timeout_milliseconds: DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS,
                items: DEFAULT_DIRECT_QUERY_ITEMS,
                bytes: DEFAULT_DIRECT_QUERY_BYTES,
                graph_nodes: DEFAULT_DIRECT_QUERY_GRAPH_NODES,
                attribute_values: DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES,
                collection_members: DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS,
                role_players: DEFAULT_DIRECT_QUERY_ROLE_PLAYERS,
                statements: DEFAULT_DIRECT_QUERY_STATEMENTS,
            },
            AnswerTransport::Proxy => Self {
                timeout_milliseconds: DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS,
                items: DEFAULT_PROXY_QUERY_ITEMS,
                bytes: DEFAULT_PROXY_QUERY_BYTES,
                graph_nodes: DEFAULT_PROXY_QUERY_GRAPH_NODES,
                attribute_values: DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES,
                collection_members: DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS,
                role_players: DEFAULT_PROXY_QUERY_ROLE_PLAYERS,
                statements: DEFAULT_PROXY_QUERY_STATEMENTS,
            },
        }
    }

    /// Build a policy from named dimensions; an unnamed one keeps its
    /// `transport` default. An unknown name or a value past its maximum is
    /// refused.
    pub fn from_overrides<'name>(
        transport: AnswerTransport,
        overrides: impl IntoIterator<Item = (&'name str, u64)>,
    ) -> Result<Self, QueryExecutionLimitsError> {
        let mut policy = Self::default_for(transport);
        for (name, value) in overrides {
            let field =
                answer_limit_field(name).ok_or_else(|| QueryExecutionLimitsError::Unknown {
                    name: name.to_owned(),
                })?;
            if value > field.maximum {
                return Err(QueryExecutionLimitsError::OutOfRange(
                    QueryExecutionLimitOutOfRange {
                        limit: field.name,
                        value,
                        maximum: field.maximum,
                    },
                ));
            }
            match field.name {
                "timeout_milliseconds" => policy.timeout_milliseconds = value,
                "items" => policy.items = value,
                "bytes" => policy.bytes = value,
                "graph_nodes" => policy.graph_nodes = value,
                "attribute_values" => policy.attribute_values = value,
                "collection_members" => policy.collection_members = value,
                "role_players" => policy.role_players = value,
                _ => policy.statements = u32::try_from(value).unwrap_or(u32::MAX),
            }
        }
        Ok(policy)
    }

    /// The dimensions that differ from the proxy (wire) defaults, by stable
    /// name: the form an executor advertises. A server's ceilings are read
    /// back with [`Self::from_overrides`] on [`AnswerTransport::Proxy`].
    #[must_use]
    pub fn overrides(self) -> BTreeMap<String, u64> {
        ANSWER_LIMIT_FIELDS
            .iter()
            .filter_map(|field| {
                let value = self.get(field.name)?;
                (value != field.proxy_default).then(|| (field.name.to_owned(), value))
            })
            .collect()
    }

    /// Construct a policy no looser than the direct default, the loosest
    /// default, in any dimension.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub const fn tightened(
        timeout_milliseconds: u64,
        items: u64,
        bytes: u64,
        graph_nodes: u64,
        attribute_values: u64,
        collection_members: u64,
        role_players: u64,
        statements: u32,
    ) -> Self {
        Self {
            timeout_milliseconds: min_u64(
                timeout_milliseconds,
                DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS,
            ),
            items: min_u64(items, DEFAULT_DIRECT_QUERY_ITEMS),
            bytes: min_u64(bytes, DEFAULT_DIRECT_QUERY_BYTES),
            graph_nodes: min_u64(graph_nodes, DEFAULT_DIRECT_QUERY_GRAPH_NODES),
            attribute_values: min_u64(attribute_values, DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES),
            collection_members: min_u64(
                collection_members,
                DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS,
            ),
            role_players: min_u64(role_players, DEFAULT_DIRECT_QUERY_ROLE_PLAYERS),
            statements: min_u32(statements, DEFAULT_DIRECT_QUERY_STATEMENTS),
        }
    }

    /// Check a configured policy against the supported maxima.
    ///
    /// A value above its maximum is refused by name, never clamped: a caller
    /// who asked for more must not silently get less.
    pub const fn validate(self) -> Result<Self, QueryExecutionLimitOutOfRange> {
        if self.timeout_milliseconds > MAX_QUERY_TIMEOUT_MILLISECONDS {
            return Err(QueryExecutionLimitOutOfRange {
                limit: "timeout_milliseconds",
                value: self.timeout_milliseconds,
                maximum: MAX_QUERY_TIMEOUT_MILLISECONDS,
            });
        }
        if self.statements > MAX_QUERY_STATEMENTS {
            return Err(QueryExecutionLimitOutOfRange {
                limit: "statements",
                value: self.statements as u64,
                maximum: MAX_QUERY_STATEMENTS as u64,
            });
        }
        Ok(self)
    }

    /// Read the policy a remote request budget states.
    #[doc(hidden)]
    #[must_use]
    pub fn from_remote(limits: RemoteLimitsV2) -> Self {
        Self {
            timeout_milliseconds: limits
                .deadline_ms
                .unwrap_or(DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS),
            items: limits.max_items,
            bytes: limits.max_bytes,
            graph_nodes: limits.max_graph_nodes,
            attribute_values: limits.max_attribute_values,
            collection_members: limits.max_collection_members,
            role_players: limits.max_role_players,
            statements: limits.max_statements,
        }
    }

    /// Describe the ceiling behind one resource-limit refusal.
    ///
    /// Returns the dimension's stable name, the value in force, and whether
    /// that value is `transport`'s default or one the caller configured.
    /// `None` for a code that reports no policy dimension.
    #[must_use]
    pub fn refusal_details(
        self,
        transport: AnswerTransport,
        code: &str,
    ) -> Option<(&'static str, u64, &'static str)> {
        let name = DIMENSION_BY_REFUSAL_CODE
            .iter()
            .find_map(|(refusal, name)| (*refusal == code).then_some(*name))?;
        let value = self.get(name)?;
        let source = if Self::default_for(transport).get(name) == Some(value) {
            LIMIT_SOURCE_DEFAULT
        } else {
            LIMIT_SOURCE_CONFIGURED
        };
        Some((name, value, source))
    }

    /// Return one dimension by its stable name.
    #[must_use]
    pub fn get(self, name: &str) -> Option<u64> {
        Some(match name {
            "timeout_milliseconds" => self.timeout_milliseconds,
            "items" => self.items,
            "bytes" => self.bytes,
            "graph_nodes" => self.graph_nodes,
            "attribute_values" => self.attribute_values,
            "collection_members" => self.collection_members,
            "role_players" => self.role_players,
            "statements" => u64::from(self.statements),
            _ => return None,
        })
    }

    /// Bound a struct-literal policy by the supported maxima.
    ///
    /// Internal stages call this so no arithmetic sees an unsupported
    /// value; a public entry point validates first and refuses instead.
    #[doc(hidden)]
    #[must_use]
    pub const fn effective(self) -> Self {
        Self {
            timeout_milliseconds: min_u64(
                self.timeout_milliseconds,
                MAX_QUERY_TIMEOUT_MILLISECONDS,
            ),
            statements: min_u32(self.statements, MAX_QUERY_STATEMENTS),
            ..self
        }
    }

    /// Intersect this per-terminal policy with an enclosing connection policy.
    ///
    /// Every dimension is independently tighten-only; a query session can
    /// never widen a stricter connection-time ceiling.
    #[doc(hidden)]
    #[must_use]
    pub const fn constrained_by(self, ceiling: Self) -> Self {
        let this = self.effective();
        let ceiling = ceiling.effective();
        Self {
            timeout_milliseconds: min_u64(this.timeout_milliseconds, ceiling.timeout_milliseconds),
            items: min_u64(this.items, ceiling.items),
            bytes: min_u64(this.bytes, ceiling.bytes),
            graph_nodes: min_u64(this.graph_nodes, ceiling.graph_nodes),
            attribute_values: min_u64(this.attribute_values, ceiling.attribute_values),
            collection_members: min_u64(this.collection_members, ceiling.collection_members),
            role_players: min_u64(this.role_players, ceiling.role_players),
            statements: min_u32(this.statements, ceiling.statements),
        }
    }

    /// Derive direct provider execution limits with a separate cancellation owner.
    #[doc(hidden)]
    #[must_use]
    pub fn direct(self, cancellation: AnswerCancellation) -> MatchExecutionLimits {
        self.direct_with_deadline(cancellation, QueryExecutionDeadline::for_limits(self))
    }

    /// Derive direct execution limits bound to an already captured invocation deadline.
    #[doc(hidden)]
    #[must_use]
    pub fn direct_with_deadline(
        self,
        cancellation: AnswerCancellation,
        deadline: QueryExecutionDeadline,
    ) -> MatchExecutionLimits {
        MatchExecutionLimits::from_policy(self, cancellation).with_absolute_deadline(deadline)
    }

    /// Derive all-or-nothing projected result limits.
    #[doc(hidden)]
    #[must_use]
    pub fn projected(self) -> ProjectedQueryMaterializationLimits {
        let this = self.effective();
        ProjectedQueryMaterializationLimits::new(
            usize_limit(this.items),
            usize_limit(this.collection_members),
            usize_limit(this.graph_nodes),
            usize_limit(this.attribute_values),
            usize_limit(this.bytes),
        )
    }

    /// Derive the exact additive V2 remote wire budget.
    #[doc(hidden)]
    #[must_use]
    pub const fn remote(self) -> RemoteLimitsV2 {
        let this = self.effective();
        RemoteLimitsV2 {
            deadline_ms: Some(this.timeout_milliseconds),
            max_bytes: this.bytes,
            max_items: this.items,
            max_collection_members: this.collection_members,
            max_graph_nodes: this.graph_nodes,
            max_attribute_values: this.attribute_values,
            max_role_players: this.role_players,
            max_statements: this.statements,
        }
    }
}

impl Default for QueryExecutionResourceLimits {
    /// The direct-session default, [`Self::default_for`] on
    /// [`AnswerTransport::Direct`].
    fn default() -> Self {
        Self::default_for(AnswerTransport::Direct)
    }
}

const fn min_u64(left: u64, right: u64) -> u64 {
    if left < right { left } else { right }
}

const fn min_u32(left: u32, right: u32) -> u32 {
    if left < right { left } else { right }
}

fn usize_limit(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_policy_clamps_and_derives_every_direct_and_projected_dimension() {
        let limits = QueryExecutionResourceLimits::tightened(17, 2, 19, 3, 5, 7, 11, 1);
        let projected = limits.projected();
        assert_eq!(projected.rows(), 2);
        assert_eq!(projected.cells(), 7);
        assert_eq!(projected.things(), 3);
        assert_eq!(projected.attribute_values(), 5);
        assert_eq!(projected.bytes(), 19);
        assert_eq!(limits.role_players, 11);
        assert_eq!(limits.statements, 1);
        assert_eq!(
            limits
                .direct(AnswerCancellation::default())
                .resource_dimensions(),
            (3, 5, 7, 11, 1)
        );
        assert_eq!(
            limits.remote(),
            RemoteLimitsV2 {
                deadline_ms: Some(17),
                max_bytes: 19,
                max_items: 2,
                max_collection_members: 7,
                max_graph_nodes: 3,
                max_attribute_values: 5,
                max_role_players: 11,
                max_statements: 1,
            }
        );
    }

    #[test]
    fn the_policy_tightens_per_dimension_and_validates_what_is_raised() {
        let plus_one = QueryExecutionResourceLimits::tightened(
            DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS + 1,
            DEFAULT_DIRECT_QUERY_ITEMS + 1,
            DEFAULT_DIRECT_QUERY_BYTES + 1,
            DEFAULT_DIRECT_QUERY_GRAPH_NODES + 1,
            DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES + 1,
            DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS + 1,
            DEFAULT_DIRECT_QUERY_ROLE_PLAYERS + 1,
            DEFAULT_DIRECT_QUERY_STATEMENTS + 1,
        );
        assert_eq!(plus_one, QueryExecutionResourceLimits::default());

        let literal = QueryExecutionResourceLimits {
            timeout_milliseconds: u64::MAX,
            items: u64::MAX,
            bytes: u64::MAX,
            graph_nodes: u64::MAX,
            attribute_values: u64::MAX,
            collection_members: u64::MAX,
            role_players: u64::MAX,
            statements: u32::MAX,
        };
        // A configured policy keeps what it asked for. Only the timeout and
        // the statement count have a supported maximum: internal stages
        // bound them, and a public entry point refuses them by name.
        let bounded = literal.effective();
        assert_eq!(
            bounded,
            QueryExecutionResourceLimits {
                timeout_milliseconds: MAX_QUERY_TIMEOUT_MILLISECONDS,
                statements: MAX_QUERY_STATEMENTS,
                ..literal
            }
        );
        assert_eq!(
            literal.validate(),
            Err(QueryExecutionLimitOutOfRange {
                limit: "timeout_milliseconds",
                value: u64::MAX,
                maximum: MAX_QUERY_TIMEOUT_MILLISECONDS,
            })
        );
        assert_eq!(
            QueryExecutionResourceLimits {
                statements: MAX_QUERY_STATEMENTS + 1,
                ..QueryExecutionResourceLimits::default()
            }
            .validate()
            .map_err(|refusal| refusal.limit),
            Err("statements")
        );

        let default = QueryExecutionResourceLimits::default();
        let raised = QueryExecutionResourceLimits {
            items: 1_000_000,
            bytes: 1 << 30,
            ..default
        };
        assert_eq!(raised.validate(), Ok(raised));
        assert_eq!(raised.effective(), raised);
        assert_eq!(raised.get("items"), Some(1_000_000));
        assert_eq!(raised.remote().max_items, 1_000_000);
        assert_eq!(
            raised.constrained_by(default),
            default,
            "a raised terminal policy cannot widen its connection's ceiling"
        );
        for name in QueryExecutionResourceLimits::NAMES {
            assert!(default.get(name).is_some(), "{name}");
        }
        assert_eq!(default.get("unknown"), None);

        let independent = QueryExecutionResourceLimits::tightened(0, 1, 2, 3, 4, 5, 6, 0);
        assert_eq!(
            independent
                .direct(AnswerCancellation::default())
                .resource_dimensions(),
            (3, 4, 5, 6, 0)
        );
        assert_eq!(independent.remote().max_statements, 0);

        let widened = QueryExecutionResourceLimits::default();
        assert_eq!(
            widened.constrained_by(independent),
            independent,
            "a per-terminal default cannot widen any connection-time dimension"
        );

        let per_terminal = QueryExecutionResourceLimits::tightened(900, 2, 700, 4, 500, 6, 300, 2);
        let connection = QueryExecutionResourceLimits::tightened(100, 800, 3, 600, 5, 400, 7, 1);
        assert_eq!(
            per_terminal.constrained_by(connection),
            QueryExecutionResourceLimits::tightened(100, 2, 3, 4, 5, 6, 7, 1),
            "each field is intersected independently instead of inheriting one aggregate ceiling",
        );
    }

    #[test]
    fn each_transport_has_its_own_defaults() {
        let direct = QueryExecutionResourceLimits::default_for(AnswerTransport::Direct);
        assert_eq!(
            direct,
            QueryExecutionResourceLimits {
                timeout_milliseconds: 30_000,
                items: 100_000,
                bytes: 64 * 1024 * 1024,
                graph_nodes: 1_000_000,
                attribute_values: 4_000_000,
                collection_members: 1_000_000,
                role_players: 1_000_000,
                statements: 3,
            },
            "a direct session keeps the 2.2 in-process defaults"
        );
        assert_eq!(QueryExecutionResourceLimits::default(), direct);

        let proxy = QueryExecutionResourceLimits::default_for(AnswerTransport::Proxy);
        assert_eq!(
            proxy,
            QueryExecutionResourceLimits {
                timeout_milliseconds: 30_000,
                items: 65_536,
                bytes: 32 * 1024 * 1024,
                graph_nodes: 65_536,
                attribute_values: 65_536,
                collection_members: 65_536,
                role_players: 65_536,
                statements: 3,
            },
            "a proxy connection defaults to the wire ceilings"
        );
        assert_eq!(proxy.constrained_by(direct), proxy);
    }

    #[test]
    fn overrides_are_relative_to_the_wire_defaults() {
        let proxy = QueryExecutionResourceLimits::default_for(AnswerTransport::Proxy);
        assert!(
            proxy.overrides().is_empty(),
            "a default-configured server advertises nothing"
        );
        assert_eq!(
            QueryExecutionResourceLimits::from_overrides(AnswerTransport::Proxy, []),
            Ok(proxy)
        );
        assert_eq!(
            QueryExecutionResourceLimits::from_overrides(AnswerTransport::Direct, []),
            Ok(QueryExecutionResourceLimits::default())
        );

        let advertised =
            QueryExecutionResourceLimits::from_overrides(AnswerTransport::Proxy, [("items", 7)])
                .expect("known dimension");
        assert_eq!(
            advertised,
            QueryExecutionResourceLimits { items: 7, ..proxy }
        );
        assert_eq!(
            advertised.overrides(),
            BTreeMap::from([("items".to_owned(), 7)])
        );
        assert_eq!(
            QueryExecutionResourceLimits::from_overrides(
                AnswerTransport::Proxy,
                advertised
                    .overrides()
                    .iter()
                    .map(|(name, value)| (name.as_str(), *value)),
            ),
            Ok(advertised),
            "an advertisement round-trips"
        );
        assert_eq!(
            QueryExecutionResourceLimits::default()
                .overrides()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "attribute_values",
                "bytes",
                "collection_members",
                "graph_nodes",
                "items",
                "role_players",
            ]
        );
        assert!(matches!(
            QueryExecutionResourceLimits::from_overrides(AnswerTransport::Direct, [("made_up", 1)]),
            Err(QueryExecutionLimitsError::Unknown { .. })
        ));
        assert!(matches!(
            QueryExecutionResourceLimits::from_overrides(
                AnswerTransport::Proxy,
                [("statements", 4)]
            ),
            Err(QueryExecutionLimitsError::OutOfRange(_))
        ));
    }

    #[test]
    fn refusals_name_the_default_of_their_own_transport() {
        let direct = QueryExecutionResourceLimits::default_for(AnswerTransport::Direct);
        let proxy = QueryExecutionResourceLimits::default_for(AnswerTransport::Proxy);
        let code = "processed_item_limit";
        assert_eq!(
            direct.refusal_details(AnswerTransport::Direct, code),
            Some(("items", 100_000, LIMIT_SOURCE_DEFAULT))
        );
        assert_eq!(
            proxy.refusal_details(AnswerTransport::Proxy, code),
            Some(("items", 65_536, LIMIT_SOURCE_DEFAULT))
        );
        assert_eq!(
            proxy.refusal_details(AnswerTransport::Direct, code),
            Some(("items", 65_536, LIMIT_SOURCE_CONFIGURED)),
            "the proxy default is a configured value on a direct session"
        );
        assert_eq!(
            direct.refusal_details(AnswerTransport::Proxy, code),
            Some(("items", 100_000, LIMIT_SOURCE_CONFIGURED)),
            "the direct default is a configured value on a proxy connection"
        );
        assert_eq!(
            direct.refusal_details(AnswerTransport::Direct, "unrelated"),
            None
        );
    }
}
