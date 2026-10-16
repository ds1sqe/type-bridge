# perf

- `harness/` and `schema/`: the perf result format, its validator, the
  converters and the history store used by `.github/workflows/perf.yml`.
- `*.md` and `raw/*.toml`: hand-run Python-vs-Rust reports from the 1.4.0
  development line, kept as history. Nothing reads or updates them.

Current numbers live at [/perf/](https://ds1sqe.github.io/type-bridge/perf/)
on the docs site. To run the benchmarks locally or add a metric, see
[Performance tracking](../docs/development/performance.md).
