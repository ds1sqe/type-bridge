/*
 * The major TypeBridge pipelines through the C SDK, against a live TypeDB.
 *
 *   pipelines --out FILE --workspace DIR [--iterations N] [--warmup N]
 *       [--pipeline NAME]...
 *
 * The same data pipelines, seeds and plans as the Rust driver
 * (type-bridge-core/crates/perf/src/bin/pipelines.rs), written as the
 * `type-bridge-perf-pipelines/v1` report. `migration_apply` is a CLI
 * operation with no C SDK entry, so only the Rust report has it.
 *
 * Plans are built with the ABI 1.7 plan builder against <tb_perf/models.h>,
 * the C package `python -m perf.drivers.workspace create` generates into
 * DIR/workspace/generated/c, and staged on one ABI 1.7 staged transaction
 * per iteration. Stage times come from `tb_perf_stages_json`, which only a
 * `type-bridge-c` built with `--features perf-stages` exports; it is no part
 * of the C ABI, so it is declared here and in no header. Build:
 *
 *   cargo build --release -p type-bridge-c --features perf-stages
 *   cc -std=c17 -O2 -Wall -Wextra -Werror -I DIR/workspace/generated/c/include \
 *     -I type-bridge-core/crates/c/include perf/drivers/c/pipelines.c \
 *     DIR/workspace/generated/c/src/models.c -L type-bridge-core/target/release \
 *     -ltype_bridge_c -Wl,-rpath,type-bridge-core/target/release -o pipelines
 *
 * `--workspace` is that workspace's DIR/workspace: this driver runs its
 * `apply` and `drop` commands through TB_PERF_SETUP, a JSON array of strings
 * naming a command line that takes `apply|drop DATABASE`. TypeDB comes from
 * TYPEDB_ADDRESS, TYPEDB_HTTP_PORT, TYPEDB_USERNAME and TYPEDB_PASSWORD.
 */
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include <typebridge/type_bridge.h>
#include <tb_perf/models.h>

/* The private recorder of a `perf-stages` build; see the header comment. */
size_t tb_perf_stages_json(char *buffer, size_t capacity);

#define SEEDED_ITEMS 1000
#define FILTER_STOCK 900
#define BATCH_ROWS 1000
#define MIXED_BATCH_ROWS 50
#define DOCTORS 10

#define MAX_PLANS 3u
#define MAX_ARGS 32u

typedef struct pipeline_spec {
  const char *name;
  unsigned operations;
} pipeline_spec_t;

static const pipeline_spec_t PIPELINES[] = {
    {"insert_one", 1},
    {"fetch_by_key", 1},
    {"fetch_filtered", 1},
    {"update", 1},
    {"delete", 1},
    {"batch_given_rows_1k", BATCH_ROWS},
    {"staged_mixed", 1},
};
#define PIPELINE_COUNT (sizeof(PIPELINES) / sizeof(PIPELINES[0]))

#define CHECK(call)                                                            \
  do {                                                                         \
    if ((status = (call)) != TYPE_BRIDGE_STATUS_OK) {                          \
      fprintf(stderr, "%s:%d: status %d\n", __func__, __LINE__, (int)status);  \
      goto done;                                                               \
    }                                                                          \
  } while (0)

static void fail(const char *message) {
  fprintf(stderr, "error: %s\n", message);
  exit(1);
}

static type_bridge_byte_view_t view(const char *text) {
  type_bridge_byte_view_t value;
  value.data = (const uint8_t *)text;
  value.length = text == NULL ? 0u : strlen(text);
  return value;
}

/* ---------------------------------------------------------------- tokens */

/* The package's types, fields and roles, registered once per builder. */
typedef struct identities {
  uint32_t item, account, doctor, purchase;
  uint32_t item_id, stock, price, account_id, balance, name, on_call, order_id;
  uint32_t buyer, bought;
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
  CHECK(type_bridge_plan_builder_role(plan, id->purchase, view("item"), &id->bought));
done:
  return status;
}

static type_bridge_status_t input(type_bridge_plan_builder_t *plan, const char *name,
                                  type_bridge_projected_value_kind_t kind, uint32_t *out) {
  return type_bridge_plan_builder_input(plan, view(name), kind, 0u, out);
}

static type_bridge_status_t compare(type_bridge_plan_builder_t *plan, uint32_t binding,
                                    uint32_t op, uint32_t kind,
                                    uint32_t operand) {
  return type_bridge_plan_builder_compare(plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, binding, op,
                                          kind, operand);
}

