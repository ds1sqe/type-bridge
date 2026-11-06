#include <typebridge/type_bridge.h>

void raw_function_types(void) {
    type_bridge_status_t (*database_open)(const type_bridge_database_t *, uint32_t,
        type_bridge_raw_transaction_t **, type_bridge_diagnostics_t **) = type_bridge_database_raw_transaction;
    type_bridge_status_t (*proxy_open)(const type_bridge_proxy_connection_t *, uint32_t,
        type_bridge_raw_transaction_t **, type_bridge_diagnostics_t **) = type_bridge_proxy_connection_raw_transaction;
    type_bridge_status_t (*database_execute)(const type_bridge_database_t *, type_bridge_byte_view_t, uint32_t,
        type_bridge_raw_result_t **, type_bridge_diagnostics_t **) = type_bridge_database_execute_query;
    type_bridge_status_t (*proxy_execute)(const type_bridge_proxy_connection_t *, type_bridge_byte_view_t, uint32_t,
        type_bridge_raw_result_t **, type_bridge_diagnostics_t **) = type_bridge_proxy_connection_execute_query;
    type_bridge_status_t (*execute)(type_bridge_raw_transaction_t *, type_bridge_byte_view_t,
        type_bridge_raw_result_t **, type_bridge_diagnostics_t **) = type_bridge_raw_transaction_execute_query;
    type_bridge_status_t (*commit)(type_bridge_raw_transaction_t *, type_bridge_diagnostics_t **) = type_bridge_raw_transaction_commit;
    type_bridge_status_t (*rollback)(type_bridge_raw_transaction_t *, type_bridge_diagnostics_t **) = type_bridge_raw_transaction_rollback;
    type_bridge_status_t (*close_transaction)(type_bridge_raw_transaction_t **, type_bridge_diagnostics_t **) = type_bridge_raw_transaction_close;
    type_bridge_status_t (*json)(const type_bridge_raw_result_t *, type_bridge_byte_view_t *) = type_bridge_raw_result_json;
    type_bridge_status_t (*close_result)(type_bridge_raw_result_t **) = type_bridge_raw_result_close;
    (void)database_open; (void)proxy_open; (void)database_execute; (void)proxy_execute;
    (void)execute; (void)commit; (void)rollback; (void)close_transaction; (void)json; (void)close_result;
}

void iid_function_types(void) {
    type_bridge_status_t (*open)(type_bridge_byte_view_t, type_bridge_iid_t **) = type_bridge_iid_open;
    type_bridge_status_t (*text)(const type_bridge_iid_t *, type_bridge_byte_view_t *) = type_bridge_iid_text;
    type_bridge_status_t (*close)(type_bridge_iid_t **) = type_bridge_iid_close;
    (void)open; (void)text; (void)close;
}

void iid_query_function_types(void) {
    type_bridge_status_t (*single)(const type_bridge_query_binding_t *,
        const type_bridge_projected_token_v1_t *, uint32_t, const type_bridge_iid_t *,
        type_bridge_query_predicate_t **, type_bridge_execution_diagnostics_t **) = type_bridge_query_binding_iid;
    type_bridge_status_t (*list)(const type_bridge_query_binding_t *,
        const type_bridge_projected_token_v1_t *, uint32_t, const type_bridge_iid_t *const *, size_t,
        type_bridge_query_predicate_t **, type_bridge_execution_diagnostics_t **) = type_bridge_query_binding_iid_in;
    (void)single; (void)list;
}
