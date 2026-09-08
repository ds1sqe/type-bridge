#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <projected/models.h>
#include <projected_foreign/models.h>

#define CHECK(condition)                                                       \
  do {                                                                         \
    if (!(condition)) {                                                        \
      fprintf(stderr, "C Projected live failure at line %d\n", __LINE__);      \
      return __LINE__;                                                         \
    }                                                                          \
  } while (0)

#define VIEW(text)                                                             \
  ((type_bridge_byte_view_t){(const uint8_t *)(text), sizeof(text) - 1u})

typedef struct iid_copy {
  uint8_t bytes[256];
  size_t length;
} iid_copy_t;

typedef struct person_inputs {
  projected_identifier *identifier;
  projected_nickname *nickname;
  projected_score *score;
  projected_foozuzubar *foo_bar;
  projected_scorezuzugte *score_gte;
  projected_valzubool *boolean_value;
  projected_valzuconstrained *constrained;
  projected_valzudate *date;
  projected_valzudatetime *datetime;
  projected_valzudatetimezutzz *datetime_tz;
  projected_valzudecimal *decimal;
  projected_valzudouble *double_value;
  projected_valzuduration *duration;
} person_inputs_t;

static type_bridge_byte_view_t view_of(const char *text) {
  return (type_bridge_byte_view_t){(const uint8_t *)text, strlen(text)};
}

static uint32_t required_http_port(void) {
  const char *value = getenv("TYPE_BRIDGE_PROJECTED_LIVE_HTTP_PORT");
  char *end = NULL;
  unsigned long parsed;
  const unsigned char *cursor;
  if (value == NULL || value[0] == '\0') {
    return 0u;
  }
  for (cursor = (const unsigned char *)value; *cursor != '\0'; ++cursor) {
    if (*cursor < (unsigned char)'0' || *cursor > (unsigned char)'9') {
      return 0u;
    }
  }
  parsed = strtoul(value, &end, 10);
  if (end == NULL || *end != '\0' || parsed == 0ul || parsed > 65535ul) {
    return 0u;
  }
  return (uint32_t)parsed;
}

static int text_is(type_bridge_byte_view_t actual, const char *expected) {
  size_t length = strlen(expected);
  return actual.data != NULL && actual.length == length &&
         memcmp(actual.data, expected, length) == 0;
}

static int copy_iid(type_bridge_byte_view_t iid, iid_copy_t *output) {
  if (iid.data == NULL || iid.length == 0u || iid.length > sizeof(output->bytes)) {
    return 0;
  }
  memcpy(output->bytes, iid.data, iid.length);
  output->length = iid.length;
  return 1;
}

static type_bridge_byte_view_t iid_view(const iid_copy_t *iid) {
  return (type_bridge_byte_view_t){iid->bytes, iid->length};
}

static int close_person_inputs(person_inputs_t *values) {
  return projected_valzuduration_close(&values->duration) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_valzudouble_close(&values->double_value) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_valzudecimal_close(&values->decimal) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_valzudatetimezutzz_close(&values->datetime_tz) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_valzudatetime_close(&values->datetime) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_valzudate_close(&values->date) == TYPE_BRIDGE_STATUS_OK &&
         projected_valzuconstrained_close(&values->constrained) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_valzubool_close(&values->boolean_value) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_scorezuzugte_close(&values->score_gte) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_foozuzubar_close(&values->foo_bar) == TYPE_BRIDGE_STATUS_OK &&
         projected_score_close(&values->score) == TYPE_BRIDGE_STATUS_OK &&
         projected_nickname_close(&values->nickname) == TYPE_BRIDGE_STATUS_OK &&
         projected_identifier_close(&values->identifier) == TYPE_BRIDGE_STATUS_OK;
}

