# Upgrading to 3.0

This guide covers the 3.0 breaking changes. Regenerate every language package
from its Split-YAML workspace, upgrade the matching runtime, and rebuild your
application. Generated projection resources and nominal types must come from
one package generation; mixing old resources with a new runtime is unsupported.

The C release example now uses `tb_sdk` / `TB_SDK`, replacing `tb_sdkv3` /
`TB_SDKV3`. Regenerate its consumers and update includes, symbols, CMake and
pkg-config package names together.

## Runtime and server migration

- General TypeDB admission starts at 3.12.0. Supported lines are 3.12–3.14;
  generated packages and migrations additionally require 3.12.3 on the
  `typedb-3.12.1/v1` profile or 3.13.6/3.14.0 on `typedb-3.13.6/v1`.
  TypeDB 3.11, band 8 and its semantic profile are removed.
- Rebuild 2.3.x C packages, including ABI 1.7 projections, with the current
  generator and ABI 2.0 runtime. Old headers, binary consumers and 1.x package
  descriptors are not a supported ABI 2.0 combination. Use the current exact
  CMake/pkg-config dependency and regenerate embedded projection resources.
- Server `/query`, `/query/raw`, `/query/validate`, `/schema`, `/typed/query`,
  `/v2/query` and `/v2/capabilities` return
  HTTP 404. There are no redirect or compatibility execution routes. Use
  `/v3/staged` and `/v3/capabilities`; raw TypeQL uses opt-in `/v3/raw`.
  Rename the server `[v2]` configuration section to `[staged]` and migrate
  interceptors to staged authorization and audit hooks.
- Typed execution defaults to 60,000 ms in every SDK, including C. A
  nonpositive signed timeout selects no deadline; server ceilings still apply.
  Proxy admission expiry remains a separate positive finite window.
- The former 2.3.3 `filter(...).where(...)` predicate correction is part of
  this 3.0 migration: chained predicates preserve the original filter. There
  is no separate 2.3.x backport in this work.

