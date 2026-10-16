# Rust Client

TypeBridge 2.1 ships a public, generated-schema Rust application surface with
the same model, CRUD, immutable-query, transaction, and one-exchange remote
contracts as the Python and TypeScript bindings. Rust 1.88 or newer is
required.

## Distribution

Starting with 2.0.1, the Rust SDK is distributed from crates.io as a complete,
version-locked package graph. Generated application crates depend only on the
public `type-bridge` package; they do not expose the supporting engine crates.

```toml
[dependencies]
models = { package = "type-bridge-generated-schema", path = "generated/rust" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
type-bridge = "2"
```

Commit `Cargo.lock` after resolving the SDK and generated schema crate.

### Historical 2.0.0 source dependency

TypeBridge 2.0.0 was released before the complete package graph became
publishable. Use the 40-character commit recorded in its GitHub release:

```toml
[dependencies]
models = { package = "type-bridge-generated-schema", path = "generated/rust" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
type-bridge = { version = "=2.0.0" }

[patch.crates-io]
# Replace this value with the exact release commit. Do not use a branch.
type-bridge = { git = "https://github.com/ds1sqe/type-bridge", rev = "<40-character-v2.0.0-commit>" }
```

The patch satisfies both the application's dependency and the generated
crate's exact `type-bridge` requirement from one immutable source revision.

## Generate the schema crate

Declare the Rust output in the [Split-YAML workspace](split-yaml-v1.md):

```yaml
bindings:
  rust:
    output: generated/rust
```

Then validate and generate all configured projections:

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
```

The Rust output is an ordinary application-owned crate. It contains the model
and create types, type/field/role tokens, schema fingerprints, canonical
compiled authority, and the owner-branded `SCHEMA` package used at connection
time. Its managers and query sessions need no external JSON authority.
Regenerate the crate whenever the canonical Split-YAML schema changes; do not
hand-edit generated code.

## Apply the schema

Generation is offline and never changes TypeDB. Commit a migration and apply
it through an environment whose workspace policy explicitly permits
migration:

```bash
type-bridge --manifest typebridge.yaml migration make --name initial
type-bridge --manifest typebridge.yaml migration plan
type-bridge --manifest typebridge.yaml migration apply --environment development
```

Credentials remain environment references in `typebridge.yaml`; provide their
values through the named environment rather than committing them.

## Connect and bind the generated authority

```rust
use models::{AppSchema, SCHEMA};
use type_bridge::{ConnectionOptions, Database};

async fn connect() -> type_bridge::Result<Database<AppSchema>> {
    let options = ConnectionOptions::new("localhost:1729", "example-development")
        .http_port(8000)
        .credentials(
            std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into()),
            std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into()),
        );

    Database::connect(options).await?.with_schema(SCHEMA)
}
```

`with_schema` verifies and installs the generated runtime projection before it
changes `Database<Unbound>` into `Database<AppSchema>`. Models from another
generated package cannot be used with that handle. Enable verified TypeDB TLS
with `.tls(true)` and provision custom roots through the process's native
trust store.

## CRUD

Generated constructors require complete schema-valid input. For the reference
workspace's `person` type:

```rust
use models::{DisplayName, EmployeeId, Person, PersonCreate};

let create = PersonCreate::try_new(
    DisplayName::new("Ada Lovelace".to_owned())?,
    EmployeeId::new("EMP-001".to_owned())?,
)?;

// put is idempotent by the generated key and returns a complete model.
let ada = db.entities::<Person>().put(create).await?;
let iid = ada.iid().to_owned();

let same = db
    .entities::<Person>()
    .get_by_iid(&iid)
    .await?
    .expect("person exists");
assert_eq!(same.employee_id().value(), "EMP-001");

let all = db.entities::<Person>().all().await?;
let count = db.entities::<Person>().count().await?;
assert_eq!(all.len() as u64, count);

