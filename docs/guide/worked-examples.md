# Worked examples

The walkthrough follows a typed call through its plan, lowered TypeQL and
committed generated models. The shop schema uses `Account`, `Item` and
`OrderLine`: an item's key is `sku`, and an order line links its buyer and
item. The source programs live in `examples/walkthrough/`.

## Purchase

Start with account `A1` holding a balance of 10 and item `I1` costing 3,
with stock 5. Match both, require stock and sufficient balance, subtract
one item and its price, then insert order line `O1`. These operations
belong to one plan and commit in one transaction.

```python
with Plan():
    item = Item.match("I1").where(Item.stock > 0)
    buyer = Account.match("A1").where(Account.balance >= item.price)
    item.set(stock=item.stock - 1)
    buyer.set(balance=buyer.balance - item.price)
    order = OrderLine.insert(order_id="O1", quantity=1, buyer=buyer, item=item)

with db.staged_transaction() as tx:
    pending = tx.stage(order)
purchases = pending.result()
```

```typescript
const order = Plan.build(() => {
  const item = Item.match("I1").where(Item.stock.gt(0n));
  const buyer = Account.match("A1").where(Account.balance.ge(item.price));
  item.set({ stock: item.stock.sub(1n) });
  buyer.set({ balance: buyer.balance.sub(item.price) });
  return OrderLine.insert({ orderId: "O1", quantity: 1n, buyer, item });
});
const tx = db.stagedTransaction();
const pending = tx.stage(order);
await tx.commit();
const purchases = pending.result();
```

Holding the returned insert plan selects the generated `OrderLine` models
as the result. The result contains one order line, with its buyer's balance
now 7 and its item's stock now 4. The scripts assert those values from the
hydrated objects and print their canonical snapshots.

`order.explain()` reports the shared bindings, inputs and exact lowered
statements. Input values travel separately as given rows; the explanation
reports their count and encoded size. See [Debugging plans](debugging.md)
for the explanation format and provider statement tracing.

### Plan

