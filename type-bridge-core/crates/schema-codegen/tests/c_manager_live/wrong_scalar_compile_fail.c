#include <manager/models.h>

type_bridge_status_t manager_wrong_scalar_compile_negative(
    const manager_person_manager *manager,
    manager_person_manager_filter_ref_v1_t source,
    const manager_identifier *value, manager_person_manager_filter **out_filter,
    type_bridge_execution_diagnostics_t **diagnostics) {
  return manager_person_manager_filter_foozuzubar_eq(manager, source, value,
                                                    out_filter, diagnostics);
}
