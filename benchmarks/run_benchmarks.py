#!/usr/bin/env python3
"""Benchmark runner and report generator for Python vs Rust comparison.

Usage:
    # Run benchmarks (auto-saves TOML to .results/{host}_{commit}.toml)
    uv run python benchmarks/run_benchmarks.py

    # Save with a custom name
    uv run python benchmarks/run_benchmarks.py --save laptop-v1

    # Run and write a markdown report
    uv run python benchmarks/run_benchmarks.py --report benchmarks/RESULTS.md

    # Compare current run against a saved baseline
    uv run python benchmarks/run_benchmarks.py --compare laptop-v1

    # Compare two saved results without running benchmarks
    uv run python benchmarks/run_benchmarks.py --diff laptop-v1 server-v1
    uv run python benchmarks/run_benchmarks.py --diff results/a.toml results/b.toml

    # Run only a subset (validation or compilation)
    uv run python benchmarks/run_benchmarks.py --suite validation
    uv run python benchmarks/run_benchmarks.py --suite compilation
"""

from __future__ import annotations

import argparse
import json
import math
import subprocess
import sys
import tomllib
from datetime import UTC, datetime
from pathlib import Path

BENCHMARKS_DIR = Path(__file__).parent
PROJECT_ROOT = BENCHMARKS_DIR.parent
RESULTS_DIR = BENCHMARKS_DIR / ".results"
CRITERION_DIR = PROJECT_ROOT / "type-bridge-core" / "target" / "criterion"


# ---------------------------------------------------------------------------
# Run benchmarks via pytest-benchmark
# ---------------------------------------------------------------------------


def run_benchmarks(
    suite: str | None = None,
    save_name: str | None = None,
    json_path: Path | None = None,
) -> dict:
    """Run pytest-benchmark and return the JSON results."""
    cmd = [
        "uv",
        "run",
        "pytest",
        str(BENCHMARKS_DIR),
        "-m",
        "benchmark",
        "--benchmark-only",
        "--benchmark-columns=min,max,mean,stddev,median,rounds",
        "--benchmark-sort=name",
        "-q",
    ]

    if suite == "validation":
        cmd.append("-k=validation or validate")
    elif suite == "compilation":
        cmd.append("-k=compile or serde")

    if save_name:
        cmd.append(f"--benchmark-save={save_name}")

    out_path = json_path or RESULTS_DIR / "latest.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    cmd.append(f"--benchmark-json={out_path}")

    print(f"Running: {' '.join(cmd)}\n", file=sys.stderr)
    result = subprocess.run(cmd, cwd=PROJECT_ROOT, capture_output=True, text=True)

    if result.stdout:
        print(result.stdout, file=sys.stderr)
    if result.stderr:
        print(result.stderr, file=sys.stderr)

    if result.returncode != 0:
        print(f"Benchmark run failed with exit code {result.returncode}", file=sys.stderr)
        sys.exit(1)

    with open(out_path) as f:
        return json.load(f)


# ---------------------------------------------------------------------------
# Parse results into a structured format
# ---------------------------------------------------------------------------


def parse_results(data: dict) -> list[dict]:
    """Extract benchmark entries from pytest-benchmark JSON."""
    entries = []
    for bench in data.get("benchmarks", []):
        name = bench["name"]
        stats = bench["stats"]
        entries.append(
            {
                "name": name,
                "min": stats["min"],
                "max": stats["max"],
                "mean": stats["mean"],
                "stddev": stats["stddev"],
                "median": stats["median"],
                "rounds": stats["rounds"],
                "iqr": stats.get("iqr", 0),
                "q1": stats.get("q1", 0),
                "q3": stats.get("q3", 0),
                "ops": stats.get("ops", 0),
                "iterations": stats.get("iterations", 1),
            }
        )
    return entries


