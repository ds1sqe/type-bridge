/* The runner owns schema setup and deletes its database on every exit. */
#include "json.h"
#include <assert.h>
#include <inttypes.h>
#include <stdbool.h>
#include <tb_walkthrough/models.h>
#include <typebridge/type_bridge.h>

static type_bridge_execution_diagnostics_t *diagnostics;
static type_bridge_database_t *database;
static type_bridge_schema_package_t *package;
static char *purchase_buyer_iid, *purchase_item_iid;
static void checked(type_bridge_status_t status, const char *call, int line) {
  if (status != TYPE_BRIDGE_STATUS_OK) {
    fprintf(stderr, "line %d: %s returned %d\n", line, call, (int)status);
    type_bridge_execution_diagnostic_view_v1_t diagnostic;
    if (diagnostics &&
        type_bridge_execution_diagnostics_get_v1(diagnostics, 0, &diagnostic) ==
            TYPE_BRIDGE_STATUS_OK) {
      fprintf(stderr, "%.*s: %.*s\n", (int)diagnostic.code.length,
              diagnostic.code.data, (int)diagnostic.message.length,
              diagnostic.message.data);
    }
    exit(1);
  }
}
#define OK(call) checked((call), #call, __LINE__)
static type_bridge_byte_view_t view(const char *s) {
  type_bridge_byte_view_t v = {(const uint8_t *)s, strlen(s)};
  return v;
}
static char *copy(type_bridge_byte_view_t v) {
  char *s = malloc(v.length + 1);
  assert(s);
  memcpy(s, v.data, v.length);
  s[v.length] = 0;
  return s;
}
static char *events[32];
static size_t event_count;
static void TYPE_BRIDGE_CALL listener(type_bridge_byte_view_t json,
                                      void *context) {
  (void)context;
  char *text = copy(json);
  const char *name = json_member(text, "name");
  if (name && strncmp(name, "\"type_bridge.io.provider.statement\"", 35) == 0) {
    assert(event_count < 32);
    events[event_count++] = text;
  } else
    free(text);
}
static void clear_events(size_t expected) {
  assert(event_count == expected);
  for (size_t i = 0; i < event_count; ++i)
    free(events[i]);
  event_count = 0;
}
static void trace(const char *name) {
  begin(name, "plan");
  fputs("{\"source\":\"TRACE\",\"fingerprints\":[", stdout);
  for (size_t i = 0; i < event_count; ++i) {
    if (i)
      putchar(',');
    const char *fingerprint =
        json_member(json_member(events[i], "fields"), "fingerprint");
    if (fingerprint)
      json_value(fingerprint);
    else
      json_string("");
  }
  fputs("]}", stdout);
  end();
  begin(name, "explain");
  fputs("{\"source\":\"TRACE\",\"statements\":[", stdout);
  for (size_t i = 0; i < event_count; ++i) {
    if (i)
      putchar(',');
    json_value(json_member(json_member(events[i], "fields"), "statement"));
  }
  fputs("]}", stdout);
  end();
}
static void call(const char *name, const char *code) {
  begin(name, "call");
  json_string(code);
  end();
}
static void identity(const char *type, const char *iid, bool deleted) {
  fputs("{\"type\":", stdout);
  json_string(type);
  fputs(",\"iid\":", stdout);
  json_string(iid);
  printf(",\"deleted\":%s", deleted ? "true" : "false");
}
static void string_field(const char *name, const char *value) {
  putchar(',');
  json_string(name);
  putchar(':');
  json_string(value);
}
static void long_field(const char *name, int64_t value, bool present) {
  putchar(',');
  json_string(name);
  putchar(':');
  if (present)
    printf("%" PRId64, value);
  else
    fputs("null", stdout);
}
#define ACCOUNT_FIELDS(v)                                                      \
  do {                                                                         \
    identity("account", (v).iid, (v).deleted);                                 \
    string_field("account_id", (v).accountzhid);                               \
    long_field("balance", (v).balance, (v).has_balance);                       \
    putchar('}');                                                              \
  } while (0)
#define ITEM_FIELDS(v)                                                         \
  do {                                                                         \
    identity("item", (v).iid, (v).deleted);                                    \
    string_field("sku", (v).sku);                                              \
    long_field("price", (v).price, (v).has_price);                             \
    long_field("stock", (v).stock, (v).has_stock);                             \
    putchar('}');                                                              \
  } while (0)
#define DOCUMENT_FIELDS(type, v)                                               \
  do {                                                                         \
    identity(type, (v).iid, (v).deleted);                                      \
    string_field("doc_id", (v).doczhid);                                       \
    string_field("status", (v).status);                                        \
    putchar('}');                                                              \
  } while (0)
