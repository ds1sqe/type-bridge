#include <manager/models.h>

type_bridge_status_t manager_wrong_owner_compile_negative(
    const manager_robot_query_exact_binding *robot,
    manager_person_foozuzubar_query_field **field,
    type_bridge_execution_diagnostics_t **diagnostics) {
  return manager_person_foozuzubar_query_field_from_exact(robot, field,
                                                         diagnostics);
}
