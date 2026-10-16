# Writes suite

One staged program, written once per binding, must give the same plan bytes
and the same results in Python, TypeScript, Rust and C, directly and over the
proxy.

`purchase/` holds the program contract:

- `typebridge.yaml` and `schema/`: the workspace every binding generates its
  package from. It is the schema of the SDK golden purchase plan; like the
  schema the live fixture defines, it has no keys or cardinalities.
- `program.json`: the steps and their data. `seed` inserts one item, account
  and doctor. `program` stages the purchase, a touch of every doctor's on-call
  value and a batch insert of 50 orders (given rows) in one transaction. `read`
  counts the state back in a read transaction.
- `expected.json`: the canonical bytes of every plan (hex), every answer as
  `[kind, value]` (rows report their count, exists 0 or 1), the provider state
  and the proxy's commit outcomes.

The programs are `examples/purchase/{python,ts,rust,c}/`. Each builds its
plans from the generated package's tokens and authority and prints one JSON
report; with `TYPE_BRIDGE_SDK_WRITES_DIRECT_DATABASE` and
`TYPE_BRIDGE_SDK_WRITES_PROXY_ENDPOINT` set it also runs the program on both
transports.

```bash
# Plan bytes only
uv run python scripts/ci/run_sdk_conformance.py writes --offline \
    --semantic-profile typedb-3.13.6/v1 --output <new directory>
# Plan bytes, answers and state on a live TypeDB 3.13.6
uv run python scripts/ci/run_sdk_conformance.py writes \
    --semantic-profile typedb-3.13.6/v1 --address <host:port> --http-port <port> \
    --output <new directory>
```

The runner generates the four packages, builds the programs, and for a live
run serves the server crate's `sdk_writes_fixture_serves_until_stopped`
fixture: an empty direct database and an empty database behind a vouching
proxy per binding. It publishes `reports/<binding>.json`
(`typebridge.sdk-writes-report/v1`) and the comparison summary.
`sdk_writes_compare.py` requires every report to equal `expected.json`, which
`schema-codegen`'s `rust_plan_builder` tests tie to the hand-built golden
purchase.
