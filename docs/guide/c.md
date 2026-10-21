# C SDK

**Security update:** Upgrade to TypeBridge 2.2.5. Its prebuilt distributions
use rustls 0.23.46, which fixes RUSTSEC-2026-0346 through RUSTSEC-2026-0349
and the earlier
[RUSTSEC-2026-0285](https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc).
The [2.2.1 post-release notice](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.1)
records the affected earlier release.

TypeBridge 2.2.5 provides a generated C SDK with native ABI 1.6.0 for
**Ubuntu 24.04, x86_64 GNU/Linux, shared runtime**. C17 and C++17 applications
consume generated schema packages through CMake or pkg-config. The public
release was independently verified on 2026-10-15.

The SDK uses the same Rust-owned schema, CRUD, query, migration, serialization,
cancellation, diagnostics, and resource-limit contracts as the other SDKs.
Connected acceptance uses exactly TypeDB 3.12.3; the canonical semantic profile
is `typedb-3.12.1/v1`. The selected C distribution does not cover static linking,
macOS, Windows, or other architectures.

## Install the runtime

Download the runtime archive and its Sigstore bundle from the
[2.2.5 release](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.5):

- `type-bridge-c-runtime-2.2.5-abi-1.6-x86_64-unknown-linux-gnu.tar.gz`
- `type-bridge-cli-2.2.5-x86_64-unknown-linux-gnu.tar.gz` for standalone generation
  and migration commands
- `type-bridge-c-sdk-example-2.2.5.tar.gz` for the verified `tb_sdkv3` example

