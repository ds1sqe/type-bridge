/* Core-builder equality oracles for the generated authoring entrypoints. */
static type_bridge_status_t purchase_explicit(type_bridge_plan_builder_t *p, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t item, buyer, key, stock, account, balance, price, order;
  uint32_t item_input, minimum, account_input, decrement, order_input, expr, next;
  uint32_t row[5];
  CHECK(type_bridge_plan_builder_exact(p, id->item, &item));
  CHECK(type_bridge_plan_builder_field(p, item, id->item_id, &key));
  CHECK(input(p, "item_item_id_2", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &item_input));
  CHECK(type_bridge_plan_builder_compare(p, TYPE_BRIDGE_PLAN_OPERAND_BINDING, key, TYPE_BRIDGE_PLAN_COMPARE_EQUAL, TYPE_BRIDGE_PLAN_OPERAND_INPUT, item_input));
  CHECK(type_bridge_plan_builder_field(p, item, id->stock, &stock));
  CHECK(input(p, "item_stock_2", TYPE_BRIDGE_PROJECTED_VALUE_LONG, &minimum));
  CHECK(type_bridge_plan_builder_compare(p, TYPE_BRIDGE_PLAN_OPERAND_BINDING, stock, TYPE_BRIDGE_PLAN_COMPARE_GREATER, TYPE_BRIDGE_PLAN_OPERAND_INPUT, minimum));
  CHECK(type_bridge_plan_builder_exact(p, id->account, &buyer));
  CHECK(type_bridge_plan_builder_field(p, buyer, id->account_id, &account));
  CHECK(input(p, "account_account_id_2", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &account_input));
  CHECK(type_bridge_plan_builder_compare(p, TYPE_BRIDGE_PLAN_OPERAND_BINDING, account, TYPE_BRIDGE_PLAN_COMPARE_EQUAL, TYPE_BRIDGE_PLAN_OPERAND_INPUT, account_input));
  CHECK(type_bridge_plan_builder_field(p, buyer, id->balance, &balance));
  CHECK(type_bridge_plan_builder_field(p, item, id->price, &price));
  CHECK(type_bridge_plan_builder_compare(p, TYPE_BRIDGE_PLAN_OPERAND_BINDING, balance, TYPE_BRIDGE_PLAN_COMPARE_GREATER_OR_EQUAL, TYPE_BRIDGE_PLAN_OPERAND_BINDING, price));
  CHECK(input(p, "item_stock_3", TYPE_BRIDGE_PROJECTED_VALUE_LONG, &decrement));
  CHECK(type_bridge_plan_builder_expr_arith(p, TYPE_BRIDGE_PLAN_ARITH_SUB, TYPE_BRIDGE_PLAN_OPERAND_BINDING, stock, TYPE_BRIDGE_PLAN_OPERAND_INPUT, decrement, &expr));
  CHECK(type_bridge_plan_builder_let(p, view("item_stock_4"), TYPE_BRIDGE_PLAN_OPERAND_EXPR, expr, &next));
  CHECK(type_bridge_plan_builder_update(p, item, id->stock, TYPE_BRIDGE_PLAN_OPERAND_BINDING, next));
  CHECK(type_bridge_plan_builder_expr_arith(p, TYPE_BRIDGE_PLAN_ARITH_SUB, TYPE_BRIDGE_PLAN_OPERAND_BINDING, balance, TYPE_BRIDGE_PLAN_OPERAND_BINDING, price, &expr));
  CHECK(type_bridge_plan_builder_let(p, view("account_balance_2"), TYPE_BRIDGE_PLAN_OPERAND_EXPR, expr, &next));
  CHECK(type_bridge_plan_builder_update(p, buyer, id->balance, TYPE_BRIDGE_PLAN_OPERAND_BINDING, next));
  CHECK(type_bridge_plan_builder_insert(p, id->purchase, &order));
  CHECK(input(p, "purchase_order_id", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &order_input));
  CHECK(type_bridge_plan_builder_thing_field(p, order, id->order_id, TYPE_BRIDGE_PLAN_OPERAND_INPUT, order_input));
  CHECK(type_bridge_plan_builder_thing_player(p, order, id->buyer, buyer));
  CHECK(type_bridge_plan_builder_thing_player(p, order, id->item_role, item));
  CHECK(type_bridge_plan_builder_value_string(p, view("I1"), &row[0]));
  CHECK(type_bridge_plan_builder_value_long(p, 0, &row[1]));
  CHECK(type_bridge_plan_builder_value_string(p, view("A1"), &row[2]));
  CHECK(type_bridge_plan_builder_value_long(p, 1, &row[3]));
  CHECK(type_bridge_plan_builder_value_string(p, view("O1"), &row[4]));
  CHECK(type_bridge_plan_builder_row(p, row, 5u));
done:
  return status;
}

static type_bridge_status_t touch_explicit(type_bridge_plan_builder_t *p, const identities_t *id) {
  type_bridge_status_t status;
  uint32_t doctor, state;
  CHECK(type_bridge_plan_builder_exact(p, id->doctor, &doctor));
  CHECK(type_bridge_plan_builder_field(p, doctor, id->on_call, &state));
  CHECK(type_bridge_plan_builder_update(p, doctor, id->on_call, TYPE_BRIDGE_PLAN_OPERAND_BINDING, state));
done:
  return status;
}

static type_bridge_status_t doctor_explicit(type_bridge_plan_builder_t *p, const identities_t *id, uint8_t state) {
  type_bridge_status_t status;
  uint32_t doctor, name, on_call, row[2];
  CHECK(type_bridge_plan_builder_insert(p, id->doctor, &doctor));
  CHECK(input(p, "doctor_name", TYPE_BRIDGE_PROJECTED_VALUE_STRING, &name));
  CHECK(input(p, "doctor_on_call", TYPE_BRIDGE_PROJECTED_VALUE_BOOLEAN, &on_call));
  CHECK(type_bridge_plan_builder_thing_field(p, doctor, id->name, TYPE_BRIDGE_PLAN_OPERAND_INPUT, name));
  CHECK(type_bridge_plan_builder_thing_field(p, doctor, id->on_call, TYPE_BRIDGE_PLAN_OPERAND_INPUT, on_call));
  CHECK(type_bridge_plan_builder_value_string(p, view("D1"), &row[0]));
  CHECK(type_bridge_plan_builder_value_boolean(p, state, &row[1]));
  CHECK(type_bridge_plan_builder_row(p, row, 2u));
done:
  return status;
}

static int same_plan(const type_bridge_plan_t *a, const type_bridge_plan_t *b) {
  size_t an = 0u, bn = 0u;
  uint8_t *ab, *bb;
  int equal = 0;
  if (type_bridge_plan_v3_canonical_bytes(a, NULL, 0u, &an) != TYPE_BRIDGE_STATUS_RESOURCE_LIMIT ||
      type_bridge_plan_v3_canonical_bytes(b, NULL, 0u, &bn) != TYPE_BRIDGE_STATUS_RESOURCE_LIMIT || an != bn) return 0;
  ab = (uint8_t *)malloc(an); bb = (uint8_t *)malloc(bn);
  if (ab != NULL && bb != NULL &&
      type_bridge_plan_v3_canonical_bytes(a, ab, an, &an) == TYPE_BRIDGE_STATUS_OK &&
      type_bridge_plan_v3_canonical_bytes(b, bb, bn, &bn) == TYPE_BRIDGE_STATUS_OK) equal = memcmp(ab, bb, an) == 0;
  free(ab); free(bb);
  return equal;
}
