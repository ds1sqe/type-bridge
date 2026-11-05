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
