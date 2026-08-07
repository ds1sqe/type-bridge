# Schema and migration workflows

TypeBridge separates desired schema, migration history, and generated
application projections. For new V2 systems, a versioned Split-YAML workspace
is the canonical authoring authority.

## Recommended V2 flow

1. Author a [Split-YAML schema set and workspace](split-yaml-v1.md).
2. Validate it offline with `type-bridge … schema check`.
3. [Generate](generator.md) the configured Python, TypeScript, and Rust
   projections plus the generic-server authority artifact from one snapshot.
4. Create and review a [migration](migrations.md).
5. Apply the migration only in an environment that permits it.
6. Deploy application and server artifacts bound to the resulting schema
   fingerprint.

Split-YAML is the only active authoring path. Existing TOML and historical
migration material is handled only by the documented read-only conversion and
recovery workflows. Generated files are projections, not alternate schema
authorities: regenerate them after an accepted schema change and never edit or
subclass their runtime bases.

Generated packages embed compiled authority for their normal remote sessions.
The separately emitted `typebridge.schema-authority/v1` JSON is a source-free
generic-server deployment codec, not a second authoring format. Never edit it;
regenerate it with every package after changing Split-YAML.

## API boundary projections

Application-boundary DTOs should be defined by the application around generated
models. Generated Python, TypeScript, and Rust packages retain language-specific
types while sharing canonical field, role, and schema identities.
