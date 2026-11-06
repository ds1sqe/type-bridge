# Immutable generated queries

Each generated package exports a query facade bound to that package's verified
projection. It provides the same supported application outcomes in Python,
TypeScript/Node, and Rust without routing generated models through handwritten
model descriptors.

See [Debugging plans](debugging.md) to inspect built plans and follow native
execution and hydration through the host logger.

## Start a session

```python
from app_models import Age, Employment, Person

session = Person.query(db)
person = session.exact(Person)
employment = session.exact(Employment)
```

`Person.query(db)` and the package's `QuerySession(db)` constructor produce the
same package-scoped session. The connection decides where terminals run: a
direct `Database` or `TransactionContext`, or a proxy connection (see
[Proxy connections](#proxy-connections)). Composition is synchronous; every
terminal is a coroutine on every connection, so Python code awaits it.
TypeScript is the same: `new QuerySession(connection)` takes a `Database`,
a transaction, or a proxy connection, and every terminal returns a `Promise`.

Use `exact(Model)` for exactly one schema type and `subtypes(Model)` for a
polymorphic binding. Concrete result evidence determines the generated class
used for hydration.

## Match fields and roles

```python
adult = person.field(Person.age).gte(Age(18))
employee = employment.role(Employment.employee).connects(person)

query = session.query(person, employment).where(adult, employee)
```

Bindings, field tokens, role tokens, predicates, selections, and queries are
immutable and carry one projection identity. Cross-package composition fails
before execution.

## Native plans

For explicit read/write programs, use `PlanBuilder` and `Plan` from
`type_bridge.plan` or `@type-bridge/node`. Build against the generated
package's schema authority, then stage the plan in a direct or proxy
transaction. Rust uses the same native V3 builder and validation.

See the [TypeScript staged-plan examples](typescript.md) and
[3.0 upgrade guide](upgrade-3.0.md#native-plan-execution-replaces-v1v2).
The former V1/V2 builders and separate invocation objects have been removed.

## Connectivity and explicit cross joins

Every selected binding must be connected by a predicate, match witness, or
bounded reachability path. An intentional Cartesian product must be declared:

```python
left = session.exact(Person)
right = session.exact(Person)

rows = await (
    session.query(left, right)
    .allow_cross_join(left, right)
    .rows(limit=10)
)
```

This prevents a missing join predicate from silently becoming an expensive
cross product.

## Bounded reachability

```python
source = session.exact(Person)
target = session.exact(Person)

path = session.reachable(
    source,
    target,
    NetworkLink,
    NetworkLink.origin,
    NetworkLink.destination,
    min_depth=1,
    max_depth=3,
)

connected = await session.query(source, target).where(path).rows(limit=100)
```

The relation and both role tokens are generated evidence. Depth bounds are
mandatory and validated before execution.

## Selection shapes

One binding produces one model; multiple bindings produce a tuple:

```python
one_person = await session.query(person).one()
pairs = await session.query(person, employment).where(employee).rows(limit=100)
```

Generated Python and TypeScript support positional selections through 16 slots.
Named selections map into an immutable declared row shape. Collection preserves
one root with zero or more related values:

```python
from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class EmploymentRow:
    person: Person
    employments: tuple[Employment, ...]


page = await (
    session.query_as(
        EmploymentRow,
        person=person,
        employments=employment.collect().distinct(),
    )
    .where(employee)
    .page_by(person, limit=25, include_total=True)
)
```

Collections may have their own generated-field ordering. `page_by` keeps root
pagination distinct from collection membership.

## Terminals

```python
await query.one()
await query.first(order_by=(person.field(Person.person_id).asc(),))
await query.rows(limit=100, offset=0)
await query.page_by(person, limit=25, include_total=True)
await query.count_by(person)
await query.exists_by(person)
```

`one` requires exactly one result. `first` returns `None` when empty. Materialized
rows and pages are bounded; unbounded implicit fetches are not part of the
generated contract.

## Direct reductions and grouping

```python
from app_models import aggregate

score = person.field(Person.score)
count, total, mean = await session.query(person).aggregate(
    person,
    aggregate.count(),
    aggregate.sum(score),
    aggregate.mean(score),
)

grouped = await (
    session.query(person, employment)
    .where(employee)
    .group_by(person, employment)
    .aggregate(aggregate.count(), aggregate.max(score))
)
```

Reducer inputs are owner/scalar checked. Grouping returns exact generated group
values plus typed reducer tuples.

## Proxy connections

A proxy connection runs the same session against a TypeBridge server instead of
TypeDB. Only the connection object changes; the query program is the same:

```python
from app_models import ExchangeConnection, Person, QueryExecutionResourceLimits, connect_proxy

# The built-in HTTP route to the server's /v3/capabilities and /v3/staged.
session = Person.query(connect_proxy("https://typebridge.example"))

# Or a host-owned transport that routes and authenticates itself.
session = Person.query(
    ExchangeConnection(advertisement_bytes, exchange),
    resources=QueryExecutionResourceLimits(items=100, timeout_milliseconds=30_000),
)
person = session.exact(Person)
rows = await session.query(person).rows(limit=50)
```

Both connections reconstruct Query V2 authority from private, verified package
evidence; no authority file or low-level `PlanAuthority` argument is
involved. `connect_proxy` fetches the advertisement over the configured TLS
connection, or takes a pinned `advertisement=`; `headers=` carries caller
authentication. `ExchangeConnection(advertisement, exchange)` takes
advertisement bytes and a coroutine that sends one request and returns its
reply; the caller owns transport, credentials, and retry policy. Either takes
`answer_limits=`, the ceiling every session on the connection runs under;
without it a proxy connection uses the proxy defaults (65,536 items,
32 MiB), lower than a direct connection's.

TypeScript passes the same connections to the same `QuerySession`:

```ts
import {
  ExchangeConnection,
  Person,
  QueryExecutionResourceLimits,
  QuerySession,
  connectProxy,
} from "app-models";

const session = new QuerySession(await connectProxy("https://typebridge.example"));

const hosted = new QuerySession(new ExchangeConnection(advertisementBytes, exchange), {
  resources: new QueryExecutionResourceLimits({ items: 100n, timeoutMilliseconds: 30_000n }),
});
const person = hosted.exact(Person);
const rows = await hosted.query(person).rows({ limit: 50n });
```

`connectProxy` takes `advertisement`, `headers`, `tls`, and `answerLimits`
options; `ExchangeConnection(advertisement, exchange, { answerLimits })` takes
a function that sends one request and resolves to its reply. Composition
remains local; one terminal performs one exchange and materializes through
the same package projection as direct execution.

Rust opens either connection from one policy with `SCHEMA.connect(policy)`
([Direct or proxy](rust.md#direct-or-proxy)); C runs each terminal on a proxy
connection with the same arguments as on a database
([Run a query on a database or a TypeBridge server](c.md#run-a-query-on-a-database-or-a-typebridge-server)).

Scripts without an event loop wrap a session in `type_bridge_core.blocking`,
which runs each terminal to completion on a private event-loop thread:

```python
from type_bridge_core import blocking

session = blocking(Person.query(db))
person = session.exact(Person)
rows = session.query(person).rows(limit=50)
```

Supported remote terminals are `one`, `first`, bounded `rows`, `page_by`,
`count_by`, `exists_by`, and typed `aggregate`/`group_by`, including exact/subtype
hydration, predicates, roles, reachability, explicit cross joins, selected
shapes, all reducers, and binding/field/tuple-field grouping. Reduction replies
echo the exact root, grouping, and reducer contract and are authenticated before
materialization through the same projection as direct execution.

### Deletion snapshots

Use `delete_returning` when you need the stored values of the things being
deleted. Plain `delete` keeps its existing return value and execution path.
The returning operation captures the matching things, then deletes them in the
same transaction. A failure does not return a successful deletion snapshot.
Deleting a relation captures its players without deleting those players.

#### 1. Code

These exact-type manager calls return a fresh model hydrated from the database,
rather than the object passed by the application:

```python
people = Person.manager(db)
deleted = people.delete_returning(person)         # WriteResult[Person]
snapshots = people.delete_many_returning(others)  # BatchWriteResult[Person]
if deleted.item is not None:
    assert deleted.item.deleted
```

```typescript
const people = Person.manager(database);
const deleted = people.deleteReturning(iid);         // WriteResult<Person>
const snapshots = people.deleteManyReturning(iids); // BatchWriteResult<Person>
```

```rust
let people = database.entities::<Person>();
let deleted: type_bridge::WriteResult<Person> = people.delete_returning(iid).await?;
let snapshots: type_bridge::BatchWriteResult<Person> =
    people.delete_many_returning(&iids).await?;
assert!(deleted.item().is_none_or(Person::deleted));
```

Batch snapshots preserve input order and omit missing things; their parallel
input indexes retain each snapshot's original row. A missing single target
returns `count: 0`, empty `items`, and absent `item`. A returned deleted root
cannot be submitted for another mutation: it is refused with `thing_deleted`.
A surviving relation player remains usable. Python exposes `deleted`; generated
Rust models and their subtype-family enums expose `deleted()`.

A manager bound to an existing transaction returns its snapshot without
committing that transaction. The caller must still commit; receiving the
snapshot does not prove that the deletion was committed. Staged-plan answers
become available only after the staged transaction completes successfully.

#### 2. Plan

The core snapshot construct is a fetch immediately before the terminal delete.
This abbreviated plan shows the operation and answer shape; bindings, authority,
inputs and the exact match are omitted:

```json
{
  "pipeline": [
    {"kind": "match", "patterns": ["exact type and requested identity"]},
    {"kind": "fetch", "thing": 0, "roles": []},
    {"kind": "delete", "items": [{"kind": "thing", "binding": 0}]}
  ],
  "operation": {"kind": "write", "returns": "rows"},
  "output": {"kind": "rows", "columns": []}
}
```

Such a plan requires `query.write.delete-snapshot`. A relation capture includes
its declared roles; hydration combines player evidence into one snapshot per
root. A subtype-inclusive plan retains each root's concrete `_type`. Manager
calls above select their exact type; they do not implicitly delete subtypes.

#### 3. TypeQL

The batch entity path transports the ordinal and IID as given rows. Its capture
statement has this shape, followed by deletion on the same transaction:

```typeql
given $ordinal: integer, $target-iid: string;
match
$thing isa! person;
let $actual-iid = iid($thing);
$actual-iid == $target-iid;
$thing isa! $thing-type;
fetch {
  "ordinal": $ordinal,
  "_iid": iid($thing),
  "_type": label($thing-type),
  "attributes": { $thing.* }
};
```

```typeql
given $ordinal: integer, $target-iid: string;
match
try {
  $thing isa! person;
  let $actual-iid = iid($thing);
  $actual-iid == $target-iid;
};
delete try { $thing; };
select $ordinal;
fetch { "ordinal": $ordinal };
```

#### 4. Result and cost

The capture's `_type` selects the generated model; `_iid` supplies its identity,
and `attributes` supplies its stored fields. The returned Python `Person` or
Rust `Person` is marked deleted. For example, a stored age of 18 remains 18 in
the snapshot even if the caller changed its local object's age to 99 without
saving it. For a subtype-inclusive core plan, `_type: "report"` and
`_type: "memo"` hydrate the corresponding concrete document types.

Capturing data adds work that plain deletion does not perform. On TypeDB
3.14.0, deleting 5,000 entities through the same subtype-inclusive filter gave
these medians:

| Execution route | Plain delete | Snapshot delete | Overhead |
| --- | ---: | ---: | ---: |
| Direct staged transaction | 0.450 s | 1.366 s | +203% |
| In-process staged proxy router | 0.515 s | 3.366 s | +553% |

Each median uses three measured runs after one warm-up, alternating plain and
snapshot deletion. Timing includes commit and answer materialization; seeding
and absence checks are excluded. The proxy measurement includes request/reply
encoding, signing and verification through `/v3/staged`, but excludes external
TCP/TLS transport. These are local measurements, not a latency guarantee.
Snapshot overhead is measured and documented; it is not subject to a +10% gate.

### Generated plan authoring

Generated models author plans inside `with Plan()` (Python), `Plan.build`
(TypeScript and Rust), or a generated package recording (C). Handles belong
to that recording. Comparisons and assignments forward to the shared Rust
builder. Caller literals become typed `given` columns; see the
[implicit input naming rule](../reference/query-plan.md#implicit-input-names).
Building and inspecting a plan performs no database I/O. Staging defers its
execution until commit. A result can be read only after commit succeeds.

#### Purchase: code

The following examples select inserted `Purchase` models. The
[conformance purchase program](https://github.com/ds1sqe/type-bridge/tree/master/examples/purchase)
keeps the outer recording's `Matched` marker instead, so its existing journey
continues to measure affected rows.

=== "Python"

    ```python
    from type_bridge.plan import Plan
    from purchase_models import Account, Item, Purchase

    with Plan():
        item = Item.match(item_id="I1").where(Item.stock > 0)  # ItemHandle
        buyer = Account.match(account_id="A1")  # AccountHandle
        buyer.where(buyer.balance >= item.price)
        item.set(stock=item.stock - 1)
        buyer.set(balance=buyer.balance - item.price)
        order = Purchase.insert(order_id="O1", buyer=buyer, item=item)  # Plan[list[Purchase]]
    with db.staged_transaction() as tx:
        pending = tx.stage(order)  # Pending[list[Purchase]]
    orders = pending.result()  # list[Purchase]
    assert orders[0].order_id.value == "O1"
    ```

=== "TypeScript"

    ```typescript
    const order = Plan.build(() => { // Plan<Purchase[]>
      const item = Item.match({ itemId: "I1" }).where(Item.stock.gt(0n)); // ItemHandle
      const buyer = Account.match({ accountId: "A1" }); // AccountHandle
      buyer.where(buyer.balance.ge(item.price));
      item.set({ stock: item.stock.sub(1n) });
      buyer.set({ balance: buyer.balance.sub(item.price) });
      return Purchase.insert({ orderId: "O1", buyer, item });
    });
    const pending = tx.stage(order); // Pending<Purchase[]>
    await tx.commit();
    const orders = pending.result(); // Purchase[]; orders[0].orderId.value === "O1"
    ```

=== "Rust"

    ```rust
    let order: Plan<Vec<Purchase>> = Plan::build(|p| {
        let item = Item::matching_by(p).item_id("I1").handle()
            .filter(ItemType::stock.gt(0_i64)); // ItemHandle
        let buyer = Account::matching_by(p).account_id("A1").handle(); // AccountHandle
        let balance = buyer.balance(); // Value<i64>
        let buyer = buyer.filter(balance.ge(item.price())); // AccountHandle
        item.set(p).stock(item.stock() - 1_i64);
        buyer.set(p).balance(buyer.balance() - item.price());
        Purchase::insert(p, PurchaseInsert {
            order_id: Argument::from("O1").into(), buyer: Some(&buyer), item: Some(&item),
        })
    })?;
    let pending = tx.stage(&order)?; // Pending<Vec<Purchase>>
    tx.commit().await?;
    let orders = pending.result()?; // Vec<Purchase>; orders[0].order_id.value == "O1"
    ```

=== "C"

    ```c
    tb_writes_item_handle_t item;
    tb_writes_account_handle_t buyer;
    tb_writes_item_match_t item_match = {0};
    tb_writes_account_match_t account_match = {0};
    tb_writes_item_set_t stock = {0};
    tb_writes_account_set_t balance = {0};
    tb_writes_purchase_insert_t values = {0};
    tb_writes_purchase_plan_t order;
    item_match.itemzhid = tb_string(view("I1"));
    CHECK(tb_writes_item_match_by(p, &item_match, &item));
    CHECK(tb_writes_item_where(p, item, tb_long_gt(tb_writes_item_plan_stock(p, item), tb_long(0))));
    account_match.accountzhid = tb_string(view("A1"));
    CHECK(tb_writes_account_match_by(p, &account_match, &buyer));
    CHECK(tb_writes_account_where(p, buyer, tb_long_ge(tb_writes_account_plan_balance(p, buyer), tb_writes_item_plan_price(p, item))));
    stock.stock = tb_long_sub(tb_writes_item_plan_stock(p, item), tb_long(1));
    CHECK(tb_writes_item_set(p, item, &stock));
    balance.balance = tb_long_sub(tb_writes_account_plan_balance(p, buyer), tb_writes_item_plan_price(p, item));
    CHECK(tb_writes_account_set(p, buyer, &balance));
    values.orderzhid = "O1"; values.has_orderzhid = true;
    values.buyer = buyer; values.item = item;
    CHECK(tb_writes_purchase_insert(p, &values, &order));
    ```

    `p` is opened by `tb_writes_plan_open`; `view` and `CHECK` are the
    byte-view/status helpers in the executable C example. Stage `order` with
    `tb_writes_purchase_stage`, commit, then read a
    `tb_writes_purchase_plan_view_t` through its result owner. Its `orderzhid`
    member is the borrowed string for `order-id`.

#### Purchase: plan

The abbreviated plan has one match, two updates, and one insert, followed by
an explicit model fetch. It binds the item identifier, minimum stock, account
identifier, decrement, and order identifier as five typed input columns.
The result selection records the inserted purchase binding and its role
projection. All four bindings use the same Core fetch stack and validate the
entire answer before the transaction commits.

```json
{"operation":{"kind":"write","returns":"rows"},"pipeline":[{"kind":"match"},{"kind":"update"},{"kind":"insert"},{"kind":"fetch","thing":9}]}
```

#### Purchase: TypeQL

`order.explain()` exposes the executed statements and their input declarations.
The match checks stock and balance; the updates subtract one item and its price;
the insert links the buyer and item. Input rows carry `I1`, `A1`, `0`, `1`, and
`O1`; these values do not appear in the TypeQL. See
[Debugging plans](debugging.md) for printing the exact text in each language.



```typeql
given $item_item_id_2: string, $item_stock_2: integer, $account_account_id_2: string, $item_stock_3: integer, $purchase_order_id: string;
match $item isa! item;
$item has item-id $item_item_id;
$item_item_id isa! item-id;
$item_item_id == $item_item_id_2;
$item has stock $item_stock;
$item_stock isa! stock;
$item_stock > $item_stock_2;
$account isa! account;
$account has account-id $account_account_id;
$account_account_id isa! account-id;
$account_account_id == $account_account_id_2;
$account has balance $account_balance;
$account_balance isa! balance;
$item has price $item_price;
$item_price isa! price;
$account_balance >= $item_price;
let $item_stock_4 = ($item_stock - $item_stock_3);
let $account_balance_2 = ($account_balance - $item_price);
update $item has stock == $item_stock_4;
$account has balance == $account_balance_2;
insert $purchase isa purchase, links (buyer: $account, item: $item), has order-id == $purchase_order_id;
match $purchase isa! $PlanFetchType;
try { $purchase links (buyer: $PlanFetchRole0);

 $PlanFetchRole0 isa! $PlanFetchRole0Type;

};
try { $purchase links (item: $PlanFetchRole1);

 $PlanFetchRole1 isa! $PlanFetchRole1Type;

};
fetch { "_iid": iid($purchase), "_type": label($PlanFetchType), "attributes": {  $purchase.*}, "_role_0_iid": iid($PlanFetchRole0), "_role_0_type": label($PlanFetchRole0Type), "_role_0_attributes": {  $PlanFetchRole0.*}, "_role_1_iid": iid($PlanFetchRole1), "_role_1_type": label($PlanFetchRole1Type), "_role_1_attributes": {  $PlanFetchRole1.*}};
```

#### Purchase: result

A successful model selection returns generated `Purchase` models in Python,
TypeScript, and Rust, with their existing attribute wrappers. C returns an
owner with borrowed typed views. A failed stock/balance match returns an empty
model sequence. A staged result retains the Core proof and its origin; it does
not perform a second query while it is read.

#### Delete snapshot: code

=== "Python"

    ```python
    with Plan():
        removed = Item.match(item_id="I1").delete_returning()  # Plan[list[Item]]
    with db.staged_transaction() as tx:
        pending = tx.stage(removed)  # Pending[list[Item]]
    snapshots = pending.result()  # list[Item], marked deleted
    ```

=== "TypeScript"

    ```typescript
    const removed = Plan.build(() => Item.match({ itemId: "I1" }).deleteReturning()); // Plan<Item[]>
    const pending = tx.stage(removed); // Pending<Item[]>
    await tx.commit();
    const snapshots = pending.result(); // Item[], marked deleted
    ```

=== "Rust"

    ```rust
    let removed: Plan<Vec<Item>> = Plan::build(|p| {
        Item::matching_by(p).item_id("I1").handle().delete_returning(p)
    })?;
    let pending = tx.stage(&removed)?; // Pending<Vec<Item>>
    tx.commit().await?;
    let snapshots = pending.result()?; // Vec<Item>, marked deleted
    ```

=== "C"

    ```c
    shop_item_plan_t removed; /* typed model selection */
    CHECK(shop_item_delete_returning(p, item, &removed));
    /* Stage removed, commit, then read shop_item_plan_view_t with deleted=true. */
    ```

#### Delete snapshot: plan

```json
{"operation":{"kind":"write","returns":"rows"},"pipeline":[{"kind":"match"},{"kind":"fetch","thing":0},{"kind":"delete","items":[{"binding":0,"kind":"thing"}]}]}
```

This abbreviated view omits authority and binding tables. Core captures the
pre-delete model before executing the deletion.

#### Delete snapshot: TypeQL


```typeql
given $item_item_id_2: string;
match $item isa! item;
$item has item-id $item_item_id;
$item_item_id isa! item-id;
$item_item_id == $item_item_id_2;
match $item isa! $PlanFetchType;
fetch { "_iid": iid($item), "_type": label($PlanFetchType), "attributes": {  $item.*}};

given $item_item_id_2: string;
match $item isa! item;
$item has item-id $item_item_id;
$item_item_id isa! item-id;
$item_item_id == $item_item_id_2;
delete $item;
```

#### Delete snapshot: result

The result is a sequence of generated `Item` snapshots, including the identifier,
stock, and price as they stood before deletion. C exposes the same snapshot as a
borrowed `shop_item_plan_view_t`. The deletion marker blocks later mutation
through these snapshots. The owner remains readable after the recording and
transaction are released.

#### Bulk insert: code

Use the generated schema's `Values` type. Required fields are constructor
arguments; optional fields may be omitted.

=== "Python"

    ```python
    with Plan():
        inserted = Item.insert_many([Item.Values(item_id="I2", price=3)])  # Plan[list[Item]]
    with db.staged_transaction() as tx:
        pending = tx.stage(inserted)  # Pending[list[Item]]
    items = pending.result()  # list[Item]
    ```

=== "TypeScript"

    ```typescript
    const inserted = Plan.build(() => Item.insertMany([{ itemId: "I2", price: 3n }])); // Plan<Item[]>
    const pending = tx.stage(inserted); // Pending<Item[]>
    await tx.commit();
    const items = pending.result(); // Item[]
    ```

=== "Rust"

    ```rust
    let inserted: Plan<Vec<Item>> = Plan::build(|p| {
        Item::insert_many(p, [ItemValues { item_id: Some("I2".into()), price: Some(3), stock: None }])
    })?;
    let pending = tx.stage(&inserted)?; // Pending<Vec<Item>>
    tx.commit().await?;
    let items = pending.result()?; // Vec<Item>
    ```

=== "C"

    ```c
    tb_writes_item_values_t rows[] = {{.itemzhid = "I2", .has_itemzhid = true,
                                    .price = 3, .has_price = true}};
    tb_writes_item_plan_t inserted;
    CHECK(tb_writes_item_insert_many(p, rows, 1u, &inserted));
    /* After commit, tb_writes_item_result_at copies a view borrowing its result owner. */
    ```

#### Bulk insert: plan

```json
{"given_rows":[[{"kind":"string","value":"I2"},{"kind":"long","value":3}]],"operation":{"kind":"write","returns":"rows"},"pipeline":[{"kind":"insert"},{"kind":"fetch","thing":0}]}
```

This abbreviated view omits authority, ownership, and binding tables. One insert
program consumes a typed given table. Core groups optional field presence,
splits within captured limits, and correlates documents with authored rows
before commit.

#### Bulk insert: TypeQL


```typeql
given $item_item_id: string, $item_price: integer;
insert $item isa item, has item-id == $item_item_id, has price == $item_price;
match $item isa! $PlanFetchType;
fetch { "_iid": iid($item), "_type": label($PlanFetchType), "attributes": {  $item.*}};
```

#### Bulk insert: result

The result is an ordered sequence of generated `Item` models, or borrowed C
views. An empty input is an empty model result and performs no provider I/O.

#### Reuse with `Plan.template()`

Declare explicit columns when the same recording is reused. Binding creates a
fresh immutable invocation; it does not change the original template.

```python
with Plan.template() as template:
    key = template.input("key", "string")  # PlanInput[str]
    quantity = template.input("quantity", "long")  # PlanInput[int]
    adjusted = Item.match(item_id=key).set(stock=quantity)  # Plan[list[Item]]
invocation = adjusted.bind([[key.value("I2"), quantity.value(4)]])  # Plan[list[Item]]
```

TypeScript exposes `Plan.template()` with `template.input`, `template.build`,
and `.bind`. Rust retains explicit input/row authoring through `Plan::builder()`.
C recordings capture
immutable options when opened and retain their compiled selections for repeated
staging. A frozen C recording rejects later mutations and keeps its published
results readable; see [C plan lifetimes](c.md).

#### Escape hatch: explicit Core-builder calls

`Plan.builder()` remains available when a program needs explicit bindings,
input columns, row tables, or lower-level tokens. Its calls forward to the same
Rust builder. An explicit program with the same input names, order, and binding
schedule has identical canonical bytes to the generated authoring form.

#### Explicit Python builder

A plan is one typed `query-plan/v3` pipeline: a match with comparisons and
`let_` values, then updates, inserts, puts and deletes. `Plan.builder()`
(`type_bridge.plan`) takes the generated model, field and role tokens and
forwards every call to the shared Rust builder, so the bytes equal the same
plan built in Rust, TypeScript or C. `where_` and `let_` carry a trailing
underscore because `where` and `let` read as keywords; comparisons are
`eq`, `ne`, `lt`, `le`, `gt` and `ge`, and `+ - * / %` and
`PlanExpr.{pow,abs,ceil,floor,round,min,max,length}` build expressions.
`bool`, `int`, `float`, `str`, `Decimal`, `date` and `datetime` literals are
typed automatically; `literal("duration", "P1D")` types the rest.

```python
from type_bridge.plan import Matched, Plan

from generated.python import Account, Item, Purchase, schema_authority


def purchase(order: str, item_id: str, account_id: str) -> Plan:
    plan = Plan.builder()
    item = plan.exact(Item)
    buyer = plan.exact(Account)
    order_input = plan.input("order_id", "string")
    item_input = plan.input("item_id", "string")
    account_input = plan.input("account_id", "string")
    stock = plan.field(item, Item.stock)
    price = plan.field(item, Item.price)
    balance = plan.field(buyer, Account.balance)
    plan.where_(plan.field(item, Item.item_id).eq(item_input))
    plan.where_(plan.field(buyer, Account.account_id).eq(account_input))
    plan.where_(stock.gt(0)).where_(balance.ge(price))
    plan.update(item, Item.stock, plan.let_("next_stock", stock - 1))
    plan.update(buyer, Account.balance, plan.let_("next_balance", balance - price))
    (
        plan.insert(Purchase)
        .field(Purchase.order_id, order_input)
        .player(Purchase.buyer, buyer)
        .player(Purchase.item, item)
    )
    plan.row([order_input.value(order), item_input.value(item_id), account_input.value(account_id)])
    return plan.build(schema_authority())


with db.staged_transaction() as tx:  # db: type_bridge.Database
    bought = tx.stage(purchase("O1", "I1", "A1"))
assert bought.result() in (Matched(1), Matched(0))
```

Calls never fail on content; `build` raises the first invalid call as a
`PlanError` with `.code` (for example `query_plan_builder_duplicate_name`).
A handle from another builder raises `TypeError` at once. A generated
package's `schema_authority()` is its embedded, verified authority.

`Database.staged_transaction(kind=None, *, commit_on_exit=True)` (and
`Database.staged_transaction`) returns the core staged transaction:
`stage(plan)`, `commit()`, `rollback()` and `close()`. Nothing opens before `commit()`; an omitted kind
is inferred from the staged plans. `Pending.result()` answers `Rows(rows)`,
`Documents(documents)`, `Count(count)`, `Exists(exists)` or `Matched(count)`, raises the commit's
typed failure (`.code`, `.failure`), or raises `RuntimeError` before commit.
A `with` block commits on a clean exit unless `commit_on_exit=False`,
discards on an exception, and always closes.
`Database.transaction()` keeps its released eager context.

### Reading a thing back

`fetch(thing, roles)` is the document terminal in every plan builder. It
returns the thing's IID, exact type and all stored attributes, plus the
players of its listed roles. Bare Python/TypeScript roles are required;
`(role, False)` / `[role, false]` permits an absent player. Rust uses
`FetchRole::new(role, required)` and C uses `type_bridge_fetch_role_t`.
A read answers `Documents`; a write with an explicit fetch answers the same
documents after its writes. Writes without a terminal still answer `Matched`.

#### 1. Code

These are the executable purchase conformance programs' read-back builders.
They select purchase R1 after the write that inserted it and both its players.

```python
--8<-- "examples/purchase/python/program.py:read_back"
```

```typescript
--8<-- "examples/purchase/ts/program.ts:read_back"
```

```rust
--8<-- "examples/purchase/rust/main.rs:read_back"
```

```c
--8<-- "examples/purchase/c/program.c:read_back"
```

Stage the built plan on a read transaction and commit it. Python's
`Pending.result()` returns `Documents(documents)`; TypeScript's `get()`
returns `{ kind: "documents", documents }`; Rust's `get()` returns
`PlanAnswer::Documents`. C reads the same answer through the
[borrowed document cursors](c.md#reading-answers).

#### 2. Plan

Every builder produces the same pipeline fragment, with `operation: rows`
and empty output columns. A write uses `write` returning `rows` instead.

```json
{"kind":"fetch","thing":0,"roles":[
  {"role":{"declaring_relation":"purchase","label":"buyer"},"required":true},
  {"role":{"declaring_relation":"purchase","label":"item"},"required":true}
]}
```

Fetch is allowed once. It cannot be combined with `returning`, `count` or
`exists` in either order. Fetch before `delete(thing)` returns the deletion
snapshot; the deleted binding stays dead. `returning` keeps its explicit row
projection and does not add a fetch.

#### 3. TypeQL

The shared fetch tail extends the matched purchase as follows. The leading
match and its parameter values select R1 in the same provider transaction.

```typeql
match
  $purchase isa! $PlanFetchType;
  $purchase links (buyer: $PlanFetchRole0);
  $PlanFetchRole0 isa! $PlanFetchRole0Type;
  $purchase links (item: $PlanFetchRole1);
  $PlanFetchRole1 isa! $PlanFetchRole1Type;
fetch {
  "_iid": iid($purchase), "_type": label($PlanFetchType),
  "attributes": { $purchase.* },
  "_role_0_iid": iid($PlanFetchRole0),
  "_role_0_type": label($PlanFetchRole0Type),
  "_role_0_attributes": { $PlanFetchRole0.* },
  "_role_1_iid": iid($PlanFetchRole1),
  "_role_1_type": label($PlanFetchRole1Type),
  "_role_1_attributes": { $PlanFetchRole1.* }
};
```

#### 4. Result

The conformance program checks one document (`["documents", 1]`) and derives
this summary from its attributes and role-player attributes in all four SDKs:

```json
{"type":"purchase","order_id":"R1","buyer":"A2","item":"I2"}
```

Documents retain `_iid`, `_type`, `attributes` and the `_role_N_*` triples.
An absent optional player omits its triple. These are schema-validated
documents; generated model hydration belongs to the typed facade.

### Replacing `update_with`

`update_with(func)` reads every match, runs `func` in Python and writes each
instance back. It was removed in 3.0. Compute the new value in the
plan with `let_` instead, so the read and the write are one query:

```python
from type_bridge.plan import Plan

from generated.python import Account, schema_authority

# Before: accounts.filter(tier=Tier("gold")).update_with(add_bonus)
plan = Plan.builder()
account = plan.exact(Account)
tier = plan.input("tier", "string")
balance = plan.field(account, Account.balance)
plan.where_(plan.field(account, Account.tier).eq(tier))
plan.update(account, Account.balance, plan.let_("next_balance", balance + 10))
plan.row([tier.value("gold")])

with db.staged_transaction() as tx:
    updated = tx.stage(plan.build(schema_authority()))
```

### Staged transactions over the proxy

Writes cross the proxy as staged transactions. `transaction(kind)` on a proxy
connection (Rust `ProxyDatabase::transaction(options)`, Python and TypeScript
`ProxyConnection.transaction(kind)`) returns the core staged transaction: stage
plans (Rust and TypeScript `stage` and Python `stage` of a built `Plan`;
canonical `query-plan/v3` bytes also through Python `stage` and TypeScript
`stagePlan`), then `commit()`. The commit sends the whole stack as one
`typebridge.query-remote-request/v3` envelope to `POST /v3/staged`; the server
runs it on one TypeDB transaction and returns a signed reply that the SDK
verifies before resolving any answer. Answers and typed failures are the same
as direct execution, and `commit_outcome_unknown` stays an unknown outcome.

Each commit is one request under a fresh nonce, and the proxy keeps no
replies. A dropped or unverifiable reply, and a cancelled commit, leave the
outcome unknown (`commit_outcome_unknown`): the writes may have applied, and
the caller reads the database to find out, as after a lost direct commit.
Every other signed refusal keeps the server's code and applied nothing.
Typed reads compile to plans in the staged stack. Raw TypeQL is refused at
stage time over the proxy.

The SDK verifies staged replies only under the signer it already trusts: the
`/v3/capabilities` advertisement must name the same executor and reply key as
the connection's advertisement, or the transaction fails with
`query_remote_v3_advertisement_mismatch`. It is fetched once per connection,
or pinned out of band (Rust `ProxyConnectionPolicy::staged_advertisement`,
Python `staged_advertisement=`, TypeScript `stagedAdvertisement`).

The server admits a staged write only from a caller an interceptor vouches for
(`on_write_authority`), or from any caller when the operator sets
`[v2] allow_unauthenticated_staged_writes`; otherwise the commit fails with the
typed `staged_write_not_authorized` failure. The [staged remote envelope](../reference/query-plan.md#staged-remote-envelope-v3)
defines the wire format.

## Query limits

A query is held to limits on its size and shape: encoded bytes, nesting depth,
bindings, predicate nodes, and so on. They default to fixed ceilings. A
session can lower them for an untrusted workload or raise them for a query the
defaults refuse, such as a predicate with a literal over 16 MiB:

```python
from app_models import Person, QueryLimits

session = Person.query(
    db,
    query_limits=QueryLimits(canonical_bytes=64 * 1024 * 1024),
)
```

```ts
import { QueryLimits, QuerySession } from "app-models";

const session = new QuerySession(database, {
  queryLimits: new QueryLimits({ canonical_bytes: 64n * 1024n * 1024n }),
});
```

```rust
use type_bridge::QueryLimits;

let session = db
    .query()?
    .with_query_limits(QueryLimits::from_overrides([("canonical_bytes", 64 << 20)])?)?;
```

Proxy sessions take the same option, since a proxy connection is passed to
the same session. C callers use
`<prefix>_query_session_open_with_limits` with an array of named overrides.

Limits are named identically in every language. The engine validates them, so
an unknown name or an unsupported value fails when the limits are built.

Raising a limit locally is not enough for a remote query: the executor
advertises the limits it enforces and a request is held to the stricter of the
two. A refusal names the `limit`, the `maximum` it was held to, and a `source`
of `default`, `configured`, or `advertised`, so it is clear whether changing
the session's limits can help. The
[typed-query contract](../development/typed-query-contract.md#query-limits)
lists every limit with its default.

## Safety contract

- Schema and projection fingerprints are verified at package installation.
- Only exact registered generated classes and tokens are accepted.
- Queries are immutable and package-scoped.
- Disconnected selections fail unless cross joins are explicit.
- Result evidence is revalidated before hydration.
- Remote limits and capability advertisements fail closed.

The normative cross-language behavior is maintained in the
[unified typed-query contract](../development/typed-query-contract.md).


Rust staged transactions expose `tx.query()?`. Its session uses the same
`one()`, `rows()`, `first()`, `page_by()`, count, existence and aggregate
terminals as `db.query()?`; each returns `query::Pending<T>` instead of `T`.
Drop the session before `tx.commit().await?`, then call `pending.result()?`
to obtain the direct terminal's exact output type. Reads see earlier staged
writes and publish only after a successful commit, directly and over the
proxy. The pending owns the decoding evidence and survives its session.

Python's `Model.query(tx)` and `QuerySession(tx)` accept a staged transaction
from `Database.staged_transaction()` or `ProxyConnection.transaction()`.
Awaiting a terminal returns `Pending[T]`, where `T` is its usual result type.
Stage all reads, commit the transaction, then call `pending.result()`. The
pending keeps its decoding evidence even after the session is closed.

```python
tx = db.staged_transaction(commit_on_exit=False)
session = Person.query(tx)
person = session.exact(Person)
pending = await session.query(person).count_by(person)  # Pending[int]
session.close()
tx.commit()
count = pending.result()
tx.close()
```

TypeScript's `new QuerySession(tx)` infers `Promise<Pending<T>>` terminal
results for a staged transaction. Use `db.stagedTransaction()` directly or
`await proxy.transaction()` over the proxy. The session may close before
commit; committed answers retain their exact generated model and scalar types.

```typescript
const tx = db.stagedTransaction();
const session = new QuerySession(tx);
const person = session.exact(Person);
const pending = await session.query(person).countBy(person); // Pending<bigint>
session.close();
await tx.commit();
const count = pending.result();
tx.close();
```