db.entities::<Person>().delete(&iid).await?;
```

`insert`, `insert_many`, `put_many`, and `update` are also available.
Generated `put` and `update` replace the complete non-key state: an omitted
optional or empty multivalue deletes its prior ownership. Relation managers
provide the corresponding keyed lifecycle, role replacement, repeated
players, and IID/key-fallback references.

For several operations that must commit together, borrow managers from one
write transaction:

```rust
let write = db.write().await?;
let person = write.entities::<Person>().put(create).await?;
// Other entity/relation operations borrow the same open context.
write.commit().await?;
```

Dropping an open write transaction never commits it; `commit` and `rollback`
consume the transaction. Use `rollback_keep(&mut self)` to discard writes and
continue using the same transaction before an explicit commit.

## Exact manager filters

Generated field tokens also build immutable single-model filters without
opening the broader typed-query grammar:

```rust
use type_bridge::ProjectedManagerComparison;

let adult = db.entities::<Person>().where_(
    PersonType::age,
    ProjectedManagerComparison::Gte,
    &Age::new(18)?,
)?;
let people: Vec<Person> = adult.all().await?;

let ada = db.entities::<Person>().where_(
    PersonType::person_id,
    ProjectedManagerComparison::Eq,
    &PersonId::new("ada")?,
)?.first().await?;
```

`where_` returns a sibling and accepts only the six canonical scalar
comparisons. `first` requires equality evidence for every effective
reference-key field and proves that at most one exact IID matched. Use an
explicit read transaction when several manager terminals and typed queries
must share one snapshot:

```rust
let read = db.read().await?;
let adults = read.entities::<Person>().where_(
    PersonType::age,
    ProjectedManagerComparison::Gte,
    &Age::new(18)?,
)?;
let count = adults.count().await?;
let exists = adults.exists().await?;
assert_eq!(exists, count != 0);
drop(adults);
read.close().await?;
```

Generated token types provide the package fence for ordinary Rust code, and
foreign-package tokens fail to compile against a local manager. The public
low-level `FieldToken` constructor remains available for compatibility; a
manually reconstructed token is therefore admitted only when its canonical
owner, ownership fact, attribute domain, and metadata are indistinguishable
from the installed projection.

## Immutable typed queries

```rust
use models::{EmployeeId, Person, PersonType};
use type_bridge::value::Text;
use type_bridge::RowsOptions;

let mut session = db.query()?;
let person = session.exact::<Person>()?;
let employee_id = person.field(PersonType::employee_id);

let query = session
    .query(person)?
    .where_(employee_id.starts_with(Text::new("EMP-")?))?;

let people: Vec<Person> = query
    .rows(RowsOptions::new(50).order_by(employee_id.asc()))
    .await?;
let exists = query.exists().await?;
let count = query.count().await?;
```

The same grammar supports exact and subtype bindings, attribute predicates,
role connections, bounded reachability, tuples, derived `SelectedRow`
shapes, collected children, pages, aggregates, and grouping. All bindings
carry the generated schema owner, so cross-schema comparisons fail at
compile time.

## Reuse one read transaction

Database-owned query terminals may open their own read context. To preserve
one snapshot across several terminals, borrow a session from an explicit read
transaction:

```rust
let read = db.read().await?;
let mut session = read.query();
let person = session.exact::<Person>()?;
let query = session.query(person)?;

let before = query.count().await?;
let any = query.exists().await?;
let after = query.count().await?;
assert_eq!(before, after);
assert_eq!(any, before != 0);

drop(query);
drop(session);
read.close().await?;
```

## Staged transactions and plans

A plan is one typed `query-plan/v3` pipeline: a match with comparisons and
`let` values, then updates, inserts, puts and deletes. A staged transaction
queues plans and runs them all at commit, in order, as one TypeDB
transaction. Nothing opens before `commit`, and a plan that matches no row
writes nothing, so one plan is a complete conditional write.

The one-query purchase checks the stock and the balance, takes the item and
the price, and records the purchase:

<!-- rust-plan-example:begin -->
```rust
use models::{Account, AccountType, AppSchema, Item, ItemType, Purchase, PurchaseType};
use type_bridge::plan::Plan;
use type_bridge::staged::{PlanAnswer, TransactionOptions};
use type_bridge::Database;