def parse_criterion_results() -> list[dict]:
    """Load criterion benchmark results from target/criterion/*/new/estimates.json.

    Returns list of dicts with: name, mean, median, stddev, ci_lower, ci_upper
    (all in seconds).
    """
    if not CRITERION_DIR.exists():
        return []

    entries = []
    for bench_dir in sorted(CRITERION_DIR.iterdir()):
        estimates_path = bench_dir / "new" / "estimates.json"
        benchmark_path = bench_dir / "new" / "benchmark.json"
        if not estimates_path.exists():
            continue

        name = bench_dir.name
        if benchmark_path.exists():
            with open(benchmark_path) as f:
                bm = json.load(f)
                name = bm.get("full_id", name)

        with open(estimates_path) as f:
            est = json.load(f)

        # Criterion stores times in nanoseconds
        entries.append(
            {
                "name": name,
                "mean": est["mean"]["point_estimate"] * 1e-9,
                "median": est["median"]["point_estimate"] * 1e-9,
                "stddev": est["std_dev"]["point_estimate"] * 1e-9,
                "ci_lower": est["mean"]["confidence_interval"]["lower_bound"] * 1e-9,
                "ci_upper": est["mean"]["confidence_interval"]["upper_bound"] * 1e-9,
            }
        )

    return entries


# ---------------------------------------------------------------------------
# TOML save/load
# ---------------------------------------------------------------------------


def _enrich_entries(entries: list[dict]) -> list[dict]:
    """Add group, impl, ci_lower, ci_upper to each entry."""
    enriched = []
    for entry in entries:
        group, _base, impl_tag = _classify(entry["name"])
        ci_lo, ci_hi = _compute_ci(entry["mean"], entry["stddev"], entry["rounds"])
        enriched.append(
            {
                **entry,
                "group": group,
                "impl": impl_tag,
                "ci_lower": ci_lo,
                "ci_upper": ci_hi,
            }
        )
    return enriched


def _toml_str(val: object) -> str:
    """Format a value for TOML output."""
    if isinstance(val, bool):
        return "true" if val else "false"
    if isinstance(val, int):
        return str(val)
    if isinstance(val, float):
        return f"{val:.15e}"
    return f'"{val}"'


def write_toml(path: Path, info: dict, entries: list[dict], save_name: str) -> None:
    """Write benchmark results as a self-contained TOML file."""
    commit_info = info.get("_commit_info", {})
    commit_hash = commit_info.get("id", "")

    lines: list[str] = []
    lines.append("[metadata]")
    lines.append(f'timestamp = "{datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")}"')
    lines.append(f'save_name = "{save_name}"')
    lines.append("")

    lines.append("[metadata.commit]")
    lines.append(f'hash = "{commit_hash}"')
    lines.append(f'short = "{info["commit_short"]}"')
    lines.append(f"dirty = {_toml_str(info['commit_dirty'])}")
    lines.append(f'branch = "{info["branch"]}"')
    lines.append("")

    lines.append("[metadata.environment]")
    lines.append(f'host = "{info["node"]}"')
    lines.append(f'os = "{info["system"]}"')
    lines.append(f'kernel = "{info["release"]}"')
    lines.append(f'arch = "{info["machine_arch"]}"')
    lines.append(f'cpu = "{info["cpu_brand"]}"')
    lines.append(f"cpu_cores = {info['cpu_count']}")
    lines.append(f'python = "{info["python_impl"]} {info["python_ver"]}"')
    lines.append(f'python_compiler = "{info["python_compiler"]}"')
    lines.append(f'rust = "{info["rust_version"]}"')
    lines.append("")

    float_keys = ("mean", "median", "stddev", "ci_lower", "ci_upper", "min", "max")
    for entry in entries:
        lines.append("[[benchmarks]]")
        lines.append(f'name = "{entry["name"]}"')
        lines.append(f'group = "{entry["group"]}"')
        lines.append(f'impl = "{entry["impl"]}"')
        for key in float_keys:
            lines.append(f"{key} = {entry[key]:.15e}")
        lines.append(f"rounds = {entry['rounds']}")
        lines.append("")

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines))


