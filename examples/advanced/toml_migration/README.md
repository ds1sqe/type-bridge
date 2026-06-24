# TOML Schema Migrations

This example shows the Django-style workflow for a `schema.toml` project:

1. Edit `schema.toml`.
2. Generate model bindings.
3. Run `makemigrations`.
4. Run `migrate`.
5. Edit `schema.toml` again and repeat.

Run these commands from this directory.

```bash
python -m type_bridge.generator schema.toml -o generated_models

python -m type_bridge.migration makemigrations \
  --models generated_models \
  --migrations-dir migrations \
  --name initial \
  --database support

python -m type_bridge.migration migrate \
  --migrations-dir migrations \
  --database support
```

The generated `0001_initial.py` uses typed operations imported from the
historical snapshot for that migration:

```python
from migrations.snapshots.v0001 import Customer, CustomerName

operations = [
    ops.AddAttribute(CustomerName),
    ops.AddEntity(Customer),
]
```

The JSON sidecar beside the `.py` file keeps the same typed Rust
`OperationSpec` shape, for example `add_attribute` and `add_entity`. The
snapshot import matters because `generated_models` is the current application
API, while `migrations.snapshots.v0001` is the schema state after migration
`0001`.

For the next change, copy `schema.v2.toml` over `schema.toml`, regenerate
bindings, and make the second migration:

```bash
cp schema.v2.toml schema.toml
python -m type_bridge.generator schema.toml -o generated_models

python -m type_bridge.migration makemigrations \
  --models generated_models \
  --migrations-dir migrations \
  --name add_email \
  --database support

python -m type_bridge.migration migrate \
  --migrations-dir migrations \
  --database support
```

The generated `0002_add_email.py` should use typed operations from the next
snapshot for the delta:

```python
from migrations.snapshots.v0002 import Customer, Email

operations = [
    ops.AddAttribute(Email),
    ops.AddOwnership(Customer, Email, optional=True),
]
```

`ops.RunTypeQL` is still supported for hand-authored escape hatches, but it is
not the default output for schema diffs. Existing files in `--migrations-dir`
set the next number and dependency; the current diff source is the live TypeDB
schema after the previous migrations have been applied. See
`docs/guide/migrations.md` for the full workflow, including why snapshots are
required for replayable migration history and how to use `ops.RunPython` for
ORM-backed data migrations.