/* `thing.field == literal`, binding the field. */
static type_bridge_status_t key_is(type_bridge_plan_builder_t *plan, uint32_t thing,
                                   uint32_t field, const char *key) {
  type_bridge_status_t status;
  uint32_t binding, value;
  CHECK(type_bridge_plan_builder_field(plan, thing, field, &binding));
  CHECK(type_bridge_plan_builder_value_string(plan, view(key), &value));
  CHECK(compare(plan, binding, TYPE_BRIDGE_PLAN_COMPARE_EQUAL, TYPE_BRIDGE_PLAN_OPERAND_VALUE,
                value));
done:
  return status;
}

/* `thing.field == input`, the keyed match of one given row. */
static type_bridge_status_t key_is_input(type_bridge_plan_builder_t *plan, uint32_t thing,
                                         uint32_t field, uint32_t id) {
  type_bridge_status_t status;
  uint32_t binding;
  CHECK(type_bridge_plan_builder_field(plan, thing, field, &binding));
  CHECK(compare(plan, binding, TYPE_BRIDGE_PLAN_COMPARE_EQUAL, TYPE_BRIDGE_PLAN_OPERAND_INPUT, id));
done:
  return status;
}

static type_bridge_status_t string_row(type_bridge_plan_builder_t *plan, const char *text) {
  type_bridge_status_t status;
  uint32_t value;
  CHECK(type_bridge_plan_builder_value_string(plan, view(text), &value));
  CHECK(type_bridge_plan_builder_row(plan, &value, 1u));
done:
  return status;
}

static type_bridge_status_t set_long(type_bridge_plan_builder_t *plan, uint32_t thing,
                                     uint32_t field, int64_t literal) {
  type_bridge_status_t status;
  uint32_t value;
  CHECK(type_bridge_plan_builder_value_long(plan, literal, &value));
  CHECK(type_bridge_plan_builder_thing_field(plan, thing, field, TYPE_BRIDGE_PLAN_OPERAND_VALUE,
                                             value));
done:
  return status;
}

static type_bridge_status_t set_string(type_bridge_plan_builder_t *plan, uint32_t thing,
                                       uint32_t field, const char *literal) {
  type_bridge_status_t status;
  uint32_t value;
  CHECK(type_bridge_plan_builder_value_string(plan, view(literal), &value));
  CHECK(type_bridge_plan_builder_thing_field(plan, thing, field, TYPE_BRIDGE_PLAN_OPERAND_VALUE,
                                             value));
done:
  return status;
}

/* ----------------------------------------------------------------- plans */

/* What one plan builds: the builder, its parameters, and its rows. */
typedef struct plan_args {
  const char *prefix;
  const char *key;
  unsigned from, to;
} plan_args_t;

typedef type_bridge_status_t (*plan_fn)(type_bridge_plan_builder_t *, const identities_t *,
                                        const plan_args_t *);