def _read_toml(path: Path) -> tuple[dict, list[dict]]:
    """Read a TOML benchmark file. Returns (metadata, entries)."""
    with open(path, "rb") as f:
        data = tomllib.load(f)

    meta = data.get("metadata", {})
    env = meta.get("environment", {})
    commit = meta.get("commit", {})

    # Normalize metadata into the same format as _get_version_info()
    info = {
        "commit_short": commit.get("short", "unknown"),
        "commit_dirty": commit.get("dirty", False),
        "branch": commit.get("branch", "unknown"),
        "rust_version": env.get("rust", "unknown"),
        "python_ver": env.get("python", "unknown"),
        "python_impl": "",
        "python_compiler": env.get("python_compiler", "unknown"),
        "system": env.get("os", "unknown"),
        "release": env.get("kernel", "unknown"),
        "machine_arch": env.get("arch", "unknown"),
        "node": env.get("host", "unknown"),
        "cpu_brand": env.get("cpu", "unknown"),
        "cpu_count": env.get("cpu_cores", "?"),
        "bench_datetime": meta.get("timestamp", ""),
        "_commit_info": {"id": commit.get("hash", "")},
    }

    entries = []
    for b in data.get("benchmarks", []):
        entries.append(
            {
                "name": b["name"],
                "mean": b["mean"],
                "median": b["median"],
                "stddev": b["stddev"],
                "ci_lower": b.get("ci_lower", b["mean"]),
                "ci_upper": b.get("ci_upper", b["mean"]),
                "min": b.get("min", 0),
                "max": b.get("max", 0),
                "rounds": b.get("rounds", 0),
                "group": b.get("group", ""),
                "impl": b.get("impl", ""),
            }
        )

    return info, entries


def _load_json_as_standard(path: Path) -> tuple[dict, list[dict]]:
    """Load a legacy pytest-benchmark JSON as (metadata, entries)."""
    with open(path) as f:
        data = json.load(f)
    info = _get_version_info(data)
    info["_commit_info"] = data.get("commit_info", {})
    entries = parse_results(data)
    return info, _enrich_entries(entries)


def _load_result(name_or_path: str) -> tuple[dict, list[dict]]:
    """Load benchmark results by saved name or file path.

    Returns (metadata, entries). Supports .toml and .json (legacy).
    """
    path = Path(name_or_path)
    # Direct path
    if path.is_file():
        if path.suffix == ".toml":
            return _read_toml(path)
        return _load_json_as_standard(path)
    # Try as saved name (TOML first, then JSON fallback)
    toml_path = RESULTS_DIR / f"{name_or_path}.toml"
    if toml_path.exists():
        return _read_toml(toml_path)
    json_path = RESULTS_DIR / f"{name_or_path}.json"
    if json_path.exists():
        return _load_json_as_standard(json_path)
    print(
        f"Result '{name_or_path}' not found (tried as path and saved name in {RESULTS_DIR})",
        file=sys.stderr,
    )
    sys.exit(1)


# ---------------------------------------------------------------------------
# Pair Python vs Rust benchmarks
# ---------------------------------------------------------------------------


def _classify(name: str) -> tuple[str, str, str]:
    """Classify a benchmark name into (group, operation, impl).

    Returns (group, operation, implementation) where implementation
    is one of: 'python', 'rust', 'rust_e2e', 'serde'.
    """
    # Determine implementation
    if name.endswith("_python"):
        impl_tag = "python"
        base = name.removesuffix("_python")
    elif name.endswith("_e2e_rust"):
        impl_tag = "rust_e2e"
        base = name.removesuffix("_e2e_rust")
    elif name.endswith("_rust"):
        impl_tag = "rust"
        base = name.removesuffix("_rust")
    elif "serde_" in name or "serde_overhead" in name:
        impl_tag = "serde"
        base = name
    else:
        impl_tag = "other"
        base = name

    # Determine group
    if "validate" in name:
        group = "validation"
    elif "serde" in name:
        group = "serde_overhead"
    else:
        group = "compilation"

    return group, base, impl_tag


def build_comparison(entries: list[dict]) -> list[dict]:
    """Build paired comparison rows: Python, Rust (pre-serialized), Rust E2E, Serde."""
    by_base: dict[str, dict[str, dict]] = {}
    base_group: dict[str, str] = {}

    for entry in entries:
        group, base, impl_tag = _classify(entry["name"])
        by_base.setdefault(base, {})[impl_tag] = entry
        base_group[base] = group

    rows = []
    for base, impls in sorted(by_base.items()):
        py = impls.get("python")
        rust = impls.get("rust")
        rust_e2e = impls.get("rust_e2e")
        serde = impls.get("serde")

        row: dict = {"operation": _format_operation_name(base), "group": base_group[base]}

        if py:
            row["python_mean"] = py["mean"]
            row["python_median"] = py["median"]
        if rust:
            row["rust_mean"] = rust["mean"]
            row["rust_median"] = rust["median"]
        if rust_e2e:
            row["e2e_mean"] = rust_e2e["mean"]
            row["e2e_median"] = rust_e2e["median"]
        if serde:
            row["serde_mean"] = serde["mean"]
            row["serde_median"] = serde["median"]

        # Speedup: Python / Rust (pre-serialized dict path)
        if py and rust:
            row["speedup_rust"] = py["mean"] / rust["mean"]
        # E2E speedup (includes serde conversion)
        if py and rust_e2e:
            row["speedup_e2e"] = py["mean"] / rust_e2e["mean"]

        rows.append(row)

    return rows


