//! Resource budgets for generated model execution and hydrated replies.

use crate::limits::{
    CodecLimits, MAX_CANONICAL_COLLECTION_LEN, MAX_REMOTE_ENVELOPE_BYTES,
    REMOTE_ENVELOPE_CODEC_LIMITS,
};
use serde::{Deserialize, Serialize};

/// Default statement ceiling when a model request omits the wire member.
pub const DEFAULT_MODEL_MAX_STATEMENTS: u32 = 3;

/// Caller-owned transport and hydration budgets for one model request.
///
/// This is the wire form of an answer policy; generated sessions derive it
/// from their `QueryExecutionResourceLimits`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteModelLimits {
    /// Optional execution deadline in milliseconds.
    pub deadline_ms: Option<u64>,
    /// Maximum authenticated successful reply bytes.
    pub max_bytes: u64,
    /// Maximum rows, page entries, or scalar evidence.
    pub max_items: u64,
    /// Maximum aggregate output collection members.
    pub max_collection_members: u64,
    /// Maximum hydration graph nodes.
    pub max_graph_nodes: u64,
    /// Maximum aggregate hydration attribute values.
    pub max_attribute_values: u64,
    /// Maximum aggregate hydration role-player references.
    pub max_role_players: u64,
    /// Maximum provider statements for this terminal.
    ///
    /// The value three is omitted on encode and reconstructed on decode so
    /// request bytes emitted before this additive member remain canonical.
    #[serde(
        default = "default_model_max_statements",
        skip_serializing_if = "is_default_model_max_statements"
    )]
    pub max_statements: u32,
}

const fn default_model_max_statements() -> u32 {
    DEFAULT_MODEL_MAX_STATEMENTS
}

fn is_default_model_max_statements(value: &u32) -> bool {
    *value == DEFAULT_MODEL_MAX_STATEMENTS
}

/// Caller budgets checked before model reply evidence is allocated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelReplyDecodeLimits {
    /// Maximum authenticated successful outer-reply bytes.
    pub max_bytes: u64,
    /// Maximum rows, page entries, count value, or positive existence item.
    pub max_items: u64,
    /// Maximum aggregate output collection members.
    pub max_collection_members: u64,
    /// Maximum hydration graph nodes.
    pub max_graph_nodes: u64,
    /// Maximum canonical values across hydration attribute evidence.
    pub max_attribute_values: u64,
    /// Maximum references across hydration role-player evidence.
    pub max_role_players: u64,
}

impl ModelReplyDecodeLimits {
    /// The budgets a reply is bound to when its peers configured none: the
    /// default envelope and hydration ceilings, with scalar evidence (a
    /// count) left to the executor's own item budget.
    pub const DEFAULT_CEILING: Self = Self {
        max_bytes: MAX_REMOTE_ENVELOPE_BYTES as u64,
        max_items: u64::MAX,
        max_collection_members: MAX_CANONICAL_COLLECTION_LEN as u64,
        max_graph_nodes: MAX_CANONICAL_COLLECTION_LEN as u64,
        max_attribute_values: MAX_CANONICAL_COLLECTION_LEN as u64,
        max_role_players: MAX_CANONICAL_COLLECTION_LEN as u64,
    };

    /// The codec limits a reply under these budgets is encoded and decoded
    /// with.
    ///
    /// They are the default envelope limits, widened where a budget asks for
    /// more. A budget tighter than the default leaves the codec at the
    /// default; the budget checks enforce it.
    #[must_use]
    pub fn codec(self) -> CodecLimits {
        let widest = self
            .max_items
            .max(self.max_collection_members)
            .max(self.max_graph_nodes)
            .max(self.max_attribute_values)
            .max(self.max_role_players);
        CodecLimits {
            max_bytes: self.envelope_bytes(),
            max_collection_len: REMOTE_ENVELOPE_CODEC_LIMITS
                .max_collection_len
                .max(usize::try_from(widest).unwrap_or(usize::MAX)),
            ..REMOTE_ENVELOPE_CODEC_LIMITS
        }
    }

    /// The largest signed reply envelope these budgets admit: the default
    /// envelope ceiling, or more when the byte budget was raised past it.
    #[must_use]
    pub fn envelope_bytes(self) -> usize {
        MAX_REMOTE_ENVELOPE_BYTES.max(usize::try_from(self.max_bytes).unwrap_or(usize::MAX))
    }
}
