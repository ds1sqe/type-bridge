# Performance tracking

The `Perf` workflow (`.github/workflows/perf.yml`) measures TypeBridge on
GitHub-hosted runners and the docs site renders the history at
[Performance](../perf/index.md).

## Results

Every producer writes one `perf-result/v1` JSON file per run: a `run` block
(commit, branch, version, trigger, runner, toolchains) and a list of
`metrics`. `perf/schema/perf-result-v1.json` is the normative schema;
`python -m perf.harness.validate FILE...` checks a file.

A metric id is a stable dotted name that starts with its family, for
example `micro.core.compilation.insert_entity`. Build ids with
`perf.harness.schema.metric_id`. Keep ids stable: the history is keyed by
id.

## Run the micro benchmarks locally

The micro family is the Criterion wall time and gungraun instruction count
of the core and ORM bench targets. Instruction counts need valgrind and
`gungraun-runner` at the version in `perf.yml`. Under `cargo test` the
`*_iai` targets do nothing, so `cargo test --all-targets` needs neither.

```bash
cd type-bridge-core
CRITERION_HOME=../tmp/perf/criterion/core cargo bench -p type-bridge-core-lib \
  --bench validation --bench compilation --bench schema -- --noplot
GUNGRAUN_SAVE_SUMMARY=json GUNGRAUN_HOME=../tmp/perf/iai/core \
  cargo bench -p type-bridge-core-lib \
  --bench validation_iai --bench compilation_iai --bench schema_iai
cd ..
uv run python -m perf.harness.micro \
  --criterion-dir tmp/perf/criterion --iai-dir tmp/perf/iai --out tmp/perf/micro.json
```

Each directory under `--criterion-dir` and `--iai-dir` names the crate in the
metric ids; the ORM crate (`type-bridge-orm`, benches `serde`,
`query_building` and `staged`) goes under `orm/`. Either directory may be
omitted.

## Stage spans

The core names each pipeline stage with a `tracing` span at the layer that
owns it:

| Span | Covers | Owner |
| --- | --- | --- |
| `tb.orm.plan` | building and staging a `query-plan/v3` plan | contract builder, ORM `stage_plan` |
| `tb.query.compile` | lowering a plan to the TypeQL AST and rendering it | ORM plan executor |
| `tb.runtime.execute` | a driver round trip: open, query, rollback, close | TypeDB runtime |
| `tb.decode` | converting one provider answer to JSON | TypeDB runtime |
| `tb.commit` | the provider commit | TypeDB runtime |

The spans carry no fields, so no query text or value is recorded. They are
always compiled in. With no subscriber they cost a callsite check each: the
`staged/mixed_commit` bench runs about 70 more instructions per commit with
them than with `tracing/max_level_off` (0.02% of about 326,000). A
subscriber at `INFO` sees them as ordinary spans.

## Run the Python binding benchmarks locally

`benchmarks/` is a pytest-benchmark suite over the Python binding: native
query compilation, clause-to-dict marshalling and validation through
`type_bridge_core`. Build the native module in release mode first (as
`uv sync` does), then:

```bash
uv run pytest benchmarks --benchmark-only --benchmark-json tmp/perf/pytest-benchmark.json
uv run python -m perf.harness.micro \
  --pytest-benchmark tmp/perf/pytest-benchmark.json --out tmp/perf/micro.json
```

A bench `test_<name>` in `benchmarks/test_<group>_bench.py` becomes the
metric `binding.python.<group>.<name>`. The micro job passes all three inputs
to one `perf.harness.micro` call, so they share one result file.

## Where history lives

Results from master pushes, the nightly schedule and manual dispatch are
appended to the orphan branch `perf-data` by the workflow's `store` job.
Pull requests never write history.

```text
runs/<YYYY-MM-DD>/<sha>-<trigger>.json   one result per UTC day, commit and trigger
index.json                               per-metric series, rebuilt on store
```

Results with the same day, commit and trigger are merged into one file: an
incoming metric replaces the stored one with its id and other metrics are
kept, so every producer of a workflow run lands in one file and a same-day
re-run replaces only its own metrics. Nightly runs of an unchanged master
therefore add one point per day. Only pull request runs cancel each other;
every master push, nightly and dispatch run has its own concurrency group, so
none is dropped while it waits. The `store` job has no concurrency group
(GitHub cancels all but one pending job per group): concurrent stores retry
the push up to 10 times and the job fails if every attempt is rejected.

Store into a local checkout of the branch, or any directory, with:

```bash
uv run python -m perf.harness.store --data-dir tmp/perf-data tmp/perf/micro.json
```

A manual dispatch can write to a scratch branch through its `data_branch`
input. To preview the page with some history, point the docs build at a
data directory:

```bash
PERF_DATA_DIR=tmp/perf-data uv run --extra docs mkdocs build --strict
```

Without `PERF_DATA_DIR` the page shows "no data yet". The MkDocs hook
`perf/harness/render.py` publishes the index as `perf/data.json` and the
newest run as `perf/latest.json`. After each successful Perf run on master,
the Documentation workflow rebuilds the site from `perf-data`.

## Add a metric

- **A Criterion or gungraun bench** in an existing target needs no harness
  change: its id follows from the bench id. A new bench target must be added
  to both bench lists in `perf.yml`.
- **A Python binding bench** is a `test_*` function in
  `benchmarks/test_<group>_bench.py`; a new module needs only that name.