def _format_operation_name(base: str) -> str:
    """Clean up a base benchmark name into a readable operation label."""
    name = base.removeprefix("test_compile_").removeprefix("test_validate_")
    name = name.removeprefix("test_serde_conversion_overhead_").removeprefix("test_serde_overhead_")
    return name.replace("_", " ").title()


# ---------------------------------------------------------------------------
# Time formatting
# ---------------------------------------------------------------------------


def _fmt_time(seconds: float) -> str:
    """Format a time value with appropriate unit."""
    if seconds < 1e-6:
        return f"{seconds * 1e9:.2f} ns"
    elif seconds < 1e-3:
        return f"{seconds * 1e6:.2f} us"
    elif seconds < 1:
        return f"{seconds * 1e3:.2f} ms"
    else:
        return f"{seconds:.2f} s"


def _fmt_speedup(val: float | None) -> str:
    if val is None:
        return "-"
    if val >= 1:
        return f"{val:.1f}x"
    else:
        return f"1/{1 / val:.1f}x"


def _compute_ci(mean: float, stddev: float, rounds: int) -> tuple[float, float]:
    """Compute 95% confidence interval: mean +/- 1.96 * stddev / sqrt(n)."""
    if rounds <= 1:
        return (mean, mean)
    margin = 1.96 * stddev / math.sqrt(rounds)
    return (mean - margin, mean + margin)


def _fmt_ci(ci_lower: float, ci_upper: float) -> str:
    """Format a confidence interval as [lower, upper] with matching units."""
    return f"[{_fmt_time(ci_lower)}, {_fmt_time(ci_upper)}]"


# ---------------------------------------------------------------------------
# Version and environment info
# ---------------------------------------------------------------------------


def _get_version_info(data: dict) -> dict:
    """Extract version and environment info from benchmark data and system."""
    commit_info = data.get("commit_info", {})
    machine_info = data.get("machine_info", {})
    cpu_info = machine_info.get("cpu", {})

    # Git commit
    commit_id = commit_info.get("id", "")
    commit_short = commit_id[:7] if commit_id else "unknown"
    commit_dirty = commit_info.get("dirty", False)
    branch = commit_info.get("branch", "unknown")

    # Rust version (best-effort)
    rust_version = "unknown"
    try:
        result = subprocess.run(["rustc", "--version"], capture_output=True, text=True, timeout=5)
        if result.returncode == 0:
            rust_version = result.stdout.strip().removeprefix("rustc ")
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    # Python info
    python_ver = machine_info.get("python_implementation_version", "unknown")
    python_impl = machine_info.get("python_implementation", "CPython")
    python_compiler = machine_info.get("python_compiler", "unknown")

    # System info
    system = machine_info.get("system", "unknown")
    release = machine_info.get("release", "unknown")
    machine_arch = machine_info.get("machine", "unknown")
    node = machine_info.get("node", "unknown")

    # CPU info
    cpu_brand = (
        cpu_info.get("brand_raw", "unknown") if isinstance(cpu_info, dict) else str(cpu_info)
    )
    cpu_count = cpu_info.get("count", "?") if isinstance(cpu_info, dict) else "?"

    # Benchmark datetime
    bench_datetime = data.get("datetime", "")

    return {
        "commit_short": commit_short,
        "commit_dirty": commit_dirty,
        "branch": branch,
        "rust_version": rust_version,
        "python_ver": python_ver,
        "python_impl": python_impl,
        "python_compiler": python_compiler,
        "system": system,
        "release": release,
        "machine_arch": machine_arch,
        "node": node,
        "cpu_brand": cpu_brand,
        "cpu_count": cpu_count,
        "bench_datetime": bench_datetime,
        "_commit_info": commit_info,
    }


# ---------------------------------------------------------------------------
# Detailed per-benchmark tables
# ---------------------------------------------------------------------------