static int open_person_create(
    const type_bridge_schema_package_t *package, const char *identifier,
    const char *nickname, int64_t score, int64_t foo_bar, int64_t score_gte,
    uint8_t boolean_value, int64_t constrained, const char *date,
    const char *datetime, const char *datetime_tz, const char *decimal,
    uint64_t double_bits, const char *duration,
    projected_person_create **out_create,
    type_bridge_execution_diagnostics_t **diagnostics) {
  person_inputs_t values = {0};
  projected_person_create_args_v1_t args = {0};
  int ok = 0;
  if (projected_identifier_open(package, view_of(identifier), &values.identifier,
                             diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      projected_score_open(package, score, &values.score, diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      projected_foozuzubar_open(package, foo_bar, &values.foo_bar, diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      projected_scorezuzugte_open(package, score_gte, &values.score_gte,
                               diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      projected_valzubool_open(package, boolean_value, &values.boolean_value,
                            diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      projected_valzuconstrained_open(package, constrained, &values.constrained,
                                   diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      projected_valzudate_open(package, view_of(date), &values.date, diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      projected_valzudatetime_open(package, view_of(datetime), &values.datetime,
                                diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      projected_valzudatetimezutzz_open(package, view_of(datetime_tz),
                                     &values.datetime_tz, diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      projected_valzudecimal_open(package, view_of(decimal), &values.decimal,
                               diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      projected_valzudouble_open(package, double_bits, &values.double_value,
                              diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      projected_valzuduration_open(package, view_of(duration), &values.duration,
                                diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    goto close;
  }
  if (nickname != NULL &&
      projected_nickname_open(package, view_of(nickname), &values.nickname,
                           diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    goto close;
  }
  args.struct_size = sizeof(args);
  args.version = PROJECTED_CREATE_ARGS_VERSION;
  args.field_identifier = values.identifier;
  args.field_nickname = values.nickname;
  args.field_score = values.score;
  args.field_foozuzubar = values.foo_bar;
  args.field_scorezuzugte = values.score_gte;
  args.field_valzubool = values.boolean_value;
  args.field_valzuconstrained = values.constrained;
  args.field_valzudate = values.date;
  args.field_valzudatetime = values.datetime;
  args.field_valzudatetimezutzz = values.datetime_tz;
  args.field_valzudecimal = values.decimal;
  args.field_valzudouble = values.double_value;
  args.field_valzuduration = values.duration;
  ok = projected_person_create_open(package, &args, out_create, diagnostics) ==
           TYPE_BRIDGE_STATUS_OK &&
       *out_create != NULL && *diagnostics == NULL;

close:
  if (!close_person_inputs(&values)) {
    return 0;
  }
  return ok;
}

static int open_robot_create(
    const type_bridge_schema_package_t *package, int64_t key,
    int64_t constrained, projected_robot_create **out_create,
    type_bridge_execution_diagnostics_t **diagnostics) {
  projected_robotzuid *robot_id = NULL;
  projected_valzuconstrained *value = NULL;
  projected_robot_create_args_v1_t args = {0};
  int ok = projected_robotzuid_open(package, key, &robot_id, diagnostics) ==
               TYPE_BRIDGE_STATUS_OK &&
           projected_valzuconstrained_open(package, constrained, &value,
                                        diagnostics) == TYPE_BRIDGE_STATUS_OK;
  if (ok) {
    args.struct_size = sizeof(args);
    args.version = PROJECTED_CREATE_ARGS_VERSION;
    args.field_robotzuid = robot_id;
    args.field_valzuconstrained = value;
    ok = projected_robot_create_open(package, &args, out_create, diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         *out_create != NULL && *diagnostics == NULL;
  }
  return projected_valzuconstrained_close(&value) == TYPE_BRIDGE_STATUS_OK &&
         projected_robotzuid_close(&robot_id) == TYPE_BRIDGE_STATUS_OK && ok;
}

static int person_ref_from_iid(
    const type_bridge_schema_package_t *package, const iid_copy_t *iid,
    projected_person_ref **reference,
    type_bridge_execution_diagnostics_t **diagnostics) {
  return projected_person_ref_from_iid(package, iid_view(iid), reference,
                                    diagnostics) == TYPE_BRIDGE_STATUS_OK &&
         *reference != NULL && *diagnostics == NULL;
}

static int observe_person_scalars(
    const projected_person *person,
    type_bridge_execution_diagnostics_t **diagnostics) {
  projected_valzubool *boolean_value = NULL;
  projected_valzudate *date = NULL;
  projected_valzudatetime *datetime = NULL;
  projected_valzudatetimezutzz *datetime_tz = NULL;
  projected_valzudecimal *decimal = NULL;
  projected_valzudouble *double_value = NULL;
  projected_valzuduration *duration = NULL;
  projected_score *score = NULL;
  projected_nickname *nickname = NULL;
  uint8_t bool_value = 1u;
  int64_t long_value = 0;
  uint64_t bits = 0u;
  type_bridge_byte_view_t text = {0};
  int ok =
      projected_person_valzubool(person, &boolean_value, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      projected_valzubool_value(boolean_value, &bool_value, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      bool_value == 0u &&
      projected_person_valzudate(person, &date, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      projected_valzudate_value(date, &text, diagnostics) == TYPE_BRIDGE_STATUS_OK &&
      text_is(text, "2026-08-12") &&
      projected_person_valzudatetime(person, &datetime, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      projected_valzudatetime_value(datetime, &text, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      text_is(text, "2026-08-12T09:30:00") &&
      projected_person_valzudatetimezutzz(person, &datetime_tz, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      projected_valzudatetimezutzz_value(datetime_tz, &text, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      text_is(text, "2026-08-12T09:30:00Z") &&
      projected_person_valzudecimal(person, &decimal, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      projected_valzudecimal_value(decimal, &text, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      text_is(text, "38.5") &&
      projected_person_valzudouble(person, &double_value, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      projected_valzudouble_value(double_value, &bits, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      bits == UINT64_C(0x4043000000000000) &&
      projected_person_valzuduration(person, &duration, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      projected_valzuduration_value(duration, &text, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      text_is(text, "PT38S") &&
      projected_person_score(person, &score, diagnostics) == TYPE_BRIDGE_STATUS_OK &&
      projected_score_value(score, &long_value, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      long_value == 38 &&
      projected_person_nickname(person, &nickname, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      projected_nickname_value(nickname, &text, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      text_is(text, "Ada");
  ok = projected_nickname_close(&nickname) == TYPE_BRIDGE_STATUS_OK &&
       projected_score_close(&score) == TYPE_BRIDGE_STATUS_OK &&
       projected_valzuduration_close(&duration) == TYPE_BRIDGE_STATUS_OK &&
       projected_valzudouble_close(&double_value) == TYPE_BRIDGE_STATUS_OK &&
       projected_valzudecimal_close(&decimal) == TYPE_BRIDGE_STATUS_OK &&
       projected_valzudatetimezutzz_close(&datetime_tz) == TYPE_BRIDGE_STATUS_OK &&
       projected_valzudatetime_close(&datetime) == TYPE_BRIDGE_STATUS_OK &&
       projected_valzudate_close(&date) == TYPE_BRIDGE_STATUS_OK &&
       projected_valzubool_close(&boolean_value) == TYPE_BRIDGE_STATUS_OK && ok;
  return ok && *diagnostics == NULL;
}

static int interaction_actor_is(
    const projected_interaction *interaction, int expected_kind,
    type_bridge_execution_diagnostics_t **diagnostics) {
  projected_interaction_actor_player *actor = NULL;
  int ok = projected_interaction_actor(interaction, &actor, diagnostics) ==
           TYPE_BRIDGE_STATUS_OK;
  if (!ok) {
    return 0;
  }
  if (expected_kind == 0) {
    return actor == NULL && *diagnostics == NULL;
  }
  if (expected_kind == 1) {
    projected_person_ref *person = NULL;
    projected_identifier *key = NULL;
    type_bridge_byte_view_t text = {0};
    ok = projected_interaction_actor_player_as_person(actor, &person, diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_person_ref_identifier_key(person, &key, diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_identifier_value(key, &text, diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         text_is(text, "data-ada");
    ok = projected_identifier_close(&key) == TYPE_BRIDGE_STATUS_OK &&
         projected_person_ref_close(&person) == TYPE_BRIDGE_STATUS_OK && ok;
  } else {
    projected_robot_ref *robot = NULL;
    projected_robotzuid *key = NULL;
    int64_t value = 0;
    ok = projected_interaction_actor_player_as_robot(actor, &robot, diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_robot_ref_robotzuid_key(robot, &key, diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         projected_robotzuid_value(key, &value, diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         value == 7;
    ok = projected_robotzuid_close(&key) == TYPE_BRIDGE_STATUS_OK &&
         projected_robot_ref_close(&robot) == TYPE_BRIDGE_STATUS_OK && ok;
  }
  return projected_interaction_actor_player_close(&actor) == TYPE_BRIDGE_STATUS_OK &&
         ok && *diagnostics == NULL;
}

int main(void) {
  const char *address = getenv("TYPE_BRIDGE_PROJECTED_LIVE_ADDRESS");
  const char *database_name = getenv("TYPE_BRIDGE_PROJECTED_LIVE_DATABASE");
  const char *username = getenv("TYPEDB_USERNAME");
  const char *password = getenv("TYPEDB_PASSWORD");
  uint32_t http_port = required_http_port();
  type_bridge_schema_package_t *package = NULL;
  type_bridge_schema_package_t *foreign_package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_database_t *database = NULL;
  type_bridge_runtime_config_v1_t runtime_config = {
      sizeof(type_bridge_runtime_config_v1_t),
      TYPE_BRIDGE_RUNTIME_CONFIG_VERSION,
      TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MIN,
      0u,
      {0u, 0u, 0u, 0u}};
  type_bridge_database_config_v1_t database_config = {0};
  type_bridge_byte_view_t version = {0};
  projected_person_create *ada_create = NULL;
  projected_person_create *dana_create = NULL;
  projected_person *person = NULL;
  projected_robot_create *robot_positive_create = NULL;
  projected_robot_create *robot_negative_create = NULL;
  projected_robot *robot = NULL;
  iid_copy_t iids[10] = {{{0}, 0u}};
  projected_person_ref *ada_ref = NULL;
  projected_person_ref *dana_ref = NULL;
  projected_robot_ref *robot_ref = NULL;
  projected_interaction_target_player *ada_target = NULL;
  projected_interaction_target_player *dana_target = NULL;
  projected_interaction_actor_player *person_actor = NULL;
  projected_interaction_actor_player *robot_actor = NULL;
  projected_identifier *interaction_ids[3] = {NULL, NULL, NULL};
  projected_interaction_create *interaction_creates[3] = {NULL, NULL, NULL};
  projected_interaction *interaction = NULL;
  projected_plainzhactivity_participant_player *plain_player = NULL;
  projected_plainzhactivity_create *plain_create = NULL;
  projected_plainzhactivity *plain = NULL;
  projected_event_subject_player *event_subject = NULL;
  projected_event_create *event_create = NULL;
  projected_event *event = NULL;
  projected_event_ref *event_ref = NULL;
  projected_container_item_player *event_item = NULL;
  projected_container_create *container_create = NULL;
  projected_container *container = NULL;
  uint64_t count = 0u;
  size_t item_count = 0u;

  CHECK(address != NULL && address[0] != '\0');
  CHECK(database_name != NULL && database_name[0] != '\0');
  CHECK(username != NULL && username[0] != '\0');
  CHECK(password != NULL);
  CHECK(http_port != 0u);
  CHECK(projected_schema_package_open(&package, &package_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        package != NULL && package_diagnostics == NULL);
  CHECK(projected_foreign_schema_package_open(&foreign_package,
                                           &package_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        foreign_package != NULL && package_diagnostics == NULL);
  database_config.struct_size = sizeof(database_config);
  database_config.version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION;
  database_config.address = view_of(address);
  database_config.database = view_of(database_name);
  database_config.username = view_of(username);
  database_config.password = view_of(password);
  database_config.http_port = http_port;
  database_config.tls_mode = TYPE_BRIDGE_TLS_DISABLED;
  CHECK(type_bridge_runtime_open_v1(&runtime_config, &runtime, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        runtime != NULL && diagnostics == NULL);
  CHECK(type_bridge_database_open_v1(runtime, package, &database_config, NULL,
                                     &database, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        database != NULL && diagnostics == NULL);
  CHECK(type_bridge_database_server_version(database, &version) ==
            TYPE_BRIDGE_STATUS_OK &&
        text_is(version, "3.12.3"));

  {
    projected_foreign_identifier *foreign_key = NULL;
    projected_foreign_person_ref *foreign_ref = NULL;
    projected_plainzhactivity_participant_player *rejected =
        (projected_plainzhactivity_participant_player *)(uintptr_t)1u;
    CHECK(projected_foreign_identifier_open(foreign_package, VIEW("data-ada"),
                                         &foreign_key,
                                         &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          projected_foreign_person_ref_from_key(foreign_package, foreign_key,
                                             &foreign_ref,
                                             &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
    CHECK(projected_plainzhactivity_participant_player_from_person(
              (const projected_person_ref *)foreign_ref, &rejected,
              &diagnostics) == TYPE_BRIDGE_STATUS_EXECUTION_FAILED &&
          rejected == NULL && diagnostics != NULL);
    CHECK(type_bridge_execution_diagnostics_close(&diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          diagnostics == NULL &&
          projected_foreign_person_ref_close(&foreign_ref) ==
              TYPE_BRIDGE_STATUS_OK &&
          projected_foreign_identifier_close(&foreign_key) ==
              TYPE_BRIDGE_STATUS_OK);
  }

  CHECK(open_person_create(
      package, "data-ada", "Ada", 38, 7, 40, 0u, 38, "2026-08-12",
      "2026-08-12T09:30:00", "2026-08-12T09:30:00Z", "38.5",
      UINT64_C(0x4043000000000000), "PT38S", &ada_create, &diagnostics));
  CHECK(projected_person_database_insert(database, ada_create, NULL, &person,
                                      &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  {
    type_bridge_byte_view_t iid = {0};
    CHECK(projected_person_iid(person, &iid, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          copy_iid(iid, &iids[0]) && observe_person_scalars(person, &diagnostics));
  }
  CHECK(projected_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(open_person_create(
      package, "data-dana", NULL, 45, 9, 40, 1u, 45, "2026-08-13",
      "2026-08-13T10:45:00", "2026-08-13T10:45:00Z", "45.5",
      UINT64_C(0x4046800000000000), "PT45S", &dana_create, &diagnostics));
  CHECK(projected_person_database_insert(database, dana_create, NULL, &person,
                                      &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  {
    type_bridge_byte_view_t iid = {0};
    CHECK(projected_person_iid(person, &iid, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          copy_iid(iid, &iids[1]));
  }
  CHECK(projected_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(person_ref_from_iid(package, &iids[0], &ada_ref, &diagnostics));
  CHECK(person_ref_from_iid(package, &iids[1], &dana_ref, &diagnostics));

  CHECK(open_robot_create(package, 7, 12, &robot_positive_create,
                          &diagnostics));
  CHECK(projected_robot_database_insert(database, robot_positive_create, NULL,
                                     &robot, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  {
    type_bridge_byte_view_t iid = {0};
    projected_robotzuid *key = NULL;
    int64_t value = 0;
    CHECK(projected_robot_iid(robot, &iid, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          copy_iid(iid, &iids[2]) &&
          projected_robot_robotzuid(robot, &key, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          projected_robotzuid_value(key, &value, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          value == 7 && projected_robotzuid_close(&key) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(projected_robot_close(&robot) == TYPE_BRIDGE_STATUS_OK);
  CHECK(open_robot_create(package, -7, 13, &robot_negative_create,
                          &diagnostics));
  CHECK(projected_robot_database_insert(database, robot_negative_create, NULL,
                                     &robot, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  {
    type_bridge_byte_view_t iid = {0};
    projected_robotzuid *key = NULL;
    int64_t value = 0;
    CHECK(projected_robot_iid(robot, &iid, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          copy_iid(iid, &iids[3]) &&
          projected_robot_robotzuid(robot, &key, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          projected_robotzuid_value(key, &value, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          value == -7 && projected_robotzuid_close(&key) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(projected_robot_close(&robot) == TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_robot_ref_from_iid(package, iid_view(&iids[2]), &robot_ref,
                                  &diagnostics) == TYPE_BRIDGE_STATUS_OK);

  CHECK(projected_interaction_target_player_from_person(
            ada_ref, &ada_target, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
        projected_interaction_target_player_from_person(
            dana_ref, &dana_target, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
        projected_interaction_actor_player_from_person(
            ada_ref, &person_actor, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
        projected_interaction_actor_player_from_robot(
            robot_ref, &robot_actor, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  {
    const char *keys[3] = {"data-interaction-robot",
                           "data-interaction-absent",
                           "data-interaction-person"};
    projected_interaction_actor_player *actors[3] = {robot_actor, NULL,
                                                  person_actor};
    projected_interaction_target_player *targets[3] = {ada_target, dana_target,
                                                     dana_target};
    int expected_kinds[3] = {2, 0, 1};
    size_t index;
    for (index = 0u; index < 3u; ++index) {
      projected_interaction_create_args_v1_t args = {0};
      type_bridge_byte_view_t iid = {0};
      CHECK(projected_identifier_open(package, view_of(keys[index]),
                                   &interaction_ids[index], &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      args.struct_size = sizeof(args);
      args.version = PROJECTED_CREATE_ARGS_VERSION;
      args.field_identifier = interaction_ids[index];
      args.role_actor = actors[index];
      args.role_target = targets[index];
      CHECK(projected_interaction_create_open(
                package, &args, &interaction_creates[index], &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(projected_interaction_database_insert(
                database, interaction_creates[index], NULL, &interaction,
                &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            interaction_actor_is(interaction, expected_kinds[index],
                                 &diagnostics) &&
            projected_interaction_iid(interaction, &iid, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            copy_iid(iid, &iids[4u + index]));
      CHECK(projected_interaction_close(&interaction) == TYPE_BRIDGE_STATUS_OK);
    }
  }

  CHECK(projected_plainzhactivity_participant_player_from_person(
            ada_ref, &plain_player, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  {
    projected_plainzhactivity_create_args_v1_t args = {0};
    type_bridge_byte_view_t iid = {0};
    projected_plainzhactivity_participant_player *hydrated = NULL;
    projected_person_ref *participant = NULL;
    projected_identifier *key = NULL;
    type_bridge_byte_view_t text = {0};
    args.struct_size = sizeof(args);
    args.version = PROJECTED_CREATE_ARGS_VERSION;
    args.role_participant = plain_player;
    CHECK(projected_plainzhactivity_create_open(package, &args, &plain_create,
                                             &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(projected_plainzhactivity_database_insert(
              database, plain_create, NULL, &plain, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          projected_plainzhactivity_iid(plain, &iid, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          copy_iid(iid, &iids[7]) &&
          projected_plainzhactivity_participant(plain, &hydrated, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          projected_plainzhactivity_participant_player_as_person(
              hydrated, &participant, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          projected_person_ref_identifier_key(participant, &key, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          projected_identifier_value(key, &text, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          text_is(text, "data-ada"));
    CHECK(projected_identifier_close(&key) == TYPE_BRIDGE_STATUS_OK &&
          projected_person_ref_close(&participant) == TYPE_BRIDGE_STATUS_OK &&
          projected_plainzhactivity_participant_player_close(&hydrated) ==
              TYPE_BRIDGE_STATUS_OK &&
          projected_plainzhactivity_close(&plain) == TYPE_BRIDGE_STATUS_OK);
    CHECK(projected_plainzhactivity_database_get_by_iid(
              database, iid_view(&iids[7]), NULL, &plain, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          plain != NULL && diagnostics == NULL);
    CHECK(projected_plainzhactivity_close(&plain) == TYPE_BRIDGE_STATUS_OK);
  }

  CHECK(projected_event_subject_player_from_person(ada_ref, &event_subject,
                                                 &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  {
    projected_event_create_args_v1_t args = {0};
    type_bridge_byte_view_t iid = {0};
    args.struct_size = sizeof(args);
    args.version = PROJECTED_CREATE_ARGS_VERSION;
    args.role_subject = event_subject;
    CHECK(projected_event_create_open(package, &args, &event_create,
                                   &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          projected_event_database_insert(database, event_create, NULL, &event,
                                       &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          projected_event_iid(event, &iid, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          copy_iid(iid, &iids[8]));
    CHECK(projected_event_close(&event) == TYPE_BRIDGE_STATUS_OK);
    CHECK(projected_event_ref_from_iid(package, iid_view(&iids[8]), &event_ref,
                                    &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          projected_container_item_player_from_event(event_ref, &event_item,
                                                   &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
  }
  {
    const projected_container_item_player *items[] = {event_item};
    projected_container_create_role_item_chunks_v1_t chunk = {0};
    projected_container_create_args_v1_t args = {0};
    type_bridge_byte_view_t iid = {0};
    projected_container_item_player *hydrated_item = NULL;
    projected_event_ref *hydrated_event = NULL;
    type_bridge_byte_view_t hydrated_iid = {0};
    chunk.struct_size = sizeof(chunk);
    chunk.version = PROJECTED_CREATE_ARGS_VERSION;
    chunk.values = items;
    chunk.count = 1u;
    args.struct_size = sizeof(args);
    args.version = PROJECTED_CREATE_ARGS_VERSION;
    args.role_item_chunks = &chunk;
    CHECK(projected_container_create_open(package, &args, &container_create,
                                       &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          projected_container_database_insert(database, container_create, NULL,
                                           &container, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          projected_container_iid(container, &iid, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          copy_iid(iid, &iids[9]) &&
          projected_container_item_count(container, &item_count, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          item_count == 1u &&
          projected_container_item_at(container, 0u, &hydrated_item,
                                   &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          projected_container_item_player_as_event(
              hydrated_item, &hydrated_event, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          projected_event_ref_iid(hydrated_event, &hydrated_iid, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          hydrated_iid.length == iids[8].length &&
          memcmp(hydrated_iid.data, iids[8].bytes, iids[8].length) == 0);
    CHECK(projected_event_ref_close(&hydrated_event) == TYPE_BRIDGE_STATUS_OK &&
          projected_container_item_player_close(&hydrated_item) ==
              TYPE_BRIDGE_STATUS_OK &&
          projected_container_close(&container) == TYPE_BRIDGE_STATUS_OK);
  }

  CHECK(projected_container_database_delete_by_iid(
            database, iid_view(&iids[9]), NULL, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_event_database_delete_by_iid(database, iid_view(&iids[8]), NULL,
                                            &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_plainzhactivity_database_delete_by_iid(
            database, iid_view(&iids[7]), NULL, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_plainzhactivity_database_get_by_iid(
            database, iid_view(&iids[7]), NULL, &plain, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        plain == NULL && diagnostics == NULL);
  CHECK(projected_plainzhactivity_database_count(database, NULL, &count,
                                              &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(projected_interaction_database_delete_by_iid(
            database, iid_view(&iids[6]), NULL, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_interaction_database_delete_by_iid(
            database, iid_view(&iids[5]), NULL, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_interaction_database_delete_by_iid(
            database, iid_view(&iids[4]), NULL, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_robot_database_delete_by_iid(database, iid_view(&iids[3]), NULL,
                                            &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_robot_database_delete_by_iid(database, iid_view(&iids[2]), NULL,
                                            &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_person_database_delete_by_iid(database, iid_view(&iids[1]), NULL,
                                             &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_person_database_delete_by_iid(database, iid_view(&iids[0]), NULL,
                                             &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_container_database_count(database, NULL, &count, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(projected_event_database_count(database, NULL, &count, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(projected_interaction_database_count(database, NULL, &count,
                                          &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(projected_robot_database_count(database, NULL, &count, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(projected_person_database_count(database, NULL, &count, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);

  CHECK(projected_container_create_close(&container_create) ==
            TYPE_BRIDGE_STATUS_OK &&
        projected_container_item_player_close(&event_item) ==
            TYPE_BRIDGE_STATUS_OK &&
        projected_event_ref_close(&event_ref) == TYPE_BRIDGE_STATUS_OK &&
        projected_event_create_close(&event_create) == TYPE_BRIDGE_STATUS_OK &&
        projected_event_subject_player_close(&event_subject) ==
            TYPE_BRIDGE_STATUS_OK &&
        projected_plainzhactivity_create_close(&plain_create) ==
            TYPE_BRIDGE_STATUS_OK &&
        projected_plainzhactivity_participant_player_close(&plain_player) ==
            TYPE_BRIDGE_STATUS_OK);
  {
    size_t index;
    for (index = 0u; index < 3u; ++index) {
      CHECK(projected_interaction_create_close(&interaction_creates[index]) ==
                TYPE_BRIDGE_STATUS_OK &&
            projected_identifier_close(&interaction_ids[index]) ==
                TYPE_BRIDGE_STATUS_OK);
    }
  }
  CHECK(projected_interaction_actor_player_close(&robot_actor) ==
            TYPE_BRIDGE_STATUS_OK &&
        projected_interaction_actor_player_close(&person_actor) ==
            TYPE_BRIDGE_STATUS_OK &&
        projected_interaction_target_player_close(&dana_target) ==
            TYPE_BRIDGE_STATUS_OK &&
        projected_interaction_target_player_close(&ada_target) ==
            TYPE_BRIDGE_STATUS_OK &&
        projected_robot_ref_close(&robot_ref) == TYPE_BRIDGE_STATUS_OK &&
        projected_robot_create_close(&robot_negative_create) ==
            TYPE_BRIDGE_STATUS_OK &&
        projected_robot_create_close(&robot_positive_create) ==
            TYPE_BRIDGE_STATUS_OK &&
        projected_person_ref_close(&dana_ref) == TYPE_BRIDGE_STATUS_OK &&
        projected_person_ref_close(&ada_ref) == TYPE_BRIDGE_STATUS_OK &&
        projected_person_create_close(&dana_create) == TYPE_BRIDGE_STATUS_OK &&
        projected_person_create_close(&ada_create) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_database_close(&database, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        database == NULL && diagnostics == NULL);
  CHECK(type_bridge_runtime_close(&runtime, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        runtime == NULL && diagnostics == NULL);
  CHECK(type_bridge_schema_package_close(&foreign_package) ==
            TYPE_BRIDGE_STATUS_OK &&
        type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);

  puts("TYPE_BRIDGE_C_PROJECTED_LIVE_FACT\tcanonical_scalar_values\t{\"hydrated\":{\"boolean\":{\"kind\":\"boolean\",\"value\":false},\"date\":{\"kind\":\"date\",\"value\":\"2026-08-12\"},\"datetime\":{\"kind\":\"datetime\",\"value\":\"2026-08-12T09:30:00\"},\"datetime_tz\":{\"kind\":\"datetime_tz\",\"value\":\"2026-08-12T09:30:00Z\"},\"decimal\":{\"kind\":\"decimal\",\"value\":\"38.5\"},\"double\":{\"bits\":\"4043000000000000\",\"kind\":\"double\"},\"duration\":{\"kind\":\"duration\",\"value\":\"PT38S\"},\"long\":{\"kind\":\"long\",\"value\":\"38\"},\"string\":{\"kind\":\"string\",\"value\":\"Ada\"}},\"model\":\"person\",\"ref\":\"data-ada\"}");
  puts("TYPE_BRIDGE_C_PROJECTED_LIVE_FACT\tcleanup\t{\"order\":[\"container-event\",\"event-ada\",\"plain-activity-ada\",\"interaction-person\",\"interaction-absent\",\"interaction-robot\",\"robot-negative-7\",\"robot-7\",\"data-dana\",\"data-ada\"],\"zero_checks\":[{\"count_after_cleanup\":0,\"model\":\"container\"},{\"count_after_cleanup\":0,\"model\":\"event\"},{\"count_after_cleanup\":0,\"model\":\"interaction\"},{\"count_after_cleanup\":0,\"model\":\"person\"},{\"count_after_cleanup\":0,\"model\":\"plain-activity\"},{\"count_after_cleanup\":0,\"model\":\"robot\"}]}");
  puts("TYPE_BRIDGE_C_PROJECTED_LIVE_FACT\tinherited_plain_activity_role_lifecycle\t{\"count_after_delete\":0,\"created\":true,\"deleted\":true,\"inherited_relation\":\"base-activity\",\"model\":\"plain-activity\",\"participant\":{\"key\":\"data-ada\",\"model\":\"person\"},\"read_after_create\":true,\"read_after_delete\":false,\"ref\":\"plain-activity-ada\",\"role\":\"participant\",\"role_identity_preserved\":true}");
  puts("TYPE_BRIDGE_C_PROJECTED_LIVE_FACT\tinteger_key_polymorphic_optional_role\t{\"integer_keys\":[{\"model\":\"robot\",\"ref\":\"robot-negative-7\",\"value\":\"-7\"},{\"model\":\"robot\",\"ref\":\"robot-7\",\"value\":\"7\"}],\"optional_role\":\"actor\",\"relation\":\"interaction\",\"states\":[{\"actor\":null,\"relation_ref\":\"interaction-absent\"},{\"actor\":{\"key\":\"data-ada\",\"model\":\"person\"},\"relation_ref\":\"interaction-person\"},{\"actor\":{\"key\":\"7\",\"model\":\"robot\"},\"relation_ref\":\"interaction-robot\"}]}");
  puts("TYPE_BRIDGE_C_PROJECTED_LIVE_FACT\trelation_as_player\t{\"owner\":{\"model\":\"container\",\"ref\":\"container-event\"},\"player\":{\"model\":\"event\",\"ref\":\"event-ada\"},\"preserved\":true,\"role\":\"item\"}");
  return 0;
}
