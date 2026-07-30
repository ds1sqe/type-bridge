# Schema and migration workflows

TypeBridge separates desired schema, migration history, and generated
application projections. For new V2 systems, a versioned Split-YAML workspace
is the canonical authoring authority.

## Recommended V2 flow

1. Author a [Split-YAML schema set and workspace](split-yaml-v1.md).
2. Validate it offline with `type-bridge … schema check`.
3. Create and review a [migration](migrations.md).
4. Apply the migration only in an environment that permits it.
5. [Generate](generator.md) configured Python, TypeScript, and Rust
   projections.
6. Deploy application and server artifacts bound to the resulting schema
   fingerprint.

## Compatibility authoring paths

- [Python model-driven schema management](schema.md) remains available in the
  2.0 compatibility line.
- Existing TypeQL files can generate application models.
- Direct [TOML schema authoring](toml.md) is deprecated for removal in 2.1;
  translate it to Split-YAML before upgrading.

Generated files are projections, not alternate schema authorities. Regenerate
them after an accepted schema change; do not hand-edit them.

## API boundary projections

[API DTO generation](dto.md) creates Pydantic request and response shapes from
the model contract. Generated TypeScript and Rust packages retain language
specific types while sharing canonical field, role, and schema identities.
