# Python static contracts

`documented_examples.py` imports the emitted `generated_v2` acceptance package
and is compiled by the schema-codegen acceptance runner.

`documented_examples.py` is the ordered concatenation of all five Python code
blocks marked in `docs/development/typed-query-contract.md`. The corpus unit
test rejects source drift, and generated-package acceptance compiles the file.

The generated package's own positive and negative Pyright fixtures live in
`type-bridge-core/crates/schema-codegen/tests/acceptance/`.
