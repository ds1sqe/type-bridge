#define _POSIX_C_SOURCE 200809L
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <typebridge/type_bridge.h>
#include <fixture/models.h>

#define CHECK(c) do { if (!(c)) { fprintf(stderr, "deadline check failed: %d\n", __LINE__); return 1; } } while (0)
static type_bridge_byte_view_t view(const char *s) {
    type_bridge_byte_view_t v = {(const uint8_t *)s, strlen(s)}; return v;
}
static int same(type_bridge_byte_view_t v, const char *s) {
    return v.length == strlen(s) && memcmp(v.data, s, v.length) == 0;
}
static double now(void) {
    struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (double)t.tv_sec + (double)t.tv_nsec / 1e9;
}
static uint8_t *read_bytes(const char *file, size_t *length) {
    FILE *f = fopen(file, "rb"); if (!f) return NULL;
    fseek(f, 0, SEEK_END); long n = ftell(f); rewind(f);
    uint8_t *bytes = malloc((size_t)n);
    if (bytes == NULL || fread(bytes, 1, (size_t)n, f) != (size_t)n) { fclose(f); free(bytes); return NULL; }
    fclose(f); *length = (size_t)n; return bytes;
}
int main(int argc, char **argv) {
    CHECK(argc == 8);
    const int proxy_mode = strcmp(argv[1], "proxy") == 0;
    type_bridge_schema_package_t *package = NULL;
    type_bridge_diagnostics_t *package_errors = NULL;
    type_bridge_execution_diagnostics_t *errors = NULL;
    CHECK(fixture_schema_package_open(&package, &package_errors) == TYPE_BRIDGE_STATUS_OK);
    size_t schema_len = 0, plan_len = 0;
    uint8_t *schema = read_bytes(argv[6], &schema_len), *bytes = read_bytes(argv[7], &plan_len);
    CHECK(schema != NULL && bytes != NULL);
    type_bridge_plan_t *plan = NULL;
    CHECK(type_bridge_plan_decode(schema, schema_len, bytes, plan_len, &plan, &errors) == TYPE_BRIDGE_STATUS_OK);
    free(schema); free(bytes);
    type_bridge_runtime_config_v1_t config = {sizeof(config), TYPE_BRIDGE_RUNTIME_CONFIG_VERSION, TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MIN, 0u, {0u,0u,0u,0u}};
    type_bridge_runtime_t *runtime = NULL;
    CHECK(type_bridge_runtime_open_v1(&config, &runtime, &errors) == TYPE_BRIDGE_STATUS_OK);
    for (int attempt = 0; attempt < 2; ++attempt) {
        const int64_t timeout = attempt == 0 ? 1000 : 0;
        type_bridge_staged_transaction_options_t options = {sizeof(options), TYPE_BRIDGE_STAGED_TRANSACTION_OPTIONS_VERSION, timeout, 1};
        type_bridge_database_t *database = NULL;
        type_bridge_proxy_connection_t *proxy = NULL;
        type_bridge_staged_transaction_t *tx = NULL;
        if (proxy_mode) {
            type_bridge_query_execution_limits_v1_t limits = TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;
            limits.timeout_milliseconds = timeout; limits.collect_phase_timestamps = 1;
            type_bridge_proxy_connection_options_v1_t connection = {0};
            connection.struct_size = sizeof(connection); connection.version = TYPE_BRIDGE_PROXY_CONNECTION_OPTIONS_VERSION;
            connection.endpoint = view(argv[4]); connection.tls_mode = TYPE_BRIDGE_TLS_DISABLED; connection.answer_limits = &limits;
            CHECK(type_bridge_proxy_connection_open_v1(package, &connection, NULL, &proxy, &errors) == TYPE_BRIDGE_STATUS_OK);
            CHECK(type_bridge_proxy_connection_transaction_with_options(proxy, TYPE_BRIDGE_TRANSACTION_WRITE, &options, &tx, &errors) == TYPE_BRIDGE_STATUS_OK);
        } else {
            type_bridge_database_config_t connection = {0};
            connection.struct_size = sizeof(connection); connection.version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION;
            connection.address = view(argv[2]); connection.database = view(argv[3]);
            connection.username = view("admin"); connection.password = view("password");
            connection.http_port = 43131; connection.tls_mode = TYPE_BRIDGE_TLS_DISABLED;
            CHECK(type_bridge_database_open(runtime, package, &connection, NULL, &database, &errors) == TYPE_BRIDGE_STATUS_OK);
            CHECK(type_bridge_database_transaction_with_options(database, TYPE_BRIDGE_TRANSACTION_WRITE, &options, &tx, &errors) == TYPE_BRIDGE_STATUS_OK);
        }
        type_bridge_pending_t *pending = NULL;
        CHECK(type_bridge_staged_transaction_stage(tx, plan, &pending, &errors) == TYPE_BRIDGE_STATUS_OK);
        FILE *arm = fopen(argv[5], "w"); CHECK(arm != NULL);
        fprintf(arm, "%d", attempt == 0 ? 2000 : 61000); CHECK(fclose(arm) == 0);
        double started = now();
        type_bridge_status_t result = type_bridge_staged_transaction_commit(tx, &errors);
        if (attempt == 0) {
            CHECK(result != TYPE_BRIDGE_STATUS_OK && errors != NULL);
            type_bridge_execution_diagnostic_view_v1_t diagnostic = {0}; diagnostic.struct_size = sizeof(diagnostic);
            CHECK(type_bridge_execution_diagnostics_get_v1(errors, 0, &diagnostic) == TYPE_BRIDGE_STATUS_OK);
            CHECK(same(diagnostic.code, "transaction_deadline_exceeded"));
            int found = 0;
            for (size_t i = 0; i < diagnostic.detail_count; ++i) {
                type_bridge_execution_diagnostic_detail_view_v1_t detail = {0}; detail.struct_size = sizeof(detail);
                CHECK(type_bridge_execution_diagnostics_detail_get_v1(errors, 0, i, &detail) == TYPE_BRIDGE_STATUS_OK);
                if (same(detail.key, "execution_timeline")) {
                    type_bridge_execution_timeline_view_v1_t timeline = {0}; timeline.struct_size = sizeof(timeline);
                    CHECK(type_bridge_execution_diagnostics_timeline_get_v1(errors, 0, i, &timeline) == TYPE_BRIDGE_STATUS_OK);
                    CHECK(timeline.stamp_count == (proxy_mode ? 3u : 2u));
                    type_bridge_execution_phase_view_v1_t phase = {0}; phase.struct_size = sizeof(phase);
                    CHECK(type_bridge_execution_diagnostics_phase_get_v1(errors, 0, i, timeline.stamp_count - 1, &phase) == TYPE_BRIDGE_STATUS_OK);
                    CHECK(same(phase.phase, "failed") && same(phase.failing_phase, proxy_mode ? "accepted" : "fired"));
                    found = 1;
                }
            }
            CHECK(found); type_bridge_execution_diagnostics_close(&errors);
            struct timespec wait = {2, 100000000}; nanosleep(&wait, NULL);
        } else {
            CHECK(result == TYPE_BRIDGE_STATUS_OK && errors == NULL && now() - started > 60.0);
            type_bridge_plan_answer_kind_t kind; uint64_t value;
            CHECK(type_bridge_pending_answer(pending, &kind, &value) == TYPE_BRIDGE_STATUS_OK);
            CHECK(kind == TYPE_BRIDGE_PLAN_ANSWER_MATCHED && value == 1);
        }
        CHECK(access(argv[5], F_OK) != 0);
        type_bridge_pending_free(&pending); type_bridge_staged_transaction_close(&tx);
        if (database) CHECK(type_bridge_database_close(&database, &errors) == TYPE_BRIDGE_STATUS_OK);
        if (proxy) CHECK(type_bridge_proxy_connection_close(&proxy) == TYPE_BRIDGE_STATUS_OK);
    }
    type_bridge_plan_free(&plan);
    CHECK(type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_runtime_close(&runtime, &errors) == TYPE_BRIDGE_STATUS_OK);
    return 0;
}
