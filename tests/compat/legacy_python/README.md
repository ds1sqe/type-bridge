# Legacy Python artifact compatibility

This consumer freezes the pre-#170 Python query surfaces from an installed
artifact. It never connects to TypeDB: manager terminals run against recording
objects and assert the exact specs passed to the existing Rust-backed query.

Run against prebuilt root/native wheels (dependencies may be resolved normally):

```bash
python scripts/ci/run_legacy_python_compat.py \
  --artifact dist/type_bridge-1.5.10-py3-none-any.whl \
  --artifact dist/type_bridge_core-1.5.10-*.whl
```

For an offline wheelhouse, add `--wheelhouse PATH --no-index`. To test an
already prepared environment without building or installing anything:

```bash
python scripts/ci/run_legacy_python_compat.py --python /path/to/venv/bin/python
```

The runner copies `probe.py` to a temporary directory outside the checkout,
removes ambient Python import overrides, and invokes the consumer interpreter
with `-I`. The probe also inspects `sys.path`, every loaded TypeBridge module,
and installed distribution roots; any path resolving into the source checkout
fails the run.

Pinned behavior includes:

- package-root raw `Query`/`QueryBuilder`, in-place mutation, and exact TypeQL;
- ignored-return `RustTypeDBQuery` mutation, Django-style `__lookup` filters,
  string ordering, and recording-backed read, cardinality, aggregate/group,
  update, and delete terminals;
- abstract-base manager hydration into concrete entity subtypes, including
  inherited and subtype-only fields;
- Pydantic class-level field/role references versus instance values; and
- unchanged package-root raw `Query` identity alongside the separately shipped
  `type_bridge.typed` facade.
