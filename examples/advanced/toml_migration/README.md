# Frozen TOML recovery inputs

`schema.toml` and `schema.v2.toml` are immutable examples for the retained
read-only TOML-to-TypeQL converter. They are not active desired-schema,
migration-authoring, or model-generation inputs in TypeBridge 2.1.

Convert a copy without changing either fixture:

```python
from pathlib import Path

from type_bridge.migration import toml_to_typeql

source = Path("schema.toml").read_text(encoding="utf-8")
print(toml_to_typeql(source))
```

For an application change, express the target schema in Split-YAML, use the V2
`migration make/plan/apply/verify` workflow, and generate bindings through
`type-bridge schema generate`. See the [TOML recovery
guide](../../../docs/guide/toml.md).
