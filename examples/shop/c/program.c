/*
 * The shop checkout of the `writes` SDK conformance suite, in C17.
 *
 * A checkout takes a whole cart at once: every line must be in stock and the
 * account must cover the total, and then every stock, the balance and the
 * order lines change together. So a cart is one plan, and one plan is one
 * TypeQL query. If any check fails the match is empty, nothing is written,
 * and the plan answers Matched(0): check the answer.
 *
 * Several plans staged in one transaction commit together, and each sees the
 * writes staged before it. A plan whose check fails does not stop the others,
 * so a check that must gate the whole cart belongs in the cart's own plan.
 * In the race step BUYERS buyers commit the checkout of the last lamp at
 * once; the losers get a retry-safe conflict, store nothing, and retry on a
 * fresh snapshot where the lamp is gone.
 *
 * It builds every plan with the ABI 1.7 plan builder against the generated
 * schema package, then, when a live target is configured, runs the steps of
 * tests/contracts/sdk_conformance/writes/shop/program.json directly and over
 * the proxy. It prints one JSON report. <tb_writes/models.h> is the package
 * generated from tests/contracts/sdk_conformance/writes/shop.
 */
/* pthread barriers are POSIX, hidden by -std=c17 without this. */
#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <typebridge/type_bridge.h>
#include <tb_writes/models.h>

#define PLAN_COUNT 19u
#define STEP_COUNT 10u
#define MAX_STEP_PLANS 4u
#define MAX_LINES 4u
/* The race step: this many buyers commit its one plan at once. */
#define BUYERS 8u
#define ATTEMPTS 20u

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
  uint32_t account, item, order_line;
  uint32_t account_id, balance, sku, price, stock, order_id, quantity;
  uint32_t buyer, item_role;
} identities_t;

static type_bridge_status_t identify(type_bridge_plan_builder_t *plan, identities_t *id) {
  type_bridge_status_t status;
  CHECK(type_bridge_plan_builder_type(plan, TYPE_BRIDGE_PLAN_THING_ENTITY, view("account"),
                                      &id->account));
  CHECK(type_bridge_plan_builder_type(plan, TYPE_BRIDGE_PLAN_THING_ENTITY, view("item"), &id->item));
  CHECK(type_bridge_plan_builder_type(plan, TYPE_BRIDGE_PLAN_THING_RELATION, view("order-line"),
                                      &id->order_line));
  CHECK(type_bridge_plan_builder_owns(plan, id->account, view("account-id"), &id->account_id));
  CHECK(type_bridge_plan_builder_owns(plan, id->account, view("balance"), &id->balance));
  CHECK(type_bridge_plan_builder_owns(plan, id->item, view("sku"), &id->sku));
  CHECK(type_bridge_plan_builder_owns(plan, id->item, view("price"), &id->price));
  CHECK(type_bridge_plan_builder_owns(plan, id->item, view("stock"), &id->stock));
  CHECK(type_bridge_plan_builder_owns(plan, id->order_line, view("order-id"), &id->order_id));
  CHECK(type_bridge_plan_builder_owns(plan, id->order_line, view("quantity"), &id->quantity));
  CHECK(type_bridge_plan_builder_role(plan, id->order_line, view("buyer"), &id->buyer));
  CHECK(type_bridge_plan_builder_role(plan, id->order_line, view("item"), &id->item_role));
done:
  return status;
}

static type_bridge_status_t input(type_bridge_plan_builder_t *plan, const char *name,
                                  type_bridge_projected_value_kind_t kind, uint32_t *out) {
  return type_bridge_plan_builder_input(plan, view(name), kind, 0u, out);
}

/* `binding comparator value` against a literal value id. */
static type_bridge_status_t against(type_bridge_plan_builder_t *plan, uint32_t binding,
                                    uint32_t comparator, uint32_t value) {
  return type_bridge_plan_builder_compare(plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, binding,
                                          comparator, TYPE_BRIDGE_PLAN_OPERAND_VALUE, value);
}

/* `thing.field = literal`, for an insert binding. */
static type_bridge_status_t set_value(type_bridge_plan_builder_t *plan, uint32_t thing,
                                      uint32_t field, uint32_t value) {
  return type_bridge_plan_builder_thing_field(plan, thing, field, TYPE_BRIDGE_PLAN_OPERAND_VALUE,
                                              value);
}

