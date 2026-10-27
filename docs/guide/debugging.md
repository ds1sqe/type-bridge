# Debugging plans

A built plan's `explain()` shows the TypeQL statements execution sends, in
execution order, without opening a database transaction. It uses the execution
lowerer and renderer, so unsupported programs and resource limits produce the
same diagnostics as staging. Plans with declared inputs must have bound rows.

Caller values travel as `given` rows and are never shown. Embedded plan
constants are part of the plan and are shown, just as they appear in canonical
plan bytes. Keep sensitive values in input rows rather than embedded literals.
TypeDB requires literal regexes for `starts_with`, `ends_with` and `like`.
Explain and TRACE replace input-derived regexes and their correlation guards
with `<input:column>` placeholders; explicit plan constants remain visible.
The explanation includes only the row count and canonical byte count of the
transported rows, including internal snapshot inputs.

## Inspect the purchase program

After building the purchase plan from generated model tokens:

=== "Python"

    ```python
    import json
    from dataclasses import asdict

    explanation = plan.explain()
    print(json.dumps(asdict(explanation), sort_keys=True, separators=(",", ":")))
    print(explanation.statements[0])
    ```

=== "TypeScript"

    ```typescript
    const explanation = plan.explain();
    console.log(explanation);
    console.log(explanation.statements[0]);
    ```

=== "Rust"

    ```rust
    let explanation = plan.explain()?;
    println!("{}", String::from_utf8(explanation.canonical_bytes()?)?);
    println!("{}", explanation.statements[0]);
    ```

=== "C"

    ```c
    type_bridge_canonical_bytes_t *json = NULL;
    type_bridge_execution_diagnostics_t *diagnostics = NULL;
    type_bridge_byte_view_t view = {NULL, 0};
    if (type_bridge_plan_explain(plan, &json, &diagnostics) == TYPE_BRIDGE_STATUS_OK) {
      if (type_bridge_canonical_bytes_view(json, &view) == TYPE_BRIDGE_STATUS_OK)
        fwrite(view.data, 1, view.length, stdout);
      type_bridge_canonical_bytes_close(&json);
    }
    if (diagnostics != NULL)
      type_bridge_execution_diagnostics_close(&diagnostics);
    ```

C explain requires ABI 1.9. The four purchase conformance programs emit the
same explanation through these APIs. Their conformance report is compared
exactly against a fixture for the suite's semantic profile. The core fixture
uses a separate purchase plan with its own binding and input names.

`statements` contains complete pipelines, rather than individual match patterns
or write stages. A normal read or write executes one statement. An opt-in
delete snapshot executes a capture read followed by the delete, in one
transaction. `read_statements` and `write_statements` count those provider
statements; compare them with the staged request's statement budgets.

The fingerprint identifies the canonical plan shape with `given_rows` omitted,
under the separate `typebridge.query.plan-shape` domain. Plans differing only
in caller values have the same explanation fingerprint. The real plan
fingerprint still includes those values and is used only for invocation identity
and verification; it is not logged. Bindings list each variable's schema-derived
possible types, including admitted subtypes; scalar bindings have an empty
list. `required_capabilities` lists what the receiving executor must advertise.
See the [complete explanation shape](../reference/query-plan-v3.md#explaining-a-plan).

## Send traces to the host logger

Python forwards native debug records to `logging` under `type_bridge.plan`
and `type_bridge.io`:

```python
import logging

logging.basicConfig(level=logging.DEBUG)
```

Node forwards records to a listener:

```typescript
import { setDebugListener } from "@type-bridge/node";

setDebugListener((event) => console.debug(event.name, event.fields));
// Clear when finished.
setDebugListener(null);
```

Python and Node deliver logging asynchronously through bounded queues. A slow
logger can lose debug records when its queue fills; logging never blocks a
transaction. Queued records may arrive after an operation returns or a Node listener is
cleared.

Rust applications keep ownership of their `tracing` subscriber. Enable the
`type_bridge` targets through the host's filter. An existing subscriber keeps
ownership when a binding initializes.

C installs a listener on one database handle:

```c
static void TYPE_BRIDGE_CALL debug_event(type_bridge_byte_view_t json, void *context) {
  FILE *stream = context;
  fwrite(json.data, 1, json.length, stream);
  fputc('\n', stream);
}

/* Keep stderr (the context) alive until cleared or the database is closed. */
type_bridge_debug_listener_set(database, debug_event, stderr);
/* ... stage and commit ... */
type_bridge_debug_listener_set(database, NULL, NULL);
```

A C callback receives only that database's stage, commit, provider and answer
events. The JSON bytes are borrowed until it returns. It must not retain their
pointer, unwind, or reenter TypeBridge. Schema-only build events have no
database identity and use the tracing subscriber or `RUST_LOG` fallback.

When no host listener is registered, bindings can install a stderr subscriber:

```bash
RUST_LOG=type_bridge=debug your-application
```

The server uses its existing tracing subscriber. Set the same filter in the
server environment to observe its IO events. To include the exact canonical
TypeQL statement text, select `RUST_LOG=type_bridge=trace`. Transported `given`
values remain absent at every level; embedded plan constants remain visible.
Python forwards Rust TRACE at numeric logging level 5; enable level 5 on the
host logger as well when you want statement text there. Private `perf-stages`
Python builds keep subscriber ownership with their stage recorder.

## Follow an execution

| Name | Useful metadata |
| --- | --- |
| `type_bridge.plan.build` | Origin (`builder`), plan-shape fingerprint, stage count, write flag |
| `type_bridge.plan.lower` | Plan-shape fingerprint, stage count, write flag |
| `type_bridge.plan.stage` | Plan-shape fingerprint and execution context |
| `type_bridge.plan.commit` | Plan count, outcome, duration; proxy request nonce |
| `type_bridge.plan.answer` | Plan-shape fingerprint, item and byte metering, outcome |
| `type_bridge.plan.hydrate` | Concrete type label, generated model name, counts, refusal code |
| `type_bridge.io.request` | Bytes in, decode outcome, nonce, kind, plan count, remaining deadline |
| `type_bridge.io.provider` | Statement index, write flag, bytes sent, items and bytes received, duration |
| `type_bridge.io.reply` | Bytes out, outcome, signing key identity, duration |

Use the plan-shape fingerprint to connect construction, staging and answer events.
On the server, the nonce identifies the enclosing staged stack; child
plan/provider events carry its context. Request payload digests are not logged.
A commit can contain several plans, so its plan count is separate from each
plan's stage count.

Proxy answer events report canonical `wire_bytes`; provider answer events
report semantic `bytes` from their per-item meter. These measure different
representations.

Hydration logs schema labels and generated model names, not attributes or object
contents. Refusal logs carry stable codes. Provider profiling is separate from
this surface: explain does not execute a query or return a TypeDB optimizer
cost estimate.
