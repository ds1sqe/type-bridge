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
`gungraun-runner` at the version in `perf.yml`.

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
metric ids; the ORM crate (`type-bridge-orm`, benches `serde` and
`query_building`) goes under `orm/`. Either directory may be omitted.

## Where history lives

Results from master pushes, the nightly schedule and manual dispatch are
appended to the orphan branch `perf-data` by the workflow's `store` job.
Pull requests never write history.

```text
runs/<YYYY-MM-DD>/<sha>-<trigger>.json   one result per commit and trigger
index.json                               per-metric series, rebuilt on store
```

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
- **A new producer** writes a `perf-result/v1` file, validates it, and
  uploads it as an artifact named `perf-<family>`; the `store` job stores
  every `perf-*` artifact. Add its job to the `store` job's `needs`.
- **A new family or unit** changes the result format: update the schema,
  `perf/harness/schema.py`, and the unit formats in `docs/perf/perf.js`.

The chart library is uPlot, vendored in `docs/perf/assets/` and pinned by
`docs/perf/assets/CHECKSUMS`, which a unit test verifies.