/* Three accounts, one given row each. */
static type_bridge_status_t seed_accounts(type_bridge_plan_builder_t *plan,
                                          const identities_t *id) {
  static const char *const KEYS[3] = {"A1", "A2", "R1"};
  static const int64_t BALANCES[3] = {100, 30, 100};
  type_bridge_status_t status;
  uint32_t account_id, balance, account;
  size_t index;
  CHECK(input(plan, "account_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &account_id));
  CHECK(input(plan, "balance", TYPE_BRIDGE_PROJECTED_VALUE_LONG, &balance));
  CHECK(type_bridge_plan_builder_insert(plan, id->account, &account));
  CHECK(type_bridge_plan_builder_thing_field(plan, account, id->account_id,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, account_id));
  CHECK(type_bridge_plan_builder_thing_field(plan, account, id->balance,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, balance));
  for (index = 0u; index < 3u; ++index) {
    uint32_t row[2];
    CHECK(type_bridge_plan_builder_value_string(plan, view(KEYS[index]), &row[0]));
    CHECK(type_bridge_plan_builder_value_long(plan, BALANCES[index], &row[1]));
    CHECK(type_bridge_plan_builder_row(plan, row, 2u));
  }
done:
  return status;
}

/* Four items with a price and a stock, one given row each. */
static type_bridge_status_t seed_items(type_bridge_plan_builder_t *plan, const identities_t *id) {
  static const char *const SKUS[4] = {"pen", "book", "mug", "lamp"};
  static const int64_t PRICES[4] = {3, 40, 12, 10};
  static const int64_t STOCKS[4] = {10, 1, 5, 1};
  type_bridge_status_t status;
  uint32_t sku, price, stock, item;
  size_t index;
  CHECK(input(plan, "sku", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &sku));
  CHECK(input(plan, "price", TYPE_BRIDGE_PROJECTED_VALUE_LONG, &price));
  CHECK(input(plan, "stock", TYPE_BRIDGE_PROJECTED_VALUE_LONG, &stock));
  CHECK(type_bridge_plan_builder_insert(plan, id->item, &item));
  CHECK(type_bridge_plan_builder_thing_field(plan, item, id->sku, TYPE_BRIDGE_PLAN_OPERAND_INPUT,
                                             sku));
  CHECK(type_bridge_plan_builder_thing_field(plan, item, id->price, TYPE_BRIDGE_PLAN_OPERAND_INPUT,
                                             price));
  CHECK(type_bridge_plan_builder_thing_field(plan, item, id->stock, TYPE_BRIDGE_PLAN_OPERAND_INPUT,
                                             stock));
  for (index = 0u; index < 4u; ++index) {
    uint32_t row[3];
    CHECK(type_bridge_plan_builder_value_string(plan, view(SKUS[index]), &row[0]));
    CHECK(type_bridge_plan_builder_value_long(plan, PRICES[index], &row[1]));
    CHECK(type_bridge_plan_builder_value_long(plan, STOCKS[index], &row[2]));
    CHECK(type_bridge_plan_builder_row(plan, row, 3u));
  }
done:
  return status;
}

/* One line of a cart: `quantity` of the item `sku`. */
typedef struct line {
  const char *sku;
  int64_t quantity;
} line_t;

/* --8<-- [start:checkout] */
/* The whole cart in one query: check every line and the total, then write. */
static type_bridge_status_t checkout(type_bridge_plan_builder_t *plan, const identities_t *id,
                                     const char *account_id, const char *order_id,
                                     const line_t *lines, size_t count) {
  type_bridge_status_t status;
  uint32_t buyer, balance, key, value, total_expr = 0u, total, expr, next, order;
  uint32_t items[MAX_LINES], stocks[MAX_LINES];
  char name[24];
  size_t index;
  if (count == 0u || count > MAX_LINES) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  CHECK(type_bridge_plan_builder_exact(plan, id->account, &buyer));
  CHECK(type_bridge_plan_builder_field(plan, buyer, id->balance, &balance));
  CHECK(type_bridge_plan_builder_field(plan, buyer, id->account_id, &key));
  CHECK(type_bridge_plan_builder_value_string(plan, view(account_id), &value));
  CHECK(against(plan, key, TYPE_BRIDGE_PLAN_COMPARE_EQUAL, value));

  for (index = 0u; index < count; ++index) {
    uint32_t price, cost;
    CHECK(type_bridge_plan_builder_exact(plan, id->item, &items[index]));
    CHECK(type_bridge_plan_builder_field(plan, items[index], id->stock, &stocks[index]));
    CHECK(type_bridge_plan_builder_field(plan, items[index], id->sku, &key));
    CHECK(type_bridge_plan_builder_value_string(plan, view(lines[index].sku), &value));
    CHECK(against(plan, key, TYPE_BRIDGE_PLAN_COMPARE_EQUAL, value));
    CHECK(type_bridge_plan_builder_value_long(plan, lines[index].quantity, &value));
    CHECK(against(plan, stocks[index], TYPE_BRIDGE_PLAN_COMPARE_GREATER_OR_EQUAL, value));
    /* cost = price * quantity; total = cost, or total + cost. */
    CHECK(type_bridge_plan_builder_field(plan, items[index], id->price, &price));
    CHECK(type_bridge_plan_builder_expr_arith(plan, TYPE_BRIDGE_PLAN_ARITH_MUL,
                                              TYPE_BRIDGE_PLAN_OPERAND_BINDING, price,
                                              TYPE_BRIDGE_PLAN_OPERAND_VALUE, value, &cost));
    if (index == 0u) {
      total_expr = cost;
    } else {
      CHECK(type_bridge_plan_builder_expr_arith(plan, TYPE_BRIDGE_PLAN_ARITH_ADD,
                                                TYPE_BRIDGE_PLAN_OPERAND_EXPR, total_expr,
                                                TYPE_BRIDGE_PLAN_OPERAND_EXPR, cost, &total_expr));
    }
  }
  CHECK(type_bridge_plan_builder_let(plan, view("total"), TYPE_BRIDGE_PLAN_OPERAND_EXPR,
                                     total_expr, &total));
  CHECK(type_bridge_plan_builder_compare(plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, balance,
                                         TYPE_BRIDGE_PLAN_COMPARE_GREATER_OR_EQUAL,
                                         TYPE_BRIDGE_PLAN_OPERAND_BINDING, total));

  for (index = 0u; index < count; ++index) {
    snprintf(name, sizeof(name), "stock_%u", (unsigned)(index + 1u));
    CHECK(type_bridge_plan_builder_value_long(plan, lines[index].quantity, &value));
    CHECK(type_bridge_plan_builder_expr_arith(plan, TYPE_BRIDGE_PLAN_ARITH_SUB,
                                              TYPE_BRIDGE_PLAN_OPERAND_BINDING, stocks[index],
                                              TYPE_BRIDGE_PLAN_OPERAND_VALUE, value, &expr));
    CHECK(type_bridge_plan_builder_let(plan, view(name), TYPE_BRIDGE_PLAN_OPERAND_EXPR, expr,
                                       &next));
    CHECK(type_bridge_plan_builder_update(plan, items[index], id->stock,
                                          TYPE_BRIDGE_PLAN_OPERAND_BINDING, next));
  }
  CHECK(type_bridge_plan_builder_expr_arith(plan, TYPE_BRIDGE_PLAN_ARITH_SUB,
                                            TYPE_BRIDGE_PLAN_OPERAND_BINDING, balance,
                                            TYPE_BRIDGE_PLAN_OPERAND_BINDING, total, &expr));
  CHECK(type_bridge_plan_builder_let(plan, view("balance_after"), TYPE_BRIDGE_PLAN_OPERAND_EXPR,
                                     expr, &next));
  CHECK(type_bridge_plan_builder_update(plan, buyer, id->balance,
                                        TYPE_BRIDGE_PLAN_OPERAND_BINDING, next));
  for (index = 0u; index < count; ++index) {
    CHECK(type_bridge_plan_builder_insert(plan, id->order_line, &order));
    CHECK(type_bridge_plan_builder_value_string(plan, view(order_id), &value));
    CHECK(set_value(plan, order, id->order_id, value));
    CHECK(type_bridge_plan_builder_value_long(plan, lines[index].quantity, &value));
    CHECK(set_value(plan, order, id->quantity, value));
    CHECK(type_bridge_plan_builder_thing_player(plan, order, id->buyer, buyer));
    CHECK(type_bridge_plan_builder_thing_player(plan, order, id->item_role, items[index]));
  }
done:
  return status;
}
/* --8<-- [end:checkout] */

/* Count the things of `type` whose `key` and `field` hold the given values. */
static type_bridge_status_t count_where(type_bridge_plan_builder_t *plan, uint32_t type,
                                        uint32_t key, const char *key_value, uint32_t field,
                                        int64_t value) {
  type_bridge_status_t status;
  uint32_t thing, binding, literal;
  CHECK(type_bridge_plan_builder_exact(plan, type, &thing));
  CHECK(type_bridge_plan_builder_field(plan, thing, key, &binding));
  CHECK(type_bridge_plan_builder_value_string(plan, view(key_value), &literal));
  CHECK(against(plan, binding, TYPE_BRIDGE_PLAN_COMPARE_EQUAL, literal));
  CHECK(type_bridge_plan_builder_field(plan, thing, field, &binding));
  CHECK(type_bridge_plan_builder_value_long(plan, value, &literal));
  CHECK(against(plan, binding, TYPE_BRIDGE_PLAN_COMPARE_EQUAL, literal));
  CHECK(type_bridge_plan_builder_count(plan));
done:
  return status;
}

static type_bridge_status_t balance_is(type_bridge_plan_builder_t *plan, const identities_t *id,
                                       const char *account_id, int64_t value) {
  return count_where(plan, id->account, id->account_id, account_id, id->balance, value);
}

static type_bridge_status_t stock_is(type_bridge_plan_builder_t *plan, const identities_t *id,
                                     const char *sku, int64_t value) {
  return count_where(plan, id->item, id->sku, sku, id->stock, value);
}

static type_bridge_status_t order_lines(type_bridge_plan_builder_t *plan, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t line;
  CHECK(type_bridge_plan_builder_exact(plan, id->order_line, &line));
  CHECK(type_bridge_plan_builder_count(plan));
done:
  return status;
}

static type_bridge_status_t checkout_a1(type_bridge_plan_builder_t *plan, const identities_t *id) {
  static const line_t CART[2] = {{"pen", 2}, {"book", 1}};
  return checkout(plan, id, "A1", "O1", CART, 2u);
}

static type_bridge_status_t a1_paid(type_bridge_plan_builder_t *plan, const identities_t *id) {
  return balance_is(plan, id, "A1", 54);
}

static type_bridge_status_t book_sold_out(type_bridge_plan_builder_t *plan,
                                          const identities_t *id) {
  return stock_is(plan, id, "book", 0);
}

static type_bridge_status_t pens_left(type_bridge_plan_builder_t *plan, const identities_t *id) {
  return stock_is(plan, id, "pen", 8);
}

static type_bridge_status_t checkout_a2(type_bridge_plan_builder_t *plan, const identities_t *id) {
  static const line_t CART[2] = {{"mug", 1}, {"book", 1}};
  return checkout(plan, id, "A2", "O2", CART, 2u);
}

static type_bridge_status_t checkout_short(type_bridge_plan_builder_t *plan,
                                           const identities_t *id) {
  static const line_t CART[1] = {{"mug", 3}};
  return checkout(plan, id, "A2", "O3", CART, 1u);
}

static type_bridge_status_t mugs_kept(type_bridge_plan_builder_t *plan, const identities_t *id) {
  return stock_is(plan, id, "mug", 5);
}

static type_bridge_status_t a2_kept(type_bridge_plan_builder_t *plan, const identities_t *id) {
  return balance_is(plan, id, "A2", 30);
}

static type_bridge_status_t checkout_a2_mug(type_bridge_plan_builder_t *plan,
                                            const identities_t *id) {
  static const line_t CART[1] = {{"mug", 1}};
  return checkout(plan, id, "A2", "O4", CART, 1u);
}

static type_bridge_status_t checkout_a2_pens(type_bridge_plan_builder_t *plan,
                                             const identities_t *id) {
  static const line_t CART[1] = {{"pen", 6}};
  return checkout(plan, id, "A2", "O5", CART, 1u);
}

static type_bridge_status_t a2_spent(type_bridge_plan_builder_t *plan, const identities_t *id) {
  return balance_is(plan, id, "A2", 0);
}

static type_bridge_status_t pens_final(type_bridge_plan_builder_t *plan, const identities_t *id) {
  return stock_is(plan, id, "pen", 2);
}

static type_bridge_status_t mugs_final(type_bridge_plan_builder_t *plan, const identities_t *id) {
  return stock_is(plan, id, "mug", 4);
}

static type_bridge_status_t race_lamp(type_bridge_plan_builder_t *plan, const identities_t *id) {
  static const line_t CART[1] = {{"lamp", 1}};
  return checkout(plan, id, "R1", "L1", CART, 1u);
}

static type_bridge_status_t lamp_sold_out(type_bridge_plan_builder_t *plan,
                                          const identities_t *id) {
  return stock_is(plan, id, "lamp", 0);
}

static type_bridge_status_t r1_paid_once(type_bridge_plan_builder_t *plan,
                                         const identities_t *id) {
  return balance_is(plan, id, "R1", 90);
}

typedef type_bridge_status_t (*plan_fn)(type_bridge_plan_builder_t *, const identities_t *);

static const char *const NAMES[PLAN_COUNT] = {
    "seed_accounts",   "seed_items",       "checkout_a1", "a1_paid",
    "book_sold_out",   "pens_left",        "lines",       "checkout_a2",
    "checkout_short",  "mugs_kept",        "a2_kept",     "checkout_a2_mug",
    "checkout_a2_pens", "a2_spent",        "pens_final",  "mugs_final",
    "race_lamp",       "lamp_sold_out",    "r1_paid_once"};
static const plan_fn BUILDERS[PLAN_COUNT] = {
    seed_accounts,    seed_items,     checkout_a1, a1_paid,
    book_sold_out,    pens_left,      order_lines, checkout_a2,
    checkout_short,   mugs_kept,      a2_kept,     checkout_a2_mug,
    checkout_a2_pens, a2_spent,       pens_final,  mugs_final,
    race_lamp,        lamp_sold_out,  r1_paid_once};

/* A write or read step stages the plans it names, by index into NAMES, in
 * one transaction and commits once; every read step also counts `lines`.
 * A race step commits its one plan from BUYERS transactions at once. */
typedef enum step_kind { STEP_WRITE, STEP_READ, STEP_RACE } step_kind_t;

typedef struct step {
  const char *name;
  step_kind_t kind;
  size_t count;
  size_t plans[MAX_STEP_PLANS];
} step_t;

static const step_t STEPS[STEP_COUNT] = {
    {"seed", STEP_WRITE, 2u, {0u, 1u}},
    {"checkout", STEP_WRITE, 1u, {2u}},
    {"checked_out", STEP_READ, 4u, {3u, 4u, 5u, 6u}},
    {"sold_out", STEP_WRITE, 1u, {7u}},
    {"short", STEP_WRITE, 1u, {8u}},
    {"unchanged", STEP_READ, 3u, {9u, 10u, 6u}},
    {"two_carts", STEP_WRITE, 2u, {11u, 12u}},
    {"final", STEP_READ, 4u, {13u, 14u, 15u, 6u}},
    {"race", STEP_RACE, 1u, {16u}},
    {"raced", STEP_READ, 3u, {17u, 18u, 6u}},
};

/* Build plan `index` of NAMES into `out`. */
static type_bridge_status_t build_one(const type_bridge_schema_package_t *package, size_t index,
                                      type_bridge_plan_t **out) {
  type_bridge_status_t status;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_plan_builder_t *plan = NULL;
  identities_t id;
  status = type_bridge_plan_builder_new(&plan);
  if (status == TYPE_BRIDGE_STATUS_OK) status = identify(plan, &id);
  if (status == TYPE_BRIDGE_STATUS_OK) status = BUILDERS[index](plan, &id);
  if (status == TYPE_BRIDGE_STATUS_OK)
    status = type_bridge_plan_builder_build(plan, package, out, &diagnostics);
  type_bridge_plan_builder_free(&plan);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  if (status != TYPE_BRIDGE_STATUS_OK)
    fprintf(stderr, "plan %s does not build: status %d\n", NAMES[index], (int)status);
  return status;
}

static type_bridge_status_t build_all(const type_bridge_schema_package_t *package,
                                      type_bridge_plan_t **plans) {
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_OK;
  size_t index;
  for (index = 0u; status == TYPE_BRIDGE_STATUS_OK && index < PLAN_COUNT; ++index)
    status = build_one(package, index, &plans[index]);
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

/* What a fresh connection opens: the direct database (`config`) or the
 * proxy (`options`). */
typedef struct source {
  const type_bridge_runtime_t *runtime;
  const type_bridge_schema_package_t *package;
  const type_bridge_database_config_v1_t *config;
  const type_bridge_proxy_connection_options_v1_t *options;
} source_t;

/* One open connection: a direct database or a proxy connection. */
typedef struct connection {
  type_bridge_database_t *database;
  type_bridge_proxy_connection_t *proxy;
} connection_t;

static type_bridge_status_t connect_to(const source_t *source, connection_t *connection) {
  type_bridge_status_t status;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  if (source->config != NULL)
    status = type_bridge_database_open_v1(source->runtime, source->package, source->config, NULL,
                                          &connection->database, &diagnostics);
  else
    status = type_bridge_proxy_connection_open_v1(source->package, source->options, NULL,
                                                  &connection->proxy, &diagnostics);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  return status;
}

static void disconnect(connection_t *connection) {
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  if (connection->database != NULL)
    (void)type_bridge_database_close(&connection->database, &diagnostics);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  (void)type_bridge_proxy_connection_close(&connection->proxy);
}

/* Opens one transaction of `kind` on the direct database or the proxy. */
static type_bridge_status_t open_transaction(const connection_t *connection, uint32_t kind,
                                             type_bridge_staged_transaction_t **transaction,
                                             type_bridge_execution_diagnostics_t **diagnostics) {
  if (connection->database != NULL)
    return type_bridge_database_transaction(connection->database, kind, transaction, diagnostics);
  return type_bridge_proxy_connection_transaction(connection->proxy, kind, transaction,
                                                  diagnostics);
}

/* --8<-- [start:stage] */
/* Stage `step`'s plans in one transaction, commit once, and print their
 * answers as `"name":{...}`. */
static type_bridge_status_t run_step(const connection_t *connection, const step_t *step,
                                     const char *separator, type_bridge_plan_t *const *plans) {
  type_bridge_status_t status;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_staged_transaction_t *transaction = NULL;
  type_bridge_pending_t *pending[MAX_STEP_PLANS] = {NULL, NULL, NULL, NULL};
  uint32_t kind =
      step->kind == STEP_READ ? TYPE_BRIDGE_TRANSACTION_READ : TYPE_BRIDGE_TRANSACTION_WRITE;
  size_t index;
  status = open_transaction(connection, kind, &transaction, &diagnostics);
  for (index = 0u; status == TYPE_BRIDGE_STATUS_OK && index < step->count; ++index)
    status = type_bridge_staged_transaction_stage(transaction, plans[step->plans[index]],
                                                  &pending[index], &diagnostics);
  if (status == TYPE_BRIDGE_STATUS_OK)
    status = type_bridge_staged_transaction_commit(transaction, &diagnostics);
  /* The step committed; each answer is known only now. */
  if (status == TYPE_BRIDGE_STATUS_OK) {
    printf("%s\"%s\":{", separator, step->name);
    for (index = 0u; status == TYPE_BRIDGE_STATUS_OK && index < step->count; ++index) {
      type_bridge_plan_answer_kind_t answer = TYPE_BRIDGE_PLAN_ANSWER_ROWS;
      uint64_t value = 0u;
      status = type_bridge_pending_answer(pending[index], &answer, &value);
      printf("%s\"%s\":[\"%s\",%llu]", index == 0u ? "" : ",", NAMES[step->plans[index]],
             answer_kind(answer), (unsigned long long)value);
    }
    printf("}");
  }
  type_bridge_staged_transaction_close(&transaction);
  for (index = 0u; index < MAX_STEP_PLANS; ++index) type_bridge_pending_free(&pending[index]);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  return status;
}
/* --8<-- [end:stage] */

/* 1 when the first typed failure in `diagnostics` is a retry-safe conflict:
 * the commit lost the race and stored nothing. */
static int retry_safe_conflict(const type_bridge_execution_diagnostics_t *diagnostics) {
  size_t count = 0u, index;
  if (type_bridge_execution_diagnostics_count(diagnostics, &count) != TYPE_BRIDGE_STATUS_OK)
    return 0;
  for (index = 0u; index < count; ++index) {
    type_bridge_execution_failure_v1_t failure;
    if (type_bridge_execution_diagnostics_failure_get_v1(diagnostics, index, &failure) !=
        TYPE_BRIDGE_STATUS_OK)
      return 0;
    if (failure.outcome != TYPE_BRIDGE_FAILURE_OUTCOME_NONE)
      return failure.reason == TYPE_BRIDGE_FAILURE_REASON_CONFLICT && failure.retry_safe != 0;
  }
  return 0;
}

/* One racing buyer. Its connection, plan and transactions are its own: the
 * ABI does not document a handle as shareable across threads. */
typedef struct buyer {
  connection_t connection;
  type_bridge_plan_t *plan;
  pthread_barrier_t *ready;
  uint64_t matched;
  int conflicts;
  type_bridge_status_t status;
} buyer_t;

/* --8<-- [start:race] */
/* Stage the plan, wait until every buyer has staged so the commits overlap,
 * then commit. A buyer that loses gets a retry-safe conflict and nothing it
 * staged is stored; it runs the whole transaction again on a fresh snapshot,
 * where the lamp is gone. */
static void *buy(void *argument) {
  buyer_t *buyer = (buyer_t *)argument;
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_OK;
  unsigned attempt;
  for (attempt = 0u; attempt < ATTEMPTS; ++attempt) {
    type_bridge_staged_transaction_t *transaction = NULL;
    type_bridge_pending_t *pending = NULL;
    type_bridge_execution_diagnostics_t *diagnostics = NULL;
    int retry = 0;
    status = open_transaction(&buyer->connection, TYPE_BRIDGE_TRANSACTION_WRITE, &transaction,
                              &diagnostics);
    if (status == TYPE_BRIDGE_STATUS_OK)
      status = type_bridge_staged_transaction_stage(transaction, buyer->plan, &pending,
                                                    &diagnostics);
    /* Every buyer reaches the barrier once, even after a failure, so none
     * waits forever. */
    if (attempt == 0u) (void)pthread_barrier_wait(buyer->ready);
    if (status == TYPE_BRIDGE_STATUS_OK) {
      status = type_bridge_staged_transaction_commit(transaction, &diagnostics);
      retry = status == TYPE_BRIDGE_STATUS_EXECUTION_FAILED && attempt + 1u < ATTEMPTS &&
              retry_safe_conflict(diagnostics);
    }
    if (status == TYPE_BRIDGE_STATUS_OK) {
      type_bridge_plan_answer_kind_t kind = TYPE_BRIDGE_PLAN_ANSWER_ROWS;
      status = type_bridge_pending_answer(pending, &kind, &buyer->matched);
      if (status == TYPE_BRIDGE_STATUS_OK && kind != TYPE_BRIDGE_PLAN_ANSWER_MATCHED)
        status = TYPE_BRIDGE_STATUS_PANIC;
    }
    type_bridge_staged_transaction_close(&transaction);
    type_bridge_pending_free(&pending);
    (void)type_bridge_execution_diagnostics_close(&diagnostics);
    if (!retry) break;
    ++buyer->conflicts;
  }
  buyer->status = status;
  return NULL;
}

/* BUYERS buyers commit plan `index` at once; `out_matched` is the sum of
 * their matched counts. */
static type_bridge_status_t race(const source_t *source, size_t index, uint64_t *out_matched) {
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_OK;
  buyer_t buyers[BUYERS];
  pthread_t threads[BUYERS];
  pthread_barrier_t ready;
  int conflicts = 0;
  size_t buyer;
  *out_matched = 0u;
  for (buyer = 0u; buyer < BUYERS; ++buyer) {
    buyers[buyer].connection.database = NULL;
    buyers[buyer].connection.proxy = NULL;
    buyers[buyer].plan = NULL;
    buyers[buyer].ready = &ready;
    buyers[buyer].matched = 0u;
    buyers[buyer].conflicts = 0;
    buyers[buyer].status = TYPE_BRIDGE_STATUS_OK;
  }
  /* Connections and plans are opened here, one thread at a time. */
  for (buyer = 0u; status == TYPE_BRIDGE_STATUS_OK && buyer < BUYERS; ++buyer) {
    status = connect_to(source, &buyers[buyer].connection);
    if (status == TYPE_BRIDGE_STATUS_OK)
      status = build_one(source->package, index, &buyers[buyer].plan);
  }
  if (status == TYPE_BRIDGE_STATUS_OK && pthread_barrier_init(&ready, NULL, BUYERS) != 0)
    status = TYPE_BRIDGE_STATUS_PANIC;
  if (status == TYPE_BRIDGE_STATUS_OK) {
    for (buyer = 0u; buyer < BUYERS; ++buyer) {
      if (pthread_create(&threads[buyer], NULL, buy, &buyers[buyer]) != 0) {
        /* The started buyers wait at the barrier for all BUYERS. */
        fprintf(stderr, "race: cannot start buyer %u\n", (unsigned)buyer);
        exit(EXIT_FAILURE);
      }
    }
    for (buyer = 0u; buyer < BUYERS; ++buyer) (void)pthread_join(threads[buyer], NULL);
    (void)pthread_barrier_destroy(&ready);
    for (buyer = 0u; buyer < BUYERS; ++buyer) {
      if (status == TYPE_BRIDGE_STATUS_OK) status = buyers[buyer].status;
      *out_matched += buyers[buyer].matched;
      conflicts += buyers[buyer].conflicts;
    }
    /* Diagnostic only: how many commits lost and were retried varies by run. */
    if (status == TYPE_BRIDGE_STATUS_OK) fprintf(stderr, "race: %d conflicts retried\n", conflicts);
  }
  for (buyer = 0u; buyer < BUYERS; ++buyer) {
    type_bridge_plan_free(&buyers[buyer].plan);
    disconnect(&buyers[buyer].connection);
  }
  return status;
}
/* --8<-- [end:race] */

/* Run every step on `connection`, printing `{"seed":{...},...}`; the race
 * step opens its buyers' connections from `source`. */
static type_bridge_status_t run_program(const source_t *source, const connection_t *connection,
                                        type_bridge_plan_t *const *plans) {
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_OK;
  size_t step;
  printf("{");
  for (step = 0u; step < STEP_COUNT; ++step) {
    const step_t *current = &STEPS[step];
    const char *separator = step == 0u ? "" : ",";
    if (current->kind == STEP_RACE) {
      uint64_t matched = 0u;
      status = race(source, current->plans[0], &matched);
      if (status == TYPE_BRIDGE_STATUS_OK)
        printf("%s\"%s\":{\"%s\":[\"matched\",%llu]}", separator, current->name,
               NAMES[current->plans[0]], (unsigned long long)matched);
    } else {
      status = run_step(connection, current, separator, plans);
    }
    if (status != TYPE_BRIDGE_STATUS_OK) {
      fprintf(stderr, "step %s failed: status %d\n", current->name, (int)status);
      break;
    }
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
  connection_t direct = {NULL, NULL}, proxied = {NULL, NULL};
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
    source_t source = {NULL, NULL, NULL, NULL};
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
    source.runtime = runtime;
    source.package = package;
    source.config = &config;
    CHECK(connect_to(&source, &direct));
    printf("\"direct\":");
    CHECK(run_program(&source, &direct, plans));
    separator = ",";
  }
  if (endpoint != NULL) {
    type_bridge_proxy_connection_options_v1_t options;
    source_t source = {NULL, NULL, NULL, NULL};
    memset(&options, 0, sizeof(options));
    options.struct_size = sizeof(options);
    options.version = TYPE_BRIDGE_PROXY_CONNECTION_OPTIONS_VERSION;
    options.endpoint = view(endpoint);
    options.tls_mode = TYPE_BRIDGE_TLS_DISABLED;
    source.package = package;
    source.options = &options;
    CHECK(connect_to(&source, &proxied));
    printf("%s\"proxy\":", separator);
    CHECK(run_program(&source, &proxied, plans));
  }
  printf("}}\n");

done:
  for (index = 0u; index < PLAN_COUNT; ++index) type_bridge_plan_free(&plans[index]);
  disconnect(&proxied);
  disconnect(&direct);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  if (runtime != NULL) (void)type_bridge_runtime_close(&runtime, &diagnostics);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  if (package_diagnostics != NULL) (void)type_bridge_diagnostics_close(&package_diagnostics);
  if (package != NULL) (void)type_bridge_schema_package_close(&package);
  fflush(stdout);
  return status == TYPE_BRIDGE_STATUS_OK ? 0 : 1;
}
