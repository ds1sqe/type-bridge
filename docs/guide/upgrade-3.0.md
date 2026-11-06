# Upgrading to 3.0

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
