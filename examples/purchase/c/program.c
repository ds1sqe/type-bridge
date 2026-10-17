/*
 * The purchase program of the `writes` SDK conformance suite, in C17.
 *
 * It builds every plan with the ABI 1.7 plan builder against the generated
 * schema package, then, when a live target is configured, runs the same
 * program directly and over the proxy: seed, stage the purchase, a touch and
 * a batch of 50 orders in one transaction, commit, and read the state back;
 * then the journeys (a put, an update answering rows, an insert whose
 * players it inserts, a delete) in one transaction, and a second read.
 * It prints one JSON report. <tb_writes/models.h> is the package generated
 * from tests/contracts/sdk_conformance/writes/purchase.
 */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <typebridge/type_bridge.h>
#include <tb_writes/models.h>

#define PLAN_COUNT 15u
#define BATCH_ORDERS 50u

#define CHECK(call)                                                            \
  do {                                                                         \
    if ((status = (call)) != TYPE_BRIDGE_STATUS_OK) {                          \
      fprintf(stderr, "%s:%d: status %d\n", __func__, __LINE__, (int)status);  \
      goto done;                                                               \
    }                                                                          \
  } while (0)

static type_bridge_byte_view_t view(const char *text) {
  type_bridge_byte_view_t value;
  value.data = (const uint8_t *)text;
  value.length = text == NULL ? 0u : strlen(text);
  return value;
}

/* The identities of the package's types, fields and roles, registered once
 * per builder: the C counterpart of the generated tokens of other SDKs. */
typedef struct identities {
  uint32_t item, account, doctor, purchase;
  uint32_t item_id, stock, price, account_id, balance, name, on_call, order_id;
  uint32_t buyer, item_role;
} identities_t;

static type_bridge_status_t identify(type_bridge_plan_builder_t *plan, identities_t *id) {
  type_bridge_status_t status;
  CHECK(type_bridge_plan_builder_type(plan, TYPE_BRIDGE_PLAN_THING_ENTITY, view("item"), &id->item));
  CHECK(type_bridge_plan_builder_type(plan, TYPE_BRIDGE_PLAN_THING_ENTITY, view("account"),
                                      &id->account));
  CHECK(type_bridge_plan_builder_type(plan, TYPE_BRIDGE_PLAN_THING_ENTITY, view("doctor"),
                                      &id->doctor));
  CHECK(type_bridge_plan_builder_type(plan, TYPE_BRIDGE_PLAN_THING_RELATION, view("purchase"),
                                      &id->purchase));
  CHECK(type_bridge_plan_builder_owns(plan, id->item, view("item-id"), &id->item_id));
  CHECK(type_bridge_plan_builder_owns(plan, id->item, view("stock"), &id->stock));
  CHECK(type_bridge_plan_builder_owns(plan, id->item, view("price"), &id->price));
  CHECK(type_bridge_plan_builder_owns(plan, id->account, view("account-id"), &id->account_id));
  CHECK(type_bridge_plan_builder_owns(plan, id->account, view("balance"), &id->balance));
  CHECK(type_bridge_plan_builder_owns(plan, id->doctor, view("name"), &id->name));
  CHECK(type_bridge_plan_builder_owns(plan, id->doctor, view("on-call"), &id->on_call));
  CHECK(type_bridge_plan_builder_owns(plan, id->purchase, view("order-id"), &id->order_id));
  CHECK(type_bridge_plan_builder_role(plan, id->purchase, view("buyer"), &id->buyer));
  CHECK(type_bridge_plan_builder_role(plan, id->purchase, view("item"), &id->item_role));
done:
  return status;
}

static type_bridge_status_t input(type_bridge_plan_builder_t *plan, const char *name,
                                  type_bridge_projected_value_kind_t kind, uint32_t *out) {
  return type_bridge_plan_builder_input(plan, view(name), kind, 0u, out);
}

