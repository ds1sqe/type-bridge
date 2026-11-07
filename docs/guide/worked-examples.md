# Worked examples

Follow a typed call through its V3 plan, lowered TypeQL, raw answer documents
and hydrated generated models in Python, TypeScript, Rust and C. The examples
use the shop `Account`, `Item` and `OrderLine` schema, extended with abstract
`Document` and its `Report` and `Memo` subtypes for delete returning.

## Run the walkthrough

From a configured source checkout, install the locked Python development and
Node dependencies (`uv sync --extra dev` and `npm ci --prefix
type-bridge-core/crates/node`). Start a supported TypeDB server, then run:

```bash
export TYPEDB_ADDRESS=127.0.0.1:1729
export TYPEDB_HTTP_PORT=8000
export TYPEDB_USERNAME=admin
export TYPEDB_PASSWORD=password
export TYPE_BRIDGE_WALKTHROUGH_DATABASE_PREFIX=walkthrough_
bash scripts/walkthrough.sh
```

The runner generates all four packages and builds their source-tree consumers.
It writes one JSON-line transcript per SDK under the printed output directory.
Use `--output path` to choose a new directory, or `--sdk python` (and the other
SDK names) for a focused run. Each SDK gets a fresh database; cleanup deletes
that exact database on success or failure. `TMPDIR`, `CARGO_TARGET_DIR` and
`CARGO_BUILD_JOBS` control local build storage and concurrency.

The programs live in `examples/walkthrough/{python,typescript,rust,c}/`.
The snippets below are their printed calls; connection setup, output helpers
and C's checked `OK(...)` wrapper are in those source files.

## Reading the output

Each typed call prints its code, plan, TypeQL, raw documents and typed model
fields in that order. Authored plans use `explain()` and their canonical plan
bytes, omitting transported input values. Inputs are shown as row and byte
counts. The programs compare explained statements with the provider TRACE
events captured at dispatch, including counted input redaction.

Eager managers and automatically split batches expose executed plan
fingerprints and statements through TRACE instead of a held plan explanation.
Their blocks identify this source explicitly. Manager results do not expose
retained raw documents; that step says `raw documents: not exposed by manager
result`. No extra query is issued to reconstruct those documents.

The comparator checks plans and TypeQL across all four SDKs, and compares
raw documents and typed fields after replacing IIDs with ordinals. Role
players are displayed as type and IID references. The page's marked blocks
are checked byte for byte against each run, including statement whitespace.
Large result blocks are explicitly summarized; transcripts retain every row.
See [Debugging plans](debugging.md) for TRACE and explanation details.

## Purchase

Start with account `A1` holding a balance of 10 and item `I1` costing 3, with stock 5. Match both, require stock and sufficient balance, subtract one item and its price, then insert order line `O1`. The operations belong to one plan and commit in one transaction. The programs assert the hydrated buyer balance is 7 and item stock is 4.

### Python

<!-- walkthrough:purchase:call:python -->
```python
with Plan():
    item = Item.match("I1").where(Item.stock > 0)
    buyer = Account.match("A1")
    balance = buyer.balance
    buyer = buyer.where(balance >= item.price)
    item.set(stock=item.stock - 1)
    buyer.set(balance=buyer.balance - item.price)
    order = OrderLine.insert(order_id="O1", quantity=1, buyer=buyer, item=item)
```

### Typescript

<!-- walkthrough:purchase:call:typescript -->
```typescript
Plan.build(() => {
  const item = Item.match("I1").where(Item.stock.gt(0n));
  let buyer = Account.match("A1");
  const balance = buyer.balance;
  buyer = buyer.where(balance.ge(item.price));
  item.set({ stock: item.stock.sub(1n) });
  buyer.set({ balance: buyer.balance.sub(item.price) });
  return OrderLine.insert({ orderId: "O1", quantity: 1n, buyer, item });
});
```

### Rust

<!-- walkthrough:purchase:call:rust -->
```rust
Plan::build(|p| {
    let item = Item::matching(p, "I1").where_(ItemType::stock.gt(0_i64));
    let buyer = Account::matching(p, "A1");
    let buyer = buyer.clone().where_(buyer.balance().ge(item.price()));
    item.set(p).stock(item.stock() - 1_i64);
    buyer.set(p).balance(buyer.balance() - item.price());
    OrderLine::insert(p, OrderLineInsert { order_id:Some("O1".into()), quantity:Some(1_i64.into()), buyer:Some(&buyer), item:Some(&item) })
})
```

### C