<!-- walkthrough:purchase:plan -->
```json
{
  "plan": {
    "bindings": [
      {
        "id": 0,
        "variable": "item"
      },
      {
        "id": 1,
        "variable": "item_sku"
      },
      {
        "id": 2,
        "variable": "item_stock"
      },
      {
        "id": 3,
        "variable": "account"
      },
      {
        "id": 4,
        "variable": "account_account_id"
      },
      {
        "id": 5,
        "variable": "item_price"
      },
      {
        "id": 6,
        "variable": "account_balance"
      },
      {
        "id": 7,
        "variable": "item_stock_4"
      },
      {
        "id": 8,
        "variable": "account_balance_2"
      },
      {
        "id": 9,
        "variable": "order_line"
      }
    ],
    "format": "typebridge.query-plan/v3",
    "functions": [],
    "inputs": [
      {
        "id": 0,
        "optional": false,
        "public_name": "item_key",
        "value_type": "string"
      },
      {
        "id": 1,
        "optional": false,
        "public_name": "item_stock_2",
        "value_type": "long"
      },
      {
        "id": 2,
        "optional": false,
        "public_name": "account_key",
        "value_type": "string"
      },
      {
        "id": 3,
        "optional": false,
        "public_name": "item_stock_3",
        "value_type": "long"
      },
      {
        "id": 4,
        "optional": false,
        "public_name": "order_line_order_id",
        "value_type": "string"
      },
      {
        "id": 5,
        "optional": false,
        "public_name": "order_line_quantity",
        "value_type": "long"
      }
    ],
    "managed_semantics": {
      "algorithm": "sha256",
      "canonicalization": "typebridge.managed-semantic/v1",
      "digest": "2570099d0a370ee35940a05381b04bf7174ad4d7b8a212718b9ec4536146e342",
      "domain": "typebridge.schema.managed-semantic",
      "semantic_profile": "typedb-3.13.6/v1"
    },
    "operation": {
      "kind": "write",
      "returns": "rows"
    },
    "output": {
      "columns": [],
      "kind": "rows"
    },
    "pipeline": [
      {
        "kind": "match",
        "patterns": [
          {
            "binding": 0,
            "include_subtypes": false,
            "kind": "isa",
            "type_id": {
              "kind": "entity",
              "label": "item"
            }
          },
          {
            "attribute": 1,
            "attribute_id": "sku",
            "kind": "has",
            "owner": 0
          },
          {
            "comparator": "equal",
            "kind": "value",
            "left": {
              "binding": 1,
              "kind": "binding"
            },
            "right": {
              "column": 0,
              "kind": "input"
            }
          },
          {
            "attribute": 2,
            "attribute_id": "stock",
            "kind": "has",
            "owner": 0
          },
          {
            "comparator": "greater",
            "kind": "value",
            "left": {
              "binding": 2,
              "kind": "binding"
            },
            "right": {
              "column": 1,
              "kind": "input"
            }
          },
          {
            "binding": 3,
            "include_subtypes": false,
            "kind": "isa",
            "type_id": {
              "kind": "entity",
              "label": "account"
            }
          },
          {
            "attribute": 4,
            "attribute_id": "account-id",
            "kind": "has",
            "owner": 3
          },
          {
            "comparator": "equal",
            "kind": "value",
            "left": {
              "binding": 4,
              "kind": "binding"
            },
            "right": {
              "column": 2,
              "kind": "input"
            }
          },
          {
            "attribute": 5,
            "attribute_id": "price",
            "kind": "has",
            "owner": 0
          },
          {
            "attribute": 6,
            "attribute_id": "balance",
            "kind": "has",
            "owner": 3
          },
          {
            "comparator": "greater_or_equal",
            "kind": "value",
            "left": {
              "binding": 6,
              "kind": "binding"
            },
            "right": {
              "binding": 5,
              "kind": "binding"
            }
          },
          {
            "binding": 7,
            "expr": {
              "kind": "arith",
              "value": {
                "lhs": {
                  "kind": "operand",
                  "value": {
                    "binding": 2,
                    "kind": "binding"
                  }
                },
                "op": "sub",
                "rhs": {
                  "kind": "operand",
                  "value": {
                    "column": 3,
                    "kind": "input"
                  }
                }
              }
            },
            "kind": "let"
          },
          {
            "binding": 8,
            "expr": {
              "kind": "arith",
              "value": {
                "lhs": {
                  "kind": "operand",
                  "value": {
                    "binding": 6,
                    "kind": "binding"
                  }
                },
                "op": "sub",
                "rhs": {
                  "kind": "operand",
                  "value": {
                    "binding": 5,
                    "kind": "binding"
                  }
                }
              }
            },
            "kind": "let"
          }
        ]
      },
      {
        "kind": "update",
        "sets": [
          {
            "field": {
              "attribute": "stock",
              "owner": {
                "kind": "entity",
                "label": "item"
              }
            },
            "kind": "field",
            "operand": {
              "binding": 7,
              "kind": "binding"
            },
            "owner": 0
          },
          {
            "field": {
              "attribute": "balance",
              "owner": {
                "kind": "entity",
                "label": "account"
              }
            },
            "kind": "field",
            "operand": {
              "binding": 8,
              "kind": "binding"
            },
            "owner": 3
          }
        ]
      },
      {
        "kind": "insert",
        "things": [
          {
            "binding": 9,
            "fields": [
              [
                {
                  "attribute": "order-id",
                  "owner": {
                    "kind": "relation",
                    "label": "order-line"
                  }
                },
                {
                  "column": 4,
                  "kind": "input"
                }
              ],
              [
                {
                  "attribute": "quantity",
                  "owner": {
                    "kind": "relation",
                    "label": "order-line"
                  }
                },
                {
                  "column": 5,
                  "kind": "input"
                }
              ]
            ],
            "players": [
              [
                {
                  "declaring_relation": "order-line",
                  "label": "buyer"
                },
                3
              ],
              [
                {
                  "declaring_relation": "order-line",
                  "label": "item"
                },
                0
              ]
            ],
            "type_id": {
              "kind": "relation",
              "label": "order-line"
            }
          }
        ]
      },
      {
        "kind": "fetch",
        "roles": [
          {
            "required": false,
            "role": {
              "declaring_relation": "order-line",
              "label": "buyer"
            }
          },
          {
            "required": false,
            "role": {
              "declaring_relation": "order-line",
              "label": "item"
            }
          }
        ],
        "thing": 9
      }
    ],
    "required_capabilities": [
      "query.expr.sub",
      "query.fetch.thing",
      "query.input.columns",
      "query.input.given-rows",
      "query.let",
      "query.output.rows",
      "query.pattern.has",
      "query.pattern.isa",
      "query.pattern.value",
      "query.plan",
      "query.plan.v3",
      "query.write.insert",
      "query.write.update"
    ]
  },
  "source": "canonical_bytes"
}
```

