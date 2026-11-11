# Logging

TypeBridge leaves application logging configuration to the host. Native plan
and IO events use Rust `tracing`, forwarded to Python `logging`, a Node listener
or a C database callback. Rust hosts use their subscriber. See
[Debugging plans](debugging.md) for explain, event fields and `RUST_LOG`.

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
├── type_bridge.plan          native plan construction and execution
├── type_bridge.io            native provider and server IO
├── type_bridge.session       connection and transaction facade
└── type_bridge.migration     canonical workspace migration CLI
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
