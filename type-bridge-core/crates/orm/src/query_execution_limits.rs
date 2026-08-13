//! Binding-neutral generated-query execution and materialization limits.

use std::time::{Duration, Instant};

use type_bridge_contract::limits::{MAX_CANONICAL_COLLECTION_LEN, MAX_REMOTE_ENVELOPE_BYTES};
use type_bridge_contract::query_remote_v2::RemoteLimitsV2;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
    SdkQueryDiagnosticCategory, SdkQueryDiagnosticPathKind,
};

use crate::match_request::MatchExecutionLimits;
use crate::projected_query::ProjectedQueryMaterializationLimits;
use crate::session::backend::AnswerCancellation;

/// Canonical maximum generated-query execution duration in milliseconds.
pub const MAX_QUERY_TIMEOUT_MILLISECONDS: u64 = 30_000;
/// Canonical maximum provider/result item count.
pub const MAX_QUERY_ITEMS: u64 = MAX_CANONICAL_COLLECTION_LEN as u64;
/// Canonical maximum provider, reply, and projected payload bytes.
pub const MAX_QUERY_BYTES: u64 = MAX_REMOTE_ENVELOPE_BYTES as u64;
/// Canonical maximum hydrated/projected graph-node count.
pub const MAX_QUERY_GRAPH_NODES: u64 = MAX_CANONICAL_COLLECTION_LEN as u64;
/// Canonical maximum hydrated/projected attribute-value count.
pub const MAX_QUERY_ATTRIBUTE_VALUES: u64 = MAX_CANONICAL_COLLECTION_LEN as u64;
/// Canonical maximum collection members, including reduction cells.
pub const MAX_QUERY_COLLECTION_MEMBERS: u64 = MAX_CANONICAL_COLLECTION_LEN as u64;
/// Canonical maximum relation-role-player references.
pub const MAX_QUERY_ROLE_PLAYERS: u64 = MAX_CANONICAL_COLLECTION_LEN as u64;
/// Canonical maximum provider statements per terminal.
pub const MAX_QUERY_STATEMENTS: u32 = 3;

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
    /// Capture one absolute deadline from a timeout, clamped to the common
    /// generated-query hard ceiling.
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

/// One common tighten-only policy for generated queries in every binding.
///
/// The same fields apply to direct and remote execution. `items` bounds
/// provider evidence and public rows, `bytes` bounds provider/reply bytes and
/// projected payload bytes, `graph_nodes` includes top-level things and nested
/// role players, `attribute_values` includes hydrated and projected scalars,
/// and `collection_members` includes collected identities plus group keys and
/// reducer cells. `role_players` counts role-player references independently
/// of their graph nodes. `statements` is enforced before each provider call.
/// No field is accepted and ignored on either lane.
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
    /// Construct and clamp one explicit tighten-only policy.
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
            timeout_milliseconds: min_u64(timeout_milliseconds, MAX_QUERY_TIMEOUT_MILLISECONDS),
            items: min_u64(items, MAX_QUERY_ITEMS),
            bytes: min_u64(bytes, MAX_QUERY_BYTES),
            graph_nodes: min_u64(graph_nodes, MAX_QUERY_GRAPH_NODES),
            attribute_values: min_u64(attribute_values, MAX_QUERY_ATTRIBUTE_VALUES),
            collection_members: min_u64(collection_members, MAX_QUERY_COLLECTION_MEMBERS),
            role_players: min_u64(role_players, MAX_QUERY_ROLE_PLAYERS),
            statements: min_u32(statements, MAX_QUERY_STATEMENTS),
        }
    }

    /// Clamp a struct-literal policy to the same canonical hard ceilings as
    /// [`Self::tightened`].
    #[doc(hidden)]
    #[must_use]
    pub const fn effective(self) -> Self {
        Self::tightened(
            self.timeout_milliseconds,
            self.items,
            self.bytes,
            self.graph_nodes,
            self.attribute_values,
            self.collection_members,
            self.role_players,
            self.statements,
        )
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
        Self::tightened(
            min_u64(this.timeout_milliseconds, ceiling.timeout_milliseconds),
            min_u64(this.items, ceiling.items),
            min_u64(this.bytes, ceiling.bytes),
            min_u64(this.graph_nodes, ceiling.graph_nodes),
            min_u64(this.attribute_values, ceiling.attribute_values),
            min_u64(this.collection_members, ceiling.collection_members),
            min_u64(this.role_players, ceiling.role_players),
            min_u32(this.statements, ceiling.statements),
        )
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
        let this = self.effective();
        MatchExecutionLimits::tightened(
            this.items,
            this.bytes,
            Duration::from_millis(this.timeout_milliseconds),
            cancellation,
        )
        .with_absolute_deadline(deadline)
        .with_max_hydrated_things(this.graph_nodes)
        .with_max_attribute_values(this.attribute_values)
        .with_max_collected_concepts(this.collection_members)
        .with_max_role_players(this.role_players)
        .with_max_statements(u8::try_from(this.statements).unwrap_or(u8::MAX))
    }

    /// Derive all-or-nothing projected result limits.
    #[doc(hidden)]
    #[must_use]
    pub fn projected(self) -> ProjectedQueryMaterializationLimits {
        let this = self.effective();
        ProjectedQueryMaterializationLimits::tightened(
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
    fn default() -> Self {
        Self::tightened(
            MAX_QUERY_TIMEOUT_MILLISECONDS,
            MAX_QUERY_ITEMS,
            MAX_QUERY_BYTES,
            MAX_QUERY_GRAPH_NODES,
            MAX_QUERY_ATTRIBUTE_VALUES,
            MAX_QUERY_COLLECTION_MEMBERS,
            MAX_QUERY_ROLE_PLAYERS,
            MAX_QUERY_STATEMENTS,
        )
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
    fn common_policy_clamps_each_dimension_independently_at_exact_maximum() {
        let plus_one = QueryExecutionResourceLimits::tightened(
            MAX_QUERY_TIMEOUT_MILLISECONDS + 1,
            MAX_QUERY_ITEMS + 1,
            MAX_QUERY_BYTES + 1,
            MAX_QUERY_GRAPH_NODES + 1,
            MAX_QUERY_ATTRIBUTE_VALUES + 1,
            MAX_QUERY_COLLECTION_MEMBERS + 1,
            MAX_QUERY_ROLE_PLAYERS + 1,
            MAX_QUERY_STATEMENTS + 1,
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
        assert_eq!(literal.effective(), QueryExecutionResourceLimits::default());
        assert_eq!(
            literal.remote(),
            QueryExecutionResourceLimits::default().remote()
        );

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
}
