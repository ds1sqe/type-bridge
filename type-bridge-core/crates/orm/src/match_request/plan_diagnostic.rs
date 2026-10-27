//! Stable released diagnostics for native typed model execution.
use super::result_validation::exactly_one_cardinality_error;
use super::{
    DescriptorId, FieldId, MatchError, MatchErrorCategory, MatchErrorPath, MatchErrorPathSegment,
};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticDetailValue};

/// Preserve released match errors across native plan and shared hydration diagnostics.
///
/// Direct execution and authenticated remote execution must both cross this
/// compatibility map before a language binding observes the failure.
pub(crate) fn released_plan_execution_error(diagnostic: &Diagnostic) -> MatchError {
    if diagnostic.code().as_str() == "query_v2_model_exactly_one" {
        let actual = diagnostic
            .details()
            .get("actual")
            .and_then(|value| match value {
                DiagnosticDetailValue::Long(value) => usize::try_from(*value).ok(),
                DiagnosticDetailValue::Text(_)
                | DiagnosticDetailValue::Boolean(_)
                | DiagnosticDetailValue::TextList(_) => None,
            })
            .unwrap_or(usize::MAX);
        return exactly_one_cardinality_error(actual).unwrap_or_else(|| {
            MatchError::new(
                MatchErrorCategory::ResultDecode,
                "result_operation_mismatch",
                "adapted exactly-one result lost its cardinality proof",
            )
            .at(MatchErrorPathSegment::Result)
        });
    }

    let (category, code, mapped_message) = released_execution_diagnostic(diagnostic);
    let path = released_execution_path(diagnostic);
    let message = match (category, code, path.segments()) {
        (MatchErrorCategory::ResultDecode, _, [MatchErrorPathSegment::ProviderEvidence]) => {
            "provider evidence failed canonical typed match decoding"
        }
        (
            MatchErrorCategory::ResourceLimit,
            "processed_item_counter_overflow"
            | "processed_item_limit"
            | "answer_byte_counter_overflow"
            | "response_byte_limit",
            _,
        ) => "provider resource limits prevented complete typed match evidence",
        _ => mapped_message,
    };
    let mut error = MatchError::new(category, code, message).with_path(path);
    // Keep what a refusal says about its limit.
    for key in ["limit", "maximum", "source"] {
        error = match diagnostic.details().get(key) {
            Some(DiagnosticDetailValue::Text(value)) => error.with_detail(key, value.clone()),
            Some(DiagnosticDetailValue::Long(value)) => match u64::try_from(*value) {
                Ok(value) => error.with_detail(key, value),
                Err(_) => error,
            },
            _ => error,
        };
    }
    error
}

fn released_execution_path(diagnostic: &Diagnostic) -> MatchErrorPath {
    let segments = match diagnostic.code().as_str() {
        "query_v2_model_regex" => vec![MatchErrorPathSegment::Predicate],
        "query_v2_model_rows_collection" => vec![MatchErrorPathSegment::Operation],
        "query_plan_result_stack"
        | "query_plan_result_state"
        | "query_plan_result_operation"
        | "query_v2_model_contract_missing"
        | "query_v2_model_provider_plan_missing"
        | "query_v2_model_provider_plan_mismatch"
        | "query_v2_model_tuple_plan"
        | "query_v2_model_page_total_plan" => vec![MatchErrorPathSegment::Result],
        "query_v2_model_predicate_equal_type"
        | "query_v2_model_predicate_order_type"
        | "query_v2_model_predicate_string_type" => vec![MatchErrorPathSegment::Result],
        "query_v2_model_order_non_scalar"
        | "query_v2_model_order_missing"
        | "query_v2_model_order_value_type" => {
            let owner = diagnostic.details().get("field_owner").and_then(|value| {
                if let DiagnosticDetailValue::Text(value) = value {
                    Some(value.clone())
                } else {
                    None
                }
            });
            let name = diagnostic.details().get("field_name").and_then(|value| {
                if let DiagnosticDetailValue::Text(value) = value {
                    Some(value.clone())
                } else {
                    None
                }
            });
            match (owner, name) {
                (Some(owner), Some(name)) => vec![
                    MatchErrorPathSegment::Result,
                    MatchErrorPathSegment::Field(FieldId::new(DescriptorId::new(owner), name)),
                ],
                _ => vec![MatchErrorPathSegment::Result],
            }
        }
        _ => vec![MatchErrorPathSegment::ProviderEvidence],
    };
    MatchErrorPath::from_segments(segments)
}

