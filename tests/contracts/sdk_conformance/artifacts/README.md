# SDK artifacts

This suite binds the generated language surfaces and the complete C artifact
acceptance evidence for G05 cancellation, G06 resource limits, G08 structured
diagnostics, G12 standalone CLI distribution, and G13 explicit close.

Each report names the same standalone CLI, native runtime, generated C package,
artifact acceptance aggregate, and source commit. It also binds the queries,
models, administration and serialization reports for its own binding. All four
bindings have the same predecessor inventory; no separate smoke report exists.
`c-broad-case-ledger.json` binds the cross-slice authorities and
`c-full-sdk-audit.json` requires all five suite report sets.

`scripts/ci/sdk_artifacts_assemble.py` accepts measured same-run evidence,
hashes proof fragments and generated surfaces, reconstructs artifact acceptance
from the actual archives, and checks exact source identity. Its repeated
`--predecessor SUITE=PATH` arguments use the four predecessor suite names.
`sdk_artifacts_compare.py` rejects cross-binding artifact or outcome drift,
partial transitions and noncanonical reports. Wire formats use
`typebridge.sdk-artifacts-<kind>/v1`.

`sdk_artifacts_surfaces.py build` generates Python, Node and Rust surfaces with
the captured standalone CLI. Repeating the build must produce identical
archives; `validate --binding <binding>` verifies their bounded member ledger
and source identity. `sdk_artifacts_surface_consumer.py` checks each generated
surface through the corresponding public consumer.

`run_full_c_artifact.py` keeps its entry point and CLI. Its output contains
`queries/`, `models/`, `administration/`, `serialization/`, and `artifacts/`,
each with `python.json`, `node.json`, `rust.json`, and `c.json`. Promotion
retains these below `reports/` in its evidence archive. The released C example
package prefix remains `tb_sdkv3` until the separate release-record update.

This suite grants no release, publication, registry, tag or public-support
authority. Those decisions remain governed by the C release contract.
