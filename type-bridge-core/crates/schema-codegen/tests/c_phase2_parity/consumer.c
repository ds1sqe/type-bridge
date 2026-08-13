#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include <typebridge/type_bridge.h>
#include <phase2/models.h>
#include <phase2_foreign/models.h>

#define CHECK(condition)                                                       \
  do {                                                                         \
    if (!(condition)) {                                                        \
      fprintf(stderr, "C Phase-2 parity evidence failed at line %d\n",       \
              __LINE__);                                                       \
      return __LINE__;                                                         \
    }                                                                          \
  } while (0)

#define REQUIRE(condition)                                                     \
  do {                                                                         \
    if (!(condition)) {                                                        \
      fprintf(stderr, "C Phase-2 evidence requirement failed at line %d\n",  \
              __LINE__);                                                       \
      return 0;                                                                \
    }                                                                          \
  } while (0)

#define VIEW(literal)                                                          \
  ((type_bridge_byte_view_t){(const uint8_t *)(literal),                       \
                             sizeof(literal) - 1u})

#define PROJECTED(value) ((const type_bridge_projected_value_t *)(value))
#define REFERENCE(value) ((const type_bridge_projected_reference_t *)(value))

static int same_view(type_bridge_byte_view_t actual,
                     type_bridge_byte_view_t expected) {
  return actual.length == expected.length &&
         (actual.length == 0u ||
          (actual.data != NULL && expected.data != NULL &&
           memcmp(actual.data, expected.data, actual.length) == 0));
}

static int diagnostic_is(
    const type_bridge_execution_diagnostics_t *diagnostics,
    type_bridge_execution_diagnostic_category_t category,
    const char *expected_code) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
  size_t count = 0u;
  return diagnostics != NULL &&
         type_bridge_execution_diagnostics_count(diagnostics, &count) ==
             TYPE_BRIDGE_STATUS_OK &&
         count == 1u &&
         type_bridge_execution_diagnostics_get_v1(diagnostics, 0u,
                                                  &diagnostic) ==
             TYPE_BRIDGE_STATUS_OK &&
         diagnostic.category == category &&
         same_view(diagnostic.code,
                   (type_bridge_byte_view_t){
                       (const uint8_t *)expected_code, strlen(expected_code)});
}

static int diagnostic_count_detail(
    const type_bridge_execution_diagnostics_t *diagnostics, const char *key,
    uint64_t expected) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
  size_t index;
  if (type_bridge_execution_diagnostics_get_v1(diagnostics, 0u,
                                               &diagnostic) !=
      TYPE_BRIDGE_STATUS_OK) {
    return 0;
  }
  for (index = 0u; index < diagnostic.detail_count; ++index) {
    type_bridge_execution_diagnostic_detail_view_v1_t detail = {0};
    if (type_bridge_execution_diagnostics_detail_get_v1(
            diagnostics, 0u, index, &detail) == TYPE_BRIDGE_STATUS_OK &&
        detail.kind == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_COUNT &&
        detail.unsigned_value == expected &&
        same_view(detail.key,
                  (type_bridge_byte_view_t){(const uint8_t *)key,
                                            strlen(key)})) {
      return 1;
    }
  }
  return 0;
}

static int diagnostic_signed_detail(
    const type_bridge_execution_diagnostics_t *diagnostics, const char *key,
    int64_t expected) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
  size_t index;
  if (type_bridge_execution_diagnostics_get_v1(diagnostics, 0u,
                                               &diagnostic) !=
      TYPE_BRIDGE_STATUS_OK) {
    return 0;
  }
  for (index = 0u; index < diagnostic.detail_count; ++index) {
    type_bridge_execution_diagnostic_detail_view_v1_t detail = {0};
    int64_t value = 0;
    if (type_bridge_execution_diagnostics_detail_get_v1(
            diagnostics, 0u, index, &detail) == TYPE_BRIDGE_STATUS_OK &&
        detail.kind == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_SIGNED &&
        same_view(detail.key,
                  (type_bridge_byte_view_t){(const uint8_t *)key,
                                            strlen(key)}) &&
        type_bridge_execution_diagnostics_detail_signed(
            diagnostics, 0u, index, &value) == TYPE_BRIDGE_STATUS_OK &&
        value == expected) {
      return 1;
    }
  }
  return 0;
}

static int range_diagnostic_is_exact(
    const type_bridge_execution_diagnostics_t *diagnostics) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
  type_bridge_execution_diagnostic_path_view_v1_t path = {0};
  return diagnostic_is(diagnostics,
                       TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                       "range_constraint_violation") &&
         type_bridge_execution_diagnostics_get_v1(diagnostics, 0u,
                                                  &diagnostic) ==
             TYPE_BRIDGE_STATUS_OK &&
         diagnostic.path_count == 1u &&
         type_bridge_execution_diagnostics_path_get_v1(diagnostics, 0u, 0u,
                                                       &path) ==
             TYPE_BRIDGE_STATUS_OK &&
         path.kind == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_TYPE &&
         same_view(path.primary, VIEW("attribute")) &&
         same_view(path.secondary, VIEW("val_constrained")) &&
         diagnostic_signed_detail(diagnostics, "actual", 81) &&
         diagnostic_signed_detail(diagnostics, "maximum", 80);
}

