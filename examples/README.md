# Generated-only examples

These examples start with one Split-YAML workspace, generate application
bindings, and use those bindings for data and queries. No Python or Node class
is used as schema authority.

## Generate the bindings

From this directory:

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml migration make --name initial
type-bridge --manifest typebridge.yaml migration apply --environment development
type-bridge --manifest typebridge.yaml schema generate
```

The manifest emits Python, TypeScript, and Rust packages below `generated/`.
For the Python examples, expose the generated package and run any journey:

```bash
export PYTHONPATH="$PWD/examples/generated/python${PYTHONPATH:+:$PYTHONPATH}"
uv run python examples/basic/crud.py
uv run python examples/basic/crud_02_insert.py
uv run python examples/basic/crud_03_read.py
uv run python examples/basic/crud_04_update.py
uv run python examples/basic/crud_05_filter.py
uv run python examples/basic/crud_06_aggregate.py
uv run python examples/basic/crud_07_delete.py
uv run python examples/basic/crud_08_put.py
uv run python examples/advanced/crud_07_chainable_operations.py
uv run python examples/advanced/query_01_expressions.py
```

All data values, model classes, managers, fields, and roles in those scripts
come from `app_models`, the generated package. Connection primitives come from
`type_bridge`.

## Where schema concepts live

- [`schema/application.yaml`](schema/application.yaml) demonstrates scalar
  constraints, abstract types, inheritance, optional and multivalue ownership,
  relations, role cardinality, and `plays` facts.
- [`typebridge.yaml`](typebridge.yaml) is the sole active schema, migration, and
  binding configuration.
- `schema check` reports reserved labels, invalid cardinality, ownership, and
  role-player errors before generation.

Change Split-YAML, review and apply a V2 migration, then regenerate. Do not edit
the generated packages or recreate their descriptors in application code.

## Historical TOML

[`advanced/toml_migration`](advanced/toml_migration/) and
[`basic/schema.toml`](basic/schema.toml) are frozen recovery inputs. They may be
read by the retained TOML-to-TypeQL converter; they are not active schema or
model-generation examples.