/* `binding == value` against a literal value id. */
static type_bridge_status_t equals(type_bridge_plan_builder_t *plan, uint32_t binding,
                                   uint32_t value) {
  return type_bridge_plan_builder_compare(plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, binding,
                                          TYPE_BRIDGE_PLAN_COMPARE_EQUAL,
                                          TYPE_BRIDGE_PLAN_OPERAND_VALUE, value);
}

/* One item in stock, one funded account and one on-call doctor. */
static type_bridge_status_t seed(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t item_id, stock, price, account_id, balance, doctor, on_call;
  uint32_t item, account, doctor_thing;
  uint32_t row[7];
  CHECK(input(plan, "item_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &item_id));
  CHECK(input(plan, "stock", TYPE_BRIDGE_PROJECTED_VALUE_LONG, &stock));
  CHECK(input(plan, "price", TYPE_BRIDGE_PROJECTED_VALUE_LONG, &price));
  CHECK(input(plan, "account_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &account_id));
  CHECK(input(plan, "balance", TYPE_BRIDGE_PROJECTED_VALUE_LONG, &balance));
  CHECK(input(plan, "doctor", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &doctor));
  CHECK(input(plan, "on_call", TYPE_BRIDGE_PROJECTED_VALUE_BOOLEAN, &on_call));
  CHECK(type_bridge_plan_builder_insert(plan, id->item, &item));
  CHECK(type_bridge_plan_builder_thing_field(plan, item, id->item_id,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, item_id));
  CHECK(type_bridge_plan_builder_thing_field(plan, item, id->stock, TYPE_BRIDGE_PLAN_OPERAND_INPUT,
                                             stock));
  CHECK(type_bridge_plan_builder_thing_field(plan, item, id->price, TYPE_BRIDGE_PLAN_OPERAND_INPUT,
                                             price));
  CHECK(type_bridge_plan_builder_insert(plan, id->account, &account));
  CHECK(type_bridge_plan_builder_thing_field(plan, account, id->account_id,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, account_id));
  CHECK(type_bridge_plan_builder_thing_field(plan, account, id->balance,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, balance));
  CHECK(type_bridge_plan_builder_insert(plan, id->doctor, &doctor_thing));
  CHECK(type_bridge_plan_builder_thing_field(plan, doctor_thing, id->name,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, doctor));
  CHECK(type_bridge_plan_builder_thing_field(plan, doctor_thing, id->on_call,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, on_call));
  CHECK(type_bridge_plan_builder_value_string(plan, view("I1"), &row[0]));
  CHECK(type_bridge_plan_builder_value_long(plan, 1, &row[1]));
  CHECK(type_bridge_plan_builder_value_long(plan, 40, &row[2]));
  CHECK(type_bridge_plan_builder_value_string(plan, view("A1"), &row[3]));
  CHECK(type_bridge_plan_builder_value_long(plan, 100, &row[4]));
  CHECK(type_bridge_plan_builder_value_string(plan, view("D1"), &row[5]));
  CHECK(type_bridge_plan_builder_value_boolean(plan, 1u, &row[6]));
  CHECK(type_bridge_plan_builder_row(plan, row, 7u));
done:
  return status;
}

/* --8<-- [start:purchase] */
/* The one-query purchase: take one item and its price, record the order. */
static type_bridge_status_t purchase(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t item, buyer, order_input, item_input, account_input;
  uint32_t item_key, buyer_key, stock, price, balance;
  uint32_t zero, one, stock_expr, balance_expr, next_stock, next_balance, order;
  uint32_t row[3];
  CHECK(type_bridge_plan_builder_exact(plan, id->item, &item));
  CHECK(type_bridge_plan_builder_exact(plan, id->account, &buyer));
  CHECK(input(plan, "order_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &order_input));
  CHECK(input(plan, "item_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &item_input));
  CHECK(input(plan, "account_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &account_input));
  CHECK(type_bridge_plan_builder_field(plan, item, id->item_id, &item_key));
  CHECK(type_bridge_plan_builder_field(plan, buyer, id->account_id, &buyer_key));
  CHECK(type_bridge_plan_builder_field(plan, item, id->stock, &stock));
  CHECK(type_bridge_plan_builder_field(plan, item, id->price, &price));
  CHECK(type_bridge_plan_builder_field(plan, buyer, id->balance, &balance));
  CHECK(type_bridge_plan_builder_value_long(plan, 0, &zero));
  CHECK(type_bridge_plan_builder_value_long(plan, 1, &one));
  CHECK(type_bridge_plan_builder_compare(plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, item_key,
                                         TYPE_BRIDGE_PLAN_COMPARE_EQUAL,
                                         TYPE_BRIDGE_PLAN_OPERAND_INPUT, item_input));
  CHECK(type_bridge_plan_builder_compare(plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, buyer_key,
                                         TYPE_BRIDGE_PLAN_COMPARE_EQUAL,
                                         TYPE_BRIDGE_PLAN_OPERAND_INPUT, account_input));
  CHECK(type_bridge_plan_builder_compare(plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, stock,
                                         TYPE_BRIDGE_PLAN_COMPARE_GREATER,
                                         TYPE_BRIDGE_PLAN_OPERAND_VALUE, zero));
  CHECK(type_bridge_plan_builder_compare(plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, balance,
                                         TYPE_BRIDGE_PLAN_COMPARE_GREATER_OR_EQUAL,
                                         TYPE_BRIDGE_PLAN_OPERAND_BINDING, price));
  CHECK(type_bridge_plan_builder_expr_arith(plan, TYPE_BRIDGE_PLAN_ARITH_SUB,
                                            TYPE_BRIDGE_PLAN_OPERAND_BINDING, stock,
                                            TYPE_BRIDGE_PLAN_OPERAND_VALUE, one, &stock_expr));
  CHECK(type_bridge_plan_builder_let(plan, view("next_stock"), TYPE_BRIDGE_PLAN_OPERAND_EXPR,
                                     stock_expr, &next_stock));
  CHECK(type_bridge_plan_builder_update(plan, item, id->stock, TYPE_BRIDGE_PLAN_OPERAND_BINDING,
                                        next_stock));
  CHECK(type_bridge_plan_builder_expr_arith(plan, TYPE_BRIDGE_PLAN_ARITH_SUB,
                                            TYPE_BRIDGE_PLAN_OPERAND_BINDING, balance,
                                            TYPE_BRIDGE_PLAN_OPERAND_BINDING, price,
                                            &balance_expr));
  CHECK(type_bridge_plan_builder_let(plan, view("next_balance"), TYPE_BRIDGE_PLAN_OPERAND_EXPR,
                                     balance_expr, &next_balance));
  CHECK(type_bridge_plan_builder_update(plan, buyer, id->balance,
                                        TYPE_BRIDGE_PLAN_OPERAND_BINDING, next_balance));
  CHECK(type_bridge_plan_builder_insert(plan, id->purchase, &order));
  CHECK(type_bridge_plan_builder_thing_field(plan, order, id->order_id,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, order_input));
  CHECK(type_bridge_plan_builder_thing_player(plan, order, id->buyer, buyer));
  CHECK(type_bridge_plan_builder_thing_player(plan, order, id->item_role, item));
  CHECK(type_bridge_plan_builder_value_string(plan, view("O1"), &row[0]));
  CHECK(type_bridge_plan_builder_value_string(plan, view("I1"), &row[1]));
  CHECK(type_bridge_plan_builder_value_string(plan, view("A1"), &row[2]));
  CHECK(type_bridge_plan_builder_row(plan, row, 3u));
done:
  return status;
}
/* --8<-- [end:purchase] */

/* Rewrite every doctor's on-call value with itself, taking its lock. */
static type_bridge_status_t touch(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t doctor, on_call;
  CHECK(type_bridge_plan_builder_exact(plan, id->doctor, &doctor));
  CHECK(type_bridge_plan_builder_field(plan, doctor, id->on_call, &on_call));
  CHECK(type_bridge_plan_builder_update(plan, doctor, id->on_call,
                                        TYPE_BRIDGE_PLAN_OPERAND_BINDING, on_call));
done:
  return status;
}

/* Fifty orders of the item by the account, one given row each. */
static type_bridge_status_t batch(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t item, buyer, order_input, item_key, buyer_key, item_value, account_value, order;
  uint32_t index;
  char order_id[8];
  CHECK(type_bridge_plan_builder_exact(plan, id->item, &item));
  CHECK(type_bridge_plan_builder_exact(plan, id->account, &buyer));
  CHECK(input(plan, "order_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &order_input));
  CHECK(type_bridge_plan_builder_field(plan, item, id->item_id, &item_key));
  CHECK(type_bridge_plan_builder_value_string(plan, view("I1"), &item_value));
  CHECK(equals(plan, item_key, item_value));
  CHECK(type_bridge_plan_builder_field(plan, buyer, id->account_id, &buyer_key));
  CHECK(type_bridge_plan_builder_value_string(plan, view("A1"), &account_value));
  CHECK(equals(plan, buyer_key, account_value));
  CHECK(type_bridge_plan_builder_insert(plan, id->purchase, &order));
  CHECK(type_bridge_plan_builder_thing_field(plan, order, id->order_id,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, order_input));
  CHECK(type_bridge_plan_builder_thing_player(plan, order, id->buyer, buyer));
  CHECK(type_bridge_plan_builder_thing_player(plan, order, id->item_role, item));
  for (index = 1u; index <= BATCH_ORDERS; ++index) {
    uint32_t value;
    snprintf(order_id, sizeof(order_id), "B%02u", (unsigned)index);
    CHECK(type_bridge_plan_builder_value_string(plan, view(order_id), &value));
    CHECK(type_bridge_plan_builder_row(plan, &value, 1u));
  }
done:
  return status;
}

static type_bridge_status_t sold_out(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t item, key, stock, key_value, zero;
  CHECK(type_bridge_plan_builder_exact(plan, id->item, &item));
  CHECK(type_bridge_plan_builder_field(plan, item, id->item_id, &key));
  CHECK(type_bridge_plan_builder_value_string(plan, view("I1"), &key_value));
  CHECK(equals(plan, key, key_value));
  CHECK(type_bridge_plan_builder_field(plan, item, id->stock, &stock));
  CHECK(type_bridge_plan_builder_value_long(plan, 0, &zero));
  CHECK(equals(plan, stock, zero));
  CHECK(type_bridge_plan_builder_count(plan));
done:
  return status;
}

static type_bridge_status_t charged(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t account, key, balance, key_value, sixty;
  CHECK(type_bridge_plan_builder_exact(plan, id->account, &account));
  CHECK(type_bridge_plan_builder_field(plan, account, id->account_id, &key));
  CHECK(type_bridge_plan_builder_value_string(plan, view("A1"), &key_value));
  CHECK(equals(plan, key, key_value));
  CHECK(type_bridge_plan_builder_field(plan, account, id->balance, &balance));
  CHECK(type_bridge_plan_builder_value_long(plan, 60, &sixty));
  CHECK(equals(plan, balance, sixty));
  CHECK(type_bridge_plan_builder_count(plan));
done:
  return status;
}

static type_bridge_status_t orders(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t order;
  CHECK(type_bridge_plan_builder_exact(plan, id->purchase, &order));
  CHECK(type_bridge_plan_builder_count(plan));
done:
  return status;
}

static type_bridge_status_t on_call(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t doctor, value, yes;
  CHECK(type_bridge_plan_builder_exact(plan, id->doctor, &doctor));
  CHECK(type_bridge_plan_builder_field(plan, doctor, id->on_call, &value));
  CHECK(type_bridge_plan_builder_value_boolean(plan, 1u, &yes));
  CHECK(equals(plan, value, yes));
  CHECK(type_bridge_plan_builder_exists(plan));
done:
  return status;
}

/* `thing.field = literal`, for an insert or put binding. */
static type_bridge_status_t set_value(type_bridge_plan_builder_t *plan, uint32_t thing,
                                      uint32_t field, uint32_t value) {
  return type_bridge_plan_builder_thing_field(plan, thing, field, TYPE_BRIDGE_PLAN_OPERAND_VALUE,
                                              value);
}

/* Match `type` whose `field` equals the string `key`. */
static type_bridge_status_t keyed(type_bridge_plan_builder_t *plan, uint32_t type, uint32_t field,
                                  const char *key, uint32_t *out_thing, uint32_t *out_field) {
  type_bridge_status_t status;
  uint32_t value;
  CHECK(type_bridge_plan_builder_exact(plan, type, out_thing));
  CHECK(type_bridge_plan_builder_field(plan, *out_thing, field, out_field));
  CHECK(type_bridge_plan_builder_value_string(plan, view(key), &value));
  CHECK(equals(plan, *out_field, value));
done:
  return status;
}

/* Put a second doctor, off call: created, since none matches. */
static type_bridge_status_t put_doctor(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t doctor, name, off;
  CHECK(type_bridge_plan_builder_put(plan, id->doctor, &doctor));
  CHECK(type_bridge_plan_builder_value_string(plan, view("D2"), &name));
  CHECK(set_value(plan, doctor, id->name, name));
  CHECK(type_bridge_plan_builder_value_boolean(plan, 0u, &off));
  CHECK(set_value(plan, doctor, id->on_call, off));
done:
  return status;
}

/* Put the second doctor on call and answer its name (`Write{returns}`). */
static type_bridge_status_t returns(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t doctor, name, on;
  CHECK(keyed(plan, id->doctor, id->name, "D2", &doctor, &name));
  CHECK(type_bridge_plan_builder_value_boolean(plan, 1u, &on));
  CHECK(type_bridge_plan_builder_update(plan, doctor, id->on_call, TYPE_BRIDGE_PLAN_OPERAND_VALUE,
                                        on));
  CHECK(type_bridge_plan_builder_returning(plan, &name, 1u));
done:
  return status;
}

/* A purchase whose players are the item and account this write inserts. */
static type_bridge_status_t refer(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t item, buyer, order, value;
  CHECK(type_bridge_plan_builder_insert(plan, id->item, &item));
  CHECK(type_bridge_plan_builder_value_string(plan, view("I2"), &value));
  CHECK(set_value(plan, item, id->item_id, value));
  CHECK(type_bridge_plan_builder_value_long(plan, 3, &value));
  CHECK(set_value(plan, item, id->stock, value));
  CHECK(type_bridge_plan_builder_value_long(plan, 10, &value));
  CHECK(set_value(plan, item, id->price, value));
  CHECK(type_bridge_plan_builder_insert(plan, id->account, &buyer));
  CHECK(type_bridge_plan_builder_value_string(plan, view("A2"), &value));
  CHECK(set_value(plan, buyer, id->account_id, value));
  CHECK(type_bridge_plan_builder_value_long(plan, 50, &value));
  CHECK(set_value(plan, buyer, id->balance, value));
  CHECK(type_bridge_plan_builder_insert(plan, id->purchase, &order));
  CHECK(type_bridge_plan_builder_value_string(plan, view("R1"), &value));
  CHECK(set_value(plan, order, id->order_id, value));
  CHECK(type_bridge_plan_builder_thing_player(plan, order, id->buyer, buyer));
  CHECK(type_bridge_plan_builder_thing_player(plan, order, id->item_role, item));
done:
  return status;
}

/* Delete the first purchase. */
static type_bridge_status_t remove_order(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t order, key;
  CHECK(keyed(plan, id->purchase, id->order_id, "O1", &order, &key));
  CHECK(type_bridge_plan_builder_delete(plan, order));
done:
  return status;
}

static type_bridge_status_t doctors_on_call(type_bridge_plan_builder_t *plan,
                                            const identities_t *id) {
  type_bridge_status_t status;
  uint32_t doctor, value, yes;
  CHECK(type_bridge_plan_builder_exact(plan, id->doctor, &doctor));
  CHECK(type_bridge_plan_builder_field(plan, doctor, id->on_call, &value));
  CHECK(type_bridge_plan_builder_value_boolean(plan, 1u, &yes));
  CHECK(equals(plan, value, yes));
  CHECK(type_bridge_plan_builder_count(plan));
done:
  return status;
}

static type_bridge_status_t referred(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t order, buyer, key, buyer_key, value;
  CHECK(type_bridge_plan_builder_exact(plan, id->purchase, &order));
  CHECK(type_bridge_plan_builder_exact(plan, id->account, &buyer));
  CHECK(type_bridge_plan_builder_field(plan, order, id->order_id, &key));
  CHECK(type_bridge_plan_builder_value_string(plan, view("R1"), &value));
  CHECK(equals(plan, key, value));
  CHECK(type_bridge_plan_builder_field(plan, buyer, id->account_id, &buyer_key));
  CHECK(type_bridge_plan_builder_value_string(plan, view("A2"), &value));
  CHECK(equals(plan, buyer_key, value));
  CHECK(type_bridge_plan_builder_links(plan, order, id->buyer, buyer));
  CHECK(type_bridge_plan_builder_count(plan));
done:
  return status;
}

static type_bridge_status_t removed(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t order, key;
  CHECK(keyed(plan, id->purchase, id->order_id, "O1", &order, &key));
  CHECK(type_bridge_plan_builder_exists(plan));
done:
  return status;
}

typedef type_bridge_status_t (*plan_fn)(type_bridge_plan_builder_t *, const identities_t *);

static const char *const NAMES[PLAN_COUNT] = {
    "seed",       "purchase", "touch", "batch",  "sold_out",        "charged",  "orders", "on_call",
    "put_doctor", "returns",  "refer", "remove", "doctors_on_call", "referred", "removed"};
static const plan_fn BUILDERS[PLAN_COUNT] = {
    seed,       purchase, touch, batch,        sold_out,        charged,  orders, on_call,
    put_doctor, returns,  refer, remove_order, doctors_on_call, referred, removed};

/* The program: seed, then purchase, touch and batch in one transaction, then
 * the reads in one read transaction; then the journeys in one transaction and
 * a second read. Each step names a range of plans. */
#define STEP_TOTAL 5u
static const char *const STEPS[STEP_TOTAL] = {"seed", "program", "read", "journeys", "after"};
static const size_t STEP_FIRST[STEP_TOTAL] = {0u, 1u, 4u, 8u, 12u};
static const size_t STEP_COUNT[STEP_TOTAL] = {1u, 3u, 4u, 4u, 3u};
static const int STEP_READ[STEP_TOTAL] = {0, 0, 1, 0, 1};

static type_bridge_status_t build_all(const type_bridge_schema_package_t *package,
                                      type_bridge_plan_t **plans) {
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_OK;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  size_t index;
  for (index = 0u; index < PLAN_COUNT; ++index) {
    type_bridge_plan_builder_t *plan = NULL;
    identities_t id;
    CHECK(type_bridge_plan_builder_new(&plan));
    status = identify(plan, &id);
    if (status == TYPE_BRIDGE_STATUS_OK) status = BUILDERS[index](plan, &id);
    if (status == TYPE_BRIDGE_STATUS_OK)
      status = type_bridge_plan_builder_build(plan, package, &plans[index], &diagnostics);
    type_bridge_plan_builder_free(&plan);
    (void)type_bridge_execution_diagnostics_close(&diagnostics);
    if (status != TYPE_BRIDGE_STATUS_OK) {
      fprintf(stderr, "plan %s does not build: status %d\n", NAMES[index], (int)status);
      goto done;
    }
  }
done:
  return status;
}

static int print_hex(const type_bridge_plan_t *plan) {
  size_t length = 0u, index;
  uint8_t *bytes;
  if (type_bridge_plan_v3_canonical_bytes(plan, NULL, 0u, &length) !=
      TYPE_BRIDGE_STATUS_RESOURCE_LIMIT)
    return 0;
  bytes = (uint8_t *)malloc(length);
  if (bytes == NULL) return 0;
  if (type_bridge_plan_v3_canonical_bytes(plan, bytes, length, &length) != TYPE_BRIDGE_STATUS_OK) {
    free(bytes);
    return 0;
  }
  for (index = 0u; index < length; ++index) printf("%02x", bytes[index]);
  free(bytes);
  return 1;
}

static const char *answer_kind(type_bridge_plan_answer_kind_t kind) {
  switch (kind) {
  case TYPE_BRIDGE_PLAN_ANSWER_ROWS: return "rows";
  case TYPE_BRIDGE_PLAN_ANSWER_COUNT: return "count";
  case TYPE_BRIDGE_PLAN_ANSWER_EXISTS: return "exists";
  case TYPE_BRIDGE_PLAN_ANSWER_MATCHED: return "matched";
  default: return "unknown";
  }
}

/* Opens one transaction of `kind` on the direct database or the proxy. */
typedef struct target {
  const type_bridge_database_t *database;
  const type_bridge_proxy_connection_t *proxy;
} target_t;

static type_bridge_status_t open_transaction(const target_t *target, uint32_t kind,
                                             type_bridge_staged_transaction_t **transaction,
                                             type_bridge_execution_diagnostics_t **diagnostics) {
  if (target->database != NULL)
    return type_bridge_database_transaction(target->database, kind, transaction, diagnostics);
  return type_bridge_proxy_connection_transaction(target->proxy, kind, transaction, diagnostics);
}

/* Run every step on `target`, printing `{"seed":{...},...}`. */
static type_bridge_status_t run_program(const target_t *target, type_bridge_plan_t *const *plans) {
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_OK;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  size_t step, index;
  printf("{");
  for (step = 0u; step < STEP_TOTAL; ++step) {
    type_bridge_staged_transaction_t *transaction = NULL;
    type_bridge_pending_t *pending[4] = {NULL, NULL, NULL, NULL};
    uint32_t kind = STEP_READ[step] ? TYPE_BRIDGE_TRANSACTION_READ : TYPE_BRIDGE_TRANSACTION_WRITE;
    status = open_transaction(target, kind, &transaction, &diagnostics);
    for (index = 0u; status == TYPE_BRIDGE_STATUS_OK && index < STEP_COUNT[step]; ++index)
      status = type_bridge_staged_transaction_stage(transaction, plans[STEP_FIRST[step] + index],
                                                    &pending[index], &diagnostics);
    if (status == TYPE_BRIDGE_STATUS_OK)
      status = type_bridge_staged_transaction_commit(&transaction, &diagnostics);
    if (status == TYPE_BRIDGE_STATUS_OK) {
      printf("%s\"%s\":{", step == 0u ? "" : ",", STEPS[step]);
      for (index = 0u; status == TYPE_BRIDGE_STATUS_OK && index < STEP_COUNT[step]; ++index) {
        type_bridge_plan_answer_kind_t answer = TYPE_BRIDGE_PLAN_ANSWER_ROWS;
        uint64_t value = 0u;
        status = type_bridge_pending_answer(pending[index], &answer, &value);
        printf("%s\"%s\":[\"%s\",%llu]", index == 0u ? "" : ",",
               NAMES[STEP_FIRST[step] + index], answer_kind(answer), (unsigned long long)value);
      }
      printf("}");
    } else {
      fprintf(stderr, "step %s failed: status %d\n", STEPS[step], (int)status);
    }
    type_bridge_staged_transaction_close(&transaction);
    for (index = 0u; index < 4u; ++index) type_bridge_pending_free(&pending[index]);
    (void)type_bridge_execution_diagnostics_close(&diagnostics);
    if (status != TYPE_BRIDGE_STATUS_OK) break;
  }
  printf("}");
  return status;
}

static uint32_t http_port(void) {
  const char *text = getenv("TYPEDB_HTTP_PORT");
  char *end = NULL;
  unsigned long value;
  if (text == NULL || text[0] == '\0') return 8000u;
  errno = 0;
  value = strtoul(text, &end, 10);
  if (errno != 0 || end == text || *end != '\0' || value == 0ul || value > 65535ul) return 0u;
  return (uint32_t)value;
}

static const char *env_or(const char *name, const char *fallback) {
  const char *value = getenv(name);
  return value == NULL || value[0] == '\0' ? fallback : value;
}

int main(void) {
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_OK;
  type_bridge_schema_package_t *package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_database_t *database = NULL;
  type_bridge_proxy_connection_t *proxy = NULL;
  type_bridge_plan_t *plans[PLAN_COUNT] = {NULL};
  const char *database_name = env_or("TYPE_BRIDGE_SDK_WRITES_DIRECT_DATABASE", NULL);
  const char *endpoint = env_or("TYPE_BRIDGE_SDK_WRITES_PROXY_ENDPOINT", NULL);
  const char *separator = "";
  size_t index;

  CHECK(tb_writes_schema_package_open(&package, &package_diagnostics));
  CHECK(build_all(package, plans));
  printf("{\"binding\":\"c\",\"plans\":{");
  for (index = 0u; index < PLAN_COUNT; ++index) {
    printf("%s\"%s\":\"", index == 0u ? "" : ",", NAMES[index]);
    if (!print_hex(plans[index])) {
      status = TYPE_BRIDGE_STATUS_PANIC;
      goto done;
    }
    printf("\"");
  }
  printf("},\"results\":{");
  if (database_name != NULL) {
    type_bridge_runtime_config_v1_t runtime_config = {
        sizeof(type_bridge_runtime_config_v1_t), TYPE_BRIDGE_RUNTIME_CONFIG_VERSION, 2u, 0u,
        {0u, 0u, 0u, 0u}};
    type_bridge_database_config_v1_t config;
    target_t target = {NULL, NULL};
    CHECK(type_bridge_runtime_open_v1(&runtime_config, &runtime, &diagnostics));
    memset(&config, 0, sizeof(config));
    config.struct_size = sizeof(config);
    config.version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION;
    config.address = view(env_or("TYPEDB_ADDRESS", "localhost:1729"));
    config.database = view(database_name);
    config.username = view(env_or("TYPEDB_USERNAME", "admin"));
    config.password = view(env_or("TYPEDB_PASSWORD", "password"));
    config.http_port = http_port();
    config.tls_mode = TYPE_BRIDGE_TLS_DISABLED;
    CHECK(type_bridge_database_open_v1(runtime, package, &config, NULL, &database, &diagnostics));
    target.database = database;
    printf("\"direct\":");
    CHECK(run_program(&target, plans));
    separator = ",";
  }
  if (endpoint != NULL) {
    type_bridge_proxy_connection_options_v1_t options;
    target_t target = {NULL, NULL};
    memset(&options, 0, sizeof(options));
    options.struct_size = sizeof(options);
    options.version = TYPE_BRIDGE_PROXY_CONNECTION_OPTIONS_VERSION;
    options.endpoint = view(endpoint);
    options.tls_mode = TYPE_BRIDGE_TLS_DISABLED;
    CHECK(type_bridge_proxy_connection_open_v1(package, &options, NULL, &proxy, &diagnostics));
    target.proxy = proxy;
    printf("%s\"proxy\":", separator);
    CHECK(run_program(&target, plans));
  }
  printf("}}\n");

done:
  for (index = 0u; index < PLAN_COUNT; ++index) type_bridge_plan_free(&plans[index]);
  (void)type_bridge_proxy_connection_close(&proxy);
  if (database != NULL) (void)type_bridge_database_close(&database, &diagnostics);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  if (runtime != NULL) (void)type_bridge_runtime_close(&runtime, &diagnostics);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  if (package_diagnostics != NULL) (void)type_bridge_diagnostics_close(&package_diagnostics);
  if (package != NULL) (void)type_bridge_schema_package_close(&package);
  fflush(stdout);
  return status == TYPE_BRIDGE_STATUS_OK ? 0 : 1;
}
