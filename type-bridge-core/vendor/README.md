# Vendored crates

This directory holds renamed republications of upstream TypeDB crates that
let the band-7 (TypeDB 3.8–3.10), band-8 (TypeDB 3.11), and band-9
(TypeDB 3.12) driver stacks coexist in one build. Cargo unifies
semver-compatible versions of a package, so `typedb-driver` 3.8.1 and
3.12.0 cannot live alongside 3.11.5 (all major 3) under the upstream name;
a renamed copy is outside the unification set. The band-8 line is NOT
vendored — it stays on the upstream crates.io `typedb-driver`.

| Vendored crate | Upstream package | Upstream version | License |
|---|---|---|---|
| `type-bridge-typedb-protocol-b7` | `typedb-protocol` | 3.7.0 | MPL-2.0 |
| `type-bridge-typedb-driver-b7` | `typedb-driver` | 3.8.1 | Apache-2.0 |
| `type-bridge-typedb-protocol-b9` | `typedb-protocol` | 3.12.0 | MPL-2.0 |
| `type-bridge-typedb-driver-b9` | `typedb-driver` | 3.12.0 | Apache-2.0 |

## Provenance

All trees were extracted from the published crates.io packages (the
canonical source — exactly what cargo resolves):

```
https://static.crates.io/crates/typedb-protocol/typedb-protocol-3.7.0.crate
  sha256: 0062374abd0c14afa55e5b1d8e095ac110830da29943ad43f6c6b5d5912a811f
https://static.crates.io/crates/typedb-driver/typedb-driver-3.8.1.crate
  sha256: bf5f617f8d670dd75dc752ae6f42e2bf28ca612ab4feae353c2c89d052adfab0
https://static.crates.io/crates/typedb-protocol/typedb-protocol-3.12.0.crate
  sha256: 01f6b7eb813a853349ff22f385c120c61d04d4648318c92072e7e04dd81cdc3f
https://static.crates.io/crates/typedb-driver/typedb-driver-3.12.0.crate
  sha256: 566a2e346560f2aee266ecf831862a0240d02d64bf824660f601fd14e1a49a51
```

Each crate's upstream `LICENSE` and `README.md` are preserved verbatim.
The protocol crate is MPL-2.0 (file-scoped weak copyleft: redistribution
keeps those files under MPL-2.0, which an unmodified republication does by
construction); the driver crate is Apache-2.0.

## Exact edits applied

`src/` is byte-identical to upstream in every vendored crate. Only
`Cargo.toml` differs, in these ways and no others:

All crates:
- `name =` renamed (band suffix `-b7`/`-b9`, `type-bridge-` namespace prefix);
  `version =` mirrors upstream.
- `description`/`repository` updated to reflect the republication
  (`homepage` keeps the upstream link); the upstream manifests' invalid
  `licenseFile` key (an artifact of upstream's manifest generator) dropped.
- `[lints.rust]`/`[lints.clippy]` allows added: vendored source compiled
  under this workspace's toolchain trips style lints upstream's build
  system never surfaced (`unused`, `dead_code`, `private_interfaces`,
  `clippy::all`). Allowed at manifest level instead of editing `src/`.
- `[lib] doctest = false`: upstream's doc examples are illustrative
  snippets that were never compiled as doctests (upstream builds with
  Bazel); they fail as Rust doctests, so the target is disabled.

Driver crates only:
- The `typedb-protocol` dependency is repointed at the same-band vendored
  protocol fork via a `package =` rename alias (in-source
  `use typedb_protocol::...` imports resolve unchanged), e.g.
  `typedb-protocol = { package = "type-bridge-typedb-protocol-b7", path = "../typedb-protocol-b7", version = "=3.7.0" }`
- `[dev-dependencies] rand = "0.8", serde_json = "1"` restored: upstream's
  generated manifest omits dev-dependencies, so the published crate cannot
  compile its own `#[cfg(test)]` code.
- b9 only: `deprecated = "allow"` added to `[lints.rust]` — upstream
  3.12.0 still imports `chrono::Date`, deprecated in chrono 0.4.23.

`rustfmt.toml` at the workspace root ignores `vendor/` so `cargo fmt
--check` does not demand reformatting of upstream source.

## Audit procedure

To verify the rename/repoint-only claim:

```sh
curl -sLO https://static.crates.io/crates/typedb-protocol/typedb-protocol-3.7.0.crate
sha256sum typedb-protocol-3.7.0.crate          # compare against the hash above
tar xzf typedb-protocol-3.7.0.crate
diff -rq typedb-protocol-3.7.0 vendor/typedb-protocol-b7
# expected output: exactly one line — Cargo.toml differs
```

(Same for the driver crate.)

## Publishing

Both crates publish to crates.io so that `type-bridge-orm` /
`type-bridge-server` (which publish there) can depend on them. Publish
order matters: the protocol fork must land first; the driver fork's
`cargo publish --dry-run` cannot fully verify until the protocol fork
exists in the index (first-publish ordering; `cargo package --list`
validates its packaging metadata in the meantime). The release workflow
publishes them in dependency order before orm/server.

## Refresh procedure (upstream band-7 patch release)

Band 7 is frozen upstream, so churn is expected to be near-zero. If a
band-7 patch matters:

1. Download the new `.crate` from static.crates.io; record its SHA256 here.
2. Delete the vendored tree's `src/`, `LICENSE`, `README.md` and extract
   the new ones in place (do NOT overwrite `Cargo.toml`).
3. Re-apply nothing — the existing `Cargo.toml` already carries the rename,
   repoint, lints, and dev-deps; bump its `version =` to mirror the new
   upstream patch.
4. Update every consumer's `version = "=X.Y.Z"` pin on the fork.
5. Re-run the audit procedure above; only `Cargo.toml` may differ.

## Exit path

These vendored copies exist only because crates.io forbids git
dependencies in published crates. If crates.io publishing of orm/server is
ever dropped, replace both forks with `package =` aliases on git-pinned
upstream tags and delete `vendor/`.
