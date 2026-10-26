# Logging

TypeBridge does not configure application logging. Python uses the standard
`logging` hierarchy; Rust and Node surface structured errors and diagnostics at
their public boundaries.

## Python

Enable all facade logging:

```python
import logging

logging.basicConfig(level=logging.DEBUG)
logging.getLogger("type_bridge").setLevel(logging.DEBUG)
```

Useful retained namespaces include:

```text
type_bridge
├── type_bridge.session       connection and transaction facade
├── type_bridge.query         retained raw query facade
├── type_bridge.migration     V2 CLI and read-only archive recovery
└── type_bridge.proxy         remote proxy boundary
```

Generated package managers execute in the native Rust engine. Their failures
carry canonical operation/model/field or role diagnostics; they do not depend
on a public Python manager logger.

During tests:

```bash
uv run pytest -vv -s --log-cli-level=DEBUG
```

## Native diagnostics

Use a Rust backtrace for a focused source-tree failure:

```bash
RUST_BACKTRACE=1 uv run pytest tests/integration/schema/test_generated_projection_live.py -vv -s
```

Application validation errors are stable public diagnostics and should be
handled normally. A Rust panic/backtrace is an internal defect and should be
reported with the smallest reproducible generated workspace and operation.

## Sensitive data

Do not log passwords, tokens, credential-provider output, complete TLS private
material, or unbounded query/result bytes. Projection fingerprints, schema
labels, operation names, bounded diagnostics, TypeDB versions, and request
identities are suitable for troubleshooting.


## Native plan and IO events

The shared Rust engine emits `type_bridge.plan` and `type_bridge.io` tracing
records containing codes, counts, schema labels and fingerprints. Python
forwards them through standard `logging`; Node exposes `setDebugListener`;
Rust uses the host subscriber; C exposes a database-scoped callback.
When no listener is registered, `RUST_LOG=type_bridge=debug` enables the stderr
fallback. Statement text appears only at `trace`; transported given values
never appear. Python and Node use bounded asynchronous queues, so a slow
logger can lose debug records without blocking transactions.

Python forwards Rust TRACE at numeric logging level 5. Private `perf-stages`
builds keep subscriber ownership with their stage recorder.