static type_bridge_status_t insert_items(type_bridge_plan_builder_t *plan, const identities_t *id,
                                         const plan_args_t *args) {
  type_bridge_status_t status;
  uint32_t key, stock, price, item, row[3];
  unsigned number;
  char text[64];
  CHECK(input(plan, "item_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &key));
  CHECK(input(plan, "stock", TYPE_BRIDGE_PROJECTED_VALUE_LONG, &stock));
  CHECK(input(plan, "price", TYPE_BRIDGE_PROJECTED_VALUE_LONG, &price));
  CHECK(type_bridge_plan_builder_insert(plan, id->item, &item));
  CHECK(type_bridge_plan_builder_thing_field(plan, item, id->item_id,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, key));
  CHECK(type_bridge_plan_builder_thing_field(plan, item, id->stock, TYPE_BRIDGE_PLAN_OPERAND_INPUT,
                                             stock));
  CHECK(type_bridge_plan_builder_thing_field(plan, item, id->price, TYPE_BRIDGE_PLAN_OPERAND_INPUT,
                                             price));
  for (number = args->from; number < args->to; ++number) {
    snprintf(text, sizeof(text), "%s%u", args->prefix, number);
    CHECK(type_bridge_plan_builder_value_string(plan, view(text), &row[0]));
    CHECK(type_bridge_plan_builder_value_long(plan, (int64_t)number, &row[1]));
    CHECK(type_bridge_plan_builder_value_long(plan, 5, &row[2]));
    CHECK(type_bridge_plan_builder_row(plan, row, 3u));
  }
done:
  return status;
}

static type_bridge_status_t fetch_by_key(type_bridge_plan_builder_t *plan, const identities_t *id,
                                         const plan_args_t *args) {
  type_bridge_status_t status;
  uint32_t item, key, returned[2];
  CHECK(type_bridge_plan_builder_exact(plan, id->item, &item));
  CHECK(input(plan, "item_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &key));
  CHECK(type_bridge_plan_builder_field(plan, item, id->stock, &returned[0]));
  CHECK(type_bridge_plan_builder_field(plan, item, id->price, &returned[1]));
  CHECK(key_is_input(plan, item, id->item_id, key));
  CHECK(type_bridge_plan_builder_returning(plan, returned, 2u));
  CHECK(string_row(plan, args->key));
done:
  return status;
}

static type_bridge_status_t fetch_filtered(type_bridge_plan_builder_t *plan,
                                           const identities_t *id, const plan_args_t *args) {
  type_bridge_status_t status;
  uint32_t item, returned[2], floor;
  (void)args;
  CHECK(type_bridge_plan_builder_exact(plan, id->item, &item));
  CHECK(type_bridge_plan_builder_field(plan, item, id->item_id, &returned[0]));
  CHECK(type_bridge_plan_builder_field(plan, item, id->stock, &returned[1]));
  CHECK(type_bridge_plan_builder_value_long(plan, FILTER_STOCK, &floor));
  CHECK(compare(plan, returned[1], TYPE_BRIDGE_PLAN_COMPARE_GREATER_OR_EQUAL,
                TYPE_BRIDGE_PLAN_OPERAND_VALUE, floor));
  CHECK(type_bridge_plan_builder_returning(plan, returned, 2u));
done:
  return status;
}

static type_bridge_status_t restock(type_bridge_plan_builder_t *plan, const identities_t *id,
                                    const plan_args_t *args) {
  type_bridge_status_t status;
  uint32_t item, key, stock, one, sum, next;
  CHECK(type_bridge_plan_builder_exact(plan, id->item, &item));
  CHECK(input(plan, "item_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &key));
  CHECK(type_bridge_plan_builder_field(plan, item, id->stock, &stock));
  CHECK(key_is_input(plan, item, id->item_id, key));
  CHECK(type_bridge_plan_builder_value_long(plan, 1, &one));
  CHECK(type_bridge_plan_builder_expr_arith(plan, TYPE_BRIDGE_PLAN_ARITH_ADD,
                                            TYPE_BRIDGE_PLAN_OPERAND_BINDING, stock,
                                            TYPE_BRIDGE_PLAN_OPERAND_VALUE, one, &sum));
  CHECK(type_bridge_plan_builder_let(plan, view("next_stock"), TYPE_BRIDGE_PLAN_OPERAND_EXPR, sum,
                                     &next));
  CHECK(type_bridge_plan_builder_update(plan, item, id->stock, TYPE_BRIDGE_PLAN_OPERAND_BINDING,
                                        next));
  CHECK(string_row(plan, args->key));
done:
  return status;
}

static type_bridge_status_t delete_item(type_bridge_plan_builder_t *plan, const identities_t *id,
                                        const plan_args_t *args) {
  type_bridge_status_t status;
  uint32_t item, key;
  CHECK(type_bridge_plan_builder_exact(plan, id->item, &item));
  CHECK(input(plan, "item_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &key));
  CHECK(key_is_input(plan, item, id->item_id, key));
  CHECK(type_bridge_plan_builder_delete(plan, item));
  CHECK(string_row(plan, args->key));
done:
  return status;
}

static type_bridge_status_t owners(type_bridge_plan_builder_t *plan, const identities_t *id,
                                   const plan_args_t *args) {
  type_bridge_status_t status;
  uint32_t item, account;
  (void)args;
  CHECK(type_bridge_plan_builder_insert(plan, id->item, &item));
  CHECK(set_string(plan, item, id->item_id, "I1"));
  CHECK(set_long(plan, item, id->stock, 1000000));
  CHECK(set_long(plan, item, id->price, 1));
  CHECK(type_bridge_plan_builder_insert(plan, id->account, &account));
  CHECK(set_string(plan, account, id->account_id, "A1"));
  CHECK(set_long(plan, account, id->balance, 1000000000));
done:
  return status;
}

static type_bridge_status_t doctors(type_bridge_plan_builder_t *plan, const identities_t *id,
                                    const plan_args_t *args) {
  type_bridge_status_t status;
  uint32_t name, doctor, yes;
  unsigned number;
  char text[16];
  (void)args;
  CHECK(input(plan, "name", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &name));
  CHECK(type_bridge_plan_builder_insert(plan, id->doctor, &doctor));
  CHECK(type_bridge_plan_builder_thing_field(plan, doctor, id->name,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, name));
  CHECK(type_bridge_plan_builder_value_boolean(plan, 1u, &yes));
  CHECK(type_bridge_plan_builder_thing_field(plan, doctor, id->on_call,
                                             TYPE_BRIDGE_PLAN_OPERAND_VALUE, yes));
  for (number = 0u; number < DOCTORS; ++number) {
    snprintf(text, sizeof(text), "D%u", number);
    CHECK(string_row(plan, text));
  }
done:
  return status;
}

/* The one-query purchase: take one item and its price, record the order. */
static type_bridge_status_t purchase(type_bridge_plan_builder_t *plan, const identities_t *id,
                                     const plan_args_t *args) {
  type_bridge_status_t status;
  uint32_t item, buyer, order_id, stock, price, balance, zero, one;
  uint32_t stock_expr, balance_expr, next_stock, next_balance, order;
  CHECK(type_bridge_plan_builder_exact(plan, id->item, &item));
  CHECK(type_bridge_plan_builder_exact(plan, id->account, &buyer));
  CHECK(input(plan, "order_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &order_id));
  CHECK(type_bridge_plan_builder_field(plan, item, id->stock, &stock));
  CHECK(type_bridge_plan_builder_field(plan, item, id->price, &price));
  CHECK(type_bridge_plan_builder_field(plan, buyer, id->balance, &balance));
  CHECK(key_is(plan, item, id->item_id, "I1"));
  CHECK(key_is(plan, buyer, id->account_id, "A1"));
  CHECK(type_bridge_plan_builder_value_long(plan, 0, &zero));
  CHECK(compare(plan, stock, TYPE_BRIDGE_PLAN_COMPARE_GREATER, TYPE_BRIDGE_PLAN_OPERAND_VALUE,
                zero));
  CHECK(compare(plan, balance, TYPE_BRIDGE_PLAN_COMPARE_GREATER_OR_EQUAL,
                TYPE_BRIDGE_PLAN_OPERAND_BINDING, price));
  CHECK(type_bridge_plan_builder_value_long(plan, 1, &one));
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
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, order_id));
  CHECK(type_bridge_plan_builder_thing_player(plan, order, id->buyer, buyer));
  CHECK(type_bridge_plan_builder_thing_player(plan, order, id->bought, item));
  CHECK(string_row(plan, args->key));
done:
  return status;
}

/* Rewrite every doctor's on-call value with itself, taking its lock. */
static type_bridge_status_t touch(type_bridge_plan_builder_t *plan, const identities_t *id,
                                  const plan_args_t *args) {
  type_bridge_status_t status;
  uint32_t doctor, on_call;
  (void)args;
  CHECK(type_bridge_plan_builder_exact(plan, id->doctor, &doctor));
  CHECK(type_bridge_plan_builder_field(plan, doctor, id->on_call, &on_call));
  CHECK(type_bridge_plan_builder_update(plan, doctor, id->on_call,
                                        TYPE_BRIDGE_PLAN_OPERAND_BINDING, on_call));
done:
  return status;
}

/* `to` orders of the item by the account, one given row each. */
static type_bridge_status_t orders(type_bridge_plan_builder_t *plan, const identities_t *id,
                                   const plan_args_t *args) {
  type_bridge_status_t status;
  uint32_t item, buyer, order_id, order;
  unsigned number;
  char text[64];
  CHECK(type_bridge_plan_builder_exact(plan, id->item, &item));
  CHECK(type_bridge_plan_builder_exact(plan, id->account, &buyer));
  CHECK(input(plan, "order_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &order_id));
  CHECK(key_is(plan, item, id->item_id, "I1"));
  CHECK(key_is(plan, buyer, id->account_id, "A1"));
  CHECK(type_bridge_plan_builder_insert(plan, id->purchase, &order));
  CHECK(type_bridge_plan_builder_thing_field(plan, order, id->order_id,
                                             TYPE_BRIDGE_PLAN_OPERAND_INPUT, order_id));
  CHECK(type_bridge_plan_builder_thing_player(plan, order, id->buyer, buyer));
  CHECK(type_bridge_plan_builder_thing_player(plan, order, id->bought, item));
  for (number = 0u; number < args->to; ++number) {
    snprintf(text, sizeof(text), "%s%u", args->prefix, number);
    CHECK(string_row(plan, text));
  }
done:
  return status;
}

static type_bridge_status_t build(const type_bridge_schema_package_t *package, plan_fn builder,
                                  const plan_args_t *args, type_bridge_plan_t **out) {
  type_bridge_status_t status;
  type_bridge_plan_builder_t *plan = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  identities_t id;
  CHECK(type_bridge_plan_builder_new(&plan));
  CHECK(identify(plan, &id));
  CHECK(builder(plan, &id, args));
  CHECK(type_bridge_plan_builder_build(plan, package, out, &diagnostics));
done:
  type_bridge_plan_builder_free(&plan);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  return status;
}

/* ------------------------------------------------------------ iterations */

typedef struct context {
  const type_bridge_schema_package_t *package;
  const type_bridge_database_t *database;
} context_t;

typedef struct staged {
  plan_fn builder;
  plan_args_t args;
  unsigned rows; /* the expected answer; 0 skips the check (seeding) */
} staged_t;

/* Build `plans`, stage them in one transaction, commit, check the answers. */
static type_bridge_status_t commit(const context_t *context, uint32_t kind, const staged_t *plans,
                                   size_t count) {
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_OK;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_staged_transaction_t *transaction = NULL;
  type_bridge_plan_t *built[MAX_PLANS] = {NULL, NULL, NULL};
  type_bridge_pending_t *pending[MAX_PLANS] = {NULL, NULL, NULL};
  size_t index;
  for (index = 0u; index < count; ++index)
    CHECK(build(context->package, plans[index].builder, &plans[index].args, &built[index]));
  CHECK(type_bridge_database_transaction(context->database, kind, &transaction, &diagnostics));
  for (index = 0u; index < count; ++index)
    CHECK(type_bridge_staged_transaction_stage(transaction, built[index], &pending[index],
                                               &diagnostics));
  CHECK(type_bridge_staged_transaction_commit(transaction, &diagnostics));
  for (index = 0u; index < count; ++index) {
    type_bridge_plan_answer_kind_t answer = TYPE_BRIDGE_PLAN_ANSWER_ROWS;
    uint64_t value = 0u;
    CHECK(type_bridge_pending_answer(pending[index], &answer, &value));
    if (plans[index].rows == 0u) continue;
    if ((answer != TYPE_BRIDGE_PLAN_ANSWER_ROWS && answer != TYPE_BRIDGE_PLAN_ANSWER_MATCHED &&
         answer != TYPE_BRIDGE_PLAN_ANSWER_COUNT) ||
        value != plans[index].rows) {
      fprintf(stderr, "unexpected answer kind %d value %llu, expected %u rows\n", (int)answer,
              (unsigned long long)value, plans[index].rows);
      status = TYPE_BRIDGE_STATUS_EXECUTION_FAILED;
      goto done;
    }
  }
done:
  /* ABI 1.7: commit keeps the handle; close releases it. */
  type_bridge_staged_transaction_close(&transaction);
  for (index = 0u; index < MAX_PLANS; ++index) {
    type_bridge_pending_free(&pending[index]);
    type_bridge_plan_free(&built[index]);
  }
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  return status;
}

static type_bridge_status_t seed(const char *name, const context_t *context, unsigned iterations) {
  if (!strcmp(name, "fetch_by_key") || !strcmp(name, "fetch_filtered") || !strcmp(name, "update")) {
    staged_t plans[] = {{insert_items, {"item-", NULL, 0u, SEEDED_ITEMS}, 0u}};
    return commit(context, TYPE_BRIDGE_TRANSACTION_WRITE, plans, 1u);
  }
  if (!strcmp(name, "delete")) {
    staged_t plans[] = {{insert_items, {"doomed-", NULL, 0u, iterations}, 0u}};
    return commit(context, TYPE_BRIDGE_TRANSACTION_WRITE, plans, 1u);
  }
  if (!strcmp(name, "staged_mixed")) {
    staged_t plans[] = {{owners, {NULL, NULL, 0u, 0u}, 0u}, {doctors, {NULL, NULL, 0u, 0u}, 0u}};
    return commit(context, TYPE_BRIDGE_TRANSACTION_WRITE, plans, 2u);
  }
  return TYPE_BRIDGE_STATUS_OK;
}

static type_bridge_status_t iteration(const char *name, const context_t *context,
                                      unsigned number) {
  const uint32_t read = TYPE_BRIDGE_TRANSACTION_READ, write = TYPE_BRIDGE_TRANSACTION_WRITE;
  char key[64], prefix[64];
  snprintf(key, sizeof(key), "item-%u", number % SEEDED_ITEMS);
  if (!strcmp(name, "insert_one")) {
    staged_t plans[] = {{insert_items, {"new-", NULL, number, number + 1u}, 1u}};
    return commit(context, write, plans, 1u);
  }
  if (!strcmp(name, "fetch_by_key")) {
    staged_t plans[] = {{fetch_by_key, {NULL, key, 0u, 0u}, 1u}};
    return commit(context, read, plans, 1u);
  }
  if (!strcmp(name, "fetch_filtered")) {
    staged_t plans[] = {{fetch_filtered, {NULL, NULL, 0u, 0u}, SEEDED_ITEMS - FILTER_STOCK}};
    return commit(context, read, plans, 1u);
  }
  if (!strcmp(name, "update")) {
    staged_t plans[] = {{restock, {NULL, key, 0u, 0u}, 1u}};
    return commit(context, write, plans, 1u);
  }
  if (!strcmp(name, "delete")) {
    snprintf(key, sizeof(key), "doomed-%u", number);
    staged_t plans[] = {{delete_item, {NULL, key, 0u, 0u}, 1u}};
    return commit(context, write, plans, 1u);
  }
  if (!strcmp(name, "batch_given_rows_1k")) {
    snprintf(prefix, sizeof(prefix), "batch-%u-", number);
    staged_t plans[] = {{insert_items, {prefix, NULL, 0u, BATCH_ROWS}, BATCH_ROWS}};
    return commit(context, write, plans, 1u);
  }
  if (!strcmp(name, "staged_mixed")) {
    snprintf(key, sizeof(key), "O%u", number);
    snprintf(prefix, sizeof(prefix), "B%u-", number);
    staged_t plans[] = {{purchase, {NULL, key, 0u, 0u}, 1u},
                        {touch, {NULL, NULL, 0u, 0u}, DOCTORS},
                        {orders, {prefix, NULL, 0u, MIXED_BATCH_ROWS}, MIXED_BATCH_ROWS}};
    return commit(context, write, plans, 3u);
  }
  fprintf(stderr, "%s is not a data pipeline\n", name);
  return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
}

/* ---------------------------------------------------------------- driver */

typedef struct config {
  const char *out, *workspace;
  unsigned iterations, warmup;
  const char *only[PIPELINE_COUNT];
  size_t only_count;
} config_t;

static unsigned count_of(const char *text) {
  char *end = NULL;
  unsigned long value;
  errno = 0;
  value = strtoul(text, &end, 10);
  if (errno != 0 || end == text || *end != '\0' || value < 1ul || value > 1000000ul)
    fail("counts must be positive integers");
  return (unsigned)value;
}

static int known(const char *name) {
  size_t index;
  for (index = 0u; index < PIPELINE_COUNT; ++index)
    if (!strcmp(PIPELINES[index].name, name)) return 1;
  return 0;
}

static config_t parse(int argc, char **argv) {
  config_t config;
  int index;
  memset(&config, 0, sizeof(config));
  config.iterations = 30u;
  config.warmup = 5u;
  for (index = 1; index < argc; index += 2) {
    const char *flag = argv[index], *value = index + 1 < argc ? argv[index + 1] : NULL;
    if (value == NULL) fail("every flag takes a value");
    if (!strcmp(flag, "--out")) config.out = value;
    else if (!strcmp(flag, "--workspace")) config.workspace = value;
    else if (!strcmp(flag, "--iterations")) config.iterations = count_of(value);
    else if (!strcmp(flag, "--warmup")) config.warmup = count_of(value);
    else if (!strcmp(flag, "--pipeline")) {
      if (!known(value)) fail("unknown pipeline");
      if (config.only_count < PIPELINE_COUNT) config.only[config.only_count++] = value;
    } else fail("unknown argument");
  }
  if (config.out == NULL || config.workspace == NULL)
    fail("--out FILE and --workspace DIR are required");
  if (getenv("TYPEDB_USERNAME") == NULL || getenv("TYPEDB_PASSWORD") == NULL ||
      getenv("TB_PERF_SETUP") == NULL)
    fail("set TYPEDB_USERNAME, TYPEDB_PASSWORD and TB_PERF_SETUP");
  return config;
}

/* Parse TB_PERF_SETUP, a JSON array of strings, into `argv` (NULL-ended). */
static size_t setup_command(char *text, char **argv) {
  size_t count = 0u;
  char *cursor = text, *write;
  while (*cursor == ' ') ++cursor;
  if (*cursor++ != '[') fail("TB_PERF_SETUP is not a JSON array of strings");
  for (;;) {
    while (*cursor == ' ' || *cursor == ',') ++cursor;
    if (*cursor == ']') break;
    if (*cursor++ != '"' || count + 3u >= MAX_ARGS) fail("TB_PERF_SETUP is not a JSON array of strings");
    argv[count++] = write = cursor;
    while (*cursor != '"') {
      if (*cursor == '\0') fail("TB_PERF_SETUP is not a JSON array of strings");
      if (*cursor == '\\') {
        ++cursor;
        if (*cursor != '"' && *cursor != '\\' && *cursor != '/')
          fail("TB_PERF_SETUP uses an unsupported escape");
      }
      *write++ = *cursor++;
    }
    ++cursor;
    *write = '\0';
  }
  if (count == 0u) fail("TB_PERF_SETUP is empty");
  return count;
}

/* Run the harness's workspace setup command: `apply|drop DATABASE`. */
static void setup(const char *action, const char *database) {
  char *text = strdup(getenv("TB_PERF_SETUP"));
  char *argv[MAX_ARGS];
  size_t count;
  pid_t child;
  int wstatus = 0;
  if (text == NULL) fail("out of memory");
  count = setup_command(text, argv);
  argv[count] = (char *)action;
  argv[count + 1u] = (char *)database;
  argv[count + 2u] = NULL;
  fflush(NULL);
  child = fork();
  if (child < 0) fail("fork failed");
  if (child == 0) {
    int null = open("/dev/null", O_RDWR);
    if (null >= 0) {
      dup2(null, STDIN_FILENO);
      dup2(null, STDOUT_FILENO);
    }
    execvp(argv[0], argv);
    _exit(127);
  }
  if (waitpid(child, &wstatus, 0) < 0 || !WIFEXITED(wstatus) || WEXITSTATUS(wstatus) != 0) {
    fprintf(stderr, "error: setup %s %s failed\n", action, database);
    exit(1);
  }
  free(text);
}

static uint32_t http_port(void) {
  const char *text = getenv("TYPEDB_HTTP_PORT");
  char *end = NULL;
  unsigned long value;
  if (text == NULL || text[0] == '\0') return 8000u;
  errno = 0;
  value = strtoul(text, &end, 10);
  if (errno != 0 || end == text || *end != '\0' || value == 0ul || value > 65535ul)
    fail("TYPEDB_HTTP_PORT is not a port");
  return (uint32_t)value;
}

static uint64_t now_ns(void) {
  struct timespec at;
  clock_gettime(CLOCK_MONOTONIC, &at);
  return (uint64_t)at.tv_sec * 1000000000u + (uint64_t)at.tv_nsec;
}

/* The stages since the last call as a JSON object, freshly allocated. */
static char *take_stages(void) {
  size_t capacity = 256u;
  for (;;) {
    char *buffer = malloc(capacity);
    size_t length;
    if (buffer == NULL) fail("out of memory");
    length = tb_perf_stages_json(buffer, capacity);
    if (length == SIZE_MAX) fail("another tracing subscriber prevents the stage recorder");
    if (length < capacity) return buffer;
    free(buffer);
    capacity = length + 1u;
  }
}

typedef struct sample {
  uint64_t total_ns;
  char *stages;
} sample_t;

static sample_t *run_pipeline(const char *name, const config_t *config,
                              const type_bridge_runtime_t *runtime,
                              const type_bridge_schema_package_t *package) {
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_OK;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_database_t *database = NULL;
  type_bridge_database_config_v1_t db_config;
  unsigned total = config->warmup + config->iterations, number;
  sample_t *samples = calloc(config->iterations, sizeof(sample_t));
  char database_name[128];
  context_t context;
  if (samples == NULL) fail("out of memory");
  snprintf(database_name, sizeof(database_name), "tb_perf_c_%ld_%s", (long)getpid(), name);
  setup("apply", database_name);
  memset(&db_config, 0, sizeof(db_config));
  db_config.struct_size = sizeof(db_config);
  db_config.version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION;
  db_config.address = view(getenv("TYPEDB_ADDRESS") ? getenv("TYPEDB_ADDRESS") : "localhost:1729");
  db_config.database = view(database_name);
  db_config.username = view(getenv("TYPEDB_USERNAME"));
  db_config.password = view(getenv("TYPEDB_PASSWORD"));
  db_config.http_port = http_port();
  db_config.tls_mode = TYPE_BRIDGE_TLS_DISABLED;
  CHECK(type_bridge_database_open_v1(runtime, package, &db_config, NULL, &database, &diagnostics));
  context.package = package;
  context.database = database;
  CHECK(seed(name, &context, total));
  for (number = 0u; number < total; ++number) {
    uint64_t started, elapsed;
    char *stages;
    free(take_stages());
    started = now_ns();
    CHECK(iteration(name, &context, number));
    elapsed = now_ns() - started;
    stages = take_stages();
    if (number >= config->warmup) {
      samples[number - config->warmup].total_ns = elapsed;
      samples[number - config->warmup].stages = stages;
    } else {
      free(stages);
    }
  }
done:
  (void)type_bridge_database_close(&database, &diagnostics);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  setup("drop", database_name);
  if (status != TYPE_BRIDGE_STATUS_OK) {
    fprintf(stderr, "error: pipeline %s failed\n", name);
    exit(1);
  }
  return samples;
}

/* `mkdir -p` of the directory holding `path`. */
static void make_parent(const char *path) {
  char *copy = strdup(path), *slash;
  if (copy == NULL) fail("out of memory");
  for (slash = strchr(copy + 1, '/'); slash != NULL; slash = strchr(slash + 1, '/')) {
    *slash = '\0';
    if (mkdir(copy, 0777) != 0 && errno != EEXIST) fail("cannot create the report directory");
    *slash = '/';
  }
  free(copy);
}

int main(int argc, char **argv) {
  config_t config = parse(argc, argv);
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_OK;
  type_bridge_schema_package_t *package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_runtime_config_v1_t runtime_config = {
      sizeof(type_bridge_runtime_config_v1_t), TYPE_BRIDGE_RUNTIME_CONFIG_VERSION, 2u, 0u,
      {0u, 0u, 0u, 0u}};
  sample_t *results[PIPELINE_COUNT] = {NULL};
  FILE *out;
  size_t index, written = 0u, only;
  unsigned number;
  CHECK(tb_perf_schema_package_open(&package, &package_diagnostics));
  CHECK(type_bridge_runtime_open_v1(&runtime_config, &runtime, &diagnostics));
  for (index = 0u; index < PIPELINE_COUNT; ++index) {
    int wanted = config.only_count == 0u;
    for (only = 0u; only < config.only_count; ++only)
      if (!strcmp(config.only[only], PIPELINES[index].name)) wanted = 1;
    if (!wanted) continue;
    fprintf(stderr, "pipeline %s\n", PIPELINES[index].name);
    results[index] = run_pipeline(PIPELINES[index].name, &config, runtime, package);
  }
  make_parent(config.out);
  out = fopen(config.out, "w");
  if (out == NULL) fail("cannot write the report");
  fprintf(out, "{\"format\":\"type-bridge-perf-pipelines/v1\",\"binding\":\"c\",\"warmup\":%u,"
               "\"pipelines\":[",
          config.warmup);
  for (index = 0u; index < PIPELINE_COUNT; ++index) {
    if (results[index] == NULL) continue;
    fprintf(out, "%s{\"name\":\"%s\",\"operations\":%u,\"samples\":[", written++ ? "," : "",
            PIPELINES[index].name, PIPELINES[index].operations);
    for (number = 0u; number < config.iterations; ++number) {
      /* The recorder writes its stages sorted by name. */
      fprintf(out, "%s{\"total_ns\":%llu,\"stages\":%s}", number ? "," : "",
              (unsigned long long)results[index][number].total_ns, results[index][number].stages);
      free(results[index][number].stages);
    }
    fprintf(out, "]}");
    free(results[index]);
  }
  fprintf(out, "]}\n");
  if (fclose(out) != 0) fail("cannot write the report");
done:
  (void)type_bridge_runtime_close(&runtime, &diagnostics);
  (void)type_bridge_execution_diagnostics_close(&diagnostics);
  (void)type_bridge_diagnostics_close(&package_diagnostics);
  type_bridge_schema_package_close(&package);
  return status == TYPE_BRIDGE_STATUS_OK ? 0 : 1;
}
