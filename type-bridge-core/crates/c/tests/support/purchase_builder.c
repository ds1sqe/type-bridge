/*
 * The one-query purchase written with the ABI 1.7 plan builder. It must give
 * the bytes of the hand-built purchase every SDK builder is checked against:
 * match the item and the account by key, require stock and balance, update
 * both and insert the purchase in one write plan answering MATCHED.
 */
#include <stdint.h>
#include <string.h>

#include <typebridge/type_bridge.h>

#include "purchase_builder.h"

static type_bridge_byte_view_t purchase_view(const char *text) {
  type_bridge_byte_view_t view;
  view.data = (const uint8_t *)text;
  view.length = strlen(text);
  return view;
}

#define PURCHASE_CHECK(call)                                                   \
  do {                                                                         \
    if ((status = (call)) != TYPE_BRIDGE_STATUS_OK) {                          \
      goto done;                                                               \
    }                                                                          \
  } while (0)

type_bridge_status_t purchase_build(const type_bridge_schema_package_t *package,
                                    const char *order_id, const char *item_id,
                                    const char *account_id,
                                    type_bridge_plan_t **out_plan,
                                    type_bridge_execution_diagnostics_t **out_diagnostics) {
  type_bridge_plan_builder_t *plan = NULL;
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  uint32_t item_type, account_type, purchase_type;
  uint32_t item_key_field, account_key_field, stock_field, price_field,
      balance_field, order_field, buyer_role, item_role;
  uint32_t item, account, order_input, item_input, account_input;
  uint32_t item_key, account_key, stock, price, balance;
  uint32_t zero, one, next_stock_expr, next_balance_expr, next_stock,
      next_balance, purchase;
  uint32_t order_value, item_value, account_value;
  uint32_t row[3];

  PURCHASE_CHECK(type_bridge_plan_builder_new(&plan));
  /* Identities, like the generated field and role tokens of other SDKs. */
  PURCHASE_CHECK(type_bridge_plan_builder_type(
      plan, TYPE_BRIDGE_PLAN_THING_ENTITY, purchase_view("item"), &item_type));
  PURCHASE_CHECK(type_bridge_plan_builder_type(
      plan, TYPE_BRIDGE_PLAN_THING_ENTITY, purchase_view("account"),
      &account_type));
  PURCHASE_CHECK(type_bridge_plan_builder_type(
      plan, TYPE_BRIDGE_PLAN_THING_RELATION, purchase_view("purchase"),
      &purchase_type));
  PURCHASE_CHECK(type_bridge_plan_builder_owns(
      plan, item_type, purchase_view("item-id"), &item_key_field));
  PURCHASE_CHECK(type_bridge_plan_builder_owns(
      plan, account_type, purchase_view("account-id"), &account_key_field));
  PURCHASE_CHECK(type_bridge_plan_builder_owns(plan, item_type,
                                               purchase_view("stock"), &stock_field));
  PURCHASE_CHECK(type_bridge_plan_builder_owns(plan, item_type,
                                               purchase_view("price"), &price_field));
  PURCHASE_CHECK(type_bridge_plan_builder_owns(
      plan, account_type, purchase_view("balance"), &balance_field));
  PURCHASE_CHECK(type_bridge_plan_builder_owns(
      plan, purchase_type, purchase_view("order-id"), &order_field));
  PURCHASE_CHECK(type_bridge_plan_builder_role(plan, purchase_type,
                                               purchase_view("buyer"), &buyer_role));
  PURCHASE_CHECK(type_bridge_plan_builder_role(plan, purchase_type,
                                               purchase_view("item"), &item_role));

  /* Match, in call order. */
  PURCHASE_CHECK(type_bridge_plan_builder_exact(plan, item_type, &item));
  PURCHASE_CHECK(type_bridge_plan_builder_exact(plan, account_type, &account));
  PURCHASE_CHECK(type_bridge_plan_builder_input(
      plan, purchase_view("order_id"), TYPE_BRIDGE_PROJECTED_VALUE_STRING, 0u,
      &order_input));
  PURCHASE_CHECK(type_bridge_plan_builder_input(
      plan, purchase_view("item_id"), TYPE_BRIDGE_PROJECTED_VALUE_STRING, 0u,
      &item_input));
  PURCHASE_CHECK(type_bridge_plan_builder_input(
      plan, purchase_view("account_id"), TYPE_BRIDGE_PROJECTED_VALUE_STRING, 0u,
      &account_input));
  PURCHASE_CHECK(type_bridge_plan_builder_field(plan, item, item_key_field, &item_key));
  PURCHASE_CHECK(
      type_bridge_plan_builder_field(plan, account, account_key_field, &account_key));
  PURCHASE_CHECK(type_bridge_plan_builder_field(plan, item, stock_field, &stock));
  PURCHASE_CHECK(type_bridge_plan_builder_field(plan, item, price_field, &price));
  PURCHASE_CHECK(type_bridge_plan_builder_field(plan, account, balance_field, &balance));
  PURCHASE_CHECK(type_bridge_plan_builder_value_long(plan, 0, &zero));
  PURCHASE_CHECK(type_bridge_plan_builder_value_long(plan, 1, &one));
  PURCHASE_CHECK(type_bridge_plan_builder_compare(
      plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, item_key,
      TYPE_BRIDGE_PLAN_COMPARE_EQUAL, TYPE_BRIDGE_PLAN_OPERAND_INPUT, item_input));
  PURCHASE_CHECK(type_bridge_plan_builder_compare(
      plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, account_key,
      TYPE_BRIDGE_PLAN_COMPARE_EQUAL, TYPE_BRIDGE_PLAN_OPERAND_INPUT,
      account_input));
  PURCHASE_CHECK(type_bridge_plan_builder_compare(
      plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, stock,
      TYPE_BRIDGE_PLAN_COMPARE_GREATER, TYPE_BRIDGE_PLAN_OPERAND_VALUE, zero));
  PURCHASE_CHECK(type_bridge_plan_builder_compare(
      plan, TYPE_BRIDGE_PLAN_OPERAND_BINDING, balance,
      TYPE_BRIDGE_PLAN_COMPARE_GREATER_OR_EQUAL, TYPE_BRIDGE_PLAN_OPERAND_BINDING,
      price));
  PURCHASE_CHECK(type_bridge_plan_builder_expr_arith(
      plan, TYPE_BRIDGE_PLAN_ARITH_SUB, TYPE_BRIDGE_PLAN_OPERAND_BINDING, stock,
      TYPE_BRIDGE_PLAN_OPERAND_VALUE, one, &next_stock_expr));
  PURCHASE_CHECK(type_bridge_plan_builder_let(plan, purchase_view("next_stock"),
                                              TYPE_BRIDGE_PLAN_OPERAND_EXPR,
                                              next_stock_expr, &next_stock));
  PURCHASE_CHECK(type_bridge_plan_builder_expr_arith(
      plan, TYPE_BRIDGE_PLAN_ARITH_SUB, TYPE_BRIDGE_PLAN_OPERAND_BINDING, balance,
      TYPE_BRIDGE_PLAN_OPERAND_BINDING, price, &next_balance_expr));
  PURCHASE_CHECK(type_bridge_plan_builder_let(plan, purchase_view("next_balance"),
                                              TYPE_BRIDGE_PLAN_OPERAND_EXPR,
                                              next_balance_expr, &next_balance));

  /* Both updates share one stage; the insert follows. */
  PURCHASE_CHECK(type_bridge_plan_builder_update(
      plan, item, stock_field, TYPE_BRIDGE_PLAN_OPERAND_BINDING, next_stock));
  PURCHASE_CHECK(type_bridge_plan_builder_update(
      plan, account, balance_field, TYPE_BRIDGE_PLAN_OPERAND_BINDING,
      next_balance));
  PURCHASE_CHECK(type_bridge_plan_builder_insert(plan, purchase_type, &purchase));
  PURCHASE_CHECK(type_bridge_plan_builder_thing_field(
      plan, purchase, order_field, TYPE_BRIDGE_PLAN_OPERAND_INPUT, order_input));
  PURCHASE_CHECK(
      type_bridge_plan_builder_thing_player(plan, purchase, buyer_role, account));
  PURCHASE_CHECK(type_bridge_plan_builder_thing_player(plan, purchase, item_role, item));

  PURCHASE_CHECK(type_bridge_plan_builder_value_string(
      plan, purchase_view(order_id), &order_value));
  PURCHASE_CHECK(type_bridge_plan_builder_value_string(
      plan, purchase_view(item_id), &item_value));
  PURCHASE_CHECK(type_bridge_plan_builder_value_string(
      plan, purchase_view(account_id), &account_value));
  row[0] = order_value;
  row[1] = item_value;
  row[2] = account_value;
  PURCHASE_CHECK(type_bridge_plan_builder_row(plan, row, 3u));

  status = type_bridge_plan_builder_build(plan, package, out_plan, out_diagnostics);
  type_bridge_plan_builder_free(&plan);
  return status;

done:
  type_bridge_plan_builder_free(&plan);
  return status;
}