def _generate_detailed_tables(entries: list[dict]) -> list[str]:
    """Generate detailed statistics tables grouped by category."""
    lines: list[str] = []

    # Group entries by category
    groups: dict[str, list[dict]] = {}
    for entry in entries:
        group, _base, _impl_tag = _classify(entry["name"])
        groups.setdefault(group, []).append(entry)

    group_order = [
        ("validation", "Validation"),
        ("compilation", "Compilation"),
        ("serde_overhead", "Serde Overhead"),
    ]

    for group_key, group_title in group_order:
        group_entries = groups.get(group_key, [])
        if not group_entries:
            continue

        lines.append(f"### {group_title}")
        lines.append("")
        lines.append("| Benchmark | Mean | Median | Std Dev | CI (95%) | Rounds |")
        lines.append("|-----------|------|--------|---------|----------|--------|")

        for entry in sorted(group_entries, key=lambda e: e["name"]):
            name = entry["name"].removeprefix("test_")
            mean = entry["mean"]
            median = entry["median"]
            stddev = entry["stddev"]
            rounds = entry["rounds"]
            ci_lo, ci_hi = _compute_ci(mean, stddev, rounds)

            lines.append(
                f"| {name} "
                f"| {_fmt_time(mean)} "
                f"| {_fmt_time(median)} "
                f"| {_fmt_time(stddev)} "
                f"| {_fmt_ci(ci_lo, ci_hi)} "
                f"| {rounds:,} |"
            )

        lines.append("")

    return lines


def _generate_criterion_table(criterion_entries: list[dict]) -> list[str]:
    """Generate detailed criterion (native Rust) benchmark table."""
    if not criterion_entries:
        return []

    lines: list[str] = []
    lines.append("## Native Rust Benchmarks (Criterion)")
    lines.append("")
    lines.append("Direct Rust execution without Python/serde overhead.")
    lines.append("")
    lines.append("| Benchmark | Mean | Median | Std Dev | CI (95%) |")
    lines.append("|-----------|------|--------|---------|----------|")

    for entry in criterion_entries:
        lines.append(
            f"| {entry['name']} "
            f"| {_fmt_time(entry['mean'])} "
            f"| {_fmt_time(entry['median'])} "
            f"| {_fmt_time(entry['stddev'])} "
            f"| {_fmt_ci(entry['ci_lower'], entry['ci_upper'])} |"
        )

    lines.append("")
    return lines


# ---------------------------------------------------------------------------
# Report generation
# ---------------------------------------------------------------------------