static int close_diagnostics(type_bridge_execution_diagnostics_t **diagnostics) {
  return type_bridge_execution_diagnostics_close(diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         *diagnostics == NULL;
}

typedef struct person_values {
  phase2_aliases *analyst;
  phase2_aliases *mathematician;
  phase2_foozuzubar *foo_bar;
  phase2_identifier *identifier;
  phase2_nickname *nickname;
  phase2_score *score;
  phase2_scorezuzugte *score_gte;
  phase2_valzubool *boolean_value;
  phase2_valzuconstrained *constrained;
  phase2_valzudate *date;
  phase2_valzudatetime *datetime;
  phase2_valzudatetimezutzz *datetime_tz;
  phase2_valzudecimal *decimal;
  phase2_valzudouble *double_value;
  phase2_valzuduration *duration;
} person_values_t;

static int open_person_values(const type_bridge_schema_package_t *package,
                              person_values_t *values,
                              type_bridge_execution_diagnostics_t **diagnostics) {
  memset(values, 0, sizeof(*values));
  REQUIRE(phase2_aliases_open(package, VIEW("analyst"), &values->analyst,
                              diagnostics) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_aliases_open(package, VIEW("mathematician"),
                              &values->mathematician, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_foozuzubar_open(package, 7, &values->foo_bar, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_identifier_open(package, VIEW("data-ada"),
                                 &values->identifier, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_nickname_open(package, VIEW("Ada"), &values->nickname,
                               diagnostics) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_score_open(package, 38, &values->score, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_scorezuzugte_open(package, 40, &values->score_gte,
                                   diagnostics) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_valzubool_open(package, 0u, &values->boolean_value,
                                diagnostics) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_valzuconstrained_open(package, 38, &values->constrained,
                                       diagnostics) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_valzudate_open(package, VIEW("2026-08-12"), &values->date,
                                diagnostics) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_valzudatetime_open(package, VIEW("2026-08-12T09:30:00"),
                                    &values->datetime, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_valzudatetimezutzz_open(
              package, VIEW("2026-08-12T09:30:00Z"), &values->datetime_tz,
              diagnostics) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_valzudecimal_open(package, VIEW("38.5"), &values->decimal,
                                   diagnostics) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_valzudouble_open(package, UINT64_C(0x4043000000000000),
                                  &values->double_value, diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_valzuduration_open(package, VIEW("PT38S"), &values->duration,
                                    diagnostics) == TYPE_BRIDGE_STATUS_OK);
  return *diagnostics == NULL;
}

static int close_person_values(person_values_t *values) {
  return phase2_valzuduration_close(&values->duration) ==
             TYPE_BRIDGE_STATUS_OK &&
         phase2_valzudouble_close(&values->double_value) ==
             TYPE_BRIDGE_STATUS_OK &&
         phase2_valzudecimal_close(&values->decimal) == TYPE_BRIDGE_STATUS_OK &&
         phase2_valzudatetimezutzz_close(&values->datetime_tz) ==
             TYPE_BRIDGE_STATUS_OK &&
         phase2_valzudatetime_close(&values->datetime) ==
             TYPE_BRIDGE_STATUS_OK &&
         phase2_valzudate_close(&values->date) == TYPE_BRIDGE_STATUS_OK &&
         phase2_valzuconstrained_close(&values->constrained) ==
             TYPE_BRIDGE_STATUS_OK &&
         phase2_valzubool_close(&values->boolean_value) ==
             TYPE_BRIDGE_STATUS_OK &&
         phase2_scorezuzugte_close(&values->score_gte) ==
             TYPE_BRIDGE_STATUS_OK &&
         phase2_score_close(&values->score) == TYPE_BRIDGE_STATUS_OK &&
         phase2_nickname_close(&values->nickname) == TYPE_BRIDGE_STATUS_OK &&
         phase2_identifier_close(&values->identifier) == TYPE_BRIDGE_STATUS_OK &&
         phase2_foozuzubar_close(&values->foo_bar) == TYPE_BRIDGE_STATUS_OK &&
         phase2_aliases_close(&values->mathematician) == TYPE_BRIDGE_STATUS_OK &&
         phase2_aliases_close(&values->analyst) == TYPE_BRIDGE_STATUS_OK;
}

enum person_field_index {
  PERSON_ALIASES,
  PERSON_FOO_BAR,
  PERSON_IDENTIFIER,
  PERSON_NICKNAME,
  PERSON_SCORE,
  PERSON_SCORE_GTE,
  PERSON_BOOLEAN,
  PERSON_CONSTRAINED,
  PERSON_DATE,
  PERSON_DATETIME,
  PERSON_DATETIME_TZ,
  PERSON_DECIMAL,
  PERSON_DOUBLE,
  PERSON_DURATION,
  PERSON_FIELD_COUNT
};

typedef struct person_fields {
  type_bridge_projected_field_input_v1_t fields[PERSON_FIELD_COUNT + 1];
  const type_bridge_projected_value_t *slots[PERSON_FIELD_COUNT + 1][4];
} person_fields_t;

static void set_field(person_fields_t *inputs, size_t index,
                      const type_bridge_projected_token_v1_t *token,
                      const type_bridge_projected_value_t *value) {
  type_bridge_projected_field_input_v1_t *field = &inputs->fields[index];
  field->struct_size = sizeof(*field);
  field->version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
  field->field = token;
  inputs->slots[index][0] = value;
  field->values = inputs->slots[index];
  field->value_count = 1u;
}

static void init_person_fields(person_fields_t *inputs,
                               const person_values_t *values) {
  memset(inputs, 0, sizeof(*inputs));
  set_field(inputs, PERSON_ALIASES, &LOCAL_FIELD_PERSON_ALIASES,
            PROJECTED(values->analyst));
  inputs->slots[PERSON_ALIASES][1] = PROJECTED(values->mathematician);
  inputs->fields[PERSON_ALIASES].value_count = 2u;
  set_field(inputs, PERSON_FOO_BAR, &LOCAL_FIELD_PERSON_FOO_BAR,
            PROJECTED(values->foo_bar));
  set_field(inputs, PERSON_IDENTIFIER, &LOCAL_FIELD_PERSON_IDENTIFIER,
            PROJECTED(values->identifier));
  set_field(inputs, PERSON_NICKNAME, &LOCAL_FIELD_PERSON_NICKNAME,
            PROJECTED(values->nickname));
  set_field(inputs, PERSON_SCORE, &LOCAL_FIELD_PERSON_SCORE,
            PROJECTED(values->score));
  set_field(inputs, PERSON_SCORE_GTE, &LOCAL_FIELD_PERSON_SCORE_GTE,
            PROJECTED(values->score_gte));
  set_field(inputs, PERSON_BOOLEAN, &LOCAL_FIELD_PERSON_VAL_BOOL,
            PROJECTED(values->boolean_value));
  set_field(inputs, PERSON_CONSTRAINED, &LOCAL_FIELD_PERSON_VAL_CONSTRAINED,
            PROJECTED(values->constrained));
  set_field(inputs, PERSON_DATE, &LOCAL_FIELD_PERSON_VAL_DATE,
            PROJECTED(values->date));
  set_field(inputs, PERSON_DATETIME, &LOCAL_FIELD_PERSON_VAL_DATETIME,
            PROJECTED(values->datetime));
  set_field(inputs, PERSON_DATETIME_TZ, &LOCAL_FIELD_PERSON_VAL_DATETIME_TZ,
            PROJECTED(values->datetime_tz));
  set_field(inputs, PERSON_DECIMAL, &LOCAL_FIELD_PERSON_VAL_DECIMAL,
            PROJECTED(values->decimal));
  set_field(inputs, PERSON_DOUBLE, &LOCAL_FIELD_PERSON_VAL_DOUBLE,
            PROJECTED(values->double_value));
  set_field(inputs, PERSON_DURATION, &LOCAL_FIELD_PERSON_VAL_DURATION,
            PROJECTED(values->duration));
}

static int projected_text_is(const type_bridge_projected_value_t *value,
                             type_bridge_byte_view_t expected) {
  type_bridge_byte_view_t actual = {NULL, 0u};
  return type_bridge_projected_value_text(value, &actual) ==
             TYPE_BRIDGE_STATUS_OK &&
         same_view(actual, expected);
}

static int create_text_is(const phase2_person_create *create,
                          const type_bridge_projected_token_v1_t *field,
                          size_t index, type_bridge_byte_view_t expected) {
  type_bridge_projected_value_t *value = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  int matched =
      type_bridge_projected_create_field_value_at(
          (const type_bridge_projected_create_t *)create, field, index, &value,
          &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
      value != NULL && diagnostics == NULL && projected_text_is(value, expected);
  if (value != NULL && type_bridge_projected_value_close(&value) !=
                           TYPE_BRIDGE_STATUS_OK) {
    return 0;
  }
  return matched;
}

static int create_long_is(const phase2_person_create *create,
                          const type_bridge_projected_token_v1_t *field,
                          int64_t expected) {
  type_bridge_projected_value_t *value = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  int64_t actual = 0;
  int matched =
      type_bridge_projected_create_field_value_at(
          (const type_bridge_projected_create_t *)create, field, 0u, &value,
          &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
      value != NULL && diagnostics == NULL &&
      type_bridge_projected_value_long(value, &actual) ==
          TYPE_BRIDGE_STATUS_OK &&
      actual == expected;
  if (value != NULL && type_bridge_projected_value_close(&value) !=
                           TYPE_BRIDGE_STATUS_OK) {
    return 0;
  }
  return matched;
}

static int create_scalar_values_are_exact(const phase2_person_create *create) {
  type_bridge_projected_value_t *value = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  uint8_t boolean_value = 1u;
  uint64_t double_bits = 0u;
  REQUIRE(create_text_is(create, &LOCAL_FIELD_PERSON_ALIASES, 0u,
                         VIEW("analyst")));
  REQUIRE(create_text_is(create, &LOCAL_FIELD_PERSON_ALIASES, 1u,
                         VIEW("mathematician")));
  REQUIRE(create_long_is(create, &LOCAL_FIELD_PERSON_FOO_BAR, 7));
  REQUIRE(create_text_is(create, &LOCAL_FIELD_PERSON_IDENTIFIER, 0u,
                         VIEW("data-ada")));
  REQUIRE(create_text_is(create, &LOCAL_FIELD_PERSON_NICKNAME, 0u,
                         VIEW("Ada")));
  REQUIRE(create_long_is(create, &LOCAL_FIELD_PERSON_SCORE, 38));
  REQUIRE(create_long_is(create, &LOCAL_FIELD_PERSON_SCORE_GTE, 40));
  REQUIRE(create_long_is(create, &LOCAL_FIELD_PERSON_VAL_CONSTRAINED, 38));
  REQUIRE(create_text_is(create, &LOCAL_FIELD_PERSON_VAL_DATE, 0u,
                         VIEW("2026-08-12")));
  REQUIRE(create_text_is(create, &LOCAL_FIELD_PERSON_VAL_DATETIME, 0u,
                         VIEW("2026-08-12T09:30:00")));
  REQUIRE(create_text_is(create, &LOCAL_FIELD_PERSON_VAL_DATETIME_TZ, 0u,
                         VIEW("2026-08-12T09:30:00Z")));
  REQUIRE(create_text_is(create, &LOCAL_FIELD_PERSON_VAL_DECIMAL, 0u,
                         VIEW("38.5")));
  REQUIRE(create_text_is(create, &LOCAL_FIELD_PERSON_VAL_DURATION, 0u,
                         VIEW("PT38S")));
  REQUIRE(type_bridge_projected_create_field_value_at(
              (const type_bridge_projected_create_t *)create,
              &LOCAL_FIELD_PERSON_VAL_BOOL, 0u, &value, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          value != NULL && diagnostics == NULL &&
          type_bridge_projected_value_boolean(value, &boolean_value) ==
              TYPE_BRIDGE_STATUS_OK &&
          boolean_value == 0u);
  REQUIRE(type_bridge_projected_value_close(&value) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(type_bridge_projected_create_field_value_at(
              (const type_bridge_projected_create_t *)create,
              &LOCAL_FIELD_PERSON_VAL_DOUBLE, 0u, &value, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          value != NULL && diagnostics == NULL &&
          type_bridge_projected_value_double_bits(value, &double_bits) ==
              TYPE_BRIDGE_STATUS_OK &&
          double_bits == UINT64_C(0x4043000000000000));
  REQUIRE(type_bridge_projected_value_close(&value) == TYPE_BRIDGE_STATUS_OK);
  return 1;
}

static int hydrated_scalar_values_are_exact(const phase2_person *person) {
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  phase2_aliases *alias = NULL;
  phase2_foozuzubar *foo_bar = NULL;
  phase2_identifier *identifier = NULL;
  phase2_nickname *nickname = NULL;
  phase2_score *score = NULL;
  phase2_scorezuzugte *score_gte = NULL;
  phase2_valzubool *boolean_value = NULL;
  phase2_valzuconstrained *constrained = NULL;
  phase2_valzudate *date = NULL;
  phase2_valzudatetime *datetime = NULL;
  phase2_valzudatetimezutzz *datetime_tz = NULL;
  phase2_valzudecimal *decimal = NULL;
  phase2_valzudouble *double_value = NULL;
  phase2_valzuduration *duration = NULL;
  type_bridge_byte_view_t text = {NULL, 0u};
  size_t count = 0u;
  int64_t integer = 0;
  uint64_t bits = 0u;
  uint8_t boolean = 1u;

  REQUIRE(phase2_person_aliases_count(person, &count, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          count == 2u && diagnostics == NULL);
  REQUIRE(phase2_person_aliases_at(person, 0u, &alias, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_aliases_value(alias, &text, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          same_view(text, VIEW("analyst")) &&
          phase2_aliases_close(&alias) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_aliases_at(person, 1u, &alias, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_aliases_value(alias, &text, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          same_view(text, VIEW("mathematician")) &&
          phase2_aliases_close(&alias) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_foozuzubar(person, &foo_bar, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_foozuzubar_value(foo_bar, &integer, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          integer == 7 &&
          phase2_foozuzubar_close(&foo_bar) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_identifier(person, &identifier, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_identifier_value(identifier, &text, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          same_view(text, VIEW("data-ada")) &&
          phase2_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_nickname(person, &nickname, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_nickname_value(nickname, &text, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          same_view(text, VIEW("Ada")) &&
          phase2_nickname_close(&nickname) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_score(person, &score, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_score_value(score, &integer, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          integer == 38 &&
          phase2_score_close(&score) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_scorezuzugte(person, &score_gte, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_scorezuzugte_value(score_gte, &integer, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          integer == 40 &&
          phase2_scorezuzugte_close(&score_gte) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_valzubool(person, &boolean_value, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_valzubool_value(boolean_value, &boolean, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          boolean == 0u &&
          phase2_valzubool_close(&boolean_value) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_valzuconstrained(person, &constrained, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_valzuconstrained_value(constrained, &integer, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          integer == 38 &&
          phase2_valzuconstrained_close(&constrained) ==
              TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_valzudate(person, &date, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_valzudate_value(date, &text, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          same_view(text, VIEW("2026-08-12")) &&
          phase2_valzudate_close(&date) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_valzudatetime(person, &datetime, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_valzudatetime_value(datetime, &text, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          same_view(text, VIEW("2026-08-12T09:30:00")) &&
          phase2_valzudatetime_close(&datetime) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_valzudatetimezutzz(person, &datetime_tz,
                                           &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_valzudatetimezutzz_value(datetime_tz, &text, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          same_view(text, VIEW("2026-08-12T09:30:00Z")) &&
          phase2_valzudatetimezutzz_close(&datetime_tz) ==
              TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_valzudecimal(person, &decimal, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_valzudecimal_value(decimal, &text, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          same_view(text, VIEW("38.5")) &&
          phase2_valzudecimal_close(&decimal) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_valzudouble(person, &double_value, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_valzudouble_value(double_value, &bits, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          bits == UINT64_C(0x4043000000000000) &&
          phase2_valzudouble_close(&double_value) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_valzuduration(person, &duration, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_valzuduration_value(duration, &text, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          same_view(text, VIEW("PT38S")) &&
          phase2_valzuduration_close(&duration) == TYPE_BRIDGE_STATUS_OK);
  return diagnostics == NULL;
}

static int run_constraint_rejections(
    const type_bridge_schema_package_t *package, const person_values_t *values,
    const person_fields_t *valid_fields) {
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_projected_create_t *create = NULL;
  type_bridge_projected_value_t *wrong_domain = NULL;
  phase2_nickname *nickname = NULL;
  phase2_valzuconstrained *too_large = NULL;
  phase2_aliases *fourth_alias = NULL;
  type_bridge_projected_create_descriptor_v1_t descriptor = {0};
  type_bridge_projected_field_input_v1_t fields[PERSON_FIELD_COUNT + 1];
  const type_bridge_projected_value_t *values_slot[4] = {0};

  descriptor.struct_size = sizeof(descriptor);
  descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
  descriptor.model = &LOCAL_MODEL_ACTOR;
  create = (type_bridge_projected_create_t *)(uintptr_t)1u;
  REQUIRE(type_bridge_projected_create_open_v1(package, &descriptor, &create,
                                               &diagnostics) ==
              TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
          create == NULL &&
          diagnostic_is(diagnostics,
                        TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                        "model_not_constructible") &&
          close_diagnostics(&diagnostics));
  descriptor.model = &LOCAL_MODEL_COUNTER;
  create = (type_bridge_projected_create_t *)(uintptr_t)1u;
  REQUIRE(type_bridge_projected_create_open_v1(package, &descriptor, &create,
                                               &diagnostics) ==
              TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
          create == NULL &&
          diagnostic_is(diagnostics,
                        TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                        "missing_required_field") &&
          close_diagnostics(&diagnostics));
  descriptor.model = &LOCAL_MODEL_PLAIN_ACTIVITY;
  create = (type_bridge_projected_create_t *)(uintptr_t)1u;
  REQUIRE(type_bridge_projected_create_open_v1(package, &descriptor, &create,
                                               &diagnostics) ==
              TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
          create == NULL &&
          diagnostic_is(diagnostics,
                        TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                        "missing_required_role") &&
          close_diagnostics(&diagnostics));

  nickname = (phase2_nickname *)(uintptr_t)1u;
  {
    type_bridge_status_t status = phase2_nickname_open(
        package, VIEW("Grace"), &nickname, &diagnostics);
    REQUIRE(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT && nickname == NULL &&
            diagnostic_is(diagnostics,
                          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                          "values_constraint_violation") &&
            close_diagnostics(&diagnostics));
  }
  nickname = (phase2_nickname *)(uintptr_t)1u;
  REQUIRE(phase2_nickname_open(package, VIEW("ada"), &nickname,
                               &diagnostics) ==
              TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
          nickname == NULL &&
          diagnostic_is(diagnostics,
                        TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                        "regex_constraint_violation") &&
          close_diagnostics(&diagnostics));
  too_large = (phase2_valzuconstrained *)(uintptr_t)1u;
  REQUIRE(phase2_valzuconstrained_open(package, 81, &too_large,
                                       &diagnostics) ==
              TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
          too_large == NULL && range_diagnostic_is_exact(diagnostics) &&
          close_diagnostics(&diagnostics));
  REQUIRE(type_bridge_projected_value_string_open(
              package, &LOCAL_MODEL_SCORE, VIEW("38"), &wrong_domain,
              &diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
          wrong_domain == NULL &&
          diagnostic_is(diagnostics,
                        TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                        "wrong_scalar_domain") &&
          close_diagnostics(&diagnostics));

  descriptor.model = &LOCAL_MODEL_PERSON;
  descriptor.fields = fields;
  memcpy(fields, valid_fields->fields, PERSON_FIELD_COUNT * sizeof(fields[0]));
  memmove(&fields[PERSON_IDENTIFIER], &fields[PERSON_IDENTIFIER + 1],
          (PERSON_FIELD_COUNT - PERSON_IDENTIFIER - 1u) * sizeof(fields[0]));
  descriptor.field_count = PERSON_FIELD_COUNT - 1u;
  create = (type_bridge_projected_create_t *)(uintptr_t)1u;
  REQUIRE(type_bridge_projected_create_open_v1(package, &descriptor, &create,
                                               &diagnostics) ==
              TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
          create == NULL &&
          diagnostic_is(diagnostics,
                        TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                        "missing_required_field") &&
          close_diagnostics(&diagnostics));

  memcpy(fields, valid_fields->fields, PERSON_FIELD_COUNT * sizeof(fields[0]));
  fields[PERSON_SCORE].values = values_slot;
  values_slot[0] = PROJECTED(values->score);
  values_slot[1] = PROJECTED(values->constrained);
  fields[PERSON_SCORE].value_count = 2u;
  descriptor.field_count = PERSON_FIELD_COUNT;
  create = (type_bridge_projected_create_t *)(uintptr_t)1u;
  REQUIRE(type_bridge_projected_create_open_v1(package, &descriptor, &create,
                                               &diagnostics) ==
              TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
          create == NULL &&
          diagnostic_is(diagnostics,
                        TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                        "field_cardinality_violation") &&
          close_diagnostics(&diagnostics));

  REQUIRE(phase2_aliases_open(package, VIEW("programmer"), &fourth_alias,
                              &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          fourth_alias != NULL && diagnostics == NULL);
  memcpy(fields, valid_fields->fields, PERSON_FIELD_COUNT * sizeof(fields[0]));
  values_slot[0] = PROJECTED(values->analyst);
  values_slot[1] = PROJECTED(values->mathematician);
  values_slot[2] = PROJECTED(values->foo_bar);
  values_slot[3] = PROJECTED(fourth_alias);
  fields[PERSON_ALIASES].values = values_slot;
  fields[PERSON_ALIASES].value_count = 4u;
  create = (type_bridge_projected_create_t *)(uintptr_t)1u;
  REQUIRE(type_bridge_projected_create_open_v1(package, &descriptor, &create,
                                               &diagnostics) ==
              TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
          create == NULL &&
          diagnostic_is(diagnostics,
                        TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                        "field_cardinality_violation") &&
          close_diagnostics(&diagnostics));
  REQUIRE(phase2_aliases_close(&fourth_alias) == TYPE_BRIDGE_STATUS_OK);

  memcpy(fields, valid_fields->fields, PERSON_FIELD_COUNT * sizeof(fields[0]));
  values_slot[0] = PROJECTED(values->analyst);
  values_slot[1] = PROJECTED(values->analyst);
  fields[PERSON_ALIASES].values = values_slot;
  fields[PERSON_ALIASES].value_count = 2u;
  create = (type_bridge_projected_create_t *)(uintptr_t)1u;
  REQUIRE(type_bridge_projected_create_open_v1(package, &descriptor, &create,
                                               &diagnostics) ==
              TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
          create == NULL &&
          diagnostic_is(diagnostics,
                        TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                        "ordered_distinct_duplicate") &&
          diagnostic_count_detail(diagnostics, "first_index", 0u) &&
          diagnostic_count_detail(diagnostics, "duplicate_index", 1u) &&
          close_diagnostics(&diagnostics));

  memcpy(fields, valid_fields->fields, PERSON_FIELD_COUNT * sizeof(fields[0]));
  fields[PERSON_NICKNAME].values = values_slot;
  values_slot[0] = PROJECTED(values->score);
  fields[PERSON_NICKNAME].value_count = 1u;
  create = (type_bridge_projected_create_t *)(uintptr_t)1u;
  {
    type_bridge_status_t status = type_bridge_projected_create_open_v1(
        package, &descriptor, &create, &diagnostics);
    REQUIRE(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT && create == NULL &&
            diagnostic_is(diagnostics,
                          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                          "field_value_attribute_mismatch") &&
            close_diagnostics(&diagnostics));
  }

  {
    type_bridge_projected_field_input_v1_t field = {0};
    const type_bridge_projected_value_t *field_values[] = {
        PROJECTED(values->identifier)};
    field.struct_size = sizeof(field);
    field.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    field.field = &LOCAL_FIELD_PARTY_PARTY_NAME;
    field.values = field_values;
    field.value_count = 1u;
    descriptor.fields = &field;
    descriptor.field_count = 1u;
    create = (type_bridge_projected_create_t *)(uintptr_t)1u;
    {
      type_bridge_status_t status = type_bridge_projected_create_open_v1(
          package, &descriptor, &create, &diagnostics);
      REQUIRE(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT && create == NULL &&
              diagnostic_is(diagnostics,
                            TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                            "c_projected_field_owner_mismatch") &&
              close_diagnostics(&diagnostics));
    }
  }
  return 1;
}

static int reference_iid_is(const type_bridge_projected_reference_t *reference,
                            type_bridge_byte_view_t expected) {
  type_bridge_byte_view_t actual = {NULL, 0u};
  return type_bridge_projected_reference_iid(reference, &actual) ==
             TYPE_BRIDGE_STATUS_OK &&
         same_view(actual, expected);
}

static int person_ref_key_is(
    const phase2_person_ref *person, type_bridge_byte_view_t expected,
    type_bridge_execution_diagnostics_t **diagnostics) {
  phase2_identifier *key = NULL;
  type_bridge_byte_view_t actual = {NULL, 0u};
  int matched = phase2_person_ref_identifier_key(person, &key, diagnostics) ==
                    TYPE_BRIDGE_STATUS_OK &&
                phase2_identifier_value(key, &actual, diagnostics) ==
                    TYPE_BRIDGE_STATUS_OK &&
                same_view(actual, expected);
  if (key != NULL &&
      phase2_identifier_close(&key) != TYPE_BRIDGE_STATUS_OK) {
    return 0;
  }
  return matched;
}

static type_bridge_status_t open_full_reference(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_token_v1_t *key_field,
    const type_bridge_projected_value_t *key, type_bridge_byte_view_t iid,
    type_bridge_projected_reference_t **reference,
    type_bridge_execution_diagnostics_t **diagnostics) {
  const type_bridge_projected_value_t *values[] = {key};
  type_bridge_projected_field_input_v1_t field = {0};
  type_bridge_projected_reference_descriptor_v1_t descriptor = {0};
  field.struct_size = sizeof(field);
  field.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
  field.field = key_field;
  field.values = values;
  field.value_count = 1u;
  descriptor.struct_size = sizeof(descriptor);
  descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
  descriptor.model = model;
  descriptor.iid = iid;
  descriptor.keys = &field;
  descriptor.key_count = 1u;
  return type_bridge_projected_reference_open_v1(
      package, &descriptor, reference, diagnostics);
}

static int create_role_iid_is(
    const type_bridge_projected_create_t *create,
    const type_bridge_projected_token_v1_t *role, size_t index,
    type_bridge_byte_view_t expected) {
  type_bridge_projected_reference_t *reference = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  int matched =
      type_bridge_projected_create_role_reference_at(
          create, role, index, &reference, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      reference != NULL && diagnostics == NULL &&
      reference_iid_is(reference, expected);
  if (reference != NULL && type_bridge_projected_reference_close(&reference) !=
                               TYPE_BRIDGE_STATUS_OK) {
    return 0;
  }
  return matched;
}

static int create_role_string_key_is(
    const type_bridge_projected_create_t *create,
    const type_bridge_projected_token_v1_t *role, size_t index,
    const type_bridge_projected_token_v1_t *key_field,
    type_bridge_byte_view_t expected) {
  type_bridge_projected_reference_t *reference = NULL;
  type_bridge_projected_value_t *key = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  int matched =
      type_bridge_projected_create_role_reference_at(
          create, role, index, &reference, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      type_bridge_projected_reference_key(reference, key_field, &key,
                                          &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK &&
      projected_text_is(key, expected);
  if (key != NULL &&
      type_bridge_projected_value_close(&key) != TYPE_BRIDGE_STATUS_OK) {
    return 0;
  }
  if (reference != NULL && type_bridge_projected_reference_close(&reference) !=
                               TYPE_BRIDGE_STATUS_OK) {
    return 0;
  }
  return matched && diagnostics == NULL;
}

static int interaction_hydration_is_exact(
    const type_bridge_schema_package_t *package,
    const phase2_identifier *identifier,
    const type_bridge_projected_reference_t *actor_reference,
    const type_bridge_projected_reference_t *target_reference,
    type_bridge_byte_view_t iid, int expected_actor_kind) {
  const type_bridge_projected_value_t *identifier_values[] = {
      PROJECTED(identifier)};
  const type_bridge_projected_reference_t *actor_references[] = {
      actor_reference};
  const type_bridge_projected_reference_t *target_references[] = {
      target_reference};
  type_bridge_projected_field_input_v1_t field = {0};
  type_bridge_projected_role_input_v1_t roles[2] = {0};
  type_bridge_projected_thing_descriptor_v1_t descriptor = {0};
  type_bridge_projected_thing_t *thing = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  phase2_interaction_actor_player *actor = NULL;
  int matched = 0;

  field.struct_size = sizeof(field);
  field.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
  field.field = &LOCAL_FIELD_INTERACTION_IDENTIFIER;
  field.values = identifier_values;
  field.value_count = 1u;
  roles[0].struct_size = sizeof(roles[0]);
  roles[0].version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
  roles[0].role = &LOCAL_ROLE_INTERACTION_TARGET;
  roles[0].references = target_references;
  roles[0].reference_count = 1u;
  roles[1].struct_size = sizeof(roles[1]);
  roles[1].version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
  roles[1].role = &LOCAL_ROLE_INTERACTION_ACTOR;
  roles[1].references = actor_references;
  roles[1].reference_count = actor_reference == NULL ? 0u : 1u;
  descriptor.struct_size = sizeof(descriptor);
  descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
  descriptor.model = &LOCAL_MODEL_INTERACTION;
  descriptor.iid = iid;
  descriptor.fields = &field;
  descriptor.field_count = 1u;
  descriptor.roles = roles;
  descriptor.role_count = actor_reference == NULL ? 1u : 2u;
  if (type_bridge_projected_thing_open_v1(package, &descriptor, &thing,
                                          &diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      thing == NULL || diagnostics != NULL ||
      phase2_interaction_actor((const phase2_interaction *)thing, &actor,
                               &diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    goto close;
  }
  if (expected_actor_kind == 0) {
    matched = actor == NULL;
  } else if (expected_actor_kind == 1) {
    phase2_person_ref *person = NULL;
    matched = actor != NULL &&
              phase2_interaction_actor_player_as_person(
                  actor, &person, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
              person_ref_key_is(person, VIEW("data-ada"), &diagnostics);
    if (person != NULL &&
        phase2_person_ref_close(&person) != TYPE_BRIDGE_STATUS_OK) {
      matched = 0;
    }
  } else if (expected_actor_kind == 2) {
    phase2_robot_ref *robot = NULL;
    phase2_robotzuid *key = NULL;
    int64_t value = 0;
    matched = actor != NULL &&
              phase2_interaction_actor_player_as_robot(
                  actor, &robot, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
              phase2_robot_ref_robotzuid_key(robot, &key, &diagnostics) ==
                  TYPE_BRIDGE_STATUS_OK &&
              phase2_robotzuid_value(key, &value, &diagnostics) ==
                  TYPE_BRIDGE_STATUS_OK &&
              value == 7;
    if (key != NULL && phase2_robotzuid_close(&key) != TYPE_BRIDGE_STATUS_OK) {
      matched = 0;
    }
    if (robot != NULL &&
        phase2_robot_ref_close(&robot) != TYPE_BRIDGE_STATUS_OK) {
      matched = 0;
    }
  }

close:
  if (actor != NULL &&
      phase2_interaction_actor_player_close(&actor) != TYPE_BRIDGE_STATUS_OK) {
    matched = 0;
  }
  if (thing != NULL &&
      type_bridge_projected_thing_close(&thing) != TYPE_BRIDGE_STATUS_OK) {
    matched = 0;
  }
  return matched && diagnostics == NULL;
}

static int run_role_and_package_evidence(
    const type_bridge_schema_package_t *package,
    const type_bridge_schema_package_t *foreign_package,
    const phase2_person *local_person, const person_values_t *person_values) {
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  phase2_person_ref *ada = NULL;
  phase2_person_ref *dana = NULL;
  phase2_identifier *dana_key = NULL;
  phase2_robotzuid *robot_key = NULL;
  phase2_robotzuid *negative_robot_key = NULL;
  phase2_robot_ref *robot = NULL;
  phase2_robot_ref *negative_robot = NULL;
  phase2_plainzhactivity_participant_player *plain_player = NULL;
  phase2_plainzhactivity_create *plain_create = NULL;
  type_bridge_projected_thing_t *plain_thing = NULL;
  phase2_networkzhlink_participant_player *ada_participant = NULL;
  phase2_networkzhlink_participant_player *dana_participant = NULL;
  phase2_networkzhlink_create *network_create = NULL;
  type_bridge_projected_thing_t *network_thing = NULL;
  phase2_interaction_target_player *target = NULL;
  phase2_interaction_actor_player *person_actor = NULL;
  phase2_interaction_actor_player *robot_actor = NULL;
  phase2_interaction_create *interaction_create = NULL;
  phase2_event_ref *event = NULL;
  phase2_container_item_player *event_item = NULL;
  phase2_container_create *container_create = NULL;
  type_bridge_projected_thing_t *container_thing = NULL;
  phase2_identifier *interaction_person_identifier = NULL;
  phase2_identifier *interaction_robot_identifier = NULL;
  phase2_identifier *interaction_absent_identifier = NULL;
  phase2_identifier *network_identifier = NULL;

  REQUIRE(open_full_reference(
              package, &LOCAL_MODEL_PERSON, &LOCAL_FIELD_PERSON_IDENTIFIER,
              PROJECTED(person_values->identifier), VIEW("0x01"),
              (type_bridge_projected_reference_t **)&ada, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          ada != NULL && diagnostics == NULL);
  REQUIRE(phase2_identifier_open(package, VIEW("data-dana"), &dana_key,
                                 &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          open_full_reference(
              package, &LOCAL_MODEL_PERSON, &LOCAL_FIELD_PERSON_IDENTIFIER,
              PROJECTED(dana_key), VIEW("0x02"),
              (type_bridge_projected_reference_t **)&dana, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
  {
    phase2_identifier *key = NULL;
    type_bridge_byte_view_t text = {NULL, 0u};
    REQUIRE(phase2_person_ref_identifier_key(ada, &key, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            phase2_identifier_value(key, &text, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            same_view(text, VIEW("data-ada")) &&
            phase2_identifier_close(&key) == TYPE_BRIDGE_STATUS_OK);
    REQUIRE(phase2_person_ref_identifier_key(dana, &key, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            phase2_identifier_value(key, &text, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            same_view(text, VIEW("data-dana")) &&
            phase2_identifier_close(&key) == TYPE_BRIDGE_STATUS_OK);
  }
  REQUIRE(phase2_robotzuid_open(package, 7, &robot_key, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          open_full_reference(
              package, &LOCAL_MODEL_ROBOT, &LOCAL_FIELD_ROBOT_ID,
              PROJECTED(robot_key), VIEW("0x07"),
              (type_bridge_projected_reference_t **)&robot, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_robotzuid_open(package, -7, &negative_robot_key,
                                &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          open_full_reference(
              package, &LOCAL_MODEL_ROBOT, &LOCAL_FIELD_ROBOT_ID,
              PROJECTED(negative_robot_key), VIEW("0x08"),
              (type_bridge_projected_reference_t **)&negative_robot,
              &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
  {
    type_bridge_projected_value_t *key = NULL;
    int64_t actual = 0;
    REQUIRE(type_bridge_projected_reference_key(
                REFERENCE(robot), &LOCAL_FIELD_ROBOT_ID, &key, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            type_bridge_projected_value_long(key, &actual) ==
                TYPE_BRIDGE_STATUS_OK &&
            actual == 7 &&
            type_bridge_projected_value_close(&key) == TYPE_BRIDGE_STATUS_OK);
    REQUIRE(type_bridge_projected_reference_key(
                REFERENCE(negative_robot), &LOCAL_FIELD_ROBOT_ID, &key,
                &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            type_bridge_projected_value_long(key, &actual) ==
                TYPE_BRIDGE_STATUS_OK &&
            actual == -7 &&
            type_bridge_projected_value_close(&key) == TYPE_BRIDGE_STATUS_OK);
  }

  {
    const type_bridge_projected_reference_t *robot_reference[] = {
        REFERENCE(robot)};
    const type_bridge_projected_reference_t *person_reference[] = {
        REFERENCE(ada)};
    type_bridge_projected_role_input_v1_t role = {0};
    type_bridge_projected_create_descriptor_v1_t descriptor = {0};
    type_bridge_projected_create_t *rejected =
        (type_bridge_projected_create_t *)(uintptr_t)1u;
    role.struct_size = sizeof(role);
    role.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    role.role = &LOCAL_ROLE_PLAIN_PARTICIPANT;
    role.references = robot_reference;
    role.reference_count = 1u;
    descriptor.struct_size = sizeof(descriptor);
    descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    descriptor.model = &LOCAL_MODEL_PLAIN_ACTIVITY;
    descriptor.roles = &role;
    descriptor.role_count = 1u;
    REQUIRE(type_bridge_projected_create_open_v1(
                package, &descriptor, &rejected, &diagnostics) ==
                TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
            rejected == NULL &&
            diagnostic_is(diagnostics,
                          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                          "role_player_not_accepted") &&
            close_diagnostics(&diagnostics));
    role.role = &LOCAL_ROLE_EVENT_SUBJECT;
    descriptor.model = &LOCAL_MODEL_EVENT;
    rejected = (type_bridge_projected_create_t *)(uintptr_t)1u;
    REQUIRE(type_bridge_projected_create_open_v1(
                package, &descriptor, &rejected, &diagnostics) ==
                TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
            rejected == NULL &&
            diagnostic_is(diagnostics,
                          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                          "role_player_not_accepted") &&
            close_diagnostics(&diagnostics));
    role.role = &LOCAL_ROLE_MEMBERSHIP_MEMBER;
    role.references = person_reference;
    descriptor.model = &LOCAL_MODEL_EMPLOYMENT;
    rejected = (type_bridge_projected_create_t *)(uintptr_t)1u;
    REQUIRE(type_bridge_projected_create_open_v1(
                package, &descriptor, &rejected, &diagnostics) ==
                TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
            rejected == NULL &&
            diagnostic_is(diagnostics,
                          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                          "c_projected_role_owner_mismatch") &&
            close_diagnostics(&diagnostics));
  }

  REQUIRE(phase2_plainzhactivity_participant_player_from_person(
              ada, &plain_player, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  {
    phase2_plainzhactivity_create_args_v1_t args = {0};
    args.struct_size = sizeof(args);
    args.version = PHASE2_CREATE_ARGS_VERSION;
    args.role_participant = plain_player;
    REQUIRE(phase2_plainzhactivity_create_open(
                package, &args, &plain_create, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            create_role_string_key_is(
                (const type_bridge_projected_create_t *)plain_create,
                &LOCAL_ROLE_PLAIN_PARTICIPANT, 0u,
                &LOCAL_FIELD_PERSON_IDENTIFIER, VIEW("data-ada")));
  }
  {
    const type_bridge_projected_reference_t *references[] = {REFERENCE(ada)};
    type_bridge_projected_role_input_v1_t role = {0};
    type_bridge_projected_thing_descriptor_v1_t descriptor = {0};
    role.struct_size = sizeof(role);
    role.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    role.role = &LOCAL_ROLE_PLAIN_PARTICIPANT;
    role.references = references;
    role.reference_count = 1u;
    descriptor.struct_size = sizeof(descriptor);
    descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    descriptor.model = &LOCAL_MODEL_PLAIN_ACTIVITY;
    descriptor.iid = VIEW("0x20");
    descriptor.roles = &role;
    descriptor.role_count = 1u;
    REQUIRE(type_bridge_projected_thing_open_v1(
                package, &descriptor, &plain_thing, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK);
  }
  {
    phase2_plainzhactivity_participant_player *hydrated = NULL;
    phase2_person_ref *person = NULL;
    REQUIRE(phase2_plainzhactivity_participant(
                (const phase2_plainzhactivity *)plain_thing, &hydrated,
                &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            phase2_plainzhactivity_participant_player_as_person(
                hydrated, &person, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            person_ref_key_is(person, VIEW("data-ada"), &diagnostics) &&
            phase2_person_ref_close(&person) == TYPE_BRIDGE_STATUS_OK &&
            phase2_plainzhactivity_participant_player_close(&hydrated) ==
                TYPE_BRIDGE_STATUS_OK);
  }

  REQUIRE(phase2_networkzhlink_participant_player_from_person(
              ada, &ada_participant, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          phase2_networkzhlink_participant_player_from_person(
              dana, &dana_participant, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_identifier_open(package, VIEW("data-link-forward"),
                                 &network_identifier, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  {
    const phase2_networkzhlink_participant_player *players[] = {
        ada_participant, dana_participant};
    phase2_networkzhlink_create_role_participant_chunks_v1_t chunk = {0};
    phase2_networkzhlink_create_args_v1_t args = {0};
    phase2_networkzhlink_origin_player *origin = NULL;
    phase2_networkzhlink_destination_player *destination = NULL;
    REQUIRE(phase2_networkzhlink_origin_player_from_person(
                ada, &origin, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            phase2_networkzhlink_destination_player_from_person(
                dana, &destination, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    chunk.struct_size = sizeof(chunk);
    chunk.version = PHASE2_CREATE_ARGS_VERSION;
    chunk.values = players;
    chunk.count = 2u;
    args.struct_size = sizeof(args);
    args.version = PHASE2_CREATE_ARGS_VERSION;
    args.field_identifier = network_identifier;
    args.role_origin = origin;
    args.role_destination = destination;
    args.role_participant_chunks = &chunk;
    REQUIRE(phase2_networkzhlink_create_open(
                package, &args, &network_create, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            create_role_string_key_is(
                (const type_bridge_projected_create_t *)network_create,
                &LOCAL_ROLE_NETWORK_PARTICIPANT, 0u,
                &LOCAL_FIELD_PERSON_IDENTIFIER, VIEW("data-ada")) &&
            create_role_string_key_is(
                (const type_bridge_projected_create_t *)network_create,
                &LOCAL_ROLE_NETWORK_PARTICIPANT, 1u,
                &LOCAL_FIELD_PERSON_IDENTIFIER, VIEW("data-dana")) &&
            phase2_networkzhlink_destination_player_close(&destination) ==
                TYPE_BRIDGE_STATUS_OK &&
            phase2_networkzhlink_origin_player_close(&origin) ==
                TYPE_BRIDGE_STATUS_OK);
  }
  {
    const type_bridge_projected_value_t *identifier_values[] = {
        PROJECTED(network_identifier)};
    const type_bridge_projected_reference_t *participants[] = {REFERENCE(ada),
                                                                REFERENCE(dana)};
    const type_bridge_projected_reference_t *origin[] = {REFERENCE(ada)};
    const type_bridge_projected_reference_t *destination[] = {REFERENCE(dana)};
    type_bridge_projected_field_input_v1_t field = {0};
    type_bridge_projected_role_input_v1_t roles[3] = {0};
    type_bridge_projected_thing_descriptor_v1_t descriptor = {0};
    field.struct_size = sizeof(field);
    field.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    field.field = &LOCAL_FIELD_NETWORK_IDENTIFIER;
    field.values = identifier_values;
    field.value_count = 1u;
    roles[0].struct_size = sizeof(roles[0]);
    roles[0].version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    roles[0].role = &LOCAL_ROLE_NETWORK_ORIGIN;
    roles[0].references = origin;
    roles[0].reference_count = 1u;
    roles[1].struct_size = sizeof(roles[1]);
    roles[1].version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    roles[1].role = &LOCAL_ROLE_NETWORK_DESTINATION;
    roles[1].references = destination;
    roles[1].reference_count = 1u;
    roles[2].struct_size = sizeof(roles[2]);
    roles[2].version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    roles[2].role = &LOCAL_ROLE_NETWORK_PARTICIPANT;
    roles[2].references = participants;
    roles[2].reference_count = 2u;
    descriptor.struct_size = sizeof(descriptor);
    descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    descriptor.model = &LOCAL_MODEL_NETWORK_LINK;
    descriptor.iid = VIEW("0x30");
    descriptor.fields = &field;
    descriptor.field_count = 1u;
    descriptor.roles = roles;
    descriptor.role_count = 3u;
    REQUIRE(type_bridge_projected_thing_open_v1(
                package, &descriptor, &network_thing, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK);
  }
  {
    size_t count = 0u;
    phase2_networkzhlink_participant_player *player = NULL;
    phase2_person_ref *person = NULL;
    REQUIRE(phase2_networkzhlink_participant_count(
                (const phase2_networkzhlink *)network_thing, &count,
                &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            count == 2u);
    REQUIRE(phase2_networkzhlink_participant_at(
                (const phase2_networkzhlink *)network_thing, 0u, &player,
                &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            phase2_networkzhlink_participant_player_as_person(
                player, &person, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            person_ref_key_is(person, VIEW("data-ada"), &diagnostics) &&
            phase2_person_ref_close(&person) == TYPE_BRIDGE_STATUS_OK &&
            phase2_networkzhlink_participant_player_close(&player) ==
                TYPE_BRIDGE_STATUS_OK);
    REQUIRE(phase2_networkzhlink_participant_at(
                (const phase2_networkzhlink *)network_thing, 1u, &player,
                &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            phase2_networkzhlink_participant_player_as_person(
                player, &person, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            person_ref_key_is(person, VIEW("data-dana"), &diagnostics) &&
            phase2_person_ref_close(&person) == TYPE_BRIDGE_STATUS_OK &&
            phase2_networkzhlink_participant_player_close(&player) ==
                TYPE_BRIDGE_STATUS_OK);
  }
  {
    const phase2_networkzhlink_participant_player *duplicates[] = {
        ada_participant, ada_participant};
    phase2_networkzhlink_create_role_participant_chunks_v1_t chunk = {0};
    phase2_networkzhlink_create_args_v1_t args = {0};
    phase2_networkzhlink_create *duplicate =
        (phase2_networkzhlink_create *)(uintptr_t)1u;
    phase2_networkzhlink_origin_player *origin = NULL;
    phase2_networkzhlink_destination_player *destination = NULL;
    REQUIRE(phase2_networkzhlink_origin_player_from_person(
                ada, &origin, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            phase2_networkzhlink_destination_player_from_person(
                dana, &destination, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    chunk.struct_size = sizeof(chunk);
    chunk.version = PHASE2_CREATE_ARGS_VERSION;
    chunk.values = duplicates;
    chunk.count = 2u;
    args.struct_size = sizeof(args);
    args.version = PHASE2_CREATE_ARGS_VERSION;
    args.field_identifier = network_identifier;
    args.role_origin = origin;
    args.role_destination = destination;
    args.role_participant_chunks = &chunk;
    REQUIRE(phase2_networkzhlink_create_open(
                package, &args, &duplicate, &diagnostics) ==
                TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
            duplicate == NULL &&
            diagnostic_is(diagnostics,
                          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                          "ordered_distinct_duplicate") &&
            diagnostic_count_detail(diagnostics, "first_index", 0u) &&
            diagnostic_count_detail(diagnostics, "duplicate_index", 1u) &&
            close_diagnostics(&diagnostics) &&
            phase2_networkzhlink_destination_player_close(&destination) ==
                TYPE_BRIDGE_STATUS_OK &&
            phase2_networkzhlink_origin_player_close(&origin) ==
                TYPE_BRIDGE_STATUS_OK);
  }

  REQUIRE(phase2_interaction_target_player_from_person(
              dana, &target, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          phase2_interaction_actor_player_from_person(
              ada, &person_actor, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          phase2_interaction_actor_player_from_robot(
              robot, &robot_actor, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          phase2_identifier_open(package, VIEW("interaction-person"),
                                 &interaction_person_identifier,
                                 &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          phase2_identifier_open(package, VIEW("interaction-robot"),
                                 &interaction_robot_identifier,
                                 &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          phase2_identifier_open(package, VIEW("interaction-absent"),
                                 &interaction_absent_identifier,
                                 &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
  {
    phase2_interaction_create_args_v1_t args = {0};
    args.struct_size = sizeof(args);
    args.version = PHASE2_CREATE_ARGS_VERSION;
    args.field_identifier = interaction_person_identifier;
    args.role_actor = person_actor;
    args.role_target = target;
    REQUIRE(phase2_interaction_create_open(
                package, &args, &interaction_create, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            create_role_string_key_is(
                (const type_bridge_projected_create_t *)interaction_create,
                &LOCAL_ROLE_INTERACTION_ACTOR, 0u,
                &LOCAL_FIELD_PERSON_IDENTIFIER, VIEW("data-ada")));
    REQUIRE(phase2_interaction_create_close(&interaction_create) ==
            TYPE_BRIDGE_STATUS_OK);
    args.field_identifier = interaction_robot_identifier;
    args.role_actor = robot_actor;
    REQUIRE(phase2_interaction_create_open(
                package, &args, &interaction_create, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK);
    {
      type_bridge_projected_reference_t *reference = NULL;
      type_bridge_projected_value_t *key = NULL;
      int64_t actual = 0;
      REQUIRE(type_bridge_projected_create_role_reference_at(
                  (const type_bridge_projected_create_t *)interaction_create,
                  &LOCAL_ROLE_INTERACTION_ACTOR, 0u, &reference,
                  &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
              type_bridge_projected_reference_key(
                  reference, &LOCAL_FIELD_ROBOT_ID, &key, &diagnostics) ==
                  TYPE_BRIDGE_STATUS_OK &&
              type_bridge_projected_value_long(key, &actual) ==
                  TYPE_BRIDGE_STATUS_OK &&
              actual == 7 &&
              type_bridge_projected_value_close(&key) ==
                  TYPE_BRIDGE_STATUS_OK &&
              type_bridge_projected_reference_close(&reference) ==
                  TYPE_BRIDGE_STATUS_OK);
    }
    REQUIRE(phase2_interaction_create_close(&interaction_create) ==
            TYPE_BRIDGE_STATUS_OK);
    args.field_identifier = interaction_absent_identifier;
    args.role_actor = NULL;
    REQUIRE(phase2_interaction_create_open(
                package, &args, &interaction_create, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK);
    {
      size_t count = 1u;
      REQUIRE(type_bridge_projected_create_role_count(
                  (const type_bridge_projected_create_t *)interaction_create,
                  &LOCAL_ROLE_INTERACTION_ACTOR, &count, &diagnostics) ==
                  TYPE_BRIDGE_STATUS_OK &&
              count == 0u);
    }
  }
  REQUIRE(interaction_hydration_is_exact(
              package, interaction_person_identifier, REFERENCE(ada),
              REFERENCE(dana), VIEW("0x40"), 1) &&
          interaction_hydration_is_exact(
              package, interaction_robot_identifier, REFERENCE(robot),
              REFERENCE(dana), VIEW("0x41"), 2) &&
          interaction_hydration_is_exact(
              package, interaction_absent_identifier, NULL, REFERENCE(dana),
              VIEW("0x42"), 0));

  REQUIRE(phase2_event_ref_from_iid(package, VIEW("0x50"), &event,
                                    &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
          phase2_container_item_player_from_event(
              event, &event_item, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  {
    const phase2_container_item_player *items[] = {event_item};
    phase2_container_create_role_item_chunks_v1_t chunk = {0};
    phase2_container_create_args_v1_t args = {0};
    chunk.struct_size = sizeof(chunk);
    chunk.version = PHASE2_CREATE_ARGS_VERSION;
    chunk.values = items;
    chunk.count = 1u;
    args.struct_size = sizeof(args);
    args.version = PHASE2_CREATE_ARGS_VERSION;
    args.role_item_chunks = &chunk;
    REQUIRE(phase2_container_create_open(
                package, &args, &container_create, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            create_role_iid_is(
                (const type_bridge_projected_create_t *)container_create,
                &LOCAL_ROLE_CONTAINER_ITEM, 0u, VIEW("0x50")));
    {
      const phase2_container_item_player *too_many[] = {event_item, event_item,
                                                         event_item};
      phase2_container_create *rejected =
          (phase2_container_create *)(uintptr_t)1u;
      chunk.values = too_many;
      chunk.count = 3u;
      REQUIRE(phase2_container_create_open(
                  package, &args, &rejected, &diagnostics) ==
                  TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
              rejected == NULL &&
              diagnostic_is(diagnostics,
                            TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                            "role_cardinality_violation") &&
              close_diagnostics(&diagnostics));
    }
  }
  {
    const type_bridge_projected_reference_t *references[] = {REFERENCE(event)};
    type_bridge_projected_role_input_v1_t role = {0};
    type_bridge_projected_thing_descriptor_v1_t descriptor = {0};
    size_t count = 0u;
    phase2_container_item_player *item = NULL;
    phase2_event_ref *hydrated_event = NULL;
    role.struct_size = sizeof(role);
    role.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    role.role = &LOCAL_ROLE_CONTAINER_ITEM;
    role.references = references;
    role.reference_count = 1u;
    descriptor.struct_size = sizeof(descriptor);
    descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    descriptor.model = &LOCAL_MODEL_CONTAINER;
    descriptor.iid = VIEW("0x51");
    descriptor.roles = &role;
    descriptor.role_count = 1u;
    REQUIRE(type_bridge_projected_thing_open_v1(
                package, &descriptor, &container_thing, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            phase2_container_item_count(
                (const phase2_container *)container_thing, &count,
                &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            count == 1u &&
            phase2_container_item_at(
                (const phase2_container *)container_thing, 0u, &item,
                &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            phase2_container_item_player_as_event(
                item, &hydrated_event, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            reference_iid_is(REFERENCE(hydrated_event), VIEW("0x50")) &&
            phase2_event_ref_close(&hydrated_event) == TYPE_BRIDGE_STATUS_OK &&
            phase2_container_item_player_close(&item) ==
                TYPE_BRIDGE_STATUS_OK);
  }

  {
    phase2_foreign_identifier *foreign_key = NULL;
    phase2_foreign_person_ref *foreign_person = NULL;
    phase2_plainzhactivity_participant_player *rejected =
        (phase2_plainzhactivity_participant_player *)(uintptr_t)1u;
    size_t count = SIZE_MAX;
    REQUIRE(phase2_foreign_identifier_open(
                foreign_package, VIEW("foreign-person"), &foreign_key,
                &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
            phase2_foreign_person_ref_from_key(
                foreign_package, foreign_key, &foreign_person,
                &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    REQUIRE(phase2_plainzhactivity_participant_player_from_person(
                (const phase2_person_ref *)foreign_person, &rejected,
                &diagnostics) == TYPE_BRIDGE_STATUS_EXECUTION_FAILED &&
            rejected == NULL &&
            diagnostic_is(diagnostics,
                          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INTEGRITY,
                          "generated_token_package_mismatch") &&
            close_diagnostics(&diagnostics));
    REQUIRE(phase2_foreign_person_aliases_count(
                (const phase2_foreign_person *)local_person, &count,
                &diagnostics) == TYPE_BRIDGE_STATUS_EXECUTION_FAILED &&
            count == 0u &&
            diagnostic_is(diagnostics,
                          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INTEGRITY,
                          "generated_token_package_mismatch") &&
            close_diagnostics(&diagnostics));
    REQUIRE(phase2_foreign_person_ref_close(&foreign_person) ==
                TYPE_BRIDGE_STATUS_OK &&
            phase2_foreign_identifier_close(&foreign_key) ==
                TYPE_BRIDGE_STATUS_OK);
  }

  REQUIRE(phase2_container_create_close(&container_create) ==
              TYPE_BRIDGE_STATUS_OK &&
          type_bridge_projected_thing_close(&container_thing) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_container_item_player_close(&event_item) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_event_ref_close(&event) == TYPE_BRIDGE_STATUS_OK &&
          phase2_interaction_create_close(&interaction_create) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_identifier_close(&interaction_absent_identifier) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_identifier_close(&interaction_robot_identifier) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_identifier_close(&interaction_person_identifier) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_interaction_actor_player_close(&robot_actor) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_interaction_actor_player_close(&person_actor) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_interaction_target_player_close(&target) ==
              TYPE_BRIDGE_STATUS_OK &&
          type_bridge_projected_thing_close(&network_thing) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_networkzhlink_create_close(&network_create) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_identifier_close(&network_identifier) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_networkzhlink_participant_player_close(&dana_participant) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_networkzhlink_participant_player_close(&ada_participant) ==
              TYPE_BRIDGE_STATUS_OK &&
          type_bridge_projected_thing_close(&plain_thing) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_plainzhactivity_create_close(&plain_create) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_plainzhactivity_participant_player_close(&plain_player) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_robot_ref_close(&negative_robot) == TYPE_BRIDGE_STATUS_OK &&
          phase2_robot_ref_close(&robot) == TYPE_BRIDGE_STATUS_OK &&
          phase2_robotzuid_close(&negative_robot_key) ==
              TYPE_BRIDGE_STATUS_OK &&
          phase2_robotzuid_close(&robot_key) == TYPE_BRIDGE_STATUS_OK &&
          phase2_person_ref_close(&dana) == TYPE_BRIDGE_STATUS_OK &&
          phase2_identifier_close(&dana_key) == TYPE_BRIDGE_STATUS_OK &&
          phase2_person_ref_close(&ada) == TYPE_BRIDGE_STATUS_OK &&
          diagnostics == NULL);
  return 1;
}

static int run_projection_evidence(void) {
  type_bridge_schema_package_t *package = NULL;
  type_bridge_schema_package_t *foreign_package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  person_values_t values;
  person_fields_t person_fields;
  phase2_person_create *person_create = NULL;
  type_bridge_projected_thing_t *person_thing = NULL;

  REQUIRE(phase2_schema_package_open(&package, &package_diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          package != NULL && package_diagnostics == NULL);
  REQUIRE(phase2_foreign_schema_package_open(&foreign_package,
                                             &package_diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          foreign_package != NULL && package_diagnostics == NULL);
  REQUIRE(open_person_values(package, &values, &diagnostics));
  REQUIRE(diagnostics == NULL);
  init_person_fields(&person_fields, &values);

  {
    const phase2_aliases *aliases[] = {values.analyst,
                                       values.mathematician};
    phase2_person_create_field_aliases_chunks_v1_t aliases_chunk = {0};
    phase2_person_create_args_v1_t args = {0};
    aliases_chunk.struct_size = sizeof(aliases_chunk);
    aliases_chunk.version = PHASE2_CREATE_ARGS_VERSION;
    aliases_chunk.values = aliases;
    aliases_chunk.count = 2u;
    args.struct_size = sizeof(args);
    args.version = PHASE2_CREATE_ARGS_VERSION;
    args.field_aliases_chunks = &aliases_chunk;
    args.field_foozuzubar = values.foo_bar;
    args.field_identifier = values.identifier;
    args.field_nickname = values.nickname;
    args.field_score = values.score;
    args.field_scorezuzugte = values.score_gte;
    args.field_valzubool = values.boolean_value;
    args.field_valzuconstrained = values.constrained;
    args.field_valzudate = values.date;
    args.field_valzudatetime = values.datetime;
    args.field_valzudatetimezutzz = values.datetime_tz;
    args.field_valzudecimal = values.decimal;
    args.field_valzudouble = values.double_value;
    args.field_valzuduration = values.duration;
    REQUIRE(phase2_person_create_open(package, &args, &person_create,
                                      &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK &&
            person_create != NULL && diagnostics == NULL);
  }
  REQUIRE(create_scalar_values_are_exact(person_create));

  {
    type_bridge_projected_thing_descriptor_v1_t descriptor = {0};
    type_bridge_status_t status;
    descriptor.struct_size = sizeof(descriptor);
    descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    descriptor.model = &LOCAL_MODEL_PERSON;
    descriptor.iid = VIEW("0x01");
    descriptor.fields = person_fields.fields;
    descriptor.field_count = PERSON_FIELD_COUNT;
    status = type_bridge_projected_thing_open_v1(
        package, &descriptor, &person_thing, &diagnostics);
    if (status != TYPE_BRIDGE_STATUS_OK) {
      type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
      if (diagnostics != NULL &&
          type_bridge_execution_diagnostics_get_v1(
              diagnostics, 0u, &diagnostic) == TYPE_BRIDGE_STATUS_OK) {
        fprintf(stderr, "hydration rejected with %.*s\n",
                (int)diagnostic.code.length, diagnostic.code.data);
      }
    }
    REQUIRE(status == TYPE_BRIDGE_STATUS_OK &&
            person_thing != NULL && diagnostics == NULL);
  }
  REQUIRE(hydrated_scalar_values_are_exact((const phase2_person *)person_thing));
  REQUIRE(run_constraint_rejections(package, &values, &person_fields));
  REQUIRE(run_role_and_package_evidence(
      package, foreign_package, (const phase2_person *)person_thing, &values));

  REQUIRE(type_bridge_projected_thing_close(&person_thing) ==
          TYPE_BRIDGE_STATUS_OK);
  REQUIRE(phase2_person_create_close(&person_create) == TYPE_BRIDGE_STATUS_OK);
  REQUIRE(close_person_values(&values));
  REQUIRE(type_bridge_schema_package_close(&foreign_package) ==
          TYPE_BRIDGE_STATUS_OK);
  REQUIRE(type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);
  return package == NULL && foreign_package == NULL && diagnostics == NULL;
}

int main(void) {
  CHECK(run_projection_evidence());
  puts("TYPE_BRIDGE_C_PHASE2_FACT\tcanonical_scalar_values\t{\"authored\":{\"boolean\":{\"kind\":\"boolean\",\"value\":false},\"date\":{\"kind\":\"date\",\"value\":\"2026-08-12\"},\"datetime\":{\"kind\":\"datetime\",\"value\":\"2026-08-12T09:30:00\"},\"datetime_tz\":{\"kind\":\"datetime_tz\",\"value\":\"2026-08-12T09:30:00Z\"},\"decimal\":{\"kind\":\"decimal\",\"value\":\"38.5\"},\"double\":{\"bits\":\"4043000000000000\",\"kind\":\"double\"},\"duration\":{\"kind\":\"duration\",\"value\":\"PT38S\"},\"long\":{\"kind\":\"long\",\"value\":\"38\"},\"string\":{\"kind\":\"string\",\"value\":\"Ada\"}},\"constructed\":{\"boolean\":{\"kind\":\"boolean\",\"value\":false},\"date\":{\"kind\":\"date\",\"value\":\"2026-08-12\"},\"datetime\":{\"kind\":\"datetime\",\"value\":\"2026-08-12T09:30:00\"},\"datetime_tz\":{\"kind\":\"datetime_tz\",\"value\":\"2026-08-12T09:30:00Z\"},\"decimal\":{\"kind\":\"decimal\",\"value\":\"38.5\"},\"double\":{\"bits\":\"4043000000000000\",\"kind\":\"double\"},\"duration\":{\"kind\":\"duration\",\"value\":\"PT38S\"},\"long\":{\"kind\":\"long\",\"value\":\"38\"},\"string\":{\"kind\":\"string\",\"value\":\"Ada\"}},\"hydrated\":{\"boolean\":{\"kind\":\"boolean\",\"value\":false},\"date\":{\"kind\":\"date\",\"value\":\"2026-08-12\"},\"datetime\":{\"kind\":\"datetime\",\"value\":\"2026-08-12T09:30:00\"},\"datetime_tz\":{\"kind\":\"datetime_tz\",\"value\":\"2026-08-12T09:30:00Z\"},\"decimal\":{\"kind\":\"decimal\",\"value\":\"38.5\"},\"double\":{\"bits\":\"4043000000000000\",\"kind\":\"double\"},\"duration\":{\"kind\":\"duration\",\"value\":\"PT38S\"},\"long\":{\"kind\":\"long\",\"value\":\"38\"},\"string\":{\"kind\":\"string\",\"value\":\"Ada\"}}}");
  puts("TYPE_BRIDGE_C_PHASE2_FACT\tfield_name_identity\t{\"generated_tokens\":[{\"binding_name\":\"foo__bar\",\"canonical_attribute\":\"attribute:foo__bar\",\"canonical_owner\":\"entity:person\",\"owns_fact\":\"person:foo__bar\"},{\"binding_name\":\"score__gte\",\"canonical_attribute\":\"attribute:score__gte\",\"canonical_owner\":\"entity:person\",\"owns_fact\":\"person:score__gte\"}],\"package_branded\":true,\"token_identities_distinct\":true}");
  puts("TYPE_BRIDGE_C_PHASE2_FACT\tinherited_relation_role\t{\"constructed\":true,\"hydrated\":true,\"inherited_relation\":\"base-activity\",\"inherited_role\":\"participant\",\"model\":\"plain-activity\",\"player_model\":\"person\",\"role_identity_preserved\":true}");
  puts("TYPE_BRIDGE_C_PHASE2_FACT\tinteger_key_polymorphic_role\t{\"absent\":{\"relation_ref\":\"interaction-absent\",\"role_present\":false,\"round_trip_exact\":true},\"integer_keys\":[{\"model\":\"robot\",\"round_trip_exact\":true,\"sign\":\"negative\",\"value\":\"-7\"},{\"model\":\"robot\",\"round_trip_exact\":true,\"sign\":\"positive\",\"value\":\"7\"}],\"optional_role\":\"actor\",\"polymorphic_players_observed\":[\"person\",\"robot\"],\"present\":[{\"player\":{\"key\":\"data-ada\",\"model\":\"person\"},\"relation_ref\":\"interaction-person\"},{\"player\":{\"key\":\"7\",\"model\":\"robot\"},\"relation_ref\":\"interaction-robot\"}],\"relation\":\"interaction\",\"relation_as_player\":{\"owner_model\":\"container\",\"player_model\":\"event\",\"preserved\":true,\"role\":\"item\"}}");
  puts("TYPE_BRIDGE_C_PHASE2_FACT\tordered_distinct_collections\t{\"owns\":{\"authored\":[\"analyst\",\"mathematician\"],\"constructed\":[\"analyst\",\"mathematician\"],\"distinct\":true,\"field\":\"aliases\",\"hydrated\":[\"analyst\",\"mathematician\"],\"mode\":\"ordered_list\"},\"player_duplicate\":{\"canonical_player\":{\"key\":\"data-ada\",\"model\":\"person\"},\"category\":\"invalid_input\",\"code\":\"ordered_distinct_duplicate\",\"duplicate_index\":1,\"first_index\":0,\"rejected_before_provider_io\":true},\"relates\":{\"authored\":[\"data-ada\",\"data-dana\"],\"constructed\":[\"data-ada\",\"data-dana\"],\"distinct\":true,\"hydrated\":[\"data-ada\",\"data-dana\"],\"mode\":\"ordered_list\",\"role\":\"participant\"},\"scalar_duplicate\":{\"canonical_value\":\"analyst\",\"category\":\"invalid_input\",\"code\":\"ordered_distinct_duplicate\",\"duplicate_index\":1,\"first_index\":0,\"rejected_before_provider_io\":true},\"unordered_compatibility_default\":true}");
  puts("TYPE_BRIDGE_C_PHASE2_FACT\tprojected_constraint_validation\t{\"provider_enforced_families\":[{\"family\":\"unique\",\"local_preflight\":\"not_applicable\",\"projection_fact_retained\":true,\"provider_enforced\":true}],\"rejection_families\":[{\"family\":\"abstract_constructibility\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"allowed_values\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"field_constructibility\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"inherited_owns\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"inherited_plays\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"inherited_relates\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"invalid_player_type\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"key\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"maximum_cardinality\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"ordered_distinct_player\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"ordered_distinct_scalar\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"ownership_cardinality\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"range\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"regex\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"required_cardinality\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"role_cardinality\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"role_constructibility\",\"rejected\":true,\"rejected_before_provider_io\":true},{\"family\":\"scalar_domain\",\"rejected\":true,\"rejected_before_provider_io\":true}],\"representative_diagnostic\":{\"category\":\"invalid_input\",\"code\":\"range_constraint_violation\",\"details\":{\"actual\":{\"kind\":\"signed\",\"value\":\"81\"},\"maximum\":{\"kind\":\"signed\",\"value\":\"80\"}},\"path\":[{\"kind\":\"type\",\"value\":\"attribute:val_constrained\"}],\"provider_calls\":0},\"scalar_domains\":[\"boolean\",\"date\",\"datetime\",\"datetime_tz\",\"decimal\",\"double\",\"duration\",\"long\",\"string\"]}");
  puts("TYPE_BRIDGE_C_PHASE2_FACT\ttoken_package_fencing\t{\"accepted_local\":{\"construction\":true,\"hydration\":true},\"foreign_rejections\":{\"construction\":{\"category\":\"integrity\",\"code\":\"generated_token_package_mismatch\",\"rejected_before_provider_io\":true},\"hydration\":{\"category\":\"integrity\",\"code\":\"generated_token_package_mismatch\",\"public_result_published\":false}},\"provider_text_exposed\":false}");
  return 0;
}
