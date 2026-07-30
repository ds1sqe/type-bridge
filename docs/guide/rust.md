# Rust Client

TypeBridge 2.0 ships a public, generated-schema Rust application surface with
the same model, CRUD, immutable-query, transaction, and one-exchange remote
contracts as the Python and TypeScript bindings. Rust 1.88 or newer is
required.

## Distribution in 2.0

The 2.0 Rust SDK is distributed from the exact release source/Git revision,
not crates.io. The public `type-bridge` package still has workspace-internal
path dependencies, so publishing only that package would create an
unresolvable registry artifact. Generated application crates depend only on
`type-bridge`; they do not expose the internal engine crates.

Use the 40-character commit recorded in the `v2.0.0` GitHub release:

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
Commit `Cargo.lock` after resolving it.

## Generate the schema crate

Declare the Rust output in the
[Split-YAML workspace](split-yaml-v1.md):

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
declared-schema bytes, and the owner-branded `SCHEMA` package used at
connection time. Regenerate it whenever the canonical schema changes; do not
hand-edit it.

For the older single-TypeQL input path, the equivalent target is:

```bash
python -m type_bridge.generator schema.tql \
  --target rust \
  --output generated/rust
```

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
consume the transaction.

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

## Remote execution

Implement `RemoteQueryTransport` with the application's authenticated HTTP
client. `capabilities()` fetches `/v2/capabilities`; `exchange()` performs
exactly one `POST /v2/query` for each terminal. Then bind the same generated
schema:

```rust
use models::{AppSchema, Person, SCHEMA};
use type_bridge::{
    RemoteConnectionOptions, RemoteDatabase, RemoteQueryLimits,
};

let options = RemoteConnectionOptions::new(
    "application-scope",
    "typedb-3.12.1/v1",
    RemoteQueryLimits::new(100, 8 << 20, 1_000, 1_000, 1_000, 1_000)
        .deadline_ms(30_000),
    transport,
);
let remote: RemoteDatabase<AppSchema> =
    RemoteDatabase::connect(options).await?.with_schema(SCHEMA)?;

let mut session = remote.query()?;
let person = session.exact::<Person>()?;
let rows: Vec<Person> = session
    .query(person)?
    .rows(type_bridge::RowsOptions::new(50))
    .await?;
```

The transport owns authentication and authenticated TLS. The SDK binds
capability discovery, schema authority, nonce, request fingerprint, deadline,
limits, and reply identity before materializing the same generated model
types used by local execution.

## Classified errors

Handle failures through the stable category, code, and path accessors. Human
messages are diagnostic text and are not a machine-readable contract:

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
by the engine or remote diagnostic. Implementations of `RemoteQueryTransport`
should wrap application-owned transport failures with
`Error::remote("stable_snake_case_code", message, source)`.
