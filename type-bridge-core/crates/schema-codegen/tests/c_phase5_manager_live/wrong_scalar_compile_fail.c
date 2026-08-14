#include <phase5/models.h>

type_bridge_status_t phase5_wrong_scalar_compile_negative(
    const phase5_person_manager *manager,
    phase5_person_manager_filter_ref_v1_t source,
    const phase5_identifier *value, phase5_person_manager_filter **out_filter,
    type_bridge_execution_diagnostics_t **diagnostics) {
  return phase5_person_manager_filter_foozuzubar_eq(manager, source, value,
                                                    out_filter, diagnostics);
}