def generate_report(entries: list[dict], data: dict) -> str:
    """Generate a markdown comparison report."""
    lines: list[str] = []
    info = _get_version_info(data)

    # Header with version line
    now = datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    dirty_flag = " (dirty)" if info["commit_dirty"] else ""
    lines.append("# Benchmark Report: Python vs Rust Core")
    lines.append("")
    lines.append(
        f"**{now}** | "
        f"Commit: `{info['commit_short']}`{dirty_flag} | "
        f"Branch: `{info['branch']}` | "
        f"Rust: {info['rust_version']}"
    )
    lines.append("")

    # Environment table
    rust_available = any("_rust" in e["name"] for e in entries)
    rust_core_status = "Available" if rust_available else "Not installed (Python-only)"

    lines.append("## Environment")
    lines.append("")
    lines.append("| Property | Value |")
    lines.append("|----------|-------|")
    lines.append(f"| **OS** | {info['system']} {info['release']} ({info['machine_arch']}) |")
    lines.append(f"| **CPU** | {info['cpu_brand']} ({info['cpu_count']} cores) |")
    lines.append(f"| **Host** | {info['node']} |")
    lines.append(
        f"| **Python** | {info['python_impl']} {info['python_ver']} ({info['python_compiler']}) |"
    )
    lines.append(f"| **Rust** | {info['rust_version']} |")
    lines.append(f"| **Rust core** | {rust_core_status} |")
    lines.append("")

    rows = build_comparison(entries)

    # --- Validation section ---
    val_rows = [r for r in rows if r.get("group") == "validation"]
    if val_rows:
        lines.append("## Validation")
        lines.append("")
        lines.append("| Operation | Python | Rust | Speedup |")
        lines.append("|-----------|--------|------|---------|")
        for r in val_rows:
            py_str = _fmt_time(r["python_mean"]) if "python_mean" in r else "-"
            rust_str = _fmt_time(r["rust_mean"]) if "rust_mean" in r else "-"
            speedup = _fmt_speedup(r.get("speedup_rust"))
            lines.append(f"| {r['operation']} | {py_str} | {rust_str} | {speedup} |")
        lines.append("")

    # --- Compilation section ---
    comp_rows = [
        r
        for r in rows
        if r.get("group") == "compilation" and "python_mean" in r and "rust_mean" in r
    ]
    if comp_rows:
        lines.append("## Compilation (Python vs Rust via serde bridge)")
        lines.append("")
        lines.append("| Operation | Python | Rust (dict) | E2E (dict+Rust) | Speedup (dict) |")
        lines.append("|-----------|--------|-------------|-----------------|----------------|")
        for r in comp_rows:
            py_str = _fmt_time(r["python_mean"])
            rust_str = _fmt_time(r["rust_mean"])
            e2e_str = _fmt_time(r["e2e_mean"]) if "e2e_mean" in r else "-"
            speedup = _fmt_speedup(r.get("speedup_rust"))
            lines.append(f"| {r['operation']} | {py_str} | {rust_str} | {e2e_str} | {speedup} |")
        lines.append("")

    # --- Serde overhead section ---
    serde_rows = [r for r in rows if "serde_mean" in r]
    if serde_rows:
        lines.append("## Serde Conversion Overhead")
        lines.append("")
        lines.append("Time spent converting Python AST to dicts (before Rust call).")
        lines.append("")
        lines.append("| Operation | Serde Time |")
        lines.append("|-----------|------------|")
        for r in serde_rows:
            lines.append(f"| {r['operation']} | {_fmt_time(r['serde_mean'])} |")
        lines.append("")

    # --- Detailed per-benchmark statistics ---
    lines.append("## Detailed Results")
    lines.append("")
    lines.extend(_generate_detailed_tables(entries))

    # --- Native Rust criterion results (if available on disk) ---
    criterion_entries = parse_criterion_results()
    if criterion_entries:
        lines.extend(_generate_criterion_table(criterion_entries))

    # --- Summary ---
    lines.append("## Key Takeaways")
    lines.append("")

    if comp_rows:
        speedups = [r["speedup_rust"] for r in comp_rows if "speedup_rust" in r]
        if speedups:
            avg_speedup = sum(speedups) / len(speedups)
            if avg_speedup > 1:
                lines.append(
                    f"- Rust compilation via serde bridge: **{avg_speedup:.1f}x** average "
                    f"speedup (pre-serialized dicts)"
                )
            else:
                lines.append(
                    f"- Rust compilation via serde bridge: **{avg_speedup:.2f}x** average "
                    f"(serde overhead dominates at current query sizes)"
                )

        e2e_speedups = [r["speedup_e2e"] for r in comp_rows if "speedup_e2e" in r]
        if e2e_speedups:
            avg_e2e = sum(e2e_speedups) / len(e2e_speedups)
            lines.append(f"- End-to-end (including dict conversion): **{avg_e2e:.2f}x** average")

    if val_rows:
        val_speedups = [r["speedup_rust"] for r in val_rows if "speedup_rust" in r]
        if val_speedups:
            avg_val = sum(val_speedups) / len(val_speedups)
            lines.append(f"- Validation: **{avg_val:.1f}x** average speedup")

    if serde_rows and comp_rows:
        lines.append("- Serde dict conversion is the primary bottleneck for the Rust path")
        lines.append(
            "- Direct Rust AST construction (bypassing Python dicts) would unlock "
            "the full 5-10x speedup seen in criterion benchmarks"
        )

    lines.append("")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Compare two saved results
# ---------------------------------------------------------------------------