fn purchase(order: &str, item_id: &str, account_id: &str) -> type_bridge::Result<Plan<AppSchema>> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let buyer = plan.exact::<Account>();
    let order_input = plan.input::<String>("order_id");
    let item_input = plan.input::<String>("item_id");
    let account_input = plan.input::<String>("account_id");
    let item_key = plan.field(item, ItemType::item_id);
    let buyer_key = plan.field(buyer, AccountType::account_id);
    let stock = plan.field(item, ItemType::stock);
    let price = plan.field(item, ItemType::price);
    let balance = plan.field(buyer, AccountType::balance);
    plan.where_(item_key.eq(item_input))
        .where_(buyer_key.eq(account_input))
        .where_(stock.gt(0_i64))
        .where_(balance.ge(price));
    let next_stock = plan.let_("next_stock", stock - 1_i64);
    let next_balance = plan.let_("next_balance", balance - price);
    plan.update(item, ItemType::stock, next_stock)
        .update(buyer, AccountType::balance, next_balance);
    plan.insert::<Purchase>()
        .field(PurchaseType::order_id, order_input)
        .player(PurchaseType::buyer, buyer)
        .player(PurchaseType::item, item);
    plan.row([
        order_input.value(order.to_owned()),
        item_input.value(item_id.to_owned()),
        account_input.value(account_id.to_owned()),
    ]);
    plan.build(&models::SCHEMA)
}

async fn buy(db: &Database<AppSchema>) -> type_bridge::Result<bool> {
    let mut tx = db.transaction(TransactionOptions::default());
    let bought = tx.stage(&purchase("O1", "I1", "A1")?)?;
    tx.commit().await?;
    // `Matched(0)`: out of stock or short of funds; nothing was written.
    Ok(*bought.get()? == PlanAnswer::Matched(1))
}
```
<!-- rust-plan-example:end -->

The builder's handles (`item`, `stock`, `order_input`) come from generated
model and field tokens, so a field another model owns does not compile.
`build` validates the plan against the generated schema and returns the
first invalid call; nothing is checked against TypeDB until commit.

- **Reads.** A plan without writes answers rows of the columns passed to
  `returning`, or `count()` / `exists()`.
- **Writes.** A write plan answers `Matched(n)`, the number of rows its
  pipeline wrote, or with `returning(..)` the rows from the same pipeline.
- **Batches.** Each `row` binds the inputs once; many rows run the one plan
  over all of them.
- **Answers.** Each `stage` returns a `Pending` answer that resolves after a
  successful commit. A failed commit stores nothing and reports the failing
  plan's index and its typed failure; see
  [Classified errors](#classified-errors).
- **Kinds.** `TransactionOptions::default().with_kind(..)` makes the
  transaction `Read`, `Write` or `Schema`; without one, a staged write makes
  it `Write`. `Read` refuses write plans and `Schema` refuses every plan.
- **Reuse.** `rollback` discards the queued plans and keeps the transaction;
  `close` ends it. Dropping it never commits.

`ProxyDatabase::transaction` returns the same `Transaction`. Its commit is
one signed request with a fresh idempotency key; after an unknown outcome,
`commit_retry` resends the same request so the plans apply at most once.

Building is deterministic: handles are numbered in creation order, match
terms keep their call order, consecutive writes of one kind share a stage,
and names come from labels (`item`, `item_stock`). The same program always
has the same canonical plan bytes (`Plan::canonical_bytes`).

## Direct or proxy

A program that queries through a `Connection` runs unchanged against TypeDB
or a TypeBridge server; only the policy passed to `SCHEMA.connect` differs:

```rust
use models::{AppSchema, Person, SCHEMA};
use type_bridge::{
    Connection, ConnectionPolicy, DirectConnectionPolicy, ProxyConnectionPolicy,
    ProxyEndpoint, QueryExecutionResourceLimits, TlsPolicy,
};

