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
        let items = self.items.checked_add(1).ok_or_else(|| {
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
    Ok(())
}
