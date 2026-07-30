# Generated Rust parity inventory

Plan 10 Flight 5 tracks generated Rust application parity by named behavior,
not by comparing raw Python, Node, and Rust test counts.

The checked authority is
[`tests/fixtures/rust-generated-parity-inventory.json`](https://github.com/ds1sqe/type-bridge/blob/master/tests/fixtures/rust-generated-parity-inventory.json).
Each accepted row names source anchors in the Python, Node, and generated Rust
evidence. An open row remains a release blocker unless the behavior is
implemented or an owner explicitly approves a release exclusion.

## Current status

The isolated generated Rust application has accepted live evidence for:

- entity CRUD and batches;
- exact/subtype inheritance reads;
- optional and multivalue attributes across all nine scalar domains;
- expression, Boolean, ordering, window, first, count, and exists terminals;
- typed aggregates and grouping;
- selected named and collected pages;
- write and reusable read transactions; and
- identical local and authenticated one-exchange remote materialization.

Schema generation, compile rejection, and dependency-isolated external
consumption have accepted offline evidence.

All named cross-binding semantic rows are now accepted. The generated Rust
live application covers relation-owned attribute replacement, repeated players
on one role, keyed relation put hit/miss, and bounded reachability through a
cycle with a deduplicated shared subtree.

The isolated Python + packed Node + generated Rust smoke, generated Rust TLS,
Rust 1.88, and final source/Git distribution-identity system gates are
accepted. Exact server OCI acceptance is the sole open system gate: the
landed candidate SHA must pass both `linux/amd64` and `linux/arm64` workflow
runtime proofs before the separately authorized stable publication and
attestation path can close it.

Run the inventory guard with:

```bash
uv run pytest -q tests/unit/compat/test_rust_generated_parity_inventory.py
```