pub(super) fn released_execution_diagnostic(
    diagnostic: &Diagnostic,
) -> (MatchErrorCategory, &'static str, &'static str) {
    match diagnostic.code().as_str() {
        "query_plan_hydration_ordered_role" => (
            MatchErrorCategory::ResultDecode,
            "query_plan_hydration_ordered_role",
            "populated ordered roles require provider fetch ordinals",
        ),
        "hydrated_thing_limit" => (
            MatchErrorCategory::ResourceLimit,
            "hydrated_thing_limit",
            "plan answer exceeded its semantic resource ceiling",
        ),
        "hydrated_attribute_value_limit" => (
            MatchErrorCategory::ResourceLimit,
            "hydrated_attribute_value_limit",
            "plan answer exceeded its semantic resource ceiling",
        ),
        "collected_concept_limit" => (
            MatchErrorCategory::ResourceLimit,
            "collected_concept_limit",
            "plan answer exceeded its semantic resource ceiling",
        ),
        "hydrated_role_player_limit" => (
            MatchErrorCategory::ResourceLimit,
            "hydrated_role_player_limit",
            "plan answer exceeded its semantic resource ceiling",
        ),
        "reduction_overflow" => (
            MatchErrorCategory::ResourceLimit,
            "reduction_overflow",
            "integer sum left the canonical long domain",
        ),
        "reduction_input_bound" => (
            MatchErrorCategory::ResourceLimit,
            "reduction_input_bound",
            "integer sum exceeds its exact native input bound",
        ),
        "statement_count_limit" => (
            MatchErrorCategory::ResourceLimit,
            "statement_count_limit",
            "match execution exceeded its statement ceiling",
        ),
        "provider_cancelled" => (
            MatchErrorCategory::Cancelled,
            "provider_cancelled",
            "provider answer processing was cancelled",
        ),
        "transaction_deadline_exceeded" => (
            MatchErrorCategory::ResourceLimit,
            "transaction_deadline_exceeded",
            "provider transaction deadline expired",
        ),
        "processed_item_counter_overflow" => (
            MatchErrorCategory::ResourceLimit,
            "processed_item_counter_overflow",
            "processed provider item counter overflowed",
        ),
        "processed_item_limit" => (
            MatchErrorCategory::ResourceLimit,
            "processed_item_limit",
            "provider answer exceeded the processed-item ceiling",
        ),
        "answer_byte_counter_overflow" => (
            MatchErrorCategory::ResourceLimit,
            "answer_byte_counter_overflow",
            "provider answer byte counter overflowed",
        ),
        "response_byte_limit" => (
            MatchErrorCategory::ResourceLimit,
            "response_byte_limit",
            "provider answer exceeded the response-byte ceiling",
        ),
        "query_v2_model_item_limit" => (
            MatchErrorCategory::ResourceLimit,
            "processed_item_limit",
            "provider answer exceeded the processed-item ceiling",
        ),
        "query_v2_model_byte_limit" => (
            MatchErrorCategory::ResourceLimit,
            "response_byte_limit",
            "provider answer exceeded the response-byte ceiling",
        ),
        "query_v2_model_collection_limit" => (
            MatchErrorCategory::ResourceLimit,
            "collected_concept_limit",
            "provider result exceeded the collected-concept ceiling",
        ),
        "query_v2_model_graph_limit" | "query_v2_model_hydration_limit" => (
            MatchErrorCategory::ResourceLimit,
            "hydrated_thing_limit",
            "provider result exceeded the hydrated-thing ceiling",
        ),
        "query_v2_model_attribute_limit" => (
            MatchErrorCategory::ResourceLimit,
            "hydrated_attribute_value_limit",
            "provider result exceeded the hydrated attribute-value ceiling",
        ),
        "query_v2_model_role_player_limit" => (
            MatchErrorCategory::ResourceLimit,
            "hydrated_role_player_limit",
            "provider result exceeded the hydrated role-player ceiling",
        ),
        "query_v2_model_solution_limit" => (
            MatchErrorCategory::ResourceLimit,
            "solution_scan_limit",
            "provider solution ceiling was reached before result completeness was proven",
        ),
        "query_v2_model_complete_rows_exceeded" => (
            MatchErrorCategory::ResourceLimit,
            "row_window_exceeded",
            "provider returned more distinct rows than the validated window permits",
        ),
        "query_v2_model_count_overflow" => (
            MatchErrorCategory::ResourceLimit,
            "processed_item_counter_overflow",
            "processed provider item counter overflowed",
        ),
        "query_plan_result_stack"
        | "query_plan_result_state"
        | "query_plan_result_operation"
        | "query_v2_model_contract_missing"
        | "query_v2_model_provider_plan_missing"
        | "query_v2_model_provider_plan_mismatch"
        | "query_v2_model_tuple_plan" => (
            MatchErrorCategory::ResultDecode,
            "result_operation_mismatch",
            "adapted V2 result variant does not match the validated released operation",
        ),
        "query_v2_model_rows_collection" => (
            MatchErrorCategory::InvalidPlan,
            "unsupported_collection_slot",
            "selected-row execution does not support collected output slots",
        ),
        "query_v2_model_page_total_plan" => (
            MatchErrorCategory::ResultDecode,
            "page_operation_mismatch",
            "typed page plan does not belong to a PageBy operation",
        ),
        "query_v2_model_provider_not_exhausted" => (
            MatchErrorCategory::ResultDecode,
            "provider_stream_not_exhausted",
            "provider stopped before the bounded typed statement reached its terminal frame",
        ),
        "query_v2_model_solution_kind" => (
            MatchErrorCategory::ResultDecode,
            "solution_answer_kind",
            "selected solution scan returned a document instead of a row",
        ),
        "query_plan_result_rows"
        | "query_remote_v3_answer_kind"
        | "query_remote_v3_answer_concept"
        | "query_v2_model_solution_malformed"
        | "query_v2_model_predicate_evidence"
        | "query_v2_model_role_edge_evidence" => (
            MatchErrorCategory::ResultDecode,
            "malformed_solution_row",
            "provider solution row does not match the typed evidence shape",
        ),
        "query_v2_model_solution_binding" => (
            MatchErrorCategory::ResultDecode,
            "missing_provider_binding",
            "provider solution omitted a required positive binding",
        ),
        "query_v2_model_solution_unknown_binding" => (
            MatchErrorCategory::ResultDecode,
            "unknown_provider_binding",
            "provider solution contains an unknown binding",
        ),
        "query_v2_model_solution_duplicate_binding" => (
            MatchErrorCategory::ResultDecode,
            "duplicate_provider_binding",
            "provider solution assigns one binding more than once",
        ),
        "query_v2_model_solution_malformed_iid" => (
            MatchErrorCategory::ResultDecode,
            "malformed_provider_concept_id",
            "provider solution contains a malformed provider IID",
        ),
        "query_v2_model_unstable_provider_order" => (
            MatchErrorCategory::ResultDecode,
            "unstable_provider_order",
            "provider solutions violate the validator-derived stable order",
        ),
        "query_v2_model_order_non_scalar" => (
            MatchErrorCategory::ResultDecode,
            "non_scalar_order_evidence",
            "provider returned multiple values for a validated scalar order field",
        ),
        "query_v2_model_order_missing" => (
            MatchErrorCategory::ResultDecode,
            "missing_order_value",
            "provider omitted a value required by stable ordering",
        ),
        "query_v2_model_order_value_type" => (
            MatchErrorCategory::ResultDecode,
            "order_value_type_mismatch",
            "provider order values are not mutually comparable",
        ),
        "query_v2_model_predicate_equal_type" => (
            MatchErrorCategory::ResultDecode,
            "predicate_value_type_mismatch",
            "provider predicate values do not have the same validated type",
        ),
        "query_v2_model_predicate_order_type" => (
            MatchErrorCategory::ResultDecode,
            "predicate_value_type_mismatch",
            "provider predicate values are not order-compatible",
        ),
        "query_v2_model_predicate_string_type" => (
            MatchErrorCategory::ResultDecode,
            "predicate_value_type_mismatch",
            "provider predicate values do not support the validated string operator",
        ),
        "query_v2_model_regex" => (
            MatchErrorCategory::InvalidPlan,
            "invalid_regex_pattern",
            "validated request contains an invalid regular expression",
        ),
        "query_v2_model_tuple_evidence" => (
            MatchErrorCategory::ResultDecode,
            "malformed_tuple_row",
            "provider tuple proof row does not match the typed evidence shape",
        ),
        "query_v2_model_root_kind" => (
            MatchErrorCategory::ResultDecode,
            "root_answer_kind",
            "distinct-root scan returned a document instead of a row",
        ),
        "query_v2_model_root_binding" => (
            MatchErrorCategory::ResultDecode,
            "root_binding_mismatch",
            "distinct-root row contains the wrong binding",
        ),
        "query_plan_result_count"
        | "query_plan_result_exists"
        | "query_v2_model_root_malformed" => (
            MatchErrorCategory::ResultDecode,
            "malformed_root_row",
            "distinct-root row does not match the typed evidence shape",
        ),
        "query_v2_model_page_kind" => (
            MatchErrorCategory::ResultDecode,
            "page_rematch_answer_kind",
            "page re-match returned a row instead of a hydrated document",
        ),
        "query_v2_model_page_document" => (
            MatchErrorCategory::ResultDecode,
            "malformed_page_rematch_document",
            "page re-match document does not match the typed evidence shape",
        ),
        "query_v2_model_page_binding" => (
            MatchErrorCategory::ResultDecode,
            "malformed_page_rematch_binding",
            "page re-match omitted a required positive binding",
        ),
        "query_plan_result_window" | "query_v2_model_page_root_set" => (
            MatchErrorCategory::ResultDecode,
            "selected_root_set_mismatch",
            "page re-match root set does not exactly equal root selection",
        ),
        "query_v2_model_page_unexpected_root" => (
            MatchErrorCategory::ResultDecode,
            "unexpected_hydrated_root",
            "page re-match returned a root outside the selected root set",
        ),
        "query_v2_model_page_total_length" => (
            MatchErrorCategory::ResultDecode,
            "page_total_length_mismatch",
            "provider page length is inconsistent with its same-snapshot total and window",
        ),
        "query_v2_model_hydration_missing" => (
            MatchErrorCategory::ResultDecode,
            "missing_hydrated_concept",
            "batched hydration omitted a requested binding/IID pair",
        ),
        "query_v2_model_hydration_unexpected" => (
            MatchErrorCategory::ResultDecode,
            "unexpected_hydrated_concept",
            "batched hydration returned an unrequested binding/IID pair",
        ),
        "query_v2_model_hydration_value_type" | "query_v2_model_hydration_value_order" => (
            MatchErrorCategory::ResultDecode,
            "hydrated_attribute_value_type",
            "TypeDB wildcard attribute value has the wrong value type",
        ),
        "query_v2_model_hydration_attributes" => (
            MatchErrorCategory::ResultDecode,
            "malformed_hydrated_attributes",
            "TypeDB wildcard hydration attributes are malformed",
        ),
        "query_plan_hydration_roles" | "query_v2_model_hydration_roles" => (
            MatchErrorCategory::ResultDecode,
            "malformed_hydrated_roles",
            "TypeDB nested role hydration is malformed",
        ),
        "query_v2_model_hydration_role_player" => (
            MatchErrorCategory::ResultDecode,
            "malformed_hydrated_role_player",
            "TypeDB nested hydration role player is malformed",
        ),
        "query_plan_result_documents"
        | "query_plan_result_type"
        | "query_plan_hydration_document"
        | "query_plan_hydration_fetch"
        | "query_plan_hydration_iid"
        | "query_plan_hydration_type"
        | "query_v2_model_hydration_document"
        | "query_v2_model_hydration_binding"
        | "query_v2_model_hydration_iid"
        | "query_v2_model_hydration_descriptor"
        | "query_v2_model_hydration_kind" => (
            MatchErrorCategory::ResultDecode,
            "malformed_hydration_document",
            "hydration document does not match the typed evidence shape",
        ),
        "query_plan_hydration_conflict" | "query_v2_model_hydration_conflict" => (
            MatchErrorCategory::ResultDecode,
            "duplicate_hydrated_concept",
            "batched hydration returned contradictory evidence for one provider concept",
        ),
        "query_v2_model_hydration_authority" | "query_v2_model_hydration_state" => (
            MatchErrorCategory::ResultDecode,
            "malformed_hydration_document",
            "hydration document does not match the typed evidence shape",
        ),
        "query_plan_result_reduction"
        | "query_v2_model_reduction_descriptor"
        | "query_v2_model_reduction_group" => (
            MatchErrorCategory::ResultDecode,
            "malformed_reduction_row",
            "provider reduction evidence references an unexpected concrete descriptor",
        ),
        _ if diagnostic.code().as_str().starts_with("query_v2_model_") => (
            MatchErrorCategory::ResultDecode,
            "unmapped_model_execution_diagnostic",
            "model execution emitted a diagnostic without a released compatibility mapping",
        ),
        _ if diagnostic.category() == DiagnosticCategory::ResourceLimit => (
            MatchErrorCategory::ResourceLimit,
            "processed_item_limit",
            "provider answer exceeded the processed-item ceiling",
        ),
        _ => (
            MatchErrorCategory::ResultDecode,
            "malformed_solution_row",
            "provider evidence does not match the validated request",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_manifest_covers_every_native_model_result_code() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../tests/contracts/typed_query/diagnostic-migration.json"
        ))
        .unwrap();
        let rows = manifest["match_codes"]
            .as_array()
            .unwrap()
            .iter()
            .chain(manifest["new_codes"].as_array().unwrap().iter())
            .collect::<Vec<_>>();
        let source = concat!(
            include_str!("plan_result.rs"),
            include_str!("plan_fetch_stack.rs"),
            include_str!("plan_hydration.rs")
        );
        for code in source.split('"').filter(|value| {
            value.starts_with("query_plan_result_")
                || value.starts_with("query_plan_hydration_")
                || value == &"reduction_overflow"
                || value == &"reduction_input_bound"
        }) {
            let entry = rows
                .iter()
                .find(|entry| {
                    entry["core_codes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|value| value.as_str() == Some(code))
                })
                .unwrap_or_else(|| panic!("missing diagnostic migration for {code}"));
            let diagnostic =
                crate::query_v2::failure(DiagnosticCategory::Integrity, code, "mapping probe");
            let (category, released, _) = released_execution_diagnostic(&diagnostic);
            assert_eq!(entry["new_code"], released);
            assert_eq!(entry["category"], category.as_str());
        }
        for entry in &rows {
            assert_eq!(
                entry["old_code"], entry["new_code"],
                "released match codes remain stable"
            );
        }
        for entry in manifest["transport_codes"].as_array().unwrap() {
            if let Some(reason) = entry.get("reason").and_then(serde_json::Value::as_str) {
                assert_eq!(entry["new_code"], "query_remote_binding_mismatch");
                assert_eq!(
                    reason,
                    if entry["old_code"] == "query_remote_v2_nonce_mismatch" {
                        "nonce"
                    } else {
                        "request"
                    }
                );
                continue;
            }
            assert_eq!(
                entry["new_code"],
                entry["old_code"]
                    .as_str()
                    .unwrap()
                    .replace("query_remote_v2_", "query_remote_")
            );
        }
    }
}
