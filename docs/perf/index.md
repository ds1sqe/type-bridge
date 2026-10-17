---
hide:
  - toc
---

# Performance

Benchmark history for TypeBridge, recorded on every push to `master`, nightly
and on manual runs of the `Perf` workflow. Each chart plots one metric over
the recorded commits; dashed lines mark the first commit of each package
version. Runs use GitHub-hosted runners: instruction counts are the stable
signal, and wall times carry runner noise.

<div id="perf-dashboard" markdown="0"
     data-index="data.json"
     data-script="assets/uPlot.iife.min.js"
     data-style="assets/uPlot.min.css"
     data-repo="https://github.com/ds1sqe/type-bridge">
  <p class="perf-status">Loading performance data...</p>
  <noscript><p>The dashboard needs JavaScript.</p></noscript>
</div>

**Released versions** compares `type-bridge` and `@type-bridge/node` 2.0.2,
2.1.0 and 2.2.5, installed from PyPI and npm, with the head built from the
commit, on the same six CRUD ops against TypeDB 3.12.3 (nightly, on manual
runs and on release tags). Each bar is the newest run of one version. A gap
means the version has no call for the op: the generated Node manager of
2.0.2 has only `insert`, `getByIid` and `all`, so Node 2.0.2 shows only
`insert_one`.

See [Performance tracking](../development/performance.md) for how the data is
produced and how to add a metric.