<!-- walkthrough:purchase:call:c -->
```c
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
```

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
        "variable": "account_balance"
      },
      {
        "id": 6,
        "variable": "item_price"
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
            "attribute_id": "balance",
            "kind": "has",
            "owner": 3
          },
          {
            "attribute": 6,
            "attribute_id": "price",
            "kind": "has",
            "owner": 0
          },
          {
            "comparator": "greater_or_equal",
            "kind": "value",
            "left": {
              "binding": 5,
              "kind": "binding"
            },
            "right": {
              "binding": 6,
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
                    "binding": 5,
                    "kind": "binding"
                  }
                },
                "op": "sub",
                "rhs": {
                  "kind": "operand",
                  "value": {
                    "binding": 6,
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
given $item_key: string, $item_stock_2: integer, $account_key: string, $item_stock_3: integer, $order_line_order_id: string, $order_line_quantity: integer; match $item isa! item; $item has sku $item_sku; $item_sku isa! sku; $item_sku == $item_key; $item has stock $item_stock; $item_stock isa! stock; $item_stock > $item_stock_2; $account isa! account; $account has account-id $account_account_id; $account_account_id isa! account-id; $account_account_id == $account_key; $account has balance $account_balance; $account_balance isa! balance; $item has price $item_price; $item_price isa! price; $account_balance >= $item_price; let $item_stock_4 = ($item_stock - $item_stock_3); let $account_balance_2 = ($account_balance - $item_price); update $item has stock == $item_stock_4; $account has balance == $account_balance_2; insert $order_line isa order-line, links (buyer: $account, item: $item), has order-id == $order_line_order_id, has quantity == $order_line_quantity; match $order_line isa! $PlanFetchType; try { $order_line links (buyer: $PlanFetchRole0); 
 $PlanFetchRole0 isa! $PlanFetchRole0Type; 
}; try { $order_line links (item: $PlanFetchRole1); 
 $PlanFetchRole1 isa! $PlanFetchRole1Type; 
}; fetch { "_iid": iid($order_line), "_type": label($PlanFetchType), "attributes": {  $order_line.*}, "_role_0_iid": iid($PlanFetchRole0), "_role_0_type": label($PlanFetchRole0Type), "_role_0_attributes": {  $PlanFetchRole0.*}, "_role_1_iid": iid($PlanFetchRole1), "_role_1_type": label($PlanFetchRole1Type), "_role_1_attributes": {  $PlanFetchRole1.*}};
```

### Input counts and columns

<!-- walkthrough:purchase:inputs -->
```json
[
  {
    "given": {
      "canonical_bytes": 93,
      "row_count": 1
    },
    "inputs": [
      {
        "optional": false,
        "public_name": "item_key",
        "value_type": "string"
      },
      {
        "optional": false,
        "public_name": "item_stock_2",
        "value_type": "long"
      },
      {
        "optional": false,
        "public_name": "account_key",
        "value_type": "string"
      },
      {
        "optional": false,
        "public_name": "item_stock_3",
        "value_type": "long"
      },
      {
        "optional": false,
        "public_name": "order_line_order_id",
        "value_type": "string"
      },
      {
        "optional": false,
        "public_name": "order_line_quantity",
        "value_type": "long"
      }
    ]
  }
]
```

### Raw answer

<!-- walkthrough:purchase:raw -->
```json
[
  {
    "_iid": "iid:0",
    "_role_0_attributes": {
      "account-id": "A1",
      "balance": 7
    },
    "_role_0_iid": "iid:1",
    "_role_0_type": "account",
    "_role_1_attributes": {
      "price": 3,
      "sku": "I1",
      "stock": 4
    },
    "_role_1_iid": "iid:2",
    "_role_1_type": "item",
    "_type": "order-line",
    "attributes": {
      "order-id": "O1",
      "quantity": 1
    }
  }
]
```

### Typed result

<!-- walkthrough:purchase:models -->
```json
[
  {
    "buyer": {
      "iid": "iid:0",
      "type": "account"
    },
    "deleted": false,
    "iid": "iid:1",
    "item": {
      "iid": "iid:2",
      "type": "item"
    },
    "order_id": "O1",
    "quantity": 1,
    "type": "order-line"
  }
]
```

## Delete returning concrete subtypes

`Document` is abstract. Select its concrete `Report` and `Memo` subtypes with status `archived`, then delete them and return their generated models. The returned objects retain their concrete types and have `deleted: true`.

### Python

<!-- walkthrough:delete_subtypes:call:python -->
```python
with Plan():
    gone = Document.subtypes().where(Document.status == "archived").delete_returning()
```

### Typescript

<!-- walkthrough:delete_subtypes:call:typescript -->
```typescript
Plan.build(() => Document.subtypes().where(Document.status.eq("archived")).deleteReturning())
```

### Rust

<!-- walkthrough:delete_subtypes:call:rust -->
```rust
Plan::build(|p| {
    let document = p.subtypes::<Document>(None).where_(DocumentType::status.eq("archived"));
    let selected = document.models(p, &[]);
    document.delete(p);
    selected
})
```

### C

<!-- walkthrough:delete_subtypes:call:c -->
```c
type_bridge_plan_builder_t *p = recording();
tb_walkthrough_document_handle_t document;
tb_walkthrough_document_plan_t selected;
OK(tb_walkthrough_document_subtypes(p, &document));
OK(tb_walkthrough_document_where(
    p, document,
    tb_string_eq(tb_walkthrough_document_plan_status(p, document),
                 tb_string(view("archived")))));
OK(tb_walkthrough_document_delete_returning(p, document, &selected));
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

### TypeQL

<!-- walkthrough:delete_subtypes:typeql -->
```typeql
given $document_status_2: string; match $document isa document; $document has status $document_status; $document_status isa! status; $document_status == $document_status_2; match $document isa! $PlanFetchType; fetch { "_iid": iid($document), "_type": label($PlanFetchType), "attributes": {  $document.*}};

given $document_status_2: string; match $document isa document; $document has status $document_status; $document_status isa! status; $document_status == $document_status_2; delete $document;
```

### Input counts and columns

<!-- walkthrough:delete_subtypes:inputs -->
```json
[
  {
    "given": {
      "canonical_bytes": 25,
      "row_count": 1
    },
    "inputs": [
      {
        "optional": false,
        "public_name": "document_status_2",
        "value_type": "string"
      }
    ]
  }
]
```

### Raw answer

<!-- walkthrough:delete_subtypes:raw -->
```json
[
  {
    "_iid": "iid:0",
    "_type": "memo",
    "attributes": {
      "doc-id": "M1",
      "status": "archived"
    }
  },
  {
    "_iid": "iid:1",
    "_type": "report",
    "attributes": {
      "doc-id": "R1",
      "status": "archived"
    }
  }
]
```

### Typed result

<!-- walkthrough:delete_subtypes:models -->
```json
[
  {
    "deleted": true,
    "doc_id": "M1",
    "iid": "iid:0",
    "status": "archived",
    "type": "memo"
  },
  {
    "deleted": true,
    "doc_id": "R1",
    "iid": "iid:1",
    "status": "archived",
    "type": "report"
  }
]
```

## Bulk insert with automatic splitting

Insert 10,000 items, keyed `B0` through `B9999`, with price 3 and stock 10. The SDK splits the input automatically into three statements. The transcript contains every raw document and typed model in input order; the page shows the count, first two records and last record.

### Python

<!-- walkthrough:bulk_insert:call:python -->
```python
with Plan():
    batch = Item.insert_many(Item.Values(sku=f"B{n}", price=3, stock=10) for n in range(10_000))
```

### Typescript

<!-- walkthrough:bulk_insert:call:typescript -->
```typescript
Plan.build(() => Item.insertMany(Array.from({ length: 10_000 }, (_, n) => ({ sku: `B${n}`, price: 3n, stock: 10n }))))
```

### Rust

<!-- walkthrough:bulk_insert:call:rust -->
```rust
Plan::build(|p| Item::insert_many(p, (0..10_000).map(|n| ItemValues { price:Some(3), sku:format!("B{n}"), stock:Some(10) })))
```

### C

<!-- walkthrough:bulk_insert:call:c -->
```c
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

### TypeQL

<!-- walkthrough:bulk_insert:typeql -->
```typeql
given $item_price: integer, $item_sku: string, $item_stock: integer; insert $item isa item, has price == $item_price, has sku == $item_sku, has stock == $item_stock; match $item isa! $PlanFetchType; fetch { "_iid": iid($item), "_type": label($PlanFetchType), "attributes": {  $item.*}};

given $item_price: integer, $item_sku: string, $item_stock: integer; insert $item isa item, has price == $item_price, has sku == $item_sku, has stock == $item_stock; match $item isa! $PlanFetchType; fetch { "_iid": iid($item), "_type": label($PlanFetchType), "attributes": {  $item.*}};

given $item_price: integer, $item_sku: string, $item_stock: integer; insert $item isa item, has price == $item_price, has sku == $item_sku, has stock == $item_stock; match $item isa! $PlanFetchType; fetch { "_iid": iid($item), "_type": label($PlanFetchType), "attributes": {  $item.*}};
```

### Raw answer

<!-- walkthrough:bulk_insert:raw -->
```json
{
  "count": 10000,
  "first": [
    {
      "_iid": "iid:0",
      "_type": "item",
      "attributes": {
        "price": 3,
        "sku": "B0",
        "stock": 10
      }
    },
    {
      "_iid": "iid:1",
      "_type": "item",
      "attributes": {
        "price": 3,
        "sku": "B1",
        "stock": 10
      }
    }
  ],
  "last": [
    {
      "_iid": "iid:9999",
      "_type": "item",
      "attributes": {
        "price": 3,
        "sku": "B9999",
        "stock": 10
      }
    }
  ]
}
```

### Typed result

<!-- walkthrough:bulk_insert:models -->
```json
{
  "count": 10000,
  "first": [
    {
      "deleted": false,
      "iid": "iid:0",
      "price": 3,
      "sku": "B0",
      "stock": 10,
      "type": "item"
    },
    {
      "deleted": false,
      "iid": "iid:1",
      "price": 3,
      "sku": "B1",
      "stock": 10,
      "type": "item"
    }
  ],
  "last": [
    {
      "deleted": false,
      "iid": "iid:9999",
      "price": 3,
      "sku": "B9999",
      "stock": 10,
      "type": "item"
    }
  ]
}
```

## Manager insert

Insert item `M1` with price 3 and stock 5 through the generated manager. Retain its typed IID for the later lookup and deletion.

### Python

<!-- walkthrough:insert:call:python -->
```python
Item.manager(db).insert(Item(sku=Sku("M1"), price=Price(3), stock=Stock(5)))
```

### Typescript

<!-- walkthrough:insert:call:typescript -->
```typescript
Item.manager(db).insert(Item.create({ sku: Sku.create("M1"), price: Price.create(3n), stock: Stock.create(5n) }))
```

### Rust

<!-- walkthrough:insert:call:rust -->
```rust
db.entities::<Item>().insert(item_create(5)?).await?
```

### C

<!-- walkthrough:insert:call:c -->
```c
tb_walkthrough_item_database_insert(database, created, NULL, NULL, &inserted, &diagnostics);
```

### Plan

<!-- walkthrough:insert:plan -->
```json
{
  "fingerprints": [
    "3de56439d5e6a72c7e15e893e70078643384435aeb14c2ad9ad552e3fb51eaab"
  ],
  "source": "TRACE"
}
```

### TypeQL

<!-- walkthrough:insert:typeql -->
```typeql
given $in0: integer, $in1: string, $in2: integer; insert $thing0 isa item, has price == $in0, has sku == $in1, has stock == $in2; match $thing0 isa! $PlanFetchType; fetch { "_iid": iid($thing0), "_type": label($PlanFetchType), "attributes": {  $thing0.*}};
```

### Raw answer

<!-- walkthrough:insert:raw -->
```json
"raw documents: not exposed by manager result"
```

### Typed result

<!-- walkthrough:insert:models -->
```json
[
  {
    "deleted": false,
    "iid": "iid:0",
    "price": 3,
    "sku": "M1",
    "stock": 5,
    "type": "item"
  }
]
```

## Manager put

Put the same key `M1` with stock 6. The existing item keeps its identity.

### Python

<!-- walkthrough:put:call:python -->
```python
Item.manager(db).put(Item(sku=Sku("M1"), price=Price(3), stock=Stock(6)))
```

### Typescript

<!-- walkthrough:put:call:typescript -->
```typescript
Item.manager(db).put(Item.create({ sku: Sku.create("M1"), price: Price.create(3n), stock: Stock.create(6n) }))
```

### Rust

<!-- walkthrough:put:call:rust -->
```rust
db.entities::<Item>().put(item_create(6)?).await?
```

### C

<!-- walkthrough:put:call:c -->
```c
tb_walkthrough_item_database_put(database, created, NULL, NULL, &put, &diagnostics);
```

### Plan

<!-- walkthrough:put:plan -->
```json
{
  "fingerprints": [
    "b0cdda615ae81bf6a2944cdd62cfbff644baecf2f404ead1c3a773dc73f34c1b"
  ],
  "source": "TRACE"
}
```

### TypeQL

<!-- walkthrough:put:typeql -->
```typeql
given $in0: string, $in1: integer, $in2: integer; put $thing0 isa! item, has sku == $in0; update $thing0 has price == $in1; $thing0 has stock == $in2; select $thing0; distinct; match $thing0 isa! $PlanFetchType; fetch { "_iid": iid($thing0), "_type": label($PlanFetchType), "attributes": {  $thing0.*}};
```

### Raw answer

<!-- walkthrough:put:raw -->
```json
"raw documents: not exposed by manager result"
```

### Typed result

<!-- walkthrough:put:models -->
```json
[
  {
    "deleted": false,
    "iid": "iid:0",
    "price": 3,
    "sku": "M1",
    "stock": 6,
    "type": "item"
  }
]
```

## Filter by typed IID

Read the inserted item with its typed IID. The returned model has stock 6.

### Python

<!-- walkthrough:typed_iid:call:python -->
```python
Item.manager(db).filter(iid=iid).all()
```

### Typescript

<!-- walkthrough:typed_iid:call:typescript -->
```typescript
Item.manager(db).filter({ iid }).all()
```

### Rust

<!-- walkthrough:typed_iid:call:rust -->
```rust
db.entities::<Item>().filter()?.iid(iid)?.all().await?
```

### C

<!-- walkthrough:typed_iid:call:c -->
```c
type_bridge_manager_filter_iid(root, iid, &selected, &diagnostics);
type_bridge_database_manager_all(database, selected, NULL, NULL, &result, &diagnostics);
```

### Plan

<!-- walkthrough:typed_iid:plan -->
```json
{
  "fingerprints": [
    "f8c11a615a8b14e672afd9fb07868c0cbabd3e6491238ecc84399c2ac2a472f5"
  ],
  "source": "TRACE"
}
```

### TypeQL

<!-- walkthrough:typed_iid:typeql -->
```typeql
given $input0: string; match $b0 isa! item; $b1 == $input0; let $b1 = iid($b0); select $b0, $b1; distinct; sort $b1 asc; limit 100001; match $b0 isa! $PlanFetchType; fetch { "_iid": iid($b0), "_type": label($PlanFetchType), "attributes": {  $b0.*}};
```

### Raw answer

<!-- walkthrough:typed_iid:raw -->
```json
"raw documents: not exposed by manager result"
```

### Typed result

<!-- walkthrough:typed_iid:models -->
```json
[
  {
    "deleted": false,
    "iid": "iid:0",
    "price": 3,
    "sku": "M1",
    "stock": 6,
    "type": "item"
  }
]
```

## Filtered typed read with fetch

Filter items by the generated `Sku` attribute, equal to `M1`. Fetch hydrates the complete generated item model.

### Python

<!-- walkthrough:filtered_fetch:call:python -->
```python
Item.manager(db).filter(sku=Sku("M1")).all()
```

### Typescript

<!-- walkthrough:filtered_fetch:call:typescript -->
```typescript
Item.manager(db).filter({ sku: Sku.create("M1") }).all()
```

### Rust

<!-- walkthrough:filtered_fetch:call:rust -->
```rust
db.entities::<Item>().where_(ItemType::sku, ProjectedManagerComparison::Eq, &Sku::try_new("M1")?)?.all().await?
```

### C

<!-- walkthrough:filtered_fetch:call:c -->
```c
type_bridge_manager_filter_compare(root, &tb_walkthrough_projected_field_token_5, TYPE_BRIDGE_QUERY_COMPARE_EQUAL, (const type_bridge_projected_value_t *)sku, &filtered, &diagnostics);
type_bridge_database_manager_all(database, filtered, NULL, NULL, &result, &diagnostics);
```

### Plan

<!-- walkthrough:filtered_fetch:plan -->
```json
{
  "fingerprints": [
    "59c795ac35783e6a01dd22543bceec4646f562f2bcf33ba14e7bfe756a171fd6"
  ],
  "source": "TRACE"
}
```

### TypeQL

<!-- walkthrough:filtered_fetch:typeql -->
```typeql
given $input0: string; match $b0 isa! item; $b0 has sku $b1; $b1 isa! sku; $b1 == $input0; let $b2 = iid($b0); select $b0, $b2; distinct; sort $b2 asc; limit 100001; match $b0 isa! $PlanFetchType; fetch { "_iid": iid($b0), "_type": label($PlanFetchType), "attributes": {  $b0.*}};
```

### Raw answer

<!-- walkthrough:filtered_fetch:raw -->
```json
"raw documents: not exposed by manager result"
```

### Typed result

<!-- walkthrough:filtered_fetch:models -->
```json
[
  {
    "deleted": false,
    "iid": "iid:0",
    "price": 3,
    "sku": "M1",
    "stock": 6,
    "type": "item"
  }
]
```

## Manager update

Update the selected item to price 3 and stock 8, retaining its key and identity. The return value is hydrated after the write.

### Python

<!-- walkthrough:update:call:python -->
```python
Item.manager(db).filter(iid=iid).update(Item(sku=Sku("M1"), price=Price(3), stock=Stock(8)))
```

### Typescript

<!-- walkthrough:update:call:typescript -->
```typescript
Item.manager(db).filter({ iid }).update(Item.create({ sku: Sku.create("M1"), price: Price.create(3n), stock: Stock.create(8n) }))
```

### Rust

<!-- walkthrough:update:call:rust -->
```rust
db.entities::<Item>().filter()?.iid(iid)?.update(item_create(8)?).await?
```

### C

<!-- walkthrough:update:call:c -->
```c
type_bridge_database_manager_update(database, selected, (const type_bridge_projected_create_t *)created, NULL, NULL, &updated, &diagnostics);
```

### Plan

<!-- walkthrough:update:plan -->
```json
{
  "fingerprints": [
    "6ea07bafe67a51646375eae638f9f0d67ffa033e1a69c630784b75fc302a7bd6"
  ],
  "source": "TRACE"
}
```

### TypeQL

<!-- walkthrough:update:typeql -->
```typeql
given $in0: integer, $in1: integer; match $thing0 isa! item; $thing0 iid 0x1e00020000000000002711; match try { $thing0 has price $PlanHas1x0; 
 $PlanHas1x0 isa! price; 
}; try { $thing0 has stock $PlanHas1x1; 
 $PlanHas1x1 isa! stock; 
}; delete try { has $PlanHas1x0 of $thing0; }; try { has $PlanHas1x1 of $thing0; }; insert $thing0 has price == $in0; $thing0 has stock == $in1; select $thing0; distinct; match $thing0 isa! $PlanFetchType; fetch { "_iid": iid($thing0), "_type": label($PlanFetchType), "attributes": {  $thing0.*}};
```

### Raw answer

<!-- walkthrough:update:raw -->
```json
"raw documents: not exposed by manager result"
```

### Typed result

<!-- walkthrough:update:models -->
```json
[
  {
    "deleted": false,
    "iid": "iid:0",
    "price": 3,
    "sku": "M1",
    "stock": 8,
    "type": "item"
  }
]
```

## Insert related rows

Insert order lines `L1` and `L2` in one homogeneous batch. Both refer to the existing account and item selected by the purchase, using their typed role references.

### Python

<!-- walkthrough:insert_many:call:python -->
```python
OrderLine.manager(db).insert_many([OrderLine(order_id=OrderId(key), quantity=Quantity(1), buyer=bought[0].buyer, item=bought[0].item) for key in ("L1", "L2")])
```

### Typescript

<!-- walkthrough:insert_many:call:typescript -->
```typescript
OrderLine.manager(db).insertMany(["L1", "L2"].map(key => OrderLine.create({ orderId: OrderId.create(key), quantity: Quantity.create(1n), buyer: bought[0].buyer, item: bought[0].item })))
```

### Rust

<!-- walkthrough:insert_many:call:rust -->
```rust
db.relations::<OrderLine>().insert_many(lines).await?
```

### C

<!-- walkthrough:insert_many:call:c -->
```c
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

OK(type_bridge_database_projected_batch_execute_v1(
    database, batch, NULL, NULL, &result, &diagnostics));
```

### Plan

<!-- walkthrough:insert_many:plan -->
```json
{
  "fingerprints": [
    ""
  ],
  "source": "TRACE"
}
```

### TypeQL

<!-- walkthrough:insert_many:typeql -->
```typeql
given $ordinal_input: integer, $field_0_0: string?, $field_1_0: integer?, $player_0_0: string, $player_1_0: string; match { $player2 isa! account; iid($player2) == $player_0_0; }; { $player3 isa! item; iid($player3) == $player_1_0; }; let $ordinal1 = $ordinal_input; insert $root0 isa order-line; insert try { $root0 has order-id == $field_0_0; 
}; try { $root0 has quantity == $field_1_0; 
}; $root0 links (buyer: $player2); $root0 links (item: $player3); select $root0, $ordinal1; distinct; match $root0 isa! $PlanFetchType; fetch { "_iid": iid($root0), "_type": label($PlanFetchType), "attributes": {  $root0.*}, "ordinal": $ordinal1, "_role_0_players": [ match $root0 links (buyer: $PlanFetchRole0); $PlanFetchRole0 isa! $PlanFetchRole0Type;fetch { "_iid": iid($PlanFetchRole0), "_type": label($PlanFetchRole0Type), "attributes": {  $PlanFetchRole0.*}};], "_role_1_players": [ match $root0 links (item: $PlanFetchRole1); $PlanFetchRole1 isa! $PlanFetchRole1Type;fetch { "_iid": iid($PlanFetchRole1), "_type": label($PlanFetchRole1Type), "attributes": {  $PlanFetchRole1.*}};]};
```

### Raw answer

<!-- walkthrough:insert_many:raw -->
```json
"raw documents: not exposed by manager result"
```

### Typed result

<!-- walkthrough:insert_many:models -->
```json
[
  {
    "buyer": {
      "iid": "iid:0",
      "type": "account"
    },
    "deleted": false,
    "iid": "iid:1",
    "item": {
      "iid": "iid:2",
      "type": "item"
    },
    "order_id": "L1",
    "quantity": 1,
    "type": "order-line"
  },
  {
    "buyer": {
      "iid": "iid:0",
      "type": "account"
    },
    "deleted": false,
    "iid": "iid:3",
    "item": {
      "iid": "iid:2",
      "type": "item"
    },
    "order_id": "L2",
    "quantity": 1,
    "type": "order-line"
  }
]
```

## Manager delete returning

Delete item `M1` by its typed IID and return its last values, including stock 8. The returned record is marked deleted.

### Python

<!-- walkthrough:delete_returning:call:python -->
```python
Item.manager(db).delete_returning(updated[0])
```

### Typescript

<!-- walkthrough:delete_returning:call:typescript -->
```typescript
Item.manager(db).deleteReturning(iid)
```

### Rust

<!-- walkthrough:delete_returning:call:rust -->
```rust
db.entities::<Item>().delete_returning(iid).await?
```

### C

<!-- walkthrough:delete_returning:call:c -->
```c
type_bridge_database_manager_delete_returning(database, &tb_walkthrough_projected_model_token_2, *iid, NULL, NULL, &deleted, &diagnostics);
```

### Plan

<!-- walkthrough:delete_returning:plan -->
```json
{
  "fingerprints": [
    "505ed9e41d06ca9b3350296199b5beb44fc46f0817bb735d7d7816beac54a1c3",
    "505ed9e41d06ca9b3350296199b5beb44fc46f0817bb735d7d7816beac54a1c3"
  ],
  "source": "TRACE"
}
```

### TypeQL

<!-- walkthrough:delete_returning:typeql -->
```typeql
given $in0: string; match $root0 isa! item; $identity1 == $in0; let $identity1 = iid($root0); match $root0 isa! $PlanFetchType; fetch { "_iid": iid($root0), "_type": label($PlanFetchType), "attributes": {  $root0.*}};

given $in0: string; match $root0 isa! item; $identity1 == $in0; let $identity1 = iid($root0); delete $root0;
```

### Raw answer

<!-- walkthrough:delete_returning:raw -->
```json
"raw documents: not exposed by manager result"
```

### Typed result

<!-- walkthrough:delete_returning:models -->
```json
[
  {
    "deleted": true,
    "iid": "iid:0",
    "price": 3,
    "sku": "M1",
    "stock": 8,
    "type": "item"
  }
]
```

## One transaction with dependent plans

Stage a new account `A2`, a new item `I2`, and order line `O2` in one transaction. The third plan matches the keys inserted by the first two, so those players are visible before commit. The programs assert the order line points to the new account and item. Inserted selections cannot be bound as role-player match handles, so this uses three staged plans with subsequent key matches.

### Python

<!-- walkthrough:staged_transaction:call:python -->
```python
with Plan():
    new_account = Account.insert(account_id="A2", balance=20)
with Plan():
    new_item = Item.insert(sku="I2", price=4, stock=3)
with Plan():
    buyer = Account.match("A2")
    item = Item.match("I2")
    new_order = OrderLine.insert(order_id="O2", quantity=1, buyer=buyer, item=item)
with db.staged_transaction() as tx:
    pending = [tx.stage(plan) for plan in (new_account, new_item, new_order)]
```

### Typescript

<!-- walkthrough:staged_transaction:call:typescript -->
```typescript
const newAccount = Plan.build(() => Account.insert({ accountId: "A2", balance: 20n }));
const newItem = Plan.build(() => Item.insert({ price: 4n, sku: "I2", stock: 3n }));
const newOrder = Plan.build(() => {
  const buyer = Account.match("A2");
  const item = Item.match("I2");
  return OrderLine.insert({ orderId: "O2", quantity: 1n, buyer, item });
});
const tx = db.stagedTransaction();
const pending = [tx.stage(newAccount), tx.stage(newItem), tx.stage(newOrder)];
await tx.commit();
```

### Rust

<!-- walkthrough:staged_transaction:call:rust -->
```rust
let new_account = account("A2", 20)?;
let new_item = item("I2", 4, 3)?;
let new_order = Plan::build(|p| {
    let buyer = Account::matching(p, "A2");
    let item = Item::matching(p, "I2");
    OrderLine::insert(p, OrderLineInsert { order_id:Some("O2".into()), quantity:Some(1_i64.into()), buyer:Some(&buyer), item:Some(&item) })
})?;
let mut tx = db.transaction(Default::default());
let account_pending = tx.stage(&new_account)?;
let item_pending = tx.stage(&new_item)?;
let order_pending = tx.stage(&new_order)?;
tx.commit().await?;
```

### C

<!-- walkthrough:staged_transaction:call:c -->
```c
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
type_bridge_staged_transaction_t *tx = NULL;
type_bridge_pending_t *pending[3] = {NULL};
OK(type_bridge_database_transaction(database, TYPE_BRIDGE_TRANSACTION_WRITE,
                                    &tx, &diagnostics));
for (size_t i = 0; i < 3; ++i)
  OK(type_bridge_staged_transaction_stage_selection(
      tx, p[i], selection[i], &pending[i], &diagnostics));
OK(type_bridge_staged_transaction_commit(tx, &diagnostics));
```

### Plan

<!-- walkthrough:staged_transaction:plan -->
```json
{
  "plans": [
    {
      "bindings": [
        {
          "id": 0,
          "variable": "account"
        }
      ],
      "format": "typebridge.query-plan/v3",
      "functions": [],
      "inputs": [
        {
          "id": 0,
          "optional": false,
          "public_name": "account_account_id",
          "value_type": "string"
        },
        {
          "id": 1,
          "optional": false,
          "public_name": "account_balance",
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
          "kind": "insert",
          "things": [
            {
              "binding": 0,
              "fields": [
                [
                  {
                    "attribute": "account-id",
                    "owner": {
                      "kind": "entity",
                      "label": "account"
                    }
                  },
                  {
                    "column": 0,
                    "kind": "input"
                  }
                ],
                [
                  {
                    "attribute": "balance",
                    "owner": {
                      "kind": "entity",
                      "label": "account"
                    }
                  },
                  {
                    "column": 1,
                    "kind": "input"
                  }
                ]
              ],
              "players": [],
              "type_id": {
                "kind": "entity",
                "label": "account"
              }
            }
          ]
        },
        {
          "kind": "fetch",
          "roles": [],
          "thing": 0
        }
      ],
      "required_capabilities": [
        "query.fetch.thing",
        "query.input.columns",
        "query.input.given-rows",
        "query.output.rows",
        "query.plan",
        "query.plan.v3",
        "query.write.insert"
      ]
    },
    {
      "bindings": [
        {
          "id": 0,
          "variable": "item"
        }
      ],
      "format": "typebridge.query-plan/v3",
      "functions": [],
      "inputs": [
        {
          "id": 0,
          "optional": false,
          "public_name": "item_price",
          "value_type": "long"
        },
        {
          "id": 1,
          "optional": false,
          "public_name": "item_sku",
          "value_type": "string"
        },
        {
          "id": 2,
          "optional": false,
          "public_name": "item_stock",
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
          "kind": "insert",
          "things": [
            {
              "binding": 0,
              "fields": [
                [
                  {
                    "attribute": "price",
                    "owner": {
                      "kind": "entity",
                      "label": "item"
                    }
                  },
                  {
                    "column": 0,
                    "kind": "input"
                  }
                ],
                [
                  {
                    "attribute": "sku",
                    "owner": {
                      "kind": "entity",
                      "label": "item"
                    }
                  },
                  {
                    "column": 1,
                    "kind": "input"
                  }
                ],
                [
                  {
                    "attribute": "stock",
                    "owner": {
                      "kind": "entity",
                      "label": "item"
                    }
                  },
                  {
                    "column": 2,
                    "kind": "input"
                  }
                ]
              ],
              "players": [],
              "type_id": {
                "kind": "entity",
                "label": "item"
              }
            }
          ]
        },
        {
          "kind": "fetch",
          "roles": [],
          "thing": 0
        }
      ],
      "required_capabilities": [
        "query.fetch.thing",
        "query.input.columns",
        "query.input.given-rows",
        "query.output.rows",
        "query.plan",
        "query.plan.v3",
        "query.write.insert"
      ]
    },
    {
      "bindings": [
        {
          "id": 0,
          "variable": "account"
        },
        {
          "id": 1,
          "variable": "account_account_id"
        },
        {
          "id": 2,
          "variable": "item"
        },
        {
          "id": 3,
          "variable": "item_sku"
        },
        {
          "id": 4,
          "variable": "order_line"
        }
      ],
      "format": "typebridge.query-plan/v3",
      "functions": [],
      "inputs": [
        {
          "id": 0,
          "optional": false,
          "public_name": "account_key",
          "value_type": "string"
        },
        {
          "id": 1,
          "optional": false,
          "public_name": "item_key",
          "value_type": "string"
        },
        {
          "id": 2,
          "optional": false,
          "public_name": "order_line_order_id",
          "value_type": "string"
        },
        {
          "id": 3,
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
                "label": "account"
              }
            },
            {
              "attribute": 1,
              "attribute_id": "account-id",
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
              "binding": 2,
              "include_subtypes": false,
              "kind": "isa",
              "type_id": {
                "kind": "entity",
                "label": "item"
              }
            },
            {
              "attribute": 3,
              "attribute_id": "sku",
              "kind": "has",
              "owner": 2
            },
            {
              "comparator": "equal",
              "kind": "value",
              "left": {
                "binding": 3,
                "kind": "binding"
              },
              "right": {
                "column": 1,
                "kind": "input"
              }
            }
          ]
        },
        {
          "kind": "insert",
          "things": [
            {
              "binding": 4,
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
                    "column": 2,
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
                    "column": 3,
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
                  0
                ],
                [
                  {
                    "declaring_relation": "order-line",
                    "label": "item"
                  },
                  2
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
          "thing": 4
        }
      ],
      "required_capabilities": [
        "query.fetch.thing",
        "query.input.columns",
        "query.input.given-rows",
        "query.output.rows",
        "query.pattern.has",
        "query.pattern.isa",
        "query.pattern.value",
        "query.plan",
        "query.plan.v3",
        "query.write.insert"
      ]
    }
  ],
  "source": "canonical_bytes"
}
```

### TypeQL

<!-- walkthrough:staged_transaction:typeql -->
```typeql
given $account_account_id: string, $account_balance: integer; insert $account isa account, has account-id == $account_account_id, has balance == $account_balance; match $account isa! $PlanFetchType; fetch { "_iid": iid($account), "_type": label($PlanFetchType), "attributes": {  $account.*}};

given $item_price: integer, $item_sku: string, $item_stock: integer; insert $item isa item, has price == $item_price, has sku == $item_sku, has stock == $item_stock; match $item isa! $PlanFetchType; fetch { "_iid": iid($item), "_type": label($PlanFetchType), "attributes": {  $item.*}};

given $account_key: string, $item_key: string, $order_line_order_id: string, $order_line_quantity: integer; match $account isa! account; $account has account-id $account_account_id; $account_account_id isa! account-id; $account_account_id == $account_key; $item isa! item; $item has sku $item_sku; $item_sku isa! sku; $item_sku == $item_key; insert $order_line isa order-line, links (buyer: $account, item: $item), has order-id == $order_line_order_id, has quantity == $order_line_quantity; match $order_line isa! $PlanFetchType; try { $order_line links (buyer: $PlanFetchRole0); 
 $PlanFetchRole0 isa! $PlanFetchRole0Type; 
}; try { $order_line links (item: $PlanFetchRole1); 
 $PlanFetchRole1 isa! $PlanFetchRole1Type; 
}; fetch { "_iid": iid($order_line), "_type": label($PlanFetchType), "attributes": {  $order_line.*}, "_role_0_iid": iid($PlanFetchRole0), "_role_0_type": label($PlanFetchRole0Type), "_role_0_attributes": {  $PlanFetchRole0.*}, "_role_1_iid": iid($PlanFetchRole1), "_role_1_type": label($PlanFetchRole1Type), "_role_1_attributes": {  $PlanFetchRole1.*}};
```

### Input counts and columns

<!-- walkthrough:staged_transaction:inputs -->
```json
[
  {
    "given": {
      "canonical_bytes": 34,
      "row_count": 1
    },
    "inputs": [
      {
        "optional": false,
        "public_name": "account_account_id",
        "value_type": "string"
      },
      {
        "optional": false,
        "public_name": "account_balance",
        "value_type": "long"
      }
    ]
  },
  {
    "given": {
      "canonical_bytes": 47,
      "row_count": 1
    },
    "inputs": [
      {
        "optional": false,
        "public_name": "item_price",
        "value_type": "long"
      },
      {
        "optional": false,
        "public_name": "item_sku",
        "value_type": "string"
      },
      {
        "optional": false,
        "public_name": "item_stock",
        "value_type": "long"
      }
    ]
  },
  {
    "given": {
      "canonical_bytes": 65,
      "row_count": 1
    },
    "inputs": [
      {
        "optional": false,
        "public_name": "account_key",
        "value_type": "string"
      },
      {
        "optional": false,
        "public_name": "item_key",
        "value_type": "string"
      },
      {
        "optional": false,
        "public_name": "order_line_order_id",
        "value_type": "string"
      },
      {
        "optional": false,
        "public_name": "order_line_quantity",
        "value_type": "long"
      }
    ]
  }
]
```

### Raw answer

<!-- walkthrough:staged_transaction:raw -->
```json
[
  {
    "_iid": "iid:0",
    "_type": "account",
    "attributes": {
      "account-id": "A2",
      "balance": 20
    }
  },
  {
    "_iid": "iid:1",
    "_type": "item",
    "attributes": {
      "price": 4,
      "sku": "I2",
      "stock": 3
    }
  },
  {
    "_iid": "iid:2",
    "_role_0_attributes": {
      "account-id": "A2",
      "balance": 20
    },
    "_role_0_iid": "iid:0",
    "_role_0_type": "account",
    "_role_1_attributes": {
      "price": 4,
      "sku": "I2",
      "stock": 3
    },
    "_role_1_iid": "iid:1",
    "_role_1_type": "item",
    "_type": "order-line",
    "attributes": {
      "order-id": "O2",
      "quantity": 1
    }
  }
]
```

### Typed result

<!-- walkthrough:staged_transaction:models -->
```json
[
  {
    "account_id": "A2",
    "balance": 20,
    "deleted": false,
    "iid": "iid:0",
    "type": "account"
  },
  {
    "deleted": false,
    "iid": "iid:1",
    "price": 4,
    "sku": "I2",
    "stock": 3,
    "type": "item"
  },
  {
    "buyer": {
      "iid": "iid:0",
      "type": "account"
    },
    "deleted": false,
    "iid": "iid:2",
    "item": {
      "iid": "iid:1",
      "type": "item"
    },
    "order_id": "O2",
    "quantity": 1,
    "type": "order-line"
  }
]
```

## Unchecked raw query

Count the four order lines with raw `execute_query`. This call has no typed plan and no generated-model hydration. Its result is explicitly labelled unchecked in the transcript.

### Python

<!-- walkthrough:raw_unchecked:call:python -->
```python
db.execute_query("match $x isa order-line; reduce $n = count;")
```

### Typescript

<!-- walkthrough:raw_unchecked:call:typescript -->
```typescript
db.executeQuery("match $x isa order-line; reduce $n = count;")
```

### Rust

<!-- walkthrough:raw_unchecked:call:rust -->
```rust
db.execute_query("match $x isa order-line; reduce $n = count;", TransactionKind::Read).await?
```

### C

<!-- walkthrough:raw_unchecked:call:c -->
```c
type_bridge_database_execute_query(database, view("match $x isa order-line; reduce $n = count;"), TYPE_BRIDGE_TRANSACTION_READ, &result, &errors);
```

### Raw answer

<!-- walkthrough:raw_unchecked:raw -->
```json
{
  "answerType": "conceptRows",
  "answers": [
    {
      "data": {
        "n": {
          "kind": "value",
          "value": 4,
          "valueType": "integer"
        }
      },
      "involvedBlocks": null
    }
  ],
  "query": null,
  "queryType": "read",
  "warning": null
}
```