### TypeQL

<!-- walkthrough:purchase:typeql -->
```typeql
given $item_key: string, $item_stock_2: integer, $account_key: string, $item_stock_3: integer, $order_line_order_id: string, $order_line_quantity: integer; match $item isa! item; $item has sku $item_sku; $item_sku isa! sku; $item_sku == $item_key; $item has stock $item_stock; $item_stock isa! stock; $item_stock > $item_stock_2; $account isa! account; $account has account-id $account_account_id; $account_account_id isa! account-id; $account_account_id == $account_key; $item has price $item_price; $item_price isa! price; $account has balance $account_balance; $account_balance isa! balance; $account_balance >= $item_price; let $item_stock_4 = ($item_stock - $item_stock_3); let $account_balance_2 = ($account_balance - $item_price); update $item has stock == $item_stock_4; $account has balance == $account_balance_2; insert $order_line isa order-line, links (buyer: $account, item: $item), has order-id == $order_line_order_id, has quantity == $order_line_quantity; match $order_line isa! $PlanFetchType; try { $order_line links (buyer: $PlanFetchRole0); 
 $PlanFetchRole0 isa! $PlanFetchRole0Type; 
}; try { $order_line links (item: $PlanFetchRole1); 
 $PlanFetchRole1 isa! $PlanFetchRole1Type; 
}; fetch { "_iid": iid($order_line), "_type": label($PlanFetchType), "attributes": {  $order_line.*}, "_role_0_iid": iid($PlanFetchRole0), "_role_0_type": label($PlanFetchRole0Type), "_role_0_attributes": {  $PlanFetchRole0.*}, "_role_1_iid": iid($PlanFetchRole1), "_role_1_type": label($PlanFetchRole1Type), "_role_1_attributes": {  $PlanFetchRole1.*}};
```

## Delete returning concrete subtypes

`Document` is abstract. `Report` and `Memo` inherit its key and status.
The subtype-inclusive delete returns both concrete generated models with their
`deleted` flag set. The provider first captures the documents, then deletes
them in the same transaction.

```python
with Plan():
    gone = Document.subtypes().where(Document.status == "archived").delete_returning()
```

```typescript
Plan.build(() => Document.subtypes().where(Document.status.eq("archived")).deleteReturning())
```

### Plan

<!-- walkthrough:delete_subtypes:plan -->
```json
{
  "plan": {
    "bindings": [
      {
        "id": 0,
        "variable": "document"
      },
      {
        "id": 1,
        "variable": "document_status"
      }
    ],
    "format": "typebridge.query-plan/v3",
    "functions": [],
    "inputs": [
      {
        "id": 0,
        "optional": false,
        "public_name": "document_status_2",
        "value_type": "string"
      }
    ],
    "managed_semantics": {
      "algorithm": "sha256",
      "canonicalization": "typebridge.managed-semantic/v1",
      "digest": "2570099d0a370ee35940a05381b04bf7174ad4d7b8a212718b9ec4536146e342",
      "domain": "typebridge.schema.managed-semantic",
      "semantic_profile": "typedb-3.13.6/v1"
    },
    "operation": {
      "kind": "write",
      "returns": "rows"
    },
    "output": {
      "columns": [],
      "kind": "rows"
    },
    "pipeline": [
      {
        "kind": "match",
        "patterns": [
          {
            "binding": 0,
            "include_subtypes": true,
            "kind": "isa",
            "type_id": {
              "kind": "entity",
              "label": "document"
            }
          },
          {
            "attribute": 1,
            "attribute_id": "status",
            "kind": "has",
            "owner": 0
          },
          {
            "comparator": "equal",
            "kind": "value",
            "left": {
              "binding": 1,
              "kind": "binding"
            },
            "right": {
              "column": 0,
              "kind": "input"
            }
          }
        ]
      },
      {
        "kind": "fetch",
        "roles": [],
        "thing": 0
      },
      {
        "items": [
          {
            "binding": 0,
            "kind": "thing"
          }
        ],
        "kind": "delete"
      }
    ],
    "required_capabilities": [
      "query.fetch.thing",
      "query.input.columns",
      "query.input.given-rows",
      "query.output.rows",
      "query.pattern.has",
      "query.pattern.isa",
      "query.pattern.isa-subtypes",
      "query.pattern.value",
      "query.plan",
      "query.plan.v3",
      "query.write.delete",
      "query.write.delete-snapshot"
    ]
  },
  "source": "canonical_bytes"
}
```