def generate_comparison(
    baseline_info: dict,
    baseline_entries: list[dict],
    current_info: dict,
    current_entries: list[dict],
) -> str:
    """Generate a detailed comparison report between two benchmark runs."""
    baseline_by_name = {e["name"]: e for e in baseline_entries}
    current_by_name = {e["name"]: e for e in current_entries}

    all_names = sorted(set(baseline_by_name) | set(current_by_name))

    lines: list[str] = []
    lines.append("# Benchmark Comparison Report")
    lines.append("")

    # Version lines for both runs
    b_dirty = " (dirty)" if baseline_info.get("commit_dirty") else ""
    c_dirty = " (dirty)" if current_info.get("commit_dirty") else ""
    lines.append(
        f"**Baseline:** `{baseline_info['commit_short']}`{b_dirty}"
        f" ({baseline_info['branch']}) | "
        f"Rust: {baseline_info['rust_version']}"
    )
    lines.append(
        f"**Current:** `{current_info['commit_short']}`{c_dirty}"
        f" ({current_info['branch']}) | "
        f"Rust: {current_info['rust_version']}"
    )
    lines.append("")

    # --- Environment comparison table ---
    lines.append("## Environment")
    lines.append("")
    lines.append("| Property | Baseline | Current |")
    lines.append("|----------|----------|---------|")
    lines.append(f"| **Host** | {baseline_info['node']} | {current_info['node']} |")
    lines.append(
        f"| **CPU** | {baseline_info['cpu_brand']} ({baseline_info['cpu_count']} cores)"
        f" | {current_info['cpu_brand']} ({current_info['cpu_count']} cores) |"
    )
    lines.append(
        f"| **OS** | {baseline_info['system']} {baseline_info['release']}"
        f" ({baseline_info['machine_arch']})"
        f" | {current_info['system']} {current_info['release']}"
        f" ({current_info['machine_arch']}) |"
    )
    b_py = baseline_info.get("python_ver", "unknown")
    c_py = current_info.get("python_ver", "unknown")
    lines.append(f"| **Python** | {b_py} | {c_py} |")
    lines.append(f"| **Rust** | {baseline_info['rust_version']} | {current_info['rust_version']} |")
    lines.append("")

    # --- Summary comparison table ---
    lines.append("## Summary")
    lines.append("")
    lines.append("| Test | Baseline (Mean) | Current (Mean) | Change |")
    lines.append("|------|-----------------|----------------|--------|")

    regressions = 0
    improvements = 0
    matched = 0
    added = 0
    removed = 0

    for name in all_names:
        b = baseline_by_name.get(name)
        c = current_by_name.get(name)
        short_name = name.removeprefix("test_")

        if b and c:
            matched += 1
            b_mean = b["mean"]
            c_mean = c["mean"]
            pct = ((c_mean - b_mean) / b_mean) * 100
            if pct > 5:
                marker = " :warning:"
                regressions += 1
            elif pct < -5:
                marker = " :rocket:"
                improvements += 1
            else:
                marker = ""
            lines.append(
                f"| {short_name} | {_fmt_time(b_mean)} | {_fmt_time(c_mean)} "
                f"| {pct:+.1f}%{marker} |"
            )
        elif b:
            removed += 1
            lines.append(f"| {short_name} | {_fmt_time(b['mean'])} | *removed* | - |")
        else:
            added += 1
            assert c is not None
            lines.append(f"| {short_name} | *new* | {_fmt_time(c['mean'])} | - |")

    lines.append("")
    lines.append(f"**Regressions (>5%):** {regressions}  ")
    lines.append(f"**Improvements (>5%):** {improvements}  ")
    lines.append("")

    # --- Detailed comparison grouped by category ---
    lines.append("## Detailed Comparison")
    lines.append("")

    # Group names by category
    groups: dict[str, list[str]] = {}
    for name in all_names:
        group, _base, _impl_tag = _classify(name)
        groups.setdefault(group, []).append(name)

    group_order = [
        ("validation", "Validation"),
        ("compilation", "Compilation"),
        ("serde_overhead", "Serde Overhead"),
    ]

    for group_key, group_title in group_order:
        group_names = groups.get(group_key, [])
        if not group_names:
            continue

        lines.append(f"### {group_title}")
        lines.append("")
        lines.append(
            "| Benchmark "
            "| Baseline Mean | Baseline CI (95%) "
            "| Current Mean | Current CI (95%) "
            "| Change |"
        )
        lines.append(
            "|-----------|"
            "---------------|-------------------|"
            "--------------|------------------|"
            "--------|"
        )

        for name in group_names:
            b = baseline_by_name.get(name)
            c = current_by_name.get(name)
            short_name = name.removeprefix("test_")

            if b and c:
                b_ci = _compute_ci(b["mean"], b["stddev"], b["rounds"])
                c_ci = _compute_ci(c["mean"], c["stddev"], c["rounds"])
                pct = ((c["mean"] - b["mean"]) / b["mean"]) * 100
                if pct > 5:
                    marker = " :warning:"
                elif pct < -5:
                    marker = " :rocket:"
                else:
                    marker = ""
                lines.append(
                    f"| {short_name} "
                    f"| {_fmt_time(b['mean'])} "
                    f"| {_fmt_ci(*b_ci)} "
                    f"| {_fmt_time(c['mean'])} "
                    f"| {_fmt_ci(*c_ci)} "
                    f"| {pct:+.1f}%{marker} |"
                )
            elif b:
                b_ci = _compute_ci(b["mean"], b["stddev"], b["rounds"])
                lines.append(
                    f"| {short_name} "
                    f"| {_fmt_time(b['mean'])} "
                    f"| {_fmt_ci(*b_ci)} "
                    f"| *removed* | - | - |"
                )
            else:
                assert c is not None
                c_ci = _compute_ci(c["mean"], c["stddev"], c["rounds"])
                lines.append(
                    f"| {short_name} | *new* | - | {_fmt_time(c['mean'])} | {_fmt_ci(*c_ci)} | - |"
                )

        lines.append("")

    # --- Test count footer ---
    lines.append(f"**Matched:** {matched} | **Added:** {added} | **Removed:** {removed}")
    lines.append("")

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main():
    parser = argparse.ArgumentParser(description="Run benchmarks and generate reports")
    parser.add_argument(
        "--suite",
        choices=["validation", "compilation"],
        help="Run only a subset of benchmarks",
    )
    parser.add_argument(
        "--save",
        metavar="NAME",
        help="Save results with a custom name (default: auto-tag {host}_{commit})",
    )
    parser.add_argument(
        "--compare",
        metavar="NAME",
        help="Compare current run against a saved baseline",
    )
    parser.add_argument(
        "--diff",
        nargs=2,
        metavar=("BASELINE", "CURRENT"),
        help="Compare two saved results (names or file paths) without running benchmarks",
    )
    parser.add_argument(
        "--report",
        metavar="PATH",
        help="Write a markdown report to the given file path",
    )
    parser.add_argument(
        "--json",
        metavar="PATH",
        help="Write raw JSON results to the given path",
    )
    parser.add_argument(
        "--run-criterion",
        action="store_true",
        help="Run Rust criterion benchmarks before generating the report",
    )
    args = parser.parse_args()

    # --- Diff-only mode: compare two saved results, no benchmark run ---
    if args.diff:
        b_meta, b_entries = _load_result(args.diff[0])
        c_meta, c_entries = _load_result(args.diff[1])
        report = generate_comparison(b_meta, b_entries, c_meta, c_entries)

        if args.report:
            Path(args.report).write_text(report)
            print(f"Report written to {args.report}", file=sys.stderr)
        print(report)
        return

    # Optionally run criterion benchmarks first
    if args.run_criterion:
        print("Running Rust criterion benchmarks...", file=sys.stderr)
        subprocess.run(
            ["cargo", "bench"],
            cwd=PROJECT_ROOT / "type-bridge-core",
            check=False,
        )

    json_path = Path(args.json) if args.json else None
    save_name = args.save

    # If saving, also save as named JSON (for pytest-benchmark compat)
    if save_name:
        RESULTS_DIR.mkdir(parents=True, exist_ok=True)
        json_path = json_path or RESULTS_DIR / f"{save_name}.json"

    # Run the benchmarks
    data = run_benchmarks(suite=args.suite, save_name=save_name, json_path=json_path)
    entries = parse_results(data)

    # Auto-save TOML (always)
    info = _get_version_info(data)
    auto_name = f"{info['node']}_{info['commit_short']}"
    toml_name = save_name or auto_name
    toml_path = RESULTS_DIR / f"{toml_name}.toml"
    enriched = _enrich_entries(entries)
    write_toml(toml_path, info, enriched, toml_name)
    print(f"Results saved to {toml_path}", file=sys.stderr)

    # Generate main report
    report = generate_report(entries, data)

    # If comparing against a baseline
    if args.compare:
        b_meta, b_entries = _load_result(args.compare)
        c_meta = info
        c_entries = enriched
        comparison = generate_comparison(b_meta, b_entries, c_meta, c_entries)
        report += "\n---\n\n" + comparison

    # Output
    if args.report:
        Path(args.report).write_text(report)
        print(f"Report written to {args.report}", file=sys.stderr)

    print(report)


if __name__ == "__main__":
    main()