fn policy(proxy: bool) -> ConnectionPolicy {
    if proxy {
        ProxyConnectionPolicy::new(
            ProxyEndpoint::new("https://typebridge.example.com")
                .tls(TlsPolicy::native_roots())
                .header("Authorization", "Bearer <token>"),
        )
        .into()
    } else {
        DirectConnectionPolicy::new("localhost:1729", "example", "admin", "password").into()
    }
}

let connection: Connection<AppSchema> = SCHEMA.connect(policy(use_proxy)).await?;
let mut session = connection.query_with_resources(
    QueryExecutionResourceLimits {
        items: 100,
        ..QueryExecutionResourceLimits::default()
    },
    type_bridge::AnswerCancellation::default(),
)?;
let person = session.exact::<Person>()?;
let rows: Vec<Person> = session
    .query(person)?
    .rows(type_bridge::RowsOptions::new(50))
    .await?;
```

Both policies take `answer_limits(..)`, the ceiling every session of the
connection is constrained by, and `connection_limits(..)`, the deadline for
connecting. A session's resources are the stricter of its own and the
ceiling, on either transport. With nothing configured, a session runs under
its transport's defaults: the 2.2 executor defaults direct, the wire limits
(65,536 items, 32 MiB) through a proxy.

A proxy connection fetches the server's signed `/v2/capabilities`
advertisement when it connects, or uses the exact bytes pinned with
`.advertisement(bytes)`. Fetch it only over TLS you trust, or pin it: the
advertisement carries the key every reply is verified with. Each terminal
then performs exactly one `POST /v2/query`. The SDK binds the scope and
semantic profile embedded in `SCHEMA`, the nonce, request fingerprint,
deadline, limits, and reply identity before it materializes the same model
types as a direct connection.

A host that routes or authenticates requests itself implements
`ProxyExchange` (`capabilities()` and `exchange()`) and passes it to
`ProxyConnectionPolicy::with_exchange`. `Connection::Direct` also gives the
`Database` with CRUD managers and transactions, which a proxy connection
does not offer.

## Classified errors

Handle failures through the stable category, code, path, and detail accessors.
Human messages are diagnostic text and are not a machine-readable contract:

```rust
use type_bridge::{Error, ErrorCategory, ModelValidationPhase};

fn inspect(error: &Error) {
    match error.category() {
        ErrorCategory::QueryAuthoring => {
            eprintln!("invalid query {} at {:?}", error.code().unwrap(), error.path());
        }
        ErrorCategory::ModelValidation => {
            assert!(matches!(
                error.model_validation_phase(),
                Some(ModelValidationPhase::Input | ModelValidationPhase::Hydration)
            ));
        }
        ErrorCategory::Capability | ErrorCategory::ResourceLimit => {
            eprintln!("executor rejected {}: {}", error.code().unwrap(), error.message());
        }
        _ => eprintln!("{error}"),
    }
}
```

Direct and remote query paths map canonical failures to the same
`ErrorCategory` and preserve stable codes and structured paths when supplied
by the engine or remote diagnostic. Authenticated remote failures also expose
their exact typed path through `diagnostic_path()` and deterministic detail map
through `details()`. Generated manager and projected-batch failures additionally
retain the binding-neutral category through `sdk_category()`; for example,
`wrong_scalar_domain` reports `Some("invalid_input")` even though the released
Rust compatibility category remains `ErrorCategory::ModelValidation`.
Implementations of `ProxyExchange` should wrap
application-owned transport failures with
`Error::remote("stable_snake_case_code", message, source)`.

Provider rejections expose `error.failure()` with the shared
`type_bridge::failure::Failure` vocabulary. A rejected commit carries a reason,
reported lock kind where applicable, and `retry_safe = true`; rerun the entire
transaction on a fresh snapshot. An indeterminate outcome requires
reconciliation before retry. Rejection diagnostics keep their released codes
and add a `rejection` detail naming the reason. Staged errors additionally
retain operation indexes and schema identities supplied by plan metadata.