### Typeql

<!-- walkthrough:delete_subtypes:typeql -->
```typeql
given $document_status_2: string; match $document isa document; $document has status $document_status; $document_status isa! status; $document_status == $document_status_2; match $document isa! $PlanFetchType; fetch { "_iid": iid($document), "_type": label($PlanFetchType), "attributes": {  $document.*}};

given $document_status_2: string; match $document isa document; $document has status $document_status; $document_status isa! status; $document_status == $document_status_2; delete $document;
```


## Bulk insert with automatic splitting

One typed insert contains 10,000 item rows. The engine splits it into three
provider statements and preserves input order in the returned models. The
script asserts both the row count and every SKU. A split batch has no single
`explain()` result: it refuses with `plan_batch_split`. The script instead
captures the actual per-chunk fingerprints and TypeQL through TRACE.

```python
with Plan():
    batch = Item.insert_many(Item.Values(sku=f"B{n}", price=3, stock=10) for n in range(10_000))
```

```typescript
Plan.build(() => Item.insertMany(Array.from({ length: 10_000 }, (_, n) => ({ sku: `B${n}`, price: 3n, stock: 10n }))))
```

### Plan

<!-- walkthrough:bulk_insert:plan -->
```json
{
  "fingerprints": [
    "93ef3b8c5f933cff6d3cda746f6b5e6f6b3d2c069a31abe31fa0b9b85be4d964",
    "93ef3b8c5f933cff6d3cda746f6b5e6f6b3d2c069a31abe31fa0b9b85be4d964",
    "93ef3b8c5f933cff6d3cda746f6b5e6f6b3d2c069a31abe31fa0b9b85be4d964"
  ],
  "source": "TRACE"
}
```

### Typeql

<!-- walkthrough:bulk_insert:typeql -->
```typeql
given $item_price: integer, $item_sku: string, $item_stock: integer; insert $item isa item, has price == $item_price, has sku == $item_sku, has stock == $item_stock; match $item isa! $PlanFetchType; fetch { "_iid": iid($item), "_type": label($PlanFetchType), "attributes": {  $item.*}};

given $item_price: integer, $item_sku: string, $item_stock: integer; insert $item isa item, has price == $item_price, has sku == $item_sku, has stock == $item_stock; match $item isa! $PlanFetchType; fetch { "_iid": iid($item), "_type": label($PlanFetchType), "attributes": {  $item.*}};

given $item_price: integer, $item_sku: string, $item_stock: integer; insert $item isa item, has price == $item_price, has sku == $item_sku, has stock == $item_stock; match $item isa! $PlanFetchType; fetch { "_iid": iid($item), "_type": label($PlanFetchType), "attributes": {  $item.*}};
```

## Managers and one staged transaction

The remaining calls run through generated managers: insert, put, typed-IID
lookup, filtered fetch, update, homogeneous `OrderLine.insert_many` with
existing buyer/item references, and delete returning. Managers execute eagerly;
their public result exposes generated models but not retained raw documents.
The transcript prints `raw documents: not exposed by manager result` at that
step. It prints the executed plan fingerprints and TypeQL from TRACE.

The dependency example stages a new account, a new item and their order line
in one transaction. The third plan matches the keys inserted by the first
two, so the new players are visible before commit. Generated inserts return
plans rather than bindable inserted handles; the example therefore uses
three typed plans and asserts the hydrated order line's player keys.

Finally, a raw order-line count is explicitly labeled unchecked: it has no
plan or generated-model hydration. Each runner creates a fresh database and
deletes that exact database in its cleanup block, including on failure.
