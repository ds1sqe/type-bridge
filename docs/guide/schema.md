# Schema commands

The canonical schema authority is a Split-YAML workspace. Schema changes flow
through offline checking, reviewed migrations, and regenerated application
bindings.

## Check the declared schema

```bash
type-bridge --manifest typebridge.yaml schema check
```

The command resolves the schema-set, validates the selected semantic profile,
and prints diagnostics without contacting TypeDB.

## Generate bindings

```bash
type-bridge --manifest typebridge.yaml schema generate
```

Generation captures the workspace once, renders every configured Python,
TypeScript, and Rust target, embeds the same compiled authority in each package,
and optionally writes `artifacts.schema-authority.output` when deploying a
generic server. Ordinary generated managers and query sessions need no
standalone authority JSON. Generation does not mutate a database. See
[generation](generator.md).

## Plan and apply schema changes

```bash
type-bridge --manifest typebridge.yaml migration make --name add-person
type-bridge --manifest typebridge.yaml migration plan
type-bridge --manifest typebridge.yaml migration apply --environment development
type-bridge --manifest typebridge.yaml migration verify --environment development
```

Review generated migration authority before applying it. Destructive operations
follow the workspace policy and require explicit approval when configured.

## Schema ownership

Workspace V1 uses `schema.ownership: exclusive` and a bounded `managed-scope`.
The migration ledger, immutable migration files, declared-schema fingerprint,
and generated projections must agree. A configured generic-server deployment
artifact must agree with the same generation snapshot.
Generated model classes are not scanned to reconstruct schema and cannot
register new types at runtime. Canonical JSON is an internal generated codec,
not an authored schema input.

## Existing systems

- Convert historical TOML through the retained read-only `toml_to_typeql`
  interface, then author the resulting canonical Split-YAML workspace.
- Adopt frozen V1 migration history through the one-way archive adoption
  workflow before creating new V2 migrations.
- Use the safe pre-cutover package pin while an application still depends on a
  removed handwritten authoring surface.

See [schema workflows](schema-workflows.md), [Split-YAML](split-yaml-v1.md),
[migrations](migrations.md), and the [compatibility inventory](v2-deprecations.md).
