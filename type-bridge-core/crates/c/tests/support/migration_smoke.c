#include <stdint.h>
#include <typebridge/type_bridge.h>

int migration_smoke(void) {
  type_bridge_migration_cancellation_t *cancellation = NULL;
  type_bridge_migration_identity_t *identity = NULL;
  type_bridge_diagnostics_t *diagnostics = NULL;
  static const uint8_t app[] = "example";
  static const uint8_t name[] = "9999_unknown";
  uint8_t cancelled = 9u;
  if (type_bridge_migration_cancellation_new(&cancellation) != TYPE_BRIDGE_STATUS_OK || cancellation == NULL) return 1;
  if (type_bridge_migration_cancellation_is_cancelled(cancellation, &cancelled) != TYPE_BRIDGE_STATUS_OK || cancelled != 0u) return 2;
  if (type_bridge_migration_cancellation_cancel(cancellation) != TYPE_BRIDGE_STATUS_OK) return 3;
  if (type_bridge_migration_cancellation_is_cancelled(cancellation, &cancelled) != TYPE_BRIDGE_STATUS_OK || cancelled != 1u) return 4;
  if (type_bridge_migration_cancellation_close(&cancellation) != TYPE_BRIDGE_STATUS_OK || cancellation != NULL) return 5;
  if (type_bridge_migration_identity_new((type_bridge_byte_view_t){app, sizeof(app) - 1u}, (type_bridge_byte_view_t){name, sizeof(name) - 1u}, &identity, &diagnostics) != TYPE_BRIDGE_STATUS_OK || identity == NULL || diagnostics != NULL) return 6;
  if (type_bridge_migration_identity_close(&identity) != TYPE_BRIDGE_STATUS_OK || identity != NULL) return 7;
  return 0;
}
