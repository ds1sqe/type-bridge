#ifndef TYPE_BRIDGE_TEST_PURCHASE_BUILDER_H
#define TYPE_BRIDGE_TEST_PURCHASE_BUILDER_H

#include <typebridge/type_bridge.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Build the one-query purchase for one order against `package`. */
type_bridge_status_t purchase_build(const type_bridge_schema_package_t *package,
                                    const char *order_id, const char *item_id,
                                    const char *account_id,
                                    type_bridge_plan_t **out_plan,
                                    type_bridge_execution_diagnostics_t **out_diagnostics);

#ifdef __cplusplus
}
#endif

#endif
