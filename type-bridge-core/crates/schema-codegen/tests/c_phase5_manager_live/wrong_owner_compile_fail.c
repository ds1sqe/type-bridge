#include <phase5/models.h>

type_bridge_status_t phase5_wrong_owner_compile_negative(
    const phase5_robot_query_exact_binding *robot,
    phase5_person_foozuzubar_query_field **field,
    type_bridge_execution_diagnostics_t **diagnostics) {
  return phase5_person_foozuzubar_query_field_from_exact(robot, field,
                                                         diagnostics);
}