Verify the signed release files using the
[documented signing identity and provenance](../development/c-release.md#protected-promotion).
Extract the runtime into a private installation prefix. It contains the public
header, `libtype_bridge_c.so`, CMake configuration, and pkg-config metadata.
The standalone CLI archive has its own `bin/type-bridge` executable.

Application execution needs the shared runtime and its system-library
dependencies. Building an application additionally needs a C17 or C++17
compiler and CMake 3.20+ or pkg-config. Python and Cargo are not application
runtime dependencies.

## Generate an application schema package

Add a C output to your Split-YAML workspace:

```yaml
bindings:
  c:
    output: generated/c
```

Then run:

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
```

Distribute the complete generated package with your application. It contains
typed declarations, generated C source, embedded schema authority, package
identity, and build metadata. Keep the physical schema in Split-YAML and
regenerate rather than editing emitted declarations or runtime resources.

## Link an application

For the released example, CMake discovers both the runtime and generated
schema package:

```cmake
cmake_minimum_required(VERSION 3.20)
project(example LANGUAGES C CXX)
find_package(TypeBridge 1.6.0 EXACT CONFIG REQUIRED)
find_package(tb_sdkv3 1.0.0 EXACT CONFIG REQUIRED)
add_executable(example main.c)
set_target_properties(example PROPERTIES C_STANDARD 17 C_STANDARD_REQUIRED YES)
target_link_libraries(example PRIVATE tb_sdkv3::schema)
```

Set `TypeBridge_DIR` to the runtime's `lib/cmake/TypeBridge` directory and
`tb_sdkv3_DIR` to the example package's `lib/cmake/tb_sdkv3` directory. Include
`<tb_sdkv3/tb_sdkv3.h>` from the application. Use the package name emitted for
your own schema when replacing the example.

For pkg-config, add both packages' `lib/pkgconfig` directories to
`PKG_CONFIG_PATH`. Compile the source returned by
`pkg-config --variable=generated_source tb_sdkv3` together with your application,
using `pkg-config --cflags --libs tb_sdkv3`. Configure the dynamic loader to find
the installed runtime's `lib` directory.

## Run a query on a database or a TypeBridge server

A query session authors terminals; it holds no connection. Each connection
type has one execute call, and all of them return the same result family and
diagnostics:

| Connection | Execute call |
| --- | --- |
| `type_bridge_database_t` | `type_bridge_database_query_execute_v1` |
| `type_bridge_read_transaction_t` | `type_bridge_read_transaction_query_execute_v1` |
| `type_bridge_proxy_connection_t` | `type_bridge_proxy_query_execute_v1` |

So one program runs unchanged against TypeDB directly or through a
TypeBridge server; only the connection it opens differs. The proxy
connection, `type_bridge_query_session_open_with_limits_v1`, and their types
are ABI 1.7; the ABI 1.6 header of 2.2.6 does not declare them. The proxy connection
is synchronous and blocks the calling thread:

```c
type_bridge_proxy_header_v1_t auth = {
    {(const uint8_t *)"Authorization", 13},
    {(const uint8_t *)token, token_length},
};
type_bridge_proxy_connection_options_v1_t options = {0};
options.struct_size = sizeof options;
options.version = TYPE_BRIDGE_PROXY_CONNECTION_OPTIONS_VERSION;
options.endpoint = (type_bridge_byte_view_t){(const uint8_t *)url, strlen(url)};
options.tls_mode = TYPE_BRIDGE_TLS_NATIVE_ROOTS; /* or _DISABLED, _CUSTOM_ROOT_CA */
options.headers = &auth;
options.header_count = 1;
options.answer_limits = &ceiling; /* NULL selects the proxy defaults */

type_bridge_proxy_connection_t *proxy = NULL;
type_bridge_execution_diagnostics_t *diagnostics = NULL;
status = type_bridge_proxy_connection_open_v1(
    package, &options, NULL, &proxy, &diagnostics);
/* ... */
status = type_bridge_proxy_query_execute_v1(
    proxy, terminal, kind, NULL, NULL, &result, &diagnostics);
/* ... */
type_bridge_proxy_connection_close(&proxy);
```

With TLS mode `TYPE_BRIDGE_TLS_CUSTOM_ROOT_CA`, `custom_root_ca_path` names a
PEM bundle. An empty `pinned_advertisement` fetches the server's capability
advertisement during open, bounded by `connection_limits`; a nonempty one is
used as is. `answer_limits` is a ceiling: every terminal on the connection
runs under its own limits constrained by it, and limit values are kept or
refused, never clamped. Manager terminals run only on a database or a read
transaction.

ABI 1.6 rollback consumes the write transaction and clears its owner slot,
just like commit and close. Keep-open rollback requires the later ABI 1.7 API.

### Caller-owned exchange

A host that owns the route to the server (its own HTTP stack, a message bus,
or an in-process server) uses the remote context instead of the built-in
proxy connection. It is the exchange extension point, not a second session:
`type_bridge_query_remote_context_open_v1` validates the advertisement,
`type_bridge_query_remote_prepare_v1` returns a pending request whose bytes
(`type_bridge_query_remote_pending_request_bytes`) the host sends, and the
reply goes through the one-shot `type_bridge_query_remote_pending_claim` and
`type_bridge_query_remote_claim_decode_v1`. Both paths prepare and validate
requests through the same Rust proxy connection.

The release evidence covers relocated installation, strict C17/C++17 headers,
direct and caller-transport queries, custom-root TLS, migrations, model
serialization, cancellation, resource limits, and cleanup. See the
[release procedure](../development/c-release.md#public-verification-and-support)
for the public verification record and exact support boundary.

Provider commit status is selected from the typed commit outcome. ABI 1.6
continues to return `TYPE_BRIDGE_STATUS_EXECUTION_FAILED` for proven rejections
and `TYPE_BRIDGE_STATUS_COMMIT_OUTCOME_UNKNOWN` when a commit may have applied.
Rejection diagnostics keep the released `commit_definitely_aborted` and
`provider_operation_failed` codes, paths and details. The typed failure
details other SDKs expose (`retry_safe`, `code`, `rejection`, `lock_kind`,
`annotation`) belong to ABI 1.7, rather than changing what ABI 1.6 consumers
count and read; see [Typed failures](#typed-failures).

## Stage plans in one transaction (ABI 1.7)

ABI 1.7 adds deferred transactions over `query-plan/v3` plans. A plan comes
from the plan builder or from canonical plan bytes
(`type_bridge_plan_v3_decode`); C never serializes plans itself. Staging sends
nothing: `type_bridge_staged_transaction_commit` runs every staged plan in one
provider transaction, and each pending answer is read after the commit.

### Build a plan

The builder forwards to the one core builder every SDK uses, so the same
program gives the same plan bytes in C, Rust, Python and TypeScript.
Identities, values and expressions are registered once and named by `uint32_t`
ids, and operands are `(kind, id)` pairs. The one-query purchase (check stock
and balance, take one item and the price, record the purchase):

```c
type_bridge_plan_builder_t *b = NULL;
uint32_t item_t, account_t, purchase_t, item_key, account_key, stock_f, price_f,
    balance_f, order_f, buyer_r, item_r, item, account, order_in, item_in,
    account_in, key, akey, stock, price, balance, zero, one, e, next_stock,
    next_balance, purchase, row[3];

type_bridge_plan_builder_new(&b);
type_bridge_plan_builder_type(b, TYPE_BRIDGE_PLAN_THING_ENTITY, text("item"), &item_t);
type_bridge_plan_builder_type(b, TYPE_BRIDGE_PLAN_THING_ENTITY, text("account"), &account_t);
type_bridge_plan_builder_type(b, TYPE_BRIDGE_PLAN_THING_RELATION, text("purchase"), &purchase_t);
type_bridge_plan_builder_owns(b, item_t, text("item-id"), &item_key);
type_bridge_plan_builder_owns(b, account_t, text("account-id"), &account_key);
type_bridge_plan_builder_owns(b, item_t, text("stock"), &stock_f);
type_bridge_plan_builder_owns(b, item_t, text("price"), &price_f);
type_bridge_plan_builder_owns(b, account_t, text("balance"), &balance_f);
type_bridge_plan_builder_owns(b, purchase_t, text("order-id"), &order_f);
type_bridge_plan_builder_role(b, purchase_t, text("buyer"), &buyer_r);
type_bridge_plan_builder_role(b, purchase_t, text("item"), &item_r);

/* Match, in call order. */
type_bridge_plan_builder_exact(b, item_t, &item);
type_bridge_plan_builder_exact(b, account_t, &account);
type_bridge_plan_builder_input(b, text("order_id"), TYPE_BRIDGE_PROJECTED_VALUE_STRING, 0, &order_in);
type_bridge_plan_builder_input(b, text("item_id"), TYPE_BRIDGE_PROJECTED_VALUE_STRING, 0, &item_in);
type_bridge_plan_builder_input(b, text("account_id"), TYPE_BRIDGE_PROJECTED_VALUE_STRING, 0, &account_in);
type_bridge_plan_builder_field(b, item, item_key, &key);
type_bridge_plan_builder_field(b, account, account_key, &akey);
type_bridge_plan_builder_field(b, item, stock_f, &stock);
type_bridge_plan_builder_field(b, item, price_f, &price);
type_bridge_plan_builder_field(b, account, balance_f, &balance);
type_bridge_plan_builder_value_long(b, 0, &zero);
type_bridge_plan_builder_value_long(b, 1, &one);
type_bridge_plan_builder_compare(b, TYPE_BRIDGE_PLAN_OPERAND_BINDING, key,
    TYPE_BRIDGE_PLAN_COMPARE_EQUAL, TYPE_BRIDGE_PLAN_OPERAND_INPUT, item_in);
type_bridge_plan_builder_compare(b, TYPE_BRIDGE_PLAN_OPERAND_BINDING, akey,
    TYPE_BRIDGE_PLAN_COMPARE_EQUAL, TYPE_BRIDGE_PLAN_OPERAND_INPUT, account_in);
type_bridge_plan_builder_compare(b, TYPE_BRIDGE_PLAN_OPERAND_BINDING, stock,
    TYPE_BRIDGE_PLAN_COMPARE_GREATER, TYPE_BRIDGE_PLAN_OPERAND_VALUE, zero);
type_bridge_plan_builder_compare(b, TYPE_BRIDGE_PLAN_OPERAND_BINDING, balance,
    TYPE_BRIDGE_PLAN_COMPARE_GREATER_OR_EQUAL, TYPE_BRIDGE_PLAN_OPERAND_BINDING, price);
type_bridge_plan_builder_expr_arith(b, TYPE_BRIDGE_PLAN_ARITH_SUB,
    TYPE_BRIDGE_PLAN_OPERAND_BINDING, stock, TYPE_BRIDGE_PLAN_OPERAND_VALUE, one, &e);
type_bridge_plan_builder_let(b, text("next_stock"), TYPE_BRIDGE_PLAN_OPERAND_EXPR, e, &next_stock);
type_bridge_plan_builder_expr_arith(b, TYPE_BRIDGE_PLAN_ARITH_SUB,
    TYPE_BRIDGE_PLAN_OPERAND_BINDING, balance, TYPE_BRIDGE_PLAN_OPERAND_BINDING, price, &e);
type_bridge_plan_builder_let(b, text("next_balance"), TYPE_BRIDGE_PLAN_OPERAND_EXPR, e, &next_balance);

/* Writes: both updates share one stage, then the insert. */
type_bridge_plan_builder_update(b, item, stock_f, TYPE_BRIDGE_PLAN_OPERAND_BINDING, next_stock);
type_bridge_plan_builder_update(b, account, balance_f, TYPE_BRIDGE_PLAN_OPERAND_BINDING, next_balance);
type_bridge_plan_builder_insert(b, purchase_t, &purchase);
type_bridge_plan_builder_thing_field(b, purchase, order_f, TYPE_BRIDGE_PLAN_OPERAND_INPUT, order_in);
type_bridge_plan_builder_thing_player(b, purchase, buyer_r, account);
type_bridge_plan_builder_thing_player(b, purchase, item_r, item);

/* One input row, one value id per input column. */
type_bridge_plan_builder_value_string(b, text("O1"), &row[0]);
type_bridge_plan_builder_value_string(b, text("I1"), &row[1]);
type_bridge_plan_builder_value_string(b, text("A1"), &row[2]);
type_bridge_plan_builder_row(b, row, 3);

status = type_bridge_plan_builder_build(b, package, &plan, &diagnostics);
type_bridge_plan_builder_free(&b);
```

`text` stands for a helper returning a `type_bridge_byte_view_t` over a
string. The rules every SDK builder shares:

- Binding ids are dense in creation order. `exact`, `subtypes`, `field`,
  `links`, `compare` and `let` add to one leading match in call order.
- Writes append stages in call order; consecutive `update`, `insert`, `put`
  or `delete*` calls share a stage.
- Variable names come from labels (`item`, `item_stock`, then `_2`, `_3`, ...);
  `input` and `let` names are explicit and must be fresh.
- A plan with writes answers `TYPE_BRIDGE_PLAN_ANSWER_MATCHED`, or `ROWS`
  after `returning`; a read answers `ROWS`, `COUNT` (`count`) or `EXISTS`
  (`exists`).

Values use the C value conventions: UTF-8 strings, `int64_t`, exact IEEE-754
double bits, Booleans as exactly 0 or 1, and canonical text for date,
datetime, datetime-tz, decimal and duration. Builder calls never fail on
content. `type_bridge_plan_builder_build` reports the first invalid label,
constant, value or id, else the first invalid builder call, through its
diagnostics; only a broken pointer contract returns
`TYPE_BRIDGE_STATUS_INVALID_ARGUMENT` from the call itself. `build` validates
against the schema package's compiled authority and leaves the builder usable.

### Stage and commit

```c
type_bridge_staged_transaction_t *tx = NULL;
type_bridge_pending_t *pending = NULL;
type_bridge_plan_answer_kind_t kind;
uint64_t matched = 0;

type_bridge_database_transaction(database, TYPE_BRIDGE_TRANSACTION_WRITE, &tx, &diagnostics);
type_bridge_staged_transaction_stage(tx, plan, &pending, &diagnostics);
status = type_bridge_staged_transaction_commit(tx, &diagnostics);
if (status == TYPE_BRIDGE_STATUS_OK)
    type_bridge_pending_answer(pending, &kind, &matched); /* MATCHED, 1 or 0 */
type_bridge_staged_transaction_close(&tx); /* the only release */
type_bridge_pending_free(&pending);
type_bridge_plan_free(&plan);
```

The handle is a reusable logical transaction, as in the other SDKs: commit and
`type_bridge_staged_transaction_rollback(tx)` keep it usable, and
`type_bridge_staged_transaction_close(&tx)` is the only release. After a
commit, successful or a typed failure, the handle stages new work; rollback
discards the staged plans. After
`TYPE_BRIDGE_STATUS_COMMIT_OUTCOME_UNKNOWN`, direct or over the proxy, the
writes may have applied: the caller reads the database to find out, and the
handle stages new work. The open handle keeps its database busy until close. Reading an answer
before commit returns `TYPE_BRIDGE_STATUS_IN_USE`; after a failed commit,
rollback or close it returns `TYPE_BRIDGE_STATUS_EXECUTION_FAILED`.

`type_bridge_proxy_connection_transaction(proxy, kind, &tx, &diagnostics)`
opens the same transaction on a proxy connection. It fetches the server's
staged advertisement at open; commit is one signed exchange that carries every
staged plan. The transaction may outlive the connection.

### Typed failures

`type_bridge_execution_diagnostics_failure_get_v1` reads the typed provider
failure of one diagnostic into `type_bridge_execution_failure_v1_t`: outcome
(rejected or indeterminate), phase, reason (conflict, cardinality, unique,
constraint, other), lock kind for a conflict, annotation for a constraint,
`retry_safe`, the staged `op_index` and the provider code. A diagnostic
without a typed failure reports `TYPE_BRIDGE_FAILURE_OUTCOME_NONE`. Two
purchases racing for the last item end with one commit and one rejected
commit whose failure is a retry-safe `STC2` conflict with
`TYPE_BRIDGE_FAILURE_LOCK_EXCLUSIVE`; see [Concurrency](concurrency.md).

### Require ABI 1.7

The 1.7 runtime serves 1.6 consumers unchanged: CMake accepts
`find_package(TypeBridge 1.6.0 EXACT)` and pkg-config keeps
`type-bridge = 1.6.0` resolving. Applications that use the 1.7 surface
require it explicitly:

```cmake
find_package(TypeBridge 1.7 CONFIG REQUIRED)
```

```text
Requires: type-bridge-abi >= 1.7.0
```

The runtime's pkg-config file keeps `Version: 1.6.0` and provides
`type-bridge-abi` at the native ABI version (pkgconf, the Ubuntu 24.04
`pkg-config`, resolves provided modules). At run time,
`type_bridge_c_abi_minor()` reports the loaded runtime's minor.