- **A new producer** writes a `perf-result/v1` file, validates it, and
  uploads it as an artifact named `perf-<family>`; the `store` job stores
  every `perf-*` artifact. Add its job to the `store` job's `needs`.
- **A new family or unit** changes the result format: update the schema,
  `perf/harness/schema.py`, and the unit formats in `docs/perf/perf.js`.

The chart library is uPlot, vendored in `docs/perf/assets/` and pinned by
`docs/perf/assets/CHECKSUMS`, which a unit test verifies.

## Build times

The build job measures cold release builds of the published Cargo inventory,
Python native extension plus maturin wheel assembly, and Node native addon.
Each group uses a fresh target directory; registry downloads may be cached,
compiled dependencies are never reused between groups. Wall metrics are
`build.public.wall`, `build.python.wall` and `build.node.wall` in seconds.
Stable Cargo `--timings` HTML supplies the 20 slowest `build.unit.<crate>`
metrics: cumulative unit durations per crate across the measured groups,
including build scripts. Units overlap, so they do not sum to build wall time.
The run records the Rust toolchain and the wheel builder's Python interpreter.

```bash
python -m perf.harness.build --target-dir tmp/perf-cold --out tmp/perf/build.json
# A smaller local smoke; the target directory must still be fresh.
python -m perf.harness.build --group public --package type-bridge-toml-transpiler \
  --target-dir tmp/perf-smoke --out tmp/perf/build-smoke.json
```

Install maturin 1.14.1 before selecting the Python group. Existing group target
directories are rejected, never deleted. Logs are retained beside the result;
a failed build produces no result.

## Artifact sizes

The size job builds Linux x86_64 artifacts with the release workflow's wheel,
Node, CLI and server commands, then records their actual byte lengths. It
reports `size.wheel.bytes`, `size.npm.bytes`, `size.cli_unstripped.bytes`,
`size.cli_stripped.bytes`, `size.server.bytes`, `size.c_runtime.bytes` and
`size.c_runtime_lib.bytes`. The server and canonical CLI are stripped as in
release; the unstripped CLI provides a separate baseline. Available Python, Node and C
compiler versions are recorded for the wheel, npm and C measurements.

The C metric is a perf-only archive of the current CMake install tree: ABI 1.7
and release package metadata are reconciled separately in 2.3.0 release prep.
It uses the release Cargo flags and normalized tar metadata; it is not a
published C release candidate. Release helpers and policy are unchanged.

Measure existing artifacts without rebuilding or modifying them:

```bash
python -m perf.harness.size --artifact wheel=dist/package.whl \
  --artifact npm=dist/package.tgz --out tmp/perf/size.json
```

Repeat `--artifact NAME=FILE` for each input. Missing, empty or directory inputs,
duplicate metric identities, and output paths that overwrite an artifact fail
without publishing a result. The converter test fixture is an actual tiny
`npm pack --ignore-scripts --json` archive (npm 12.2.0), renamed `package.npm`.

## Test-suite durations

The suite job runs on master pushes, schedule and dispatch; pull requests run
build and size producers without this additional full-suite run. Prepare native
Python/Node dependencies and fetch Cargo dependencies before measuring:

```bash
uv sync --extra dev
npm ci --prefix type-bridge-core/crates/node
npm run build --prefix type-bridge-core/crates/node
cargo fetch --locked --manifest-path type-bridge-core/Cargo.toml
uv run python -m perf.harness.suite --out tmp/perf/suite.json
# A short local smoke of the same producer:
uv run python -m perf.harness.suite --suite python \
  --pytest-target tests/unit/perf --out tmp/perf/suite-smoke.json
```

The existing commands are `cargo test --workspace --locked --offline`,
`pytest tests/unit`, and `npm run test:unit`. Each complete successful command
becomes `suite.<rust|python|node>.wall` in seconds, including compilation done by
that test command. Native dependency preparation is outside those timings.
No TypeDB server is configured. Failed suites retain logs and publish no result.

Pytest's `--durations=0` call observations and Node's TAP `duration_ms` produce
the ten slowest `suite.<python|node>.slow.<id>` metrics. Setup/teardown and Node
suite totals are excluded. Node names include their parent suites. Slow ids
retain a readable prefix plus a stable hash so parameter punctuation cannot
merge distinct tests. Stable Rust does not support libtest `--report-time`, so
Rust per-test metrics are omitted rather than enabling nightly-only flags.
The test fixtures are captured pytest and Node TAP output from small real runs.

The workflow shares one UTC start timestamp across every producer, so jobs
that finish on different days still merge into the same history run.

## Reading a PR comparison

The PR-only comparison job runs base and head Rust micro benchmarks sequentially
on the same runner, with separate build targets. Instruction counts are the
primary signal; Criterion wall times are noisier. The advisory job-summary table
lists regressions first and uploads both raw result files. It never comments on
the PR or writes history, and uses a read-only token without checkout credentials.
A base predating the perf harness is skipped with a notice.

Markers apply only beyond these noise thresholds: instructions and bytes 1%,
nanoseconds 10%, seconds 15%, and throughput (`ops_per_s`, higher is better) 10%.
Changes exactly at the threshold are unchanged. New/removed ids have separate
sections; changed units are not compared. A zero baseline is unchanged at zero,
otherwise an infinite increase. At most 200 metric rows are shown, with an
omitted-row count. Regressions and unavailable comparisons never fail the gate.

```bash
python -m perf.harness.compare base.json head.json --out tmp/perf/comparison.md
```
