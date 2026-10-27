//! One per-item byte and row budget for provider and semantic answer readers.
use crate::error::OrmError;
use crate::match_request::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use crate::session::backend::{AnswerItem, BoundedAnswerLimits};

pub(crate) struct AnswerMeter {
    items: u64,
    bytes: u64,
    max_items: u64,
    max_bytes: u64,
}

impl AnswerMeter {
    pub(crate) fn new(limits: &BoundedAnswerLimits) -> Self {
        Self {
            items: 0,
            bytes: 0,
            max_items: limits.max_items,
            max_bytes: limits.max_bytes,
        }
    }

    pub(crate) fn accept(&mut self, item: &AnswerItem) -> Result<(), OrmError> {
        let value = match item {
            AnswerItem::Row(value) | AnswerItem::Document(value) => value,
        };
        self.accept_value(value)
    }

    pub(crate) fn accept_value(&mut self, value: &serde_json::Value) -> Result<(), OrmError> {
        let nested = value.as_object().map_or(0, |fields| {
            fields
                .iter()
                .filter(|(key, _)| key.starts_with("_role_") && key.ends_with("_players"))
                .filter_map(|(_, value)| value.as_array())
                .map(|values| values.len() as u64)
                .sum::<u64>()
        });
        let items = self
            .items
            .checked_add(1_u64.saturating_add(nested))
            .ok_or_else(|| {
                limit(
                    "processed_item_counter_overflow",
                    "processed provider item counter overflowed",
                    u64::MAX,
                    self.max_items,
                )
            })?;
        if items > self.max_items {
            return Err(limit(
                "processed_item_limit",
                "provider answer exceeded the processed-item ceiling",
                items,
                self.max_items,
            ));
        }
        let bytes = self
            .bytes
            .checked_add(
                u64::try_from(serde_json::to_vec(value)?.len()).map_err(|_| {
                    limit(
                        "answer_byte_counter_overflow",
                        "encoded provider answer length exceeds the counter range",
                        u64::MAX,
                        self.max_bytes,
                    )
                })?,
            )
            .ok_or_else(|| {
                limit(
                    "answer_byte_counter_overflow",
                    "provider answer byte counter overflowed",
                    u64::MAX,
                    self.max_bytes,
                )
            })?;
        if bytes > self.max_bytes {
            return Err(limit(
                "response_byte_limit",
                "provider answer exceeded the response-byte ceiling",
                bytes,
                self.max_bytes,
            ));
        }
        self.items = items;
        self.bytes = bytes;
        Ok(())
    }

    pub(crate) fn items(&self) -> u64 {
        self.items
    }
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }
}

fn limit(code: &'static str, message: &'static str, actual: u64, maximum: u64) -> OrmError {
    MatchError::new(MatchErrorCategory::ResourceLimit, code, message)
        .at(MatchErrorPathSegment::ProviderEvidence)
        .with_detail("actual", actual)
        .with_detail("maximum", maximum)
        .into()
}

/// Repeat semantic metering after the provider's independent raw-stream bounds.
pub(crate) fn meter_result(
    result: &crate::session::backend::QueryResult,
    limits: &BoundedAnswerLimits,
) -> Result<(), OrmError> {
    use crate::session::backend::QueryResult;
    let items = match result {
        QueryResult::Rows(items) | QueryResult::Documents(items) => items,
        QueryResult::Ok => return Ok(()),
    };
    let mut meter = AnswerMeter::new(limits);
    for item in items {
        meter.accept_value(item)?;
    }
    tracing::debug!(target: "type_bridge.plan", items = meter.items(), bytes = meter.bytes(), "type_bridge.plan.answer");
    Ok(())
}

/// Semantic dimensions of thing-fetch documents, shared by direct and staged plans.
pub(crate) struct PlanMeter {
    totals: [u64; 4],
    limits: [u64; 4],
}

impl PlanMeter {
    pub(crate) fn new(limits: &crate::session::backend::AnswerLimits) -> Self {
        Self {
            totals: [0; 4],
            limits: [
                limits.max_graph_nodes,
                limits.max_attribute_values,
                limits.max_collection_members,
                limits.max_role_players,
            ],
        }
    }

    pub(crate) fn accept(
        &mut self,
        answer: &type_bridge_contract::query_remote_v3::StagedAnswerV3,
    ) -> Result<(), OrmError> {
        let codes = [
            "hydrated_thing_limit",
            "hydrated_attribute_value_limit",
            "collected_concept_limit",
            "hydrated_role_player_limit",
        ];
        for (index, count) in answer.resource_counts().into_iter().enumerate() {
            self.totals[index] = self.totals[index].saturating_add(count);
            if self.totals[index] > self.limits[index] {
                return Err(limit(
                    codes[index],
                    "plan answer exceeded its semantic resource ceiling",
                    self.totals[index],
                    self.limits[index],
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::session::backend::AnswerLimits;
    use serde_json::json;
    use type_bridge_contract::query_remote_v3::StagedAnswerV3;

    #[test]
    fn semantic_dimensions_are_aggregate_and_zero_means_no_values() {
        let answer: StagedAnswerV3 = serde_json::from_value(json!({"kind":"documents", "documents":[{
            "_iid":{"kind":"scalar", "value":{"kind":"string", "value":"0x01"}},
            "attributes":{"kind":"document", "fields":{"name":{"kind":"list", "values":[{"kind":"string", "value":"Ada"}, {"kind":"string", "value":"Grace"}]}}},
            "_role_0_iid":{"kind":"scalar", "value":{"kind":"string", "value":"0x02"}},
            "_role_0_attributes":{"kind":"document", "fields":{"name":{"kind":"scalar", "value":{"kind":"string", "value":"Lin"}}}}
        }]})).unwrap();
        for index in 0..4 {
            let mut meter = PlanMeter::new(&AnswerLimits::default());
            meter.limits[index] = 0;
            assert!(meter.accept(&answer).is_err());
        }
        let mut meter = PlanMeter::new(&AnswerLimits::default());
        meter.limits = [2, 3, 4, 1];
        meter.accept(&answer).unwrap();
        assert_eq!(meter.totals, [2, 3, 4, 1]);
        assert!(meter.accept(&answer).is_err());
    }
}