static void order_fields(tb_walkthrough_orderzhline_plan_view_t v) {
  if (!purchase_buyer_iid) {
    assert(v.buyer_count == 1 && v.item_count == 1);
    assert(v.buyer[0].balance == 7 && v.item[0].stock == 4);
    purchase_buyer_iid = copy(view(v.buyer[0].iid));
    purchase_item_iid = copy(view(v.item[0].iid));
  }
  identity("order-line", v.iid, v.deleted);
  string_field("order_id", v.has_orderzhid ? v.orderzhid : NULL);
  long_field("quantity", v.quantity, v.has_quantity);
  fputs(",\"buyer\":", stdout);
  if (v.buyer_count) {
    fputs("{\"type\":\"account\",\"iid\":", stdout);
    json_string(v.buyer[0].iid);
    putchar('}');
  } else
    fputs("null", stdout);
  fputs(",\"item\":", stdout);
  if (v.item_count) {
    fputs("{\"type\":\"item\",\"iid\":", stdout);
    json_string(v.item[0].iid);
    putchar('}');
  } else
    fputs("null", stdout);
  putchar('}');
}
static void print_models(type_bridge_pending_t *pending, unsigned model,
                         bool array) {
  type_bridge_plan_result_t *result = NULL;
  size_t count;
  OK(type_bridge_pending_plan_result(pending, &result, &diagnostics));
  OK(type_bridge_plan_result_count(result, &count));
  if (array)
    putchar('[');
  for (size_t i = 0; i < count; ++i) {
    if (i)
      putchar(',');
    if (model == 0) {
      tb_walkthrough_account_plan_view_t v;
      OK(tb_walkthrough_account_result_at(
          (const tb_walkthrough_account_result_t *)result, i, &v));
      ACCOUNT_FIELDS(v);
    } else if (model == 2) {
      tb_walkthrough_item_plan_view_t v;
      OK(tb_walkthrough_item_result_at(
          (const tb_walkthrough_item_result_t *)result, i, &v));
      if (count == 10000) {
        char key[32];
        snprintf(key, sizeof(key), "B%zu", i);
        assert(strcmp(v.sku, key) == 0 && v.has_stock && v.stock == 10 &&
               v.has_price && v.price == 3);
      }
      ITEM_FIELDS(v);
    } else if (model == 5) {
      tb_walkthrough_orderzhline_plan_view_t v;
      OK(tb_walkthrough_orderzhline_result_at(
          (const tb_walkthrough_orderzhline_result_t *)result, i, &v));
      order_fields(v);
    } else {
      tb_walkthrough_document_plan_view_t v;
      OK(tb_walkthrough_document_result_at(
          (const tb_walkthrough_document_result_t *)result, i, &v));
      if (v.kind == tb_walkthrough_projected_model_token_3.ordinal) {
        DOCUMENT_FIELDS("memo", v.value.memo);
      } else {
        assert(v.kind == tb_walkthrough_projected_model_token_4.ordinal);
        DOCUMENT_FIELDS("report", v.value.report);
      }
    }
  }
  if (array)
    putchar(']');
  type_bridge_plan_result_free(&result);
}
/* Reproduce the fetch document through immutable public answer cursors. */
static void document_attributes(const type_bridge_document_t *doc,
                                const char *type) {
  const char *fields[3];
  size_t count;
  if (!strcmp(type, "item")) {
    fields[0] = "price";
    fields[1] = "sku";
    fields[2] = "stock";
    count = 3;
  } else if (!strcmp(type, "account")) {
    fields[0] = "account-id";
    fields[1] = "balance";
    count = 2;
  } else if (!strcmp(type, "order-line")) {
    fields[0] = "order-id";
    fields[1] = "quantity";
    count = 2;
  } else {
    fields[0] = "doc-id";
    fields[1] = "status";
    count = 2;
  }
  putchar('{');
  for (size_t i = 0; i < count; ++i) {
    size_t n;
    OK(type_bridge_document_attribute_count(doc, view(fields[i]), &n));
    assert(n == 1);
    if (i)
      putchar(',');
    json_string(fields[i]);
    putchar(':');
    type_bridge_value_kind_t kind;
    OK(type_bridge_document_attribute_kind(doc, view(fields[i]), 0, &kind));
    if (kind == TYPE_BRIDGE_VALUE_STRING) {
      type_bridge_byte_view_t s;
      OK(type_bridge_document_attribute_string(doc, view(fields[i]), 0, &s));
      json_string_n((const char *)s.data, s.length);
    } else {
      int64_t value;
      assert(kind == TYPE_BRIDGE_VALUE_LONG);
      OK(type_bridge_document_attribute_long(doc, view(fields[i]), 0, &value));
      printf("%" PRId64, value);
    }
  }
  putchar('}');
}
static void print_raw(type_bridge_pending_t *pending, bool array) {
  size_t count;
  OK(type_bridge_pending_document_count(pending, &count));
  if (array)
    putchar('[');
  for (size_t i = 0; i < count; ++i) {
    const type_bridge_document_t *doc;
    type_bridge_byte_view_t iid, type;
    OK(type_bridge_pending_document(pending, i, &doc));
    OK(type_bridge_document_iid(doc, &iid));
    OK(type_bridge_document_type(doc, &type));
    char *label = copy(type);
    if (i)
      putchar(',');
    fputs("{\"_iid\":", stdout);
    json_string_n((const char *)iid.data, iid.length);
    fputs(",\"_type\":", stdout);
    json_string(label);
    fputs(",\"attributes\":", stdout);
    document_attributes(doc, label);
    if (!strcmp(label, "order-line"))
      for (size_t r = 0; r < 2; ++r) {
        const type_bridge_document_t *player;
        OK(type_bridge_document_role(doc, r, &player));
        assert(player);
        OK(type_bridge_document_iid(player, &iid));
        OK(type_bridge_document_type(player, &type));
        char *role_type = copy(type);
        printf(",\"_role_%zu_iid\":", r);
        json_string_n((const char *)iid.data, iid.length);
        printf(",\"_role_%zu_type\":", r);
        json_string(role_type);
        printf(",\"_role_%zu_attributes\":", r);
        document_attributes(player, role_type);
        free(role_type);
      }
    putchar('}');
    free(label);
  }
  if (array)
    putchar(']');
}
static type_bridge_plan_builder_t *recording(void) {
  type_bridge_plan_builder_t *p = NULL;
  OK(tb_walkthrough_plan_open(NULL, &p, &diagnostics));
  return p;
}
static void show(const char *name, type_bridge_plan_builder_t *p,
                 uint32_t selection, unsigned model, size_t expected,
                 bool split) {
  type_bridge_plan_t *inspection = NULL;
  type_bridge_canonical_bytes_t *explanation = NULL;
  char *explain = NULL;
  if (!split) {
    OK(type_bridge_plan_builder_selection_plan(p, selection, &inspection,
                                               &diagnostics));
    size_t n = 0;
    assert(type_bridge_plan_canonical_bytes(inspection, NULL, 0, &n) ==
           TYPE_BRIDGE_STATUS_RESOURCE_LIMIT);
    char *bytes = malloc(n + 1);
    assert(bytes);
    OK(type_bridge_plan_canonical_bytes(inspection, (uint8_t *)bytes, n, &n));
    bytes[n] = 0;
    begin(name, "plan");
    fputs("{\"source\":\"canonical_bytes\",\"plan\":", stdout);
    json_without(bytes, "given_rows");
    putchar('}');
    end();
    free(bytes);
    OK(type_bridge_plan_explain(inspection, &explanation, &diagnostics));
    type_bridge_byte_view_t text;
    OK(type_bridge_canonical_bytes_view(explanation, &text));
    explain = copy(text);
    begin(name, "explain");
    fputs("{\"source\":\"explain\",", stdout);
    fputs(explain + 1, stdout);
    end();
  }
  type_bridge_staged_transaction_t *tx = NULL;
  type_bridge_pending_t *pending = NULL;
  OK(type_bridge_database_transaction(database, TYPE_BRIDGE_TRANSACTION_WRITE,
                                      &tx, &diagnostics));
  OK(type_bridge_staged_transaction_stage_selection(tx, p, selection, &pending,
                                                    &diagnostics));
  OK(type_bridge_staged_transaction_commit(tx, &diagnostics));
  if (split)
    trace(name);
  else {
    const char *statement = json_space(json_member(explain, "statements")) + 1;
    for (size_t i = 0; i < event_count; ++i) {
      statement = json_space(statement);
      const char *actual =
          json_member(json_member(events[i], "fields"), "statement");
      assert(actual &&
             json_end(statement) - statement == json_end(actual) - actual &&
             !memcmp(statement, actual, (size_t)(json_end(actual) - actual)));
      statement = json_space(json_end(statement));
      if (*statement == ',')
        ++statement;
    }
    assert(*json_space(statement) == ']');
  }
  clear_events(expected);
  if (split) {
    size_t count;
    OK(type_bridge_pending_document_count(pending, &count));
    assert(count == 10000);
  }
  begin(name, "raw");
  print_raw(pending, true);
  end();
  begin(name, "models");
  print_models(pending, model, true);
  end();
  type_bridge_pending_free(&pending);
  type_bridge_staged_transaction_close(&tx);
  type_bridge_plan_builder_free(&p);
  type_bridge_plan_free(&inspection);
  type_bridge_canonical_bytes_close(&explanation);
  free(explain);
}
static void seed(void) {
  type_bridge_staged_transaction_t *tx = NULL;
  type_bridge_pending_t *pending[4] = {NULL};
  type_bridge_plan_builder_t *p[4] = {recording(), recording(), recording(),
                                      recording()};
  tb_walkthrough_account_plan_t account;
  tb_walkthrough_item_plan_t item;
  tb_walkthrough_report_plan_t report;
  tb_walkthrough_memo_plan_t memo;
  tb_walkthrough_account_insert_t a = {
      .accountzhid = "A1", .balance = 10, .has_balance = true};
  tb_walkthrough_item_insert_t i = {.sku = "I1",
                                    .price = 3,
                                    .has_price = true,
                                    .stock = 5,
                                    .has_stock = true};
  tb_walkthrough_report_insert_t r = {.doczhid = "R1", .status = "archived"};
  tb_walkthrough_memo_insert_t m = {.doczhid = "M1", .status = "archived"};
  OK(tb_walkthrough_account_insert(p[0], &a, &account));
  OK(tb_walkthrough_item_insert(p[1], &i, &item));
  OK(tb_walkthrough_report_insert(p[2], &r, &report));
  OK(tb_walkthrough_memo_insert(p[3], &m, &memo));
  uint32_t selections[4] = {account.selection, item.selection, report.selection,
                            memo.selection};
  OK(type_bridge_database_transaction(database, TYPE_BRIDGE_TRANSACTION_WRITE,
                                      &tx, &diagnostics));
  for (size_t j = 0; j < 4; ++j)
    OK(type_bridge_staged_transaction_stage_selection(
        tx, p[j], selections[j], &pending[j], &diagnostics));
  OK(type_bridge_staged_transaction_commit(tx, &diagnostics));
  clear_events(4);
  for (size_t j = 0; j < 4; ++j) {
    type_bridge_pending_free(&pending[j]);
    type_bridge_plan_builder_free(&p[j]);
  }
  type_bridge_staged_transaction_close(&tx);
}
static void purchase(void) {
  call(
      "purchase",
      "type_bridge_plan_builder_t *p = recording();\n"
      "tb_walkthrough_item_handle_t item;\n"
      "tb_walkthrough_account_handle_t buyer;\n"
      "OK(tb_walkthrough_item_match(p, \"I1\", &item));\n"
      "OK(tb_walkthrough_item_where(\n"
      "    p, item,\n"
      "    tb_long_gt(tb_walkthrough_item_plan_stock(p, item), tb_long(0))));\n"
      "OK(tb_walkthrough_account_match(p, \"A1\", &buyer));\n"
      "tb_long_t price = tb_walkthrough_item_plan_price(p, item);\n"
      "OK(tb_walkthrough_account_where(\n"
      "    p, buyer,\n"
      "    tb_long_ge(tb_walkthrough_account_plan_balance(p, buyer), "
      "price)));\n"
      "tb_walkthrough_item_set_t stock = {\n"
      "    .stock =\n"
      "        tb_long_sub(tb_walkthrough_item_plan_stock(p, item), "
      "tb_long(1))};\n"
      "OK(tb_walkthrough_item_set(p, item, &stock));\n"
      "tb_walkthrough_account_set_t balance = {\n"
      "    .balance =\n"
      "        tb_long_sub(tb_walkthrough_account_plan_balance(p, buyer), "
      "price)};\n"
      "OK(tb_walkthrough_account_set(p, buyer, &balance));\n"
      "tb_walkthrough_orderzhline_insert_t order = {.orderzhid = \"O1\",\n"
      "                                             .has_orderzhid = true,\n"
      "                                             .quantity = 1,\n"
      "                                             .has_quantity = true,\n"
      "                                             .buyer = buyer,\n"
      "                                             .item = item};\n"
      "tb_walkthrough_orderzhline_plan_t selected;\n"
      "OK(tb_walkthrough_orderzhline_insert(p, &order, &selected));");
  type_bridge_plan_builder_t *p = recording();
  tb_walkthrough_item_handle_t item;
  tb_walkthrough_account_handle_t buyer;
  OK(tb_walkthrough_item_match(p, "I1", &item));
  OK(tb_walkthrough_item_where(
      p, item,
      tb_long_gt(tb_walkthrough_item_plan_stock(p, item), tb_long(0))));
  OK(tb_walkthrough_account_match(p, "A1", &buyer));
  tb_long_t price = tb_walkthrough_item_plan_price(p, item);
  OK(tb_walkthrough_account_where(
      p, buyer,
      tb_long_ge(tb_walkthrough_account_plan_balance(p, buyer), price)));
  tb_walkthrough_item_set_t stock = {
      .stock =
          tb_long_sub(tb_walkthrough_item_plan_stock(p, item), tb_long(1))};
  OK(tb_walkthrough_item_set(p, item, &stock));
  tb_walkthrough_account_set_t balance = {
      .balance =
          tb_long_sub(tb_walkthrough_account_plan_balance(p, buyer), price)};
  OK(tb_walkthrough_account_set(p, buyer, &balance));
  tb_walkthrough_orderzhline_insert_t order = {.orderzhid = "O1",
                                               .has_orderzhid = true,
                                               .quantity = 1,
                                               .has_quantity = true,
                                               .buyer = buyer,
                                               .item = item};
  tb_walkthrough_orderzhline_plan_t selected;
  OK(tb_walkthrough_orderzhline_insert(p, &order, &selected));
  show("purchase", p, selected.selection, 5, 1, false);
}
static void delete_subtypes(void) {
  call("delete_subtypes",
       "type_bridge_plan_builder_t *p = recording();\n"
       "tb_walkthrough_document_handle_t document;\n"
       "tb_walkthrough_document_plan_t selected;\n"
       "OK(tb_walkthrough_document_subtypes(p, &document));\n"
       "OK(tb_walkthrough_document_where(\n"
       "    p, document,\n"
       "    tb_string_eq(tb_walkthrough_document_plan_status(p, document),\n"
       "                 tb_string(view(\"archived\")))));\n"
       "OK(tb_walkthrough_document_delete_returning(p, document, &selected));");
  type_bridge_plan_builder_t *p = recording();
  tb_walkthrough_document_handle_t document;
  tb_walkthrough_document_plan_t selected;
  OK(tb_walkthrough_document_subtypes(p, &document));
  OK(tb_walkthrough_document_where(
      p, document,
      tb_string_eq(tb_walkthrough_document_plan_status(p, document),
                   tb_string(view("archived")))));
  OK(tb_walkthrough_document_delete_returning(p, document, &selected));
  show("delete_subtypes", p, selected.selection, 1, 2, false);
}
static void bulk_insert(void) {
  call("bulk_insert",
       "type_bridge_plan_builder_t *p = recording();\n"
       "tb_walkthrough_item_plan_t selected;\n"
       "tb_walkthrough_item_values_t *rows = calloc(10000, sizeof(*rows));\n"
       "char (*keys)[16] = calloc(10000, sizeof(*keys));\n"
       "assert(rows && keys);\n"
       "for (size_t i = 0; i < 10000; ++i) {\n"
       "  snprintf(keys[i], 16, \"B%zu\", i);\n"
       "  rows[i].sku = keys[i];\n"
       "  rows[i].price = 3;\n"
       "  rows[i].has_price = true;\n"
       "  rows[i].stock = 10;\n"
       "  rows[i].has_stock = true;\n"
       "}\n"
       "OK(tb_walkthrough_item_insert_many(p, rows, 10000, &selected));\n"
       "free(rows);\n"
       "free(keys);");
  type_bridge_plan_builder_t *p = recording();
  tb_walkthrough_item_plan_t selected;
  tb_walkthrough_item_values_t *rows = calloc(10000, sizeof(*rows));
  char (*keys)[16] = calloc(10000, sizeof(*keys));
  assert(rows && keys);
  for (size_t i = 0; i < 10000; ++i) {
    snprintf(keys[i], 16, "B%zu", i);
    rows[i].sku = keys[i];
    rows[i].price = 3;
    rows[i].has_price = true;
    rows[i].stock = 10;
    rows[i].has_stock = true;
  }
  OK(tb_walkthrough_item_insert_many(p, rows, 10000, &selected));
  free(rows);
  free(keys);
  show("bulk_insert", p, selected.selection, 2, 3, true);
}
static tb_walkthrough_item_create *item_create(int64_t stock_value) {
  tb_walkthrough_price *price = NULL;
  tb_walkthrough_stock *stock = NULL;
  tb_walkthrough_sku *sku = NULL;
  tb_walkthrough_item_create *created = NULL;
  OK(tb_walkthrough_price_open(package, 3, &price, &diagnostics));
  OK(tb_walkthrough_stock_open(package, stock_value, &stock, &diagnostics));
  OK(tb_walkthrough_sku_open(package, view("M1"), &sku, &diagnostics));
  tb_walkthrough_item_create_args_v1_t args = {.struct_size = sizeof(args),
                                               .version = 1,
                                               .field_price = price,
                                               .field_sku = sku,
                                               .field_stock = stock};
  OK(tb_walkthrough_item_create_open(package, &args, &created, &diagnostics));
  OK(tb_walkthrough_price_close(&price));
  OK(tb_walkthrough_stock_close(&stock));
  OK(tb_walkthrough_sku_close(&sku));
  return created;
}
static void nominal_item(const tb_walkthrough_item *item, bool deleted) {
  type_bridge_byte_view_t iid, key;
  tb_walkthrough_sku *sku = NULL;
  tb_walkthrough_price *price = NULL;
  tb_walkthrough_stock *stock = NULL;
  int64_t p, s;
  OK(tb_walkthrough_item_iid(item, &iid, &diagnostics));
  char *id = copy(iid);
  identity("item", id, deleted);
  free(id);
  OK(tb_walkthrough_item_sku(item, &sku, &diagnostics));
  OK(tb_walkthrough_sku_value(sku, &key, &diagnostics));
  char *text = copy(key);
  string_field("sku", text);
  free(text);
  OK(tb_walkthrough_item_price(item, &price, &diagnostics));
  OK(tb_walkthrough_price_value(price, &p, &diagnostics));
  long_field("price", p, true);
  OK(tb_walkthrough_item_stock(item, &stock, &diagnostics));
  OK(tb_walkthrough_stock_value(stock, &s, &diagnostics));
  long_field("stock", s, true);
  putchar('}');
  OK(tb_walkthrough_sku_close(&sku));
  OK(tb_walkthrough_price_close(&price));
  OK(tb_walkthrough_stock_close(&stock));
}
static void manager_seam(const char *name, size_t expected) {
  trace(name);
  clear_events(expected);
  begin(name, "raw");
  json_string("raw documents: not exposed by manager result");
  end();
}
static void manager_items(const char *name,
                          type_bridge_projected_batch_result_t *result,
                          type_bridge_projected_batch_operation_t operation,
                          size_t statements) {
  size_t count;
  manager_seam(name, statements);
  begin(name, "models");
  putchar('[');
  OK(type_bridge_projected_batch_result_count(
      result, &tb_walkthrough_projected_model_token_2, operation, &count,
      &diagnostics));
  for (size_t i = 0; i < count; ++i) {
    type_bridge_projected_thing_t *thing = NULL;
    OK(type_bridge_projected_batch_result_thing_at(
        result, &tb_walkthrough_projected_model_token_2, operation, i, &thing,
        &diagnostics));
    if (i)
      putchar(',');
    nominal_item((const tb_walkthrough_item *)thing,
                 operation == TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_DELETE);
    OK(type_bridge_projected_thing_close(&thing));
  }
  putchar(']');
  end();
  OK(type_bridge_projected_batch_result_close(&result));
}
static void manager_read(const char *name,
                         const type_bridge_manager_filter_t *filter) {
  type_bridge_manager_result_t *result = NULL;
  size_t count;
  OK(type_bridge_database_manager_all(database, filter, NULL, NULL, &result,
                                      &diagnostics));
  manager_seam(name, 1);
  OK(type_bridge_manager_result_count(
      result, &tb_walkthrough_projected_model_token_2, &count, &diagnostics));
  assert(count == 1);
  begin(name, "models");
  putchar('[');
  for (size_t i = 0; i < count; ++i) {
    type_bridge_projected_thing_t *thing = NULL;
    OK(type_bridge_manager_result_at(result,
                                     &tb_walkthrough_projected_model_token_2, i,
                                     &thing, &diagnostics));
    if (i)
      putchar(',');
    nominal_item((const tb_walkthrough_item *)thing, false);
    OK(type_bridge_projected_thing_close(&thing));
  }
  putchar(']');
  end();
  OK(type_bridge_manager_result_close(&result));
}
static type_bridge_iid_t *manager_calls(void) {
  tb_walkthrough_item_create *created = item_create(5);
  tb_walkthrough_item_insert_batch_result *inserted = NULL;
  tb_walkthrough_item *item = NULL;
  type_bridge_iid_t *iid = NULL;
  type_bridge_byte_view_t text;
  call("insert", "tb_walkthrough_item_database_insert(database, created, NULL, "
                 "NULL, &inserted, &diagnostics);");
  OK(tb_walkthrough_item_database_insert(database, created, NULL, NULL,
                                         &inserted, &diagnostics));
  OK(tb_walkthrough_item_insert_batch_result_item(inserted, &item,
                                                  &diagnostics));
  OK(tb_walkthrough_item_iid(item, &text, &diagnostics));
  OK(type_bridge_iid_open(text, &iid));
  manager_items("insert", (type_bridge_projected_batch_result_t *)inserted,
                TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT, 1);
  OK(tb_walkthrough_item_close(&item));
  OK(tb_walkthrough_item_create_close(&created));
  created = item_create(6);
  tb_walkthrough_item_put_batch_result *put = NULL;
  call("put", "tb_walkthrough_item_database_put(database, created, NULL, NULL, "
              "&put, &diagnostics);");
  OK(tb_walkthrough_item_database_put(database, created, NULL, NULL, &put,
                                      &diagnostics));
  manager_items("put", (type_bridge_projected_batch_result_t *)put,
                TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_PUT, 1);
  OK(tb_walkthrough_item_create_close(&created));
  type_bridge_manager_filter_t *root = NULL, *selected = NULL, *filtered = NULL;
  OK(type_bridge_manager_filter_open(
      package, &tb_walkthrough_projected_model_token_2, &root, &diagnostics));
  call("typed_iid", "type_bridge_manager_filter_iid(root, iid, &selected, "
                    "&diagnostics);\ntype_bridge_database_manager_all(database,"
                    " selected, NULL, NULL, &result, &diagnostics);");
  OK(type_bridge_manager_filter_iid(root, iid, &selected, &diagnostics));
  manager_read("typed_iid", selected);
  tb_walkthrough_sku *sku = NULL;
  OK(tb_walkthrough_sku_open(package, view("M1"), &sku, &diagnostics));
  call("filtered_fetch",
       "type_bridge_manager_filter_compare(root, "
       "&tb_walkthrough_projected_field_token_5, "
       "TYPE_BRIDGE_QUERY_COMPARE_EQUAL, (const type_bridge_projected_value_t "
       "*)sku, &filtered, "
       "&diagnostics);\ntype_bridge_database_manager_all(database, filtered, "
       "NULL, NULL, &result, &diagnostics);");
  OK(type_bridge_manager_filter_compare(
      root, &tb_walkthrough_projected_field_token_5,
      TYPE_BRIDGE_QUERY_COMPARE_EQUAL,
      (const type_bridge_projected_value_t *)sku, &filtered, &diagnostics));
  manager_read("filtered_fetch", filtered);
  OK(tb_walkthrough_sku_close(&sku));
  created = item_create(8);
  type_bridge_projected_batch_result_t *updated = NULL;
  call("update", "type_bridge_database_manager_update(database, selected, "
                 "(const type_bridge_projected_create_t *)created, NULL, NULL, "
                 "&updated, &diagnostics);");
  OK(type_bridge_database_manager_update(
      database, selected, (const type_bridge_projected_create_t *)created, NULL,
      NULL, &updated, &diagnostics));
  manager_items("update", updated, TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_UPDATE,
                1);
  OK(tb_walkthrough_item_create_close(&created));
  OK(type_bridge_manager_filter_close(&root));
  OK(type_bridge_manager_filter_close(&selected));
  OK(type_bridge_manager_filter_close(&filtered));
  return iid;
}
static void manager_delete(type_bridge_iid_t **iid) {
  type_bridge_projected_batch_result_t *deleted = NULL;
  call("delete_returning", "type_bridge_database_manager_delete_returning("
                           "database, &tb_walkthrough_projected_model_token_2, "
                           "*iid, NULL, NULL, &deleted, &diagnostics);");
  OK(type_bridge_database_manager_delete_returning(
      database, &tb_walkthrough_projected_model_token_2, *iid, NULL, NULL,
      &deleted, &diagnostics));
  manager_items("delete_returning", deleted,
                TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_DELETE, 2);
  OK(type_bridge_iid_close(iid));
}
static void player_identity(const char *type, type_bridge_byte_view_t iid) {
  fputs("{\"type\":", stdout);
  json_string(type);
  fputs(",\"iid\":", stdout);
  json_string_n((const char *)iid.data, iid.length);
  putchar('}');
}
static void nominal_order(const tb_walkthrough_orderzhline *order) {
  type_bridge_byte_view_t iid, text;
  tb_walkthrough_orderzhid *key = NULL;
  tb_walkthrough_quantity *quantity = NULL;
  int64_t n;
  OK(tb_walkthrough_orderzhline_iid(order, &iid, &diagnostics));
  char *id = copy(iid);
  identity("order-line", id, false);
  free(id);
  OK(tb_walkthrough_orderzhline_orderzhid(order, &key, &diagnostics));
  OK(tb_walkthrough_orderzhid_value(key, &text, &diagnostics));
  char *label = copy(text);
  string_field("order_id", label);
  free(label);
  OK(tb_walkthrough_orderzhline_quantity(order, &quantity, &diagnostics));
  OK(tb_walkthrough_quantity_value(quantity, &n, &diagnostics));
  long_field("quantity", n, true);
  tb_walkthrough_orderzhline_buyer_player *buyer = NULL;
  tb_walkthrough_account_ref *account = NULL;
  tb_walkthrough_orderzhline_item_player *item = NULL;
  tb_walkthrough_item_ref *product = NULL;
  OK(tb_walkthrough_orderzhline_buyer(order, &buyer, &diagnostics));
  OK(tb_walkthrough_orderzhline_buyer_player_as_account(buyer, &account,
                                                        &diagnostics));
  OK(tb_walkthrough_account_ref_iid(account, &iid, &diagnostics));
  fputs(",\"buyer\":", stdout);
  player_identity("account", iid);
  OK(tb_walkthrough_orderzhline_item(order, &item, &diagnostics));
  OK(tb_walkthrough_orderzhline_item_player_as_item(item, &product,
                                                    &diagnostics));
  OK(tb_walkthrough_item_ref_iid(product, &iid, &diagnostics));
  fputs(",\"item\":", stdout);
  player_identity("item", iid);
  putchar('}');
  OK(tb_walkthrough_orderzhid_close(&key));
  OK(tb_walkthrough_quantity_close(&quantity));
  OK(tb_walkthrough_orderzhline_buyer_player_close(&buyer));
  OK(tb_walkthrough_account_ref_close(&account));
  OK(tb_walkthrough_orderzhline_item_player_close(&item));
  OK(tb_walkthrough_item_ref_close(&product));
}
static void insert_many(void) {
  tb_walkthrough_account_ref *account = NULL;
  tb_walkthrough_item_ref *item = NULL;
  tb_walkthrough_orderzhline_buyer_player *buyer = NULL;
  tb_walkthrough_orderzhline_item_player *product = NULL;
  OK(tb_walkthrough_account_ref_from_iid(package, view(purchase_buyer_iid),
                                         &account, &diagnostics));
  OK(tb_walkthrough_item_ref_from_iid(package, view(purchase_item_iid), &item,
                                      &diagnostics));
  OK(tb_walkthrough_orderzhline_buyer_player_from_account(account, &buyer,
                                                          &diagnostics));
  OK(tb_walkthrough_orderzhline_item_player_from_item(item, &product,
                                                      &diagnostics));
  type_bridge_projected_batch_builder_t *builder = NULL;
  type_bridge_projected_batch_t *batch = NULL;
  type_bridge_projected_batch_result_t *result = NULL;
  OK(type_bridge_projected_batch_builder_open_v1(
      package, &tb_walkthrough_projected_model_token_5,
      TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT, NULL, NULL, &builder,
      &diagnostics));
  for (size_t i = 0; i < 2; ++i) {
    tb_walkthrough_orderzhid *key = NULL;
    tb_walkthrough_quantity *quantity = NULL;
    tb_walkthrough_orderzhline_create *created = NULL;
    OK(tb_walkthrough_orderzhid_open(package, view(i ? "L2" : "L1"), &key,
                                     &diagnostics));
    OK(tb_walkthrough_quantity_open(package, 1, &quantity, &diagnostics));
    tb_walkthrough_orderzhline_create_args_v1_t args = {
        .struct_size = sizeof(args),
        .version = 1,
        .field_orderzhid = key,
        .field_quantity = quantity,
        .role_buyer = buyer,
        .role_item = product};
    OK(tb_walkthrough_orderzhline_create_open(package, &args, &created,
                                              &diagnostics));
    OK(type_bridge_projected_batch_builder_add_v1(
        builder, NULL, (const type_bridge_projected_create_t *)created,
        &diagnostics));
    OK(tb_walkthrough_orderzhline_create_close(&created));
    OK(tb_walkthrough_orderzhid_close(&key));
    OK(tb_walkthrough_quantity_close(&quantity));
  }
  OK(type_bridge_projected_batch_builder_finish(&builder, &batch,
                                                &diagnostics));
  call("insert_many",
       "tb_walkthrough_account_ref *account = NULL;\n"
       "tb_walkthrough_item_ref *item = NULL;\n"
       "tb_walkthrough_orderzhline_buyer_player *buyer = NULL;\n"
       "tb_walkthrough_orderzhline_item_player *product = NULL;\n"
       "OK(tb_walkthrough_account_ref_from_iid(package, "
       "view(purchase_buyer_iid),\n"
       "                                       &account, &diagnostics));\n"
       "OK(tb_walkthrough_item_ref_from_iid(package, view(purchase_item_iid), "
       "&item,\n"
       "                                    &diagnostics));\n"
       "OK(tb_walkthrough_orderzhline_buyer_player_from_account(account, "
       "&buyer,\n"
       "                                                        "
       "&diagnostics));\n"
       "OK(tb_walkthrough_orderzhline_item_player_from_item(item, &product,\n"
       "                                                    &diagnostics));\n"
       "type_bridge_projected_batch_builder_t *builder = NULL;\n"
       "type_bridge_projected_batch_t *batch = NULL;\n"
       "type_bridge_projected_batch_result_t *result = NULL;\n"
       "OK(type_bridge_projected_batch_builder_open_v1(\n"
       "    package, &tb_walkthrough_projected_model_token_5,\n"
       "    TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT, NULL, NULL, "
       "&builder,\n"
       "    &diagnostics));\n"
       "for (size_t i = 0; i < 2; ++i) {\n"
       "  tb_walkthrough_orderzhid *key = NULL;\n"
       "  tb_walkthrough_quantity *quantity = NULL;\n"
       "  tb_walkthrough_orderzhline_create *created = NULL;\n"
       "  OK(tb_walkthrough_orderzhid_open(package, view(i ? \"L2\" : \"L1\"), "
       "&key,\n"
       "                                   &diagnostics));\n"
       "  OK(tb_walkthrough_quantity_open(package, 1, &quantity, "
       "&diagnostics));\n"
       "  tb_walkthrough_orderzhline_create_args_v1_t args = {\n"
       "      .struct_size = sizeof(args),\n"
       "      .version = 1,\n"
       "      .field_orderzhid = key,\n"
       "      .field_quantity = quantity,\n"
       "      .role_buyer = buyer,\n"
       "      .role_item = product};\n"
       "  OK(tb_walkthrough_orderzhline_create_open(package, &args, &created,\n"
       "                                            &diagnostics));\n"
       "  OK(type_bridge_projected_batch_builder_add_v1(\n"
       "      builder, NULL, (const type_bridge_projected_create_t *)created,\n"
       "      &diagnostics));\n"
       "  OK(tb_walkthrough_orderzhline_create_close(&created));\n"
       "  OK(tb_walkthrough_orderzhid_close(&key));\n"
       "  OK(tb_walkthrough_quantity_close(&quantity));\n"
       "}\n"
       "OK(type_bridge_projected_batch_builder_finish(&builder, &batch,\n"
       "                                              &diagnostics));\n"
       "\n"
       "OK(type_bridge_database_projected_batch_execute_v1(\n"
       "    database, batch, NULL, NULL, &result, &diagnostics));");
  OK(type_bridge_database_projected_batch_execute_v1(
      database, batch, NULL, NULL, &result, &diagnostics));
  manager_seam("insert_many", 1);
  size_t count;
  OK(type_bridge_projected_batch_result_count(
      result, &tb_walkthrough_projected_model_token_5,
      TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT, &count, &diagnostics));
  assert(count == 2);
  begin("insert_many", "models");
  putchar('[');
  for (size_t i = 0; i < count; ++i) {
    type_bridge_projected_thing_t *thing = NULL;
    OK(type_bridge_projected_batch_result_thing_at(
        result, &tb_walkthrough_projected_model_token_5,
        TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT, i, &thing, &diagnostics));
    if (i)
      putchar(',');
    nominal_order((const tb_walkthrough_orderzhline *)thing);
    OK(type_bridge_projected_thing_close(&thing));
  }
  putchar(']');
  end();
  OK(type_bridge_projected_batch_result_close(&result));
  OK(type_bridge_projected_batch_close(&batch));
  OK(tb_walkthrough_account_ref_close(&account));
  OK(tb_walkthrough_item_ref_close(&item));
  OK(tb_walkthrough_orderzhline_buyer_player_close(&buyer));
  OK(tb_walkthrough_orderzhline_item_player_close(&product));
}
static void staged_dependency(void) {
  const char *name = "staged_transaction";
  call(name,
       "type_bridge_plan_builder_t *p[3] = {recording(), recording(), "
       "recording()};\n"
       "tb_walkthrough_account_plan_t account_plan;\n"
       "tb_walkthrough_item_plan_t item_plan;\n"
       "tb_walkthrough_orderzhline_plan_t order_plan;\n"
       "tb_walkthrough_account_insert_t account = {\n"
       "    .accountzhid = \"A2\", .balance = 20, .has_balance = true};\n"
       "tb_walkthrough_item_insert_t item = {.sku = \"I2\",\n"
       "                                     .price = 4,\n"
       "                                     .has_price = true,\n"
       "                                     .stock = 3,\n"
       "                                     .has_stock = true};\n"
       "OK(tb_walkthrough_account_insert(p[0], &account, &account_plan));\n"
       "OK(tb_walkthrough_item_insert(p[1], &item, &item_plan));\n"
       "tb_walkthrough_account_handle_t buyer;\n"
       "tb_walkthrough_item_handle_t product;\n"
       "OK(tb_walkthrough_account_match(p[2], \"A2\", &buyer));\n"
       "OK(tb_walkthrough_item_match(p[2], \"I2\", &product));\n"
       "tb_walkthrough_orderzhline_insert_t order = {.orderzhid = \"O2\",\n"
       "                                             .has_orderzhid = true,\n"
       "                                             .quantity = 1,\n"
       "                                             .has_quantity = true,\n"
       "                                             .buyer = buyer,\n"
       "                                             .item = product};\n"
       "OK(tb_walkthrough_orderzhline_insert(p[2], &order, &order_plan));\n"
       "uint32_t selection[3] = {account_plan.selection, item_plan.selection,\n"
       "                         order_plan.selection};\n"
       "type_bridge_staged_transaction_t *tx = NULL;\n"
       "type_bridge_pending_t *pending[3] = {NULL};\n"
       "OK(type_bridge_database_transaction(database, "
       "TYPE_BRIDGE_TRANSACTION_WRITE,\n"
       "                                    &tx, &diagnostics));\n"
       "for (size_t i = 0; i < 3; ++i)\n"
       "  OK(type_bridge_staged_transaction_stage_selection(\n"
       "      tx, p[i], selection[i], &pending[i], &diagnostics));\n"
       "OK(type_bridge_staged_transaction_commit(tx, &diagnostics));");
  type_bridge_plan_builder_t *p[3] = {recording(), recording(), recording()};
  tb_walkthrough_account_plan_t account_plan;
  tb_walkthrough_item_plan_t item_plan;
  tb_walkthrough_orderzhline_plan_t order_plan;
  tb_walkthrough_account_insert_t account = {
      .accountzhid = "A2", .balance = 20, .has_balance = true};
  tb_walkthrough_item_insert_t item = {.sku = "I2",
                                       .price = 4,
                                       .has_price = true,
                                       .stock = 3,
                                       .has_stock = true};
  OK(tb_walkthrough_account_insert(p[0], &account, &account_plan));
  OK(tb_walkthrough_item_insert(p[1], &item, &item_plan));
  tb_walkthrough_account_handle_t buyer;
  tb_walkthrough_item_handle_t product;
  OK(tb_walkthrough_account_match(p[2], "A2", &buyer));
  OK(tb_walkthrough_item_match(p[2], "I2", &product));
  tb_walkthrough_orderzhline_insert_t order = {.orderzhid = "O2",
                                               .has_orderzhid = true,
                                               .quantity = 1,
                                               .has_quantity = true,
                                               .buyer = buyer,
                                               .item = product};
  OK(tb_walkthrough_orderzhline_insert(p[2], &order, &order_plan));
  uint32_t selection[3] = {account_plan.selection, item_plan.selection,
                           order_plan.selection};
  unsigned models[3] = {0, 2, 5};
  char *bytes[3], *explain[3];
  for (size_t i = 0; i < 3; ++i) {
    type_bridge_plan_t *inspection = NULL;
    type_bridge_canonical_bytes_t *explanation = NULL;
    size_t n = 0;
    type_bridge_byte_view_t text;
    OK(type_bridge_plan_builder_selection_plan(p[i], selection[i], &inspection,
                                               &diagnostics));
    assert(type_bridge_plan_canonical_bytes(inspection, NULL, 0, &n) ==
           TYPE_BRIDGE_STATUS_RESOURCE_LIMIT);
    bytes[i] = malloc(n + 1);
    assert(bytes[i]);
    OK(type_bridge_plan_canonical_bytes(inspection, (uint8_t *)bytes[i], n,
                                        &n));
    bytes[i][n] = 0;
    OK(type_bridge_plan_explain(inspection, &explanation, &diagnostics));
    OK(type_bridge_canonical_bytes_view(explanation, &text));
    explain[i] = copy(text);
    type_bridge_plan_free(&inspection);
    OK(type_bridge_canonical_bytes_close(&explanation));
  }
  begin(name, "plan");
  fputs("{\"source\":\"canonical_bytes\",\"plans\":[", stdout);
  for (size_t i = 0; i < 3; ++i) {
    if (i)
      putchar(',');
    json_without(bytes[i], "given_rows");
  }
  fputs("]}", stdout);
  end();
  begin(name, "explain");
  fputs("{\"source\":\"explain\",\"plans\":[", stdout);
  for (size_t i = 0; i < 3; ++i) {
    if (i)
      putchar(',');
    fputs(explain[i], stdout);
  }
  fputs("]}", stdout);
  end();
  type_bridge_staged_transaction_t *tx = NULL;
  type_bridge_pending_t *pending[3] = {NULL};
  OK(type_bridge_database_transaction(database, TYPE_BRIDGE_TRANSACTION_WRITE,
                                      &tx, &diagnostics));
  for (size_t i = 0; i < 3; ++i)
    OK(type_bridge_staged_transaction_stage_selection(
        tx, p[i], selection[i], &pending[i], &diagnostics));
  OK(type_bridge_staged_transaction_commit(tx, &diagnostics));
  assert(event_count == 3);
  for (size_t i = 0; i < 3; ++i) {
    const char *statement =
        json_space(json_member(explain[i], "statements")) + 1;
    statement = json_space(statement);
    const char *actual =
        json_member(json_member(events[i], "fields"), "statement");
    assert(json_end(statement) - statement == json_end(actual) - actual &&
           !memcmp(statement, actual, (size_t)(json_end(actual) - actual)));
  }
  clear_events(3);
  begin(name, "raw");
  putchar('[');
  for (size_t i = 0; i < 3; ++i) {
    if (i)
      putchar(',');
    print_raw(pending[i], false);
  }
  putchar(']');
  end();
  begin(name, "models");
  putchar('[');
  for (size_t i = 0; i < 3; ++i) {
    if (i)
      putchar(',');
    print_models(pending[i], models[i], false);
  }
  putchar(']');
  end();
  for (size_t i = 0; i < 3; ++i) {
    free(bytes[i]);
    free(explain[i]);
    type_bridge_pending_free(&pending[i]);
    type_bridge_plan_builder_free(&p[i]);
  }
  type_bridge_staged_transaction_close(&tx);
}
static void raw_unchecked(void) {
  type_bridge_raw_result_t *result = NULL;
  type_bridge_diagnostics_t *errors = NULL;
  type_bridge_byte_view_t json;
  call("raw_unchecked",
       "type_bridge_database_execute_query(database, view(\"match $x isa "
       "order-line; reduce $n = count;\"), TYPE_BRIDGE_TRANSACTION_READ, "
       "&result, &errors);");
  begin("raw_unchecked", "unchecked");
  json_string("no plan, no hydration");
  end();
  OK(type_bridge_database_execute_query(
      database, view("match $x isa order-line; reduce $n = count;"),
      TYPE_BRIDGE_TRANSACTION_READ, &result, &errors));
  OK(type_bridge_raw_result_json(result, &json));
  begin("raw_unchecked", "raw");
  fwrite(json.data, 1, json.length, stdout);
  end();
  OK(type_bridge_raw_result_close(&result));
  OK(type_bridge_diagnostics_close(&errors));
  clear_events(0);
}
static const char *environment(const char *key, const char *fallback) {
  const char *value = getenv(key);
  return value ? value : fallback;
}
int main(void) {
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_runtime_config_v1_t runtime_config = {0};
  runtime_config.struct_size = sizeof(runtime_config);
  runtime_config.version = TYPE_BRIDGE_RUNTIME_CONFIG_VERSION;
  runtime_config.worker_threads = 2;
  OK(type_bridge_runtime_open_v1(&runtime_config, &runtime, &diagnostics));
  OK(tb_walkthrough_schema_package_open(&package, &diagnostics));
  type_bridge_database_config_t config = {0};
  config.struct_size = sizeof(config);
  config.version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION;
  config.address = view(environment("TYPEDB_ADDRESS", "127.0.0.1:1729"));
  config.database = view(getenv("TYPE_BRIDGE_WALKTHROUGH_DATABASE"));
  config.username = view(environment("TYPEDB_USERNAME", "admin"));
  config.password = view(environment("TYPEDB_PASSWORD", "password"));
  config.http_port =
      (uint32_t)strtoul(environment("TYPEDB_HTTP_PORT", "8000"), NULL, 10);
  config.tls_mode = TYPE_BRIDGE_TLS_DISABLED;
  OK(type_bridge_database_open(runtime, package, &config, NULL, &database,
                               &diagnostics));
  OK(type_bridge_debug_listener_set(database, listener, NULL));
  seed();
  purchase();
  delete_subtypes();
  bulk_insert();
  type_bridge_iid_t *iid = manager_calls();
  insert_many();
  manager_delete(&iid);
  staged_dependency();
  raw_unchecked();
  OK(type_bridge_database_close(&database, &diagnostics));
  OK(type_bridge_runtime_close(&runtime, &diagnostics));
  OK(type_bridge_schema_package_close(&package));
  free(purchase_buyer_iid);
  free(purchase_item_iid);
  type_bridge_execution_diagnostics_close(&diagnostics);
  return 0;
}
