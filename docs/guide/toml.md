# TOML recovery and conversion

TOML is not an active TypeBridge schema or model-authoring format. The retained
surface is a frozen, read-only conversion from historical TOML bytes to TypeQL
so an existing system can be recovered into a canonical Split-YAML workspace.

## Convert a frozen input

```python
from pathlib import Path

from type_bridge_core import toml_to_typeql

source = Path("historical-schema.toml").read_text(encoding="utf-8")
typeql = toml_to_typeql(source)
Path("historical-schema.typeql").write_text(typeql, encoding="utf-8")
```

The converter does not connect to TypeDB, generate application models, write a
workspace, or become desired-schema authority. Preserve the original bytes and
review the emitted TypeQL.

## Move to Split-YAML

1. Freeze and checksum the historical TOML input.
2. Convert it with `type_bridge_core.toml_to_typeql` and retain the output for
   review/recovery evidence.
3. Author an equivalent [Split-YAML workspace](split-yaml-v1.md).
4. Run `type-bridge schema check`.
5. Compare the declared schema and migration plan against the recovered TypeQL.
6. Generate Python, TypeScript, and Rust bindings from the workspace only.

Direct `.toml` generator routing and `generate_models(..., format="toml")` are
not retained. New schema changes belong in Split-YAML and canonical V2
migrations.

## Frozen parser contract

The converter remains strict and bounded for recovery. Invalid TOML, unknown
historical shapes, duplicate definitions, unresolved players, or unsupported
values fail without producing partial active authority. Frozen fixtures verify
deterministic output and diagnostics.

See the [upgrade guide](upgrade-v2.md) and
[compatibility inventory](v2-deprecations.md) before removing a pre-cutover pin.