For raw TypeQL behavior, see [the escape hatch](queries.md#raw-typeql-execution).
The following sections give common source migrations. The complete checklist
at the end includes every 3.0 Breaking entry and its replacement.

## Migrations in 3.0

Use the canonical workspace migration commands and generated SDK catalogs.
The `type-bridge-migration` crate and its independent planner, executor, state
stores and recovery API are removed; 3.0 does not publish that crate. The
canonical provider retains only the read-only legacy ledger importer and
frozen state-label helpers. Python state-label helpers keep their old labels;
they do not describe the new journal schema.

Replace legacy `CopyAttribute` authoring with a declarative copy-attribute
backfill intent. Rollback removes only destination values equal to the source;
unlike the deleted executor it preserves other pre-existing destination values.
See [migration backfills](migrations.md#declarative-data-backfills).

### Move the history directory and regenerate locks

The default is now `migrations/` beside `typebridge.yaml`. Remove the explicit
`migrations.directory` setting to use that default, or set another confined
path. An explicit `migrations/v2` path is still accepted. Without an explicit
setting, finding the old subdirectory returns `migration_directory_move_required`
(`WorkspaceConfigErrorCode::LegacyMigrationDirectory`) before history writes.

Stop migration writers and, from the manifest directory, move all entries,
including hidden authoring files, without overwriting existing files:

```bash
find migrations/v2 -mindepth 1 -maxdepth 1 -exec mv -n -t migrations/ -- {} + && rmdir migrations/v2
```

If a destination file already exists, inspect the collision before continuing.
Rename Rust workspace `MigrationV2Directory` to `MigrationDirectory`, and its
`migration_v2_directory` accessor/builder to `migration_directory`. The former
`InvalidMigrationV2Directory` suffix restriction is gone; ordinary confinement
errors remain `PathNotConfined`.

Regenerate application-owned workspace locks with `generate_workspace_lock`.
The old `migration_v2_directory` identity key was hashed into
`resolved_config_identity`, rather than stored as a top-level lock field.
Verification returns typed `WorkspaceLockErrorCode::Stale` for that old identity
and gives a regeneration hint. Lock verification never silently rewrites it.

### Await Node migration execution

`MigrationPlan.execute()` and `executeControlled()` now return Promises so
registered async callbacks can run on the JavaScript event loop. Update every
migration call, including schema-only plans:

```typescript
const report = await plan.execute(database, "deployment");
const controlled = await plan.executeControlled(database, "deployment", options);
```

Ordinary manager and transaction methods retain their existing return types.
Register callbacks before creating a preview. Regenerate Python packages so
their manager and typed-query connection types accept `MigrationTransaction`.
See [user data steps](migrations.md#registered-user-data-steps) for each SDK's
callback signature and transaction ownership.

C callback transactions become unusable when the callback ends, but their
handle allocations remain retained until the catalog and every derived plan
close. Close those owners before closing the database; database close is
refused while retained callback handles still hold it.

### Import the existing journal once

Stop all 2.x migration runners before the first 3.0 apply. Keep a backup of the
managed database and its `<database>__tbv2_journal` companion. The first 3.0
`apply`, `rollback`, or `upgrade-profile` verifies and copies that journal into
`<database>__tb_journal` under a fenced lease. It preserves old record payloads,
digests, sequences and recorded fences, and records the verified legacy applied
set and cutover sentinel in the new journal. It publishes the new managed fence
only after writing `import-complete`.

If interrupted, rerun the same command. Recovery keeps the new journal's owner
and monotonic fence, discards incomplete copied records under a new lease and
repeats the import. Read-only `verify` reports `migration_journal_import_pending`
until import completes. A later advance of the old journal reports
`migration_old_writer_after_import`; stop the old runner and reconcile the
histories before continuing.

The importer leaves the old database and managed legacy rows untouched. After
successful 3.0 verification and a checked backup, you may delete the exact old
companion database `<database>__tbv2_journal` with TypeDB administration. Retain
the old managed control declarations until schema maintenance can remove their
rows and declarations together; do not delete the new `typebridge-internal-*`
control state. Ordinary operations use the imported evidence, so the old
applied-ledger rows are no longer required after successful import. From then
on, back up and restore the managed database with `<database>__tb_journal`.

## Rust model identifiers

Generated Rust models and families return `&Iid` from `iid()`; references return
`Option<&Iid>`. Pass `Iid::new("0x1234")?` to reference `from_iid` constructors.
Use `.as_str()` to borrow the identifier text or `.to_string()` to own it. Models
validate the IID during hydration and retain it, so repeated getters do not
parse or allocate.

Rust `get_by_iid` methods on exact, subtype, and transaction managers take
`&Iid`. Pass a model's `iid()` directly, or construct `Iid::new("0x1234")?`.
The permanent redirect is equivalent to `manager.filter()?.iid(&iid)?.first().await`.
Malformed provider documents are classified as hydration failures; the previous
exact IID read path incorrectly labeled these as input failures.

Rust `update` and `delete` likewise take `&Iid`, including transaction managers.
Their permanent redirects are `filter()?.iid(&iid)?.update(input).await` and
`filter()?.iid(&iid)?.delete().await`. Lifecycle hooks run in those filter
terminals, so aliases and direct filter calls have the same events.

Rust `delete_many` and `delete_many_returning` take `&[Iid]`, `delete_returning`
takes `&Iid`, and `update_many` takes `Vec<(Iid, Create)>`, including transaction
managers. `delete_many` redirects to `filter()?.iids(&iids)?.delete().await`.
`update_many` retains heterogeneous per-row replacements and ordinal results;
`delete_many_returning` retains ordinal snapshots, and `delete_returning` wraps
one returning batch row. Their existing lifecycle events remain unchanged.

Query bindings use `binding.iid(&iid)` and `binding.iid_in([&first, &second])`.
`PlanBuilder::by_iid` and generated model `by_iid` authoring likewise borrow
`Iid`. Declare staged identifier columns with `input::<Iid>("target_iid")`;
`Input<Iid>` accepts nominal identifier values and earlier staged IID result
references. These authoring methods retain their existing staged plans.

## Python identifiers

Generated model and reference `.iid` properties return a retained `Iid` object
(or `None` for an unattached model or key-only reference). Pass `Iid("0x1234")`
to reference constructors and `attach_runtime_iid`. Repeated getters return
the same object without parsing or allocation; use `str(iid)` for text or JSON.

`get_by_iid(iid)` permanently redirects to the shared `filter(iid=iid).first()`
terminal. Manager `get_by_iid`, IID `delete`, and `filter(iid=...)`/`filter(iid__in=...)`
also require nominal identifiers. Passing a malformed string to `get_by_iid`
now raises a type error instead of returning `None`.

Python IID `delete(iid)` and the native IID-list delete permanently redirect to
`filter(iid=iid).delete()` and `filter(iid__in=iids).delete()`. Instance
`delete` and `delete_many` retain their pre/post callbacks around those native
terminals. Returning deletes keep their stored-snapshot batch semantics. Attached-instance
`update(instance)` selects `filter(iid=instance.iid).update(instance)` in the
shared executor while retaining the instance operation's callbacks.

Python IID lists are consumed once. `filter(iid__in=[])` returns no matches
without a provider request, including when combined with other read lookups.
For canonical mutations, use `filter(iid=target).delete()` or
`filter(iid__in=targets).update(replacement)`. Pure IID-list deletes
retain the batch input limits; reads and updates retain the 256-term Boolean
limit and refuse larger lists before I/O.

Native `DynamicExpr.iid`, `IidConstraint`, and `IidPattern` constructors and
IID setters also require `Iid` values. The AST's stored string spelling and
lowering remain unchanged.

Pass `Iid("0x1234")` (imported from `type_bridge`) to query binding `iid`,
`iid_in`, and generated model `by_iid` authoring. Plain strings are refused
before execution. These methods retain their existing query and staged plans.

Declare staged identity inputs with `builder.input("target", "iid")` (or
`optional_input`) and bind `target.value(Iid("0x1234"))`. Both `by_iid_input`
and `StagedResultRef` require those `PlanInput[Iid]` columns; ordinary string
columns are refused. The existing staged wire format remains unchanged.

## Node identifiers

Import `Iid` from `@type-bridge/node` or the generated package. Model and
reference `.iid` properties return a retained `Iid` object (or `null` when
unattached). Repeated getters return the same object; use `iid.toString()` for
text and `iid.equals(other)` for value equality.

Pass `new Iid("0x1234")` to reference constructors, manager IID arguments,
`filter({ iid })`, `filter({ iid__in: iids })`, query `iid`/`iidIn`, and generated
or builder `byIid`. Plain strings are refused before execution.

Declare staged identity columns with `builder.input("target", "iid")` or
`optionalInput`, then bind `target.value(new Iid("0x1234"))`. `byIidInput` and
staged result references require `PlanInput<Iid>`; ordinary string columns are
refused. Query and staged plan bytes retain the existing string transport.

Node filter `update(replacement)` replaces every match and returns `WriteResult`
with all hydrated roots. Use `filter({ iid }).update(replacement)` or the
canonical `where(...).update(replacement)` form. IID `update(iid, replacement)`
permanently redirects to that filter terminal, preserving existing predicates.
An unfiltered one-argument manager update refuses. `updateMany` keeps its
heterogeneous ordinal batch inputs. Materialization stays inside the write lease;
a failure after a possible mutation makes borrowed transactions rollback-only.

Node filter `delete()` acts on every match. Use `filter({ iid }).delete()` or
`where(field, comparison, value).delete()`; IID `delete(iid)` is a permanent
redirect to that filter terminal and retains any existing predicates. A direct
unfiltered `manager.delete()` refuses: select all explicitly with `filter({})`
or `where()`. Wider keyword filters require an explicit read followed by instance
or IID batch operations. Node has no public CRUD hook API.
`deleteMany(iids)` redirects to `filter({ iid__in: iids }).delete()`. Pure IID-list
deletes retain batch input limits and ordinal plans, including singleton lists;
reads and updates keep the 256-term Boolean limit. Returning deletes retain their
ordinal stored-snapshot batches.

Node `getByIid(iid)` permanently redirects to `filter({ iid }).first()`,
retaining existing canonical keyword selection.

Node `filter(...).where(...)` preserves IID and closed scalar comparisons.
Wider lookups remain available through keyword read terminals and refuse
`.where()` with `unsupported_manager_filter`; read matching models first, then
use their identifiers for explicit instance or batch operations. Empty IID lists
match nothing, including when combined with wider keyword reads.

## Single-item manager write results

Single-item `insert`, `put`, `update`, and `delete_returning` calls now return
`WriteResult`, including calls made in an explicit transaction. Node names the
last method `deleteReturning`. The result reports every root thing answered by
TypeDB, in answer order:

- `count`: the number of root things written;
- `items`: all hydrated root things;
- `item`: the single thing when `count == 1`, otherwise `None` / `null`.

Dynamic relation lookups using non-key fields can match two players and insert
two relations. Both appear in `items`; the call does not choose one or reject the
answer. Generated references remain key-only and cannot fan out by construction,
but use the same result shape. Repeated references to the same player follow
TypeQL behavior. Missing references retain their existing typed lookup failure.
No strict mode or additional pre-flight guard is added. The `_many` methods
continue to return lists.

For Python, change:

```python
person = people.insert(Person(name=Name("Ada")))
```

To:

```python
result = people.insert(Person(name=Name("Ada")))
person = result.item
```

Check `result.count` or iterate `result.items` when your references can fan out.
A single write that matches no root returns `count == 0`, empty `items`, and
`item is None`, including a missing referenced player or an absent update root.
It does not run a diagnostic lookup or raise the former missing-reference error.
The rare wide-row existence guard still prevents partially created relations.
Python attaches the input instance IID and refreshes that instance only when the
result contains exactly one root. For zero or several roots, the input instance
is unchanged and `items` contains the returned hydrated instances.

For Node, use `people.insert(input).item`, or iterate the returned `items`.
For Rust, retain the result with `let result = people.insert(input).await?;`,
then use `result.item()` to borrow the single model, or `result.into_items()` to
take ownership of all models. Rust exposes `count()`, `items()`, and `item()`
accessors.

## Core manager construction

Construct core query builders through a registry-backed manager’s `.query()`.
The database-only `EntityQuery::new` and `RelationQuery::new` constructors are
removed. Use `.group_by()` on those queries instead of
`GroupByEntityQuery::new` or `GroupByRelationQuery::new`.

Core descriptor managers require registry authority. Replace
`DynamicEntityManager::new` / `new_canonical` and
`DynamicRelationManager::new` / `new_canonical` with `::from_registry`.
Replace their `with_transaction` / `with_canonical_transaction` constructors
with `::with_registry_transaction`. Use `literal_values()` when the owning
provider requires literal transport.

The generic core `EntityManager<T>::new` and `RelationManager<R>::new`
constructors are replaced by `::from_registry` too. Their single writes and
the dynamic managers' single writes return `WriteResult`, including exact
put/update forms and returning deletes. Generic results contain hydrated models;
dynamic results contain hydrated rows. The many-result methods retain their
list contracts.

## Native plan execution replaces V1/V2

| Removed surface | Replacement |
| --- | --- |
| Rust `QueryPlan`, `QueryInvocation`, `QueryOperation`, legacy decoders and fingerprints | `query_plan::v3::QueryPlanV3`, `PlanSpec`, `Operation`, `decode_query_plan_v3`, and the native plan fingerprint |
| Rust `ModelQueryV2`, `QueryPlanV2Compatibility`, and the V2 model wrapper types | Generated model queries lowered directly to native plans |
| Rust `query_remote_v2` requests, responses, failures, limits and decoders | `query_remote_v3` staged requests/replies; shared limits live in `query_model_limits` |
| `ValidatedQuery`, `validate_query_plan`, `validate_query_local_function` | Native plan construction performs validation |
| ORM `query_v2`, `query_v2_builder`, `query_v2_prepared`, `query_v2_remote` and the old integration adapter | Native builders, `plan_explain::explain`, and staged execution |
| `ProxyHttpClient` / `ProxyHttpTransport` `capabilities` and `exchange` | `staged_capabilities` and `exchange_staged` |
| Python `type_bridge.query_v2`, Node `@type-bridge/node/query-v2` | `PlanBuilder` and `Plan` from `type_bridge.plan` / `@type-bridge/node` |
| `AuthoredQueryPlan`, `AuthoredQueryInvocation`, `PendingQueryV2Remote`, `QueryV2RemoteLimits` | Built native `Plan`, staged pending answers, and transaction resource limits |
| Python `query_v2_execute_local`, `query_v2_remote_capabilities`, `query_v2_prepare_remote` | Native direct/proxy staged transactions |
| Node `queryV2ExecuteLocal`, `queryV2RemoteCapabilities`, `queryV2PrepareRemote` | Native direct/proxy staged transactions |
| Rust `QueryAuthority::from_declared_bytes_query_only` | `QueryAuthority::from_declared_bytes` plus native staged execution |
| Python `PlanAuthority.query_only`, `plan_query_only_authority` | `PlanAuthority(...)` plus native staged execution |
| Node `PlanAuthority.queryOnly`, `planQueryOnlyAuthority` | `new PlanAuthority(...)` plus native staged execution |

V1/V2 plan bytes and envelopes are no longer accepted. Rebuild plans through
the native builder and regenerate schema packages. Native plan inputs are part
of the built program; there is no separate legacy invocation object.

## Python proxy connections

Replace `type_bridge.proxy.ProxyDatabase` with your generated package's
`connect_proxy(url)` and use that connection's transaction API. Catch
`type_bridge.Failure` instead of `ProxyError`. `ProxyTransaction` and
`ProxyTransactionContext` are removed with the legacy module. The public
`Connection` alias describes direct facade connections; generated proxy
connections retain their own native type.

## Rust proxy names

Replace `RemoteDatabase`, `RemoteQueryTransport`, `RemoteConnectionOptions`,
and `RemoteQueryLimits` with `ProxyDatabase`, `ProxyExchange`,
`ProxyConnectionPolicy`, and `QueryExecutionResourceLimits`. Caller-owned
transports implement `ProxyExchange` directly.

## Node query facade

Replace package-root `TypedQuery`, `TypedGroupByQuery`, `QueryExpr`, and `agg`
with your generated package's `QuerySession`. Use generated query predicates,
ordering, grouping, and reduction methods instead of `ComparisonExpr`,
`BooleanExpr`, `NotExpr`, `SortExpr`, `AggregateSpec`, and `QueryGroupField`.
Read the generated query error’s structured diagnostics instead of catching
the removed `TypedQueryError`.

## Python canonical filter mutations

Use `manager.where(field, ProjectedManagerComparison.EQ, value).delete()`
to delete every matching model in one native mutation. Use the same selection's
`update(replacement)` to replace every match and receive a `WriteResult` with
all written models. Both filter terminals run no instance CRUD hooks. For hooks, read the instances explicitly and call
`manager.delete_many(instances)`.

Call `manager.filter(...).delete()` or `manager.filter(...).update(replacement)`
to mutate a supported keyword selection directly. An unfiltered no-argument
`manager.delete()` remains refused. Chaining `where()` after a keyword filter
also preserves its supported scalar and IID predicates. Wider keyword lookups refuse the canonical path with
`unsupported_manager_filter`; use `instances = manager.filter(...).all()`
and then instance operations. Borrowed write transactions may construct
filters and mutate through them. Canonical read terminals accept borrowed
read or write transactions, preserving read-after-write in the caller's
transaction; borrowed schema transactions are refused.

Rust raw TypeQL uses `db.execute_query(text, kind)` or
`db.raw_transaction(kind).await?` followed by `tx.execute_query(text)`.
Typed staged `Transaction::query()` stays unchanged. Proxy raw contexts accept
one query and commit writes on the host; use a typed staged transaction when
several operations must commit together through a proxy.

Python `Database.execute_query(text, transaction_type)` now returns the complete
TypeDB HTTP query-answer dictionary, including `answerType`, `queryType` and
`answers`, instead of flattening answers into a list. It sends schema text
without the old annotation gate. For multiple direct raw statements use
`with db.raw_transaction("write") as tx: ...` and `tx.execute_query(text)`;
clean exit commits, exceptions close without committing, and explicit rollback
prevents an automatic commit. `commit_on_exit=False` leaves completion explicit.
Proxy raw contexts accept one query and cannot roll back a committed write.

Node raw TypeQL uses `await db.executeQuery(text, kind)` or
`await db.rawTransaction(kind)` followed by `await tx.executeQuery(text)`.
The complete provider answer preserves integer tokens outside JavaScript's
safe range as `bigint`; safe integers and doubles remain `number`. Direct raw
transactions support multiple queries and atomic rollback. Proxy contexts
accept one query, commit writes on the host, and refuse a second query or
rollback of a committed write. Close raw contexts in `finally` blocks.

Python `Database.execute_with_rows`, `TransactionContext.execute`,
`TransactionContext.execute_with_rows`, and `context.transaction.execute` are
removed. Use `Database.execute_query` or an explicit `raw_transaction` context
with `execute_query`; the answer is the complete provider dictionary. For
parameterized batches, use generated manager bulk operations.

Node `TransactionContext.query` (native `queryJson`) is removed. Use
`Database.executeQuery` or `rawTransaction` and its `executeQuery`. Existing
manager transaction contexts and typed staged queries keep their typed APIs.

## C ABI 2.0 package and database names

Regenerate C packages against ABI 2.0. These names lose their version suffix;
argument layouts and descriptor version values stay the same.

| Previous name | ABI 2.0 name |
| --- | --- |
| `type_bridge_schema_package_open_v2` | `type_bridge_schema_package_open` |
| `type_bridge_schema_package_open_chunked_v2` | `type_bridge_schema_package_open_chunked` |
| `type_bridge_database_config_validate_v2` | `type_bridge_database_config_validate` |
| `type_bridge_database_open_v2` | `type_bridge_database_open` |
| `type_bridge_database_open_with_query_limits_v2` | `type_bridge_database_open_with_query_limits` |
| `type_bridge_database_config_v2_t` | `type_bridge_database_config_t` |
| `TYPE_BRIDGE_DATABASE_CONFIG_V2_VERSION` | `TYPE_BRIDGE_DATABASE_CONFIG_VERSION` |
| `*_schema_package_open_v2` | `*_schema_package_open` |

The V1 `type_bridge_database_config_v1_t` and `type_bridge_database_open_v1`
are removed. Initialize `type_bridge_database_config_t` to zero, set
`struct_size = sizeof(config)` and
`version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION` (layout value `2`), and call
`type_bridge_database_open(runtime, package, &config, NULL, &database, &diagnostics)`. Copy the old
`address`, `database`, `username`, `password`, and `http_port` fields unchanged;
retain `tls_mode` as `TYPE_BRIDGE_TLS_DISABLED` or
`TYPE_BRIDGE_TLS_NATIVE_ROOTS`. Keep `custom_root_ca_pem` empty for these modes
and reserved words zero. All-zero `connection_limits` and `answer_limits`
select the shared direct defaults; partially initialized policy descriptors
are rejected. Custom-root TLS uses `TYPE_BRIDGE_TLS_CUSTOM_ROOT_CA` and the PEM
byte view. The current open call returns execution diagnostics: release them
with `type_bridge_execution_diagnostics_close`.

The old generic-diagnostics `type_bridge_schema_package_open_v1` and
`type_bridge_schema_package_open_chunked_v1` are removed. Use the current
unsuffixed openers with `type_bridge_execution_diagnostics_t **` as the final
argument. Generated `*_schema_package_open` likewise has one signature with
execution diagnostics. Both runtime `type_bridge_migration_catalog_open` and
generated `*_migration_catalog_open` now use that same diagnostic output type.
Release these outputs with `type_bridge_execution_diagnostics_close` instead of
`type_bridge_diagnostics_close`.

Admission failures retain the underlying structured diagnostics in detail kind
`TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_ADMISSION_CAUSE` (`16`), with key
`admission_cause`. Its `primary` byte view is the exact canonical JSON array of
original diagnostic objects, including code, message, path and details. Read
it with `type_bridge_execution_diagnostics_detail_get_v1` while the execution
diagnostics owner is live. The enclosing typed admission code is retained.

C transaction open and commit names now use the policy-aware signature:

| Previous call | Current call |
| --- | --- |
| `type_bridge_read_transaction_open(db, cancel, out, diagnostics)` | `type_bridge_read_transaction_open(db, NULL, cancel, out, diagnostics)` |
| `type_bridge_write_transaction_open(db, cancel, out, diagnostics)` | `type_bridge_write_transaction_open(db, NULL, cancel, out, diagnostics)` |
| `type_bridge_write_transaction_commit(tx, cancel, diagnostics)` | `type_bridge_write_transaction_commit(tx, NULL, cancel, diagnostics)` |

For these three functions, the `_v2` name becomes the unsuffixed name without
changing arguments. `NULL` selects the shared direct default policy; supply a
`type_bridge_query_execution_limits_v1_t` pointer to set call limits. Open and
commit retain the owning database or transaction ceilings. Cancellation uses
the current policy-aware diagnostics, including `provider_cancelled` when a
cancellation is already requested before transaction dispatch.

## C CRUD policy arguments

The C entity and relation CRUD functions now have one policy-aware signature.
Remove `_v2` from existing policy-aware calls. For old unsuffixed calls, insert
`limits` at the position below (`NULL` preserves the default policy). Runtime
names are `type_bridge_{target}_{kind}_{operation}`, where `kind` is `entity`
or `relation`; `target` is `database` or `write_transaction` for every row, and
`read_transaction` for `get_by_iid` and `count`.

| Operation | Current runtime arguments |
| --- | --- |
| `insert`, `put` | `target, model, create, limits, cancellation, out_result, diagnostics` |
| `get_by_iid` | `target, model, iid, limits, cancellation, out_thing, diagnostics` |
| `update` | `target, model, iid, create, limits, cancellation, out_result, diagnostics` |
| `delete_by_iid` | `target, model, iid, limits, cancellation, diagnostics` |
| `count` | `target, model, limits, cancellation, out_count, diagnostics` |

Generated `{model}_{target}_{operation}` wrappers have the same argument
order with the implicit `model` argument omitted and nominal input/result
owners. Regenerate packages; every supported profile now emits these wrappers
as static inline functions. Unkeyed single `put` wrappers remain available;
the existing key-admission behavior is unchanged. By-IID reads and mutations
continue to build a typed IID filter and use the shared terminal.

NULL-policy CRUD calls now use the same in-flight cancellation handling as
explicit policies: cancellation returns `provider_cancelled`, and owned writes
roll back. Invalid overlapping output slots are rejected without modifying
those slots, like other policy-aware preflight failures.

| Removed C symbol | Replacement |
| --- | --- |
| `type_bridge_database_entity_count_v2` | `type_bridge_database_entity_count` |
| `type_bridge_database_entity_delete_by_iid_v2` | `type_bridge_database_entity_delete_by_iid` |
| `type_bridge_database_entity_get_by_iid_v2` | `type_bridge_database_entity_get_by_iid` |
| `type_bridge_database_entity_insert_v2` | `type_bridge_database_entity_insert` |
| `type_bridge_database_entity_put_v2` | `type_bridge_database_entity_put` |
| `type_bridge_database_entity_update_v2` | `type_bridge_database_entity_update` |
| `type_bridge_database_relation_count_v2` | `type_bridge_database_relation_count` |
| `type_bridge_database_relation_delete_by_iid_v2` | `type_bridge_database_relation_delete_by_iid` |
| `type_bridge_database_relation_get_by_iid_v2` | `type_bridge_database_relation_get_by_iid` |
| `type_bridge_database_relation_insert_v2` | `type_bridge_database_relation_insert` |
| `type_bridge_database_relation_put_v2` | `type_bridge_database_relation_put` |
| `type_bridge_database_relation_update_v2` | `type_bridge_database_relation_update` |
| `type_bridge_read_transaction_entity_count_v2` | `type_bridge_read_transaction_entity_count` |
| `type_bridge_read_transaction_entity_get_by_iid_v2` | `type_bridge_read_transaction_entity_get_by_iid` |
| `type_bridge_read_transaction_relation_count_v2` | `type_bridge_read_transaction_relation_count` |
| `type_bridge_read_transaction_relation_get_by_iid_v2` | `type_bridge_read_transaction_relation_get_by_iid` |
| `type_bridge_write_transaction_entity_count_v2` | `type_bridge_write_transaction_entity_count` |
| `type_bridge_write_transaction_entity_delete_by_iid_v2` | `type_bridge_write_transaction_entity_delete_by_iid` |
| `type_bridge_write_transaction_entity_get_by_iid_v2` | `type_bridge_write_transaction_entity_get_by_iid` |
| `type_bridge_write_transaction_entity_insert_v2` | `type_bridge_write_transaction_entity_insert` |
| `type_bridge_write_transaction_entity_put_v2` | `type_bridge_write_transaction_entity_put` |
| `type_bridge_write_transaction_entity_update_v2` | `type_bridge_write_transaction_entity_update` |
| `type_bridge_write_transaction_relation_count_v2` | `type_bridge_write_transaction_relation_count` |
| `type_bridge_write_transaction_relation_delete_by_iid_v2` | `type_bridge_write_transaction_relation_delete_by_iid` |
| `type_bridge_write_transaction_relation_get_by_iid_v2` | `type_bridge_write_transaction_relation_get_by_iid` |
| `type_bridge_write_transaction_relation_insert_v2` | `type_bridge_write_transaction_relation_insert` |
| `type_bridge_write_transaction_relation_put_v2` | `type_bridge_write_transaction_relation_put` |
| `type_bridge_write_transaction_relation_update_v2` | `type_bridge_write_transaction_relation_update` |

## Removed Rust compatibility helpers

`Transaction` now refers to the staged transaction at the ORM session/root and
Rust SDK root. Code using the old eager ORM alias must name `OwnedTransaction`.
Checked schema generation returns `Result` instead of silently returning an
empty string on invalid input. Select the semantic profile explicitly when
constructing a schema-lowering binding or validating a migration server.

| Removed Rust API | Replacement |
| --- | --- |
| `DirectTls` | TlsPolicy |
| `MAX_QUERY_ITEMS` | DEFAULT_PROXY_QUERY_ITEMS |
| `MAX_QUERY_BYTES` | DEFAULT_PROXY_QUERY_BYTES |
| `MAX_QUERY_GRAPH_NODES` | DEFAULT_PROXY_QUERY_GRAPH_NODES |
| `MAX_QUERY_ATTRIBUTE_VALUES` | DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES |
| `MAX_QUERY_COLLECTION_MEMBERS` | DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS |
| `MAX_QUERY_ROLE_PLAYERS` | DEFAULT_PROXY_QUERY_ROLE_PLAYERS |
| `type_bridge_orm::Transaction; type_bridge_orm::session::Transaction (eager alias)` | OwnedTransaction for eager calls; Transaction now stages plans |
| `SchemaLoweringProfileFingerprint::compute(bytes)` | compute_for_profile(profile, bytes) |
| `SchemaLoweringBinding::current(capabilities)` | for_semantic_profile(profile, capabilities) or for_context(context) |
| `require_supported_migration_server(database)` | require_supported_migration_server_for_profile(database, profile) |
| `generate_define_block(info)` | try_generate_define_block(info), handling its Result |
| `card_annotation(min, max)` | checked tql::schema_card plus render_schema_annotations |
| `attribute_definition(attribute)` | attribute_node(attribute, true) plus tql::render_schema_definition |
| `attribute_constraint_definition(attribute)` | attribute_node(attribute, false) plus tql::render_schema_definition |

The backend hook `DriverBackend::open_schema_fenced_read_transaction` and its
`SchemaFencedReadTransaction::{new, into_parts}` wrapper are removed. Implement
`DriverBackend::open_with_options` with Write transactions that exclude schema
commits and enforce the deadline while opening and throughout their lifetime.
The existing database fenced-read entry point exports the schema while holding
that transaction, closes it on export failure, and never commits it.


The generated Python and TypeScript compatibility query aliases are removed:

| Removed generated API | Replacement |
| --- | --- |
| `RemoteQuery` | `Query` |
| `RemoteGroupedQuery` | `GroupedQuery` |
| `RemoteQueryLimits` | `QueryExecutionResourceLimits` |
| `RemoteQuerySession` | `QuerySession` with an `ExchangeConnection` |
| TypeScript `RemoteQueryExchange` | The exchange callback accepted by `ExchangeConnection` |

Pass limits through `resources` when constructing `QuerySession`. Rename the
legacy limit fields to the current resource names, including `deadline_ms` to
`timeout_milliseconds` in Python and `deadlineMs` to `timeoutMilliseconds` in
TypeScript.


Legacy caller-owned query ASTs are removed. Build queries through generated
models and typed plan builders; raw TypeQL goes through the explicit raw
transaction API. Rust engine callers retain `QueryCompiler::compile_typed_*`
and `prepare_typed_*` for canonical typed operations.

| Removed API | Replacement |
| --- | --- |
| Rust `ast::{Value, LiteralValue, FunctionCallValue, ArithmeticValue, RolePlayer, Constraint, Pattern, Statement, Clause, LetAssignment, FetchItem, QueryNode, SortField, ReduceAssignment}` | Canonical typed query operations and plan builders |
| Rust `QueryCompiler::{compile, compile_node, compile_clause, compile_pattern, compile_statement, compile_constraint, compile_value, try_compile, try_compile_node}` | `compile_typed_*` / `prepare_typed_*` |
| Rust `query_parser::{parse_typeql_query, parse_typeql_query_checked, QueryParseError}` | Official TypeQL parsing for parser integrations; explicit raw transactions for execution |
| Python `QueryCompiler` (including `compile`, `compile_dicts`, `compile_node`, `compile_clause_body`, `parse` and private `_compile_query`, `_compile_batch`, `_compile_fetch_items`, `_compile_node`, `_compile_clause_body`) and `parse_typeql_query` | Generated queries and typed plan builders |
| Rust and Python `ValidationEngine::{validate_pattern, validate_statement, validate_query}`; Python `_ArchivedTypeSchema.validate_query` | Typed query/plan validation; identifier and custom entity-rule validation remain available |
| Rust `_parser::parse_typeql`, `_schema::TypeSchema::from_typeql`; Python `_ArchivedTypeSchema.from_typeql` | Split-YAML schema authority and checked schema adapters |
| Rust `AttributeValue::to_ast_value`, `Expr::to_patterns`, `Agg::to_reduce_assignment` | Pass values and expressions directly to native plan builders |

The Python wrappers for the removed AST vocabulary are also removed:
`LiteralValue`, `FunctionCallValue`, `ArithmeticValue`, `RolePlayer`, `IidConstraint`,
`HasConstraint`, `IsaConstraint`, `EntityPattern`, `RelationPattern`, `SubTypePattern`,
`AttributePattern`, `HasPattern`, `ValueComparisonPattern`, `NotPattern`, `OrPattern`,
`IidPattern`, `RawPattern`, `HasStatement`, `IsaStatement`, `RelationStatement`,
`DeleteThingStatement`, `RawStatement`, `MatchClause`, `MatchLetClause`, `LetAssignment`,
`InsertClause`, `DeleteClause`, `UpdateClause`, `FetchAttribute`, `FetchVariable`,
`FetchAttributeList`, `FetchFunction`, `FetchWildcard`, `FetchNestedWildcard`, `FetchClause`,
`AggregateExpr`, `ReduceAssignment`, `ReduceClause`.

The old generated-query `update_with(func)` and direct-driver
`type_bridge.session.Transaction` are gone; use manager filter `update` and
the staged `Transaction` API. Deprecated Rust `RemoteDatabase`,
`RemoteQueryTransport`, `RemoteConnectionOptions::{generated, new}` and
`RemoteQueryLimits` have also been removed. Their replacements are
`ProxyDatabase`, `ProxyExchange`, `ProxyConnectionPolicy` and
`QueryExecutionResourceLimits`, respectively.

## Complete breaking-change checklist

This checklist uses the same source as the 3.0 changelog so that removed names,
replacements and behavioral changes cannot drift between the two pages.

--8<-- "CHANGELOG.md:breaking-3-0"

## Contract imports

Use `type_bridge_contract::query_plan` for both plan vocabulary and builders;
use `type_bridge_contract::query_remote` for staged envelopes and signing.
`QueryPlanV3` becomes `QueryPlan`, and `StagedRequestV3`, `StagedAnswerV3`,
`StagedLimitsV3` and `StagedReplyV3` lose their `V3` suffix. The complete
old-to-new public-name table is included in the breaking-change inventory above.
These source renames do not change plan or envelope bytes.

## Server policy hooks

Import the router and staged executor from `transport::staged`. Rename
`on_v3_*` hooks to `on_*` and the `V3PolicyPlan`, `V3PolicyOutcome` and
`V3CommitOutcome` views to `PolicyPlan`, `PolicyOutcome` and `CommitOutcome`.
Remove `supports_v3` implementations and explicit coverage-check calls.
`request_context(metadata)` now returns the context directly; call
`begin_request` to run transport admission before handing a request to the executor.
The endpoint metadata key is `endpoint`, replacing `v3_endpoint`.

Removing `supports_v3` (formerly false by default) and `validate_v3_coverage`
removes the fail-closed opt-in check: every configured custom interceptor is
now applied to every request. Interceptor authors must check that their hooks
handle typed plans, raw requests and writes, enforce the intended authorization
for each, and explicitly reject unsupported operations; a missing hook override
no longer causes a coverage refusal.

## Diagnostic code names

Active internal plan diagnostics lose their historical query-version prefix.
SDK diagnostic categories, typed paths and details retain their meaning.
Mixed 2.x clients with a 3.0 server, or 3.0 clients with older servers, receive
no diagnostic-code translation. The `typed_transport_diagnostic` compatibility
adapter was removed; update client/server code together and match the current
codes below rather than relying on old version-prefix normalization.

Historical `old_code` entries in the migration ledger keep the original
spelling; they are not active diagnostic producers.

| Previous code | Current code |
| --- | --- |
| `query_plan_v2_compatibility_value_chunks` | `query_plan_compatibility_value_chunks` |
| `query_plan_v2_compatibility_value_lexical` | `query_plan_compatibility_value_lexical` |
| `query_plan_v2_compatibility_value_limit` | `query_plan_compatibility_value_limit` |
| `query_remote_v2_diagnostic_detail_key_invalid` | `query_remote_diagnostic_detail_key_invalid` |
| `query_remote_v2_diagnostic_long_invalid` | `query_remote_diagnostic_long_invalid` |
| `query_remote_v2_diagnostic_too_large` | `query_remote_diagnostic_too_large` |
| `query_remote_v2_evidence_mismatch` | `query_remote_evidence_mismatch` |
| `query_remote_v2_native_only_operation` | `query_remote_native_only_operation` |
| `query_remote_v3_advertisement_mismatch` | `query_remote_advertisement_mismatch` |
| `query_remote_v3_answer_concept` | `query_remote_answer_concept` |
| `query_remote_v3_answer_count` | `query_remote_answer_count` |
| `query_remote_v3_answer_dimensions` | `query_remote_answer_dimensions` |
| `query_remote_v3_answer_invalid` | `query_remote_answer_invalid` |
| `query_remote_v3_answer_items` | `query_remote_answer_items` |
| `query_remote_v3_answer_kind` | `query_remote_answer_kind` |
| `query_remote_v3_binding_mismatch` | `query_remote_binding_mismatch` |
| `query_remote_v3_diagnostic` | `query_remote_diagnostic` |
| `query_remote_v3_diagnostic_limit` | `query_remote_diagnostic_limit` |
| `query_remote_v3_diagnostic_too_large` | `query_remote_diagnostic_too_large` |
| `query_remote_v3_document_attribute` | `query_remote_document_attribute` |
| `query_remote_v3_document_key` | `query_remote_document_key` |
| `query_remote_v3_document_role` | `query_remote_document_role` |
| `query_remote_v3_document_type` | `query_remote_document_type` |
| `query_remote_v3_document_value` | `query_remote_document_value` |
| `query_remote_v3_execution_timeline` | `query_remote_execution_timeline` |
| `query_remote_v3_execution_timeline_limit` | `query_remote_execution_timeline_limit` |
| `query_remote_v3_expired` | `query_remote_expired` |
| `query_remote_v3_format` | `query_remote_format` |
| `query_remote_v3_format_unsupported` | `query_remote_format_unsupported` |
| `query_remote_v3_given_mismatch` | `query_remote_given_mismatch` |
| `query_remote_v3_internal_failure` | `query_remote_internal_failure` |
| `query_remote_v3_kind` | `query_remote_kind` |
| `query_remote_v3_limits` | `query_remote_limits` |
| `query_remote_v3_noncanonical` | `query_remote_noncanonical` |
| `query_remote_v3_nonce` | `query_remote_nonce` |
| `query_remote_v3_partial_answers` | `query_remote_partial_answers` |
| `query_remote_v3_plan_bytes` | `query_remote_plan_bytes` |
| `query_remote_v3_plan_limit` | `query_remote_plan_limit` |
| `query_remote_v3_query_limits` | `query_remote_query_limits` |
| `query_remote_v3_reply_bytes` | `query_remote_reply_bytes` |
| `query_remote_v3_failure_binding` | `query_remote_failure_binding` |
| `query_remote_v3_failure_unexpected_binding` | `query_remote_failure_unexpected_binding` |
| `query_remote_v3_failure_wire_mismatch` | `query_remote_failure_wire_mismatch` |
| `query_remote_v3_reply_shape` | `query_remote_reply_shape` |
| `query_remote_v3_statement_limit` | `query_remote_statement_limit` |
| `query_remote_v3_time` | `query_remote_time` |
| `query_remote_v3_unexpected_rows` | `query_remote_unexpected_rows` |
| `query_v2_adapter_collection_order_proof` | `query_adapter_collection_order_proof` |
| `query_v2_adapter_descriptor_malformed` | `query_adapter_descriptor_malformed` |
| `query_v2_adapter_function_scope` | `query_adapter_function_scope` |
| `query_v2_adapter_function_state` | `query_adapter_function_state` |
| `query_v2_adapter_literal_byte_limit` | `query_adapter_literal_byte_limit` |
| `query_v2_adapter_literal_state` | `query_adapter_literal_state` |
| `query_v2_adapter_registry_authority_mismatch` | `query_adapter_registry_authority_mismatch` |
| `query_v2_adapter_registry_projection` | `query_adapter_registry_projection` |
| `query_v2_adapter_role_owner` | `query_adapter_role_owner` |
| `query_v2_adapter_schema_mismatch` | `query_adapter_schema_mismatch` |
| `query_v2_adapter_unknown_binding` | `query_adapter_unknown_binding` |
| `query_v2_adapter_validated_literal_malformed` | `query_adapter_validated_literal_malformed` |
| `query_v2_ast_lowering` | `query_ast_lowering` |
| `query_v2_diagnostic_unencodable` | `query_diagnostic_unencodable` |
| `query_v2_expected_one` | `query_expected_one` |
| `query_v2_given_value_unsupported` | `query_given_value_unsupported` |
| `query_v2_host_string_type` | `query_host_string_type` |
| `query_v2_host_string_unicode` | `query_host_string_unicode` |
| `query_v2_missing_input_value` | `query_missing_input_value` |
| `query_v2_model_attribute_limit` | `query_model_attribute_limit` |
| `query_v2_model_byte_limit` | `query_model_byte_limit` |
| `query_v2_model_collection_limit` | `query_model_collection_limit` |
| `query_v2_model_complete_rows_exceeded` | `query_model_complete_rows_exceeded` |
| `query_v2_model_contract_missing` | `query_model_contract_missing` |
| `query_v2_model_count_overflow` | `query_model_count_overflow` |
| `query_v2_model_exactly_one` | `query_model_exactly_one` |
| `query_v2_model_future_unmapped_code` | `query_model_future_unmapped_code` |
| `query_v2_model_graph_limit` | `query_model_graph_limit` |
| `query_v2_model_hydration_attributes` | `query_model_hydration_attributes` |
| `query_v2_model_hydration_authority` | `query_model_hydration_authority` |
| `query_v2_model_hydration_binding` | `query_model_hydration_binding` |
| `query_v2_model_hydration_conflict` | `query_model_hydration_conflict` |
| `query_v2_model_hydration_descriptor` | `query_model_hydration_descriptor` |
| `query_v2_model_hydration_document` | `query_model_hydration_document` |
| `query_v2_model_hydration_iid` | `query_model_hydration_iid` |
| `query_v2_model_hydration_kind` | `query_model_hydration_kind` |
| `query_v2_model_hydration_limit` | `query_model_hydration_limit` |
| `query_v2_model_hydration_missing` | `query_model_hydration_missing` |
| `query_v2_model_hydration_role_player` | `query_model_hydration_role_player` |
| `query_v2_model_hydration_roles` | `query_model_hydration_roles` |
| `query_v2_model_hydration_state` | `query_model_hydration_state` |
| `query_v2_model_hydration_unexpected` | `query_model_hydration_unexpected` |
| `query_v2_model_hydration_value_order` | `query_model_hydration_value_order` |
| `query_v2_model_hydration_value_type` | `query_model_hydration_value_type` |
| `query_v2_model_item_limit` | `query_model_item_limit` |
| `query_v2_model_order_missing` | `query_model_order_missing` |
| `query_v2_model_order_non_scalar` | `query_model_order_non_scalar` |
| `query_v2_model_order_value_type` | `query_model_order_value_type` |
| `query_v2_model_page_binding` | `query_model_page_binding` |
| `query_v2_model_page_document` | `query_model_page_document` |
| `query_v2_model_page_kind` | `query_model_page_kind` |
| `query_v2_model_page_root_set` | `query_model_page_root_set` |
| `query_v2_model_page_total_length` | `query_model_page_total_length` |
| `query_v2_model_page_total_plan` | `query_model_page_total_plan` |
| `query_v2_model_page_unexpected_root` | `query_model_page_unexpected_root` |
| `query_v2_model_predicate_equal_type` | `query_model_predicate_equal_type` |
| `query_v2_model_predicate_evidence` | `query_model_predicate_evidence` |
| `query_v2_model_predicate_order_type` | `query_model_predicate_order_type` |
| `query_v2_model_predicate_string_type` | `query_model_predicate_string_type` |
| `query_v2_model_provider_not_exhausted` | `query_model_provider_not_exhausted` |
| `query_v2_model_provider_plan_mismatch` | `query_model_provider_plan_mismatch` |
| `query_v2_model_provider_plan_missing` | `query_model_provider_plan_missing` |
| `query_v2_model_reduction_descriptor` | `query_model_reduction_descriptor` |
| `query_v2_model_reduction_group` | `query_model_reduction_group` |
| `query_v2_model_regex` | `query_model_regex` |
| `query_v2_model_role_edge_evidence` | `query_model_role_edge_evidence` |
| `query_v2_model_role_player_limit` | `query_model_role_player_limit` |
| `query_v2_model_root_binding` | `query_model_root_binding` |
| `query_v2_model_root_kind` | `query_model_root_kind` |
| `query_v2_model_root_malformed` | `query_model_root_malformed` |
| `query_v2_model_rows_collection` | `query_model_rows_collection` |
| `query_v2_model_solution_binding` | `query_model_solution_binding` |
| `query_v2_model_solution_duplicate_binding` | `query_model_solution_duplicate_binding` |
| `query_v2_model_solution_kind` | `query_model_solution_kind` |
| `query_v2_model_solution_limit` | `query_model_solution_limit` |
| `query_v2_model_solution_malformed` | `query_model_solution_malformed` |
| `query_v2_model_solution_malformed_iid` | `query_model_solution_malformed_iid` |
| `query_v2_model_solution_unknown_binding` | `query_model_solution_unknown_binding` |
| `query_v2_model_tuple_evidence` | `query_model_tuple_evidence` |
| `query_v2_model_tuple_plan` | `query_model_tuple_plan` |
| `query_v2_model_unstable_provider_order` | `query_model_unstable_provider_order` |
| `query_v2_reachable_scope` | `query_reachable_scope` |
| `query_v2_reachable_source_domain` | `query_reachable_source_domain` |
| `query_v2_shared_buffer_unsupported` | `query_shared_buffer_unsupported` |
| `query_v2_unknown_binding` | `query_unknown_binding` |

## Comparison methods and lookup tokens

Use `ne`, `le`, `ge` and `like` for typed query comparisons. Python suffix
variants (`_field`, `_value`, `_call`) and TypeScript variants (`Field`,
`Value`, `Call`) follow the same spelling. Regenerate C packages for the
corresponding manager and schema-function comparison names; use
`TYPE_BRIDGE_QUERY_COMPARE_LIKE` for the unchanged numeric comparison tag 10.

Manager keyword filters retain their protocol: `field__gte`, `field__lte`,
and explicit comparison tokens `eq`, `ne`, `lt`, `lte`, `gt`, `gte` keep their
meaning. Those lookup tokens are separate from typed method names.

## Sorting

Replace Python `order_by=` and TypeScript `orderBy:` options with `sort=`
and `sort:`. Collected selections use `.sort(order)` in all three SDKs;
Rust row/page option builders also use `.sort(order)`. In C descriptors,
rename `orders` and `order_count` to `sorts` and `sort_count`. Their order
and binary layout are unchanged; only the source spelling changes.

## Matching and role edges

Replace role `.connects(player)` calls with `.links(player)`. Python `.is_`
and TypeScript `.is` aliases are removed; use `.links` for those calls too.
Rust queries add additional bindings through `.matching(binding)`, replacing
`.match_(binding)`. C uses `*_query_match` instead of `*_query_add_hidden`;
this adds a binding to match without projecting it in the selected output.
Generated C role predicates end in `_query_role_links`.

## Query terminals

Replace query and grouped-query `.aggregate(terms)` calls with `.reduce(terms)`
in Python, TypeScript and Rust. Keep the `aggregate` namespace used to build
those terms, such as `aggregate.count()`. In generated C, explicit-root page,
count and exists factories are `*_query_page_by`, `*_query_count_by` and
`*_query_exists_by`; their result types and manager terminals keep their names.

### Plan authoring and pending answers

In Python, replace `PlanBuilder.where_` with `where` and `PlanBuilder.let_` with
`let`. In Rust typed recordings, replace `Bound.filter` and generated `<Model>Handle.filter` with `where_`. Rust keeps
`let_` because `let` is a keyword; manager `filter` methods remain unchanged.

In TypeScript, import `Pending` instead of `PendingPlanAnswer` and resolve it with
`result()` instead of `get()`. This is the same staged answer and lifecycle.
Rust's borrowed `Pending.get()` and owned `Pending.result()` remain distinct.

### Staged answer conversions

Use these unversioned Rust names; conversion behavior and wire bytes are unchanged.

| Removed name | Replacement |
| --- | --- |
| `to_staged_v3` | `to_staged` |
| `from_staged_v3` | `from_staged` |
| `QueryV2RuntimeAnswerLimits` | `QueryRuntimeAnswerLimits` |

### Native CLI bridge

Direct native callers should replace `type_bridge_core.run_v2_cli` with
`type_bridge_core.run_cli`. Console command arguments and exit codes are unchanged.
