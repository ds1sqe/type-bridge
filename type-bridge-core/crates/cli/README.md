# type-bridge-cli

The `type-bridge` command-line interface for split-YAML workspaces, generated
bindings, and canonical schema migrations. Schema checking, generation, and
migration authoring/planning are offline; apply, verify, rollback, adopt, and
upgrade-profile connect only through an explicitly selected workspace
environment.

## Install and start

```bash
cargo install type-bridge-cli --version 2.3.0 --locked
type-bridge --help
type-bridge schema check
type-bridge schema generate
```

Run commands from a workspace containing `typebridge.yaml`, or pass
`--manifest <path>`. Keep credentials as symbolic environment references and
enable migration access explicitly in the chosen environment. The same entry
point is available to Rust integrators as `type_bridge_cli::run_cli`.

Binding-neutral attribute backfills are authored from a closed YAML intent
stored directly in the configured migration directory:

```yaml
format: typebridge.migration-backfill-intent/v1
copy-attribute:
  owner-kind: entity
  owner: person
  source: legacy-name
  destination: display-name
  partition-key: person-id
  batch-rows: 128
  reverse: remove-equal-copied-destination
```

After the schema history reaches the current Split-YAML state, publish the
reviewed intent with:

```bash
type-bridge migration make --name copy-name \
  --backfill-intent copy-name.backfill.yaml
```

The intent filename must be a regular direct child of the migration directory.
The historical owner must have the partition attribute as an effective key,
and the committed head must still equal the current schema sources. The CLI
serializes discovery, parsing, generation, and no-overwrite publication under
one directory lock.

The crate has no optional features. It is released in lockstep with TypeBridge
2.3.0 and requires Rust 1.88+. Offline schema checking, generation, migration
authoring, and planning accept the frozen TypeDB 3.11.5, 3.12.1, and 3.13.6
semantic profiles. Connected migration commands run on exactly two profile/server
pairs: `typedb-3.12.1/v1` with TypeDB 3.12.3 and `typedb-3.13.6/v1` with
TypeDB 3.13.6. `migration upgrade-profile` moves a history from the first
to the second.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-cli/2.3.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
