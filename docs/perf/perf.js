/* Perf dashboard for docs/perf/index.md.
 *
 * Data comes from perf/data.json and perf/latest.json, which the MkDocs hook
 * perf/harness/render.py adds to the build. Charts use the vendored uPlot in
 * docs/perf/assets/, loaded only on this page.
 *
 * MkDocs loads this file on every page (extra_javascript); it does work only
 * where #perf-dashboard exists. Material's instant navigation swaps page
 * content without re-running scripts, so mounting follows its document$
 * stream and the previous mount is torn down first. */
(function () {
  "use strict";

  const CHART_HEIGHT = 200;
  // Divisors and suffixes, largest first, for each perf-result/v1 unit.
  const SCALES = {
    ns: [[1e9, " s"], [1e6, " ms"], [1e3, " µs"], [1, " ns"]],
    s: [[1, " s"]],
    bytes: [[2 ** 30, " GiB"], [2 ** 20, " MiB"], [2 ** 10, " KiB"], [1, " B"]],
    instructions: [[1e9, "G"], [1e6, "M"], [1e3, "k"], [1, ""]],
    ops_per_s: [[1e6, "M/s"], [1e3, "k/s"], [1, "/s"]],
  };
  const UNIT_NAMES = {
    ns: "time",
    s: "time",
    bytes: "size",
    instructions: "instructions",
    ops_per_s: "throughput",
  };

  // Stages in pipeline order; others follow alphabetically.
  const STAGE_ORDER = [
    "tb.sdk.entry",
    "tb.orm.plan",
    "tb.query.compile",
    "tb.runtime.execute",
    "tb.decode",
    "tb.commit",
  ];
  const STAGE_COLORS = ["#4e79a7", "#f28e2b", "#59a14f", "#e15759", "#76b7b2", "#edc948", "#b07aa1", "#9c755f"];
  const STAGE_LABELS = { "tb.runtime.execute": "tb.runtime.execute (network + TypeDB)" };

  let library = null;
  let unmount = function () {};

  function resolve(path) {
    return new URL(path, document.baseURI).href;
  }

  function format(value, unit) {
    if (value === null || value === undefined) return "—";
    const steps = SCALES[unit] || [[1, " " + unit]];
    const step = steps.find(([factor]) => Math.abs(value) >= factor) || steps[steps.length - 1];
    const scaled = Number((value / step[0]).toPrecision(3));
    return scaled.toLocaleString("en-US", { maximumFractionDigits: 3 }) + step[1];
  }

  function el(tag, attributes, ...children) {
    const node = document.createElement(tag);
    for (const [name, value] of Object.entries(attributes || {})) {
      if (name === "class") node.className = value;
      else if (name === "text") node.textContent = value;
      else node.setAttribute(name, value);
    }
    node.append(...children);
    return node;
  }

  function loadLibrary(root) {
    if (window.uPlot) return Promise.resolve(window.uPlot);
    if (!library) {
      library = new Promise(function (ok, fail) {
        document.head.append(el("link", { rel: "stylesheet", href: resolve(root.dataset.style) }));
        const script = el("script", { src: resolve(root.dataset.script) });
        script.onload = () => ok(window.uPlot);
        script.onerror = function () {
          library = null;
          fail(new Error("cannot load " + script.src));
        };
        document.head.append(script);
      });
    }
    return library;
  }

  async function fetchJson(url) {
    const response = await fetch(url, { cache: "no-cache" });
    if (!response.ok) throw new Error(url + ": HTTP " + response.status);
    return response.json();
  }

  // Material's scheme variables, so charts follow the light/dark toggle.
  function palette(root) {
    const style = getComputedStyle(root);
    const read = (name, fallback) => style.getPropertyValue(name).trim() || fallback;
    return {
      line: read("--md-typeset-a-color", "#4051b5"),
      text: read("--md-default-fg-color--light", "#0000008a"),
      marker: read("--md-default-fg-color--lighter", "#00000052"),
      grid: read("--md-default-fg-color--lightest", "#00000012"),
      family: read("--md-text-font-family", "system-ui, sans-serif"),
    };
  }

  function short(commit) {
    return commit.slice(0, 7);
  }

  function drawVersionMarkers(u, markers, colors) {
    const ratio = window.uPlot.pxRatio;
    const ctx = u.ctx;
    const { top, height } = u.bbox;
    ctx.save();
    ctx.strokeStyle = colors.marker;
    ctx.fillStyle = colors.text;
    ctx.lineWidth = ratio;
    ctx.setLineDash([4 * ratio, 4 * ratio]);
    ctx.font = 11 * ratio + "px " + colors.family;
    ctx.textAlign = "left";
    ctx.textBaseline = "top";
    for (const marker of markers) {
      const x = Math.round(u.valToPos(marker.index, "x", true));
      ctx.beginPath();
      ctx.moveTo(x, top);
      ctx.lineTo(x, top + height);
      ctx.stroke();
      ctx.fillText(marker.version, x + 4 * ratio, top + 2 * ratio);
    }
    ctx.restore();
  }

  function makeChart(uPlot, holder, metric, colors) {
    const points = metric.series;
    const markers = points.flatMap((point, index) =>
      index === 0 || point.version !== points[index - 1].version
        ? [{ index: index, version: point.version }]
        : [],
    );
    const axis = {
      stroke: colors.text,
      font: "12px " + colors.family,
      grid: { stroke: colors.grid, width: 1 },
      ticks: { stroke: colors.grid, width: 1 },
    };
    const options = {
      width: holder.clientWidth || 600,
      height: CHART_HEIGHT,
      scales: { x: { time: false, range: (u, min, max) => [min - 0.5, max + 0.5] } },
      cursor: { drag: { x: false, y: false }, points: { size: 8 } },
      series: [
        {
          label: "commit",
          value: function (u, index) {
            const point = points[index];
            if (!point) return "—";
            return short(point.commit) + " · " + point.version + " · " + point.at.slice(0, 10);
          },
        },
        {
          label: UNIT_NAMES[metric.unit] || metric.unit,
          stroke: colors.line,
          width: 2,
          points: { show: points.length <= 60, size: 8, fill: colors.line },
          value: (u, value) => format(value, metric.unit),
        },
      ],
      axes: [
        Object.assign({}, axis, {
          space: 72,
          incrs: [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000],
          values: (u, splits) => splits.map((index) => (points[index] ? short(points[index].commit) : "")),
        }),
        Object.assign({}, axis, {
          size: 72,
          values: (u, splits) => splits.map((value) => format(value, metric.unit)),
        }),
      ],
      hooks: { draw: [(u) => drawVersionMarkers(u, markers, colors)] },
    };
    const data = [points.map((_, index) => index), points.map((point) => point.value)];
    return new uPlot(options, data, holder);
  }

  function summary(data, ids) {
    const versions = data.versions.length ? data.versions.join(", ") : "none";
    const parts = [data.runs + " run(s)", ids.length + " metric(s)", "versions: " + versions];
    return el("p", { class: "perf-summary", text: parts.join(" · ") });
  }

  function latestSection(root, latest, latestUrl) {
    const section = el("section", { class: "perf-latest" }, el("h2", { text: "Latest run" }));
    if (!latest) {
      section.append(el("p", { text: "No latest run is published." }));
      return { section: section, rows: [] };
    }
    const run = latest.run;
    const commit = el("a", { href: root.dataset.repo + "/commit/" + run.commit }, el("code", { text: short(run.commit) }));
    const raw = el("a", { href: resolve(latestUrl), text: "raw JSON" });
    section.append(
      el(
        "p",
        {},
        "Commit ",
        commit,
        " on " + run.branch + ", version " + run.version + ", " + run.trigger + " at " + run.started_at,
        " on " + run.runner.image + " (" + run.runner.cpu + ", " + run.runner.cores + " cores). ",
        raw,
        ".",
      ),
    );
    const head = el("tr", {}, ...["Metric", "Family", "Value", "p50", "95% CI", "n"].map((name) => el("th", { text: name })));
    const rows = latest.metrics.map(function (metric) {
      const ci = metric.ci95 ? format(metric.ci95[0], metric.unit) + " – " + format(metric.ci95[1], metric.unit) : "—";
      const row = el(
        "tr",
        {},
        el("td", {}, el("code", { text: metric.id })),
        el("td", { text: metric.family }),
        el("td", { text: format(metric.value, metric.unit), title: String(metric.value) + " " + metric.unit }),
        el("td", { text: format(metric.p50, metric.unit) }),
        el("td", { text: ci }),
        el("td", { text: metric.n === undefined ? "—" : String(metric.n) }),
      );
      row.dataset.family = metric.family;
      row.dataset.id = metric.id;
      return row;
    });
    section.append(el("div", { class: "perf-table" }, el("table", {}, el("thead", {}, head), el("tbody", {}, ...rows))));
    return { section: section, rows: rows };
  }

  function stageNames(metrics) {
    const names = new Set(metrics.flatMap((metric) => Object.keys(metric.stages)));
    const known = STAGE_ORDER.filter((name) => names.has(name));
    return known.concat([...names].filter((name) => !STAGE_ORDER.includes(name)).sort());
  }

  // The SDK of a `pipeline.<sdk>.<pipeline>` id.
  function sdkOf(id) {
    return id.split(".")[1] || "";
  }

  // One stacked bar per staged metric of the latest run. Time in no stage
  // is SDK-side work and closes the bar.
  function waterfallSection(latest) {
    const staged = latest ? latest.metrics.filter((metric) => metric.stages) : [];
    if (staged.length === 0) return { section: null, rows: [] };
    const names = stageNames(staged);
    const color = (name) => STAGE_COLORS[names.indexOf(name) % STAGE_COLORS.length];
    const segment = function (label, value, total, unit, background) {
      const share = total > 0 ? (100 * value) / total : 0;
      const node = el("span", { class: "perf-stage", title: label + ": " + format(value, unit) + " (" + share.toFixed(1) + "%)" });
      node.style.width = share + "%";
      if (background) node.style.background = background;
      return node;
    };
    const legend = el(
      "ul",
      { class: "perf-stage-legend" },
      ...names.map(function (name) {
        const swatch = el("span", { class: "perf-swatch" });
        swatch.style.background = color(name);
        return el("li", {}, swatch, el("code", { text: STAGE_LABELS[name] || name }));
      }),
      el("li", {}, el("span", { class: "perf-swatch perf-swatch-sdk" }), "outside stages (SDK)"),
    );
    const rows = staged.map(function (metric) {
      const bar = el("div", { class: "perf-stage-bar", role: "img" });
      let inside = 0;
      for (const name of names) {
        const value = metric.stages[name];
        if (value === undefined) continue;
        inside += value;
        bar.append(segment(name, value, Math.max(metric.value, inside), metric.unit, color(name)));
      }
      const outside = Math.max(0, metric.value - inside);
      if (outside > 0) bar.append(segment("outside stages (SDK)", outside, metric.value, metric.unit, null));
      bar.setAttribute("aria-label", metric.id + " stage breakdown");
      const row = el(
        "div",
        { class: "perf-stage-row" },
        el("header", {}, el("code", { text: metric.id }), el("span", { text: "p50 " + format(metric.value, metric.unit) })),
        bar,
      );
      row.dataset.family = metric.family;
      row.dataset.id = metric.id;
      return row;
    });
    // One group per SDK (`pipeline.<sdk>.<pipeline>`); the selector shows
    // one group, Rust first. Search and family chips still hide rows.
    const groups = new Map();
    for (const row of rows) {
      const sdk = sdkOf(row.dataset.id);
      if (!groups.has(sdk)) groups.set(sdk, el("div", { class: "perf-stage-group" }));
      groups.get(sdk).append(row);
    }
    const sdks = [...groups.keys()].sort((a, b) => (a === "rust" ? -1 : b === "rust" ? 1 : a.localeCompare(b)));
    const selector = el("div", { class: "perf-families perf-sdks", role: "group", "aria-label": "SDK" });
    const select = function (chosen) {
      for (const [sdk, group] of groups) group.hidden = sdk !== chosen;
      for (const chip of selector.children) chip.setAttribute("aria-pressed", String(chip.dataset.sdk === chosen));
    };
    for (const sdk of sdks) {
      const chip = el("button", { type: "button", class: "perf-chip", text: sdk });
      chip.dataset.sdk = sdk;
      chip.addEventListener("click", function () {
        select(sdk);
      });
      selector.append(chip);
    }
    select(sdks[0]);
    const section = el(
      "section",
      { class: "perf-waterfall" },
      el("h2", { text: "Stage breakdown" }),
      el("p", {
        text:
          "Median time per stage of each pipeline in the latest run, per SDK. tb.runtime.execute is the network round trip plus TypeDB, against a local container; time outside every stage is SDK-side work.",
      }),
      selector,
      legend,
      ...groups.values(),
    );
    return { section: section, rows: rows };
  }

  function mount(root) {
    const state = { alive: true, cleanups: [] };
    const status = root.querySelector(".perf-status");
    const fail = function (error) {
      if (state.alive) status.textContent = "Cannot load the performance data: " + error.message;
    };

    fetchJson(resolve(root.dataset.index))
      .then(function (data) {
        const ids = Object.keys(data.metrics || {});
        if (!data.runs || ids.length === 0) {
          status.textContent = "No data yet. History appears after the first recorded run on master.";
          return null;
        }
        const latest = data.latest_url ? fetchJson(resolve(data.latest_url)) : null;
        return Promise.all([loadLibrary(root), latest]).then(function ([uPlot, latestRun]) {
          if (state.alive) render(root, state, uPlot, data, ids, latestRun);
        });
      })
      .catch(fail);

    return function () {
      state.alive = false;
      state.cleanups.forEach((cleanup) => cleanup());
    };
  }

  function render(root, state, uPlot, data, ids, latest) {
    const families = [...new Set(ids.map((id) => data.metrics[id].family))].sort();
    let family = "all";
    let query = "";

    const chips = el("div", { class: "perf-families", role: "group", "aria-label": "Family" });
    const search = el("input", { type: "search", placeholder: "Filter metric ids", "aria-label": "Filter metric ids" });
    const controls = el("div", { class: "perf-controls" }, chips, search);
    const grid = el("div", { class: "perf-grid" });
    const empty = el("p", { class: "perf-empty", text: "No metric matches the filter.", hidden: "" });
    const { section, rows: tableRows } = latestSection(root, latest, data.latest_url);
    const waterfall = waterfallSection(latest);
    const rows = tableRows.concat(waterfall.rows);

    const cards = ids.map(function (id) {
      const metric = data.metrics[id];
      const last = metric.series[metric.series.length - 1];
      const plot = el("div", { class: "perf-plot" });
      const card = el(
        "article",
        { class: "perf-card" },
        el("header", {}, el("code", { text: id }), el("span", { text: format(last.value, metric.unit) })),
        plot,
      );
      card.dataset.family = metric.family;
      card.dataset.id = id;
      return { id: id, metric: metric, card: card, plot: plot, chart: null };
    });
    const byCard = new Map(cards.map((entry) => [entry.card, entry]));
    let colors = palette(root);

    // Charts are built when a card first nears the viewport.
    const visible = new IntersectionObserver(
      function (entries) {
        for (const entry of entries) {
          const item = byCard.get(entry.target);
          if (entry.isIntersecting && item && !item.chart) {
            item.chart = makeChart(uPlot, item.plot, item.metric, colors);
          }
        }
      },
      { rootMargin: "300px 0px" },
    );
    const resized = new ResizeObserver(function (entries) {
      for (const entry of entries) {
        const item = byCard.get(entry.target);
        if (item && item.chart) item.chart.setSize({ width: item.plot.clientWidth, height: CHART_HEIGHT });
      }
    });
    const rebuild = function () {
      colors = palette(root);
      for (const item of cards) {
        if (item.chart) {
          item.chart.destroy();
          item.chart = null;
        }
        visible.unobserve(item.card);
        visible.observe(item.card);
      }
    };
    const scheme = new MutationObserver(rebuild);
    scheme.observe(document.body, { attributes: true, attributeFilter: ["data-md-color-scheme"] });

    const matches = (node) =>
      (family === "all" || node.dataset.family === family) && node.dataset.id.includes(query);
    const apply = function () {
      let shown = 0;
      for (const item of cards) {
        item.card.hidden = !matches(item.card);
        shown += item.card.hidden ? 0 : 1;
      }
      for (const row of rows) row.hidden = !matches(row);
      empty.hidden = shown > 0;
      for (const chip of chips.children) chip.setAttribute("aria-pressed", String(chip.dataset.family === family));
    };
    for (const name of ["all", ...families]) {
      const count = name === "all" ? ids.length : ids.filter((id) => data.metrics[id].family === name).length;
      const chip = el("button", { type: "button", class: "perf-chip", text: name + " (" + count + ")" });
      chip.dataset.family = name;
      chip.addEventListener("click", function () {
        family = name;
        apply();
      });
      chips.append(chip);
    }
    search.addEventListener("input", function () {
      query = search.value.trim().toLowerCase();
      apply();
    });

    grid.append(...cards.map((item) => item.card));
    const sections = waterfall.section ? [waterfall.section, section] : [section];
    root.replaceChildren(summary(data, ids), controls, grid, empty, ...sections);
    apply();
    for (const item of cards) {
      visible.observe(item.card);
      resized.observe(item.card);
    }

    state.cleanups.push(function () {
      visible.disconnect();
      resized.disconnect();
      scheme.disconnect();
      for (const item of cards) if (item.chart) item.chart.destroy();
    });
  }

  function start() {
    unmount();
    unmount = function () {};
    const root = document.getElementById("perf-dashboard");
    if (root) unmount = mount(root);
  }

  if (window.document$ && typeof window.document$.subscribe === "function") {
    window.document$.subscribe(start);
  } else if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", start);
  } else {
    start();
  }
})();
