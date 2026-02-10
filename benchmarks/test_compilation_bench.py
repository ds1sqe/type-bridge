"""Benchmarks: Python vs Rust query compilation performance."""

from __future__ import annotations

import pytest

from type_bridge.query.ast import Clause
from type_bridge.query.compiler import (
    QueryCompiler,
    _clause_to_dict,
)

pytestmark = pytest.mark.benchmark


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _compile_python(compiler: QueryCompiler, clause: Clause) -> str:
    """Force the Python codepath by calling the private method directly."""
    return compiler._compile_clause(clause)


def _compile_python_batch(compiler: QueryCompiler, clauses: list[Clause]) -> str:
    """Force Python codepath for a batch."""
    parts = [compiler._compile_clause(c) for c in clauses]
    return "\n".join(parts)


# ---------------------------------------------------------------------------
# Simple match: $p isa person
# ---------------------------------------------------------------------------


def test_compile_simple_match_python(benchmark, python_compiler, simple_match):
    benchmark(_compile_python, python_compiler, simple_match)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_simple_match_rust(benchmark, rust_compiler, simple_match):
    d = _clause_to_dict(simple_match)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ---------------------------------------------------------------------------
# Match with constraints (IID + 2 has)
# ---------------------------------------------------------------------------


def test_compile_constrained_match_python(benchmark, python_compiler, match_with_constraints):
    benchmark(_compile_python, python_compiler, match_with_constraints)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_constrained_match_rust(benchmark, rust_compiler, match_with_constraints):
    d = _clause_to_dict(match_with_constraints)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ---------------------------------------------------------------------------
# Complex query: 10 patterns (entity, relation, not, or, has, comparison)
# ---------------------------------------------------------------------------


def test_compile_complex_query_python(benchmark, python_compiler, complex_query):
    benchmark(_compile_python, python_compiler, complex_query)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_complex_query_rust(benchmark, rust_compiler, complex_query):
    d = _clause_to_dict(complex_query)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ---------------------------------------------------------------------------
# Relation insert with role players + inline attributes
# ---------------------------------------------------------------------------


def test_compile_relation_insert_python(benchmark, python_compiler, relation_insert):
    benchmark(_compile_python, python_compiler, relation_insert)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_relation_insert_rust(benchmark, rust_compiler, relation_insert):
    d = _clause_to_dict(relation_insert)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ---------------------------------------------------------------------------
# Batch: 50 mixed clauses
# ---------------------------------------------------------------------------


def test_compile_batch_python(benchmark, python_compiler, batch_clauses):
    benchmark(_compile_python_batch, python_compiler, batch_clauses)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_batch_rust(benchmark, rust_compiler, batch_clauses):
    dicts = [_clause_to_dict(c) for c in batch_clauses]

    def _run():
        rust_compiler.compile_dicts(dicts)

    benchmark(_run)


# ---------------------------------------------------------------------------
# Serde conversion overhead (dict building only, no Rust call)
# ---------------------------------------------------------------------------


def test_serde_conversion_overhead_simple(benchmark, simple_match):
    benchmark(_clause_to_dict, simple_match)


def test_serde_conversion_overhead_complex(benchmark, complex_query):
    benchmark(_clause_to_dict, complex_query)


def test_serde_conversion_overhead_batch(benchmark, batch_clauses):
    def _convert():
        for c in batch_clauses:
            _clause_to_dict(c)

    benchmark(_convert)


# ---------------------------------------------------------------------------
# End-to-end: serde conversion + Rust compilation (measures real-world cost)
# ---------------------------------------------------------------------------


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_e2e_complex_rust(benchmark, rust_compiler, complex_query):
    """Measures dict conversion + Rust deserialization + compilation together."""

    def _run():
        d = _clause_to_dict(complex_query)
        rust_compiler.compile_dicts([d])

    benchmark(_run)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_e2e_batch_rust(benchmark, rust_compiler, batch_clauses):
    """Measures dict conversion + Rust deserialization + compilation for 50 clauses."""

    def _run():
        dicts = [_clause_to_dict(c) for c in batch_clauses]
        rust_compiler.compile_dicts(dicts)

    benchmark(_run)


# ===========================================================================
# Long query: 30 patterns (wide join)
# ===========================================================================


def test_compile_long_query_python(benchmark, python_compiler, long_query):
    benchmark(_compile_python, python_compiler, long_query)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_long_query_rust(benchmark, rust_compiler, long_query):
    d = _clause_to_dict(long_query)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_long_query_e2e_rust(benchmark, rust_compiler, long_query):
    def _run():
        d = _clause_to_dict(long_query)
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Deeply nested query: or-of-not-of-or, 3 levels
# ===========================================================================


def test_compile_deeply_nested_python(benchmark, python_compiler, deeply_nested_query):
    benchmark(_compile_python, python_compiler, deeply_nested_query)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_deeply_nested_rust(benchmark, rust_compiler, deeply_nested_query):
    d = _clause_to_dict(deeply_nested_query)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_deeply_nested_e2e_rust(benchmark, rust_compiler, deeply_nested_query):
    def _run():
        d = _clause_to_dict(deeply_nested_query)
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Graph traversal: 8-hop chain of relations
# ===========================================================================


def test_compile_graph_traversal_python(benchmark, python_compiler, graph_traversal_query):
    benchmark(_compile_python, python_compiler, graph_traversal_query)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_graph_traversal_rust(benchmark, rust_compiler, graph_traversal_query):
    d = _clause_to_dict(graph_traversal_query)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_graph_traversal_e2e_rust(benchmark, rust_compiler, graph_traversal_query):
    def _run():
        d = _clause_to_dict(graph_traversal_query)
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Heavy insert: 100 entities x 5 attributes = 600 statements
# ===========================================================================


def test_compile_heavy_insert_python(benchmark, python_compiler, heavy_insert):
    benchmark(_compile_python, python_compiler, heavy_insert)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_heavy_insert_rust(benchmark, rust_compiler, heavy_insert):
    d = _clause_to_dict(heavy_insert)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_heavy_insert_e2e_rust(benchmark, rust_compiler, heavy_insert):
    def _run():
        d = _clause_to_dict(heavy_insert)
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Large fetch: 20 mixed fetch items
# ===========================================================================


def test_compile_large_fetch_python(benchmark, python_compiler, large_fetch):
    benchmark(_compile_python, python_compiler, large_fetch)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_large_fetch_rust(benchmark, rust_compiler, large_fetch):
    d = _clause_to_dict(large_fetch)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Reduce: 5 aggregations with groupby
# ===========================================================================


def test_compile_reduce_python(benchmark, python_compiler, reduce_query):
    benchmark(_compile_python, python_compiler, reduce_query)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_reduce_rust(benchmark, rust_compiler, reduce_query):
    d = _clause_to_dict(reduce_query)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Realistic pipeline: match (9 patterns) + fetch (5 items) — 2 clauses
# ===========================================================================


def test_compile_realistic_pipeline_python(benchmark, python_compiler, realistic_pipeline):
    def _run():
        for clause in realistic_pipeline:
            python_compiler._compile_clause(clause)

    benchmark(_run)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_realistic_pipeline_rust(benchmark, rust_compiler, realistic_pipeline):
    dicts = [_clause_to_dict(c) for c in realistic_pipeline]

    def _run():
        rust_compiler.compile_dicts(dicts)

    benchmark(_run)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_realistic_pipeline_e2e_rust(benchmark, rust_compiler, realistic_pipeline):
    def _run():
        dicts = [_clause_to_dict(c) for c in realistic_pipeline]
        rust_compiler.compile_dicts(dicts)

    benchmark(_run)


# ===========================================================================
# Large batch: 200 mixed clauses
# ===========================================================================


def test_compile_large_batch_python(benchmark, python_compiler, large_batch):
    benchmark(_compile_python_batch, python_compiler, large_batch)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_large_batch_rust(benchmark, rust_compiler, large_batch):
    dicts = [_clause_to_dict(c) for c in large_batch]

    def _run():
        rust_compiler.compile_dicts(dicts)

    benchmark(_run)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_large_batch_e2e_rust(benchmark, rust_compiler, large_batch):
    def _run():
        dicts = [_clause_to_dict(c) for c in large_batch]
        rust_compiler.compile_dicts(dicts)

    benchmark(_run)


# ===========================================================================
# Serde overhead for new heavy fixtures
# ===========================================================================


def test_serde_overhead_long_query(benchmark, long_query):
    benchmark(_clause_to_dict, long_query)


def test_serde_overhead_deeply_nested(benchmark, deeply_nested_query):
    benchmark(_clause_to_dict, deeply_nested_query)


def test_serde_overhead_graph_traversal(benchmark, graph_traversal_query):
    benchmark(_clause_to_dict, graph_traversal_query)


def test_serde_overhead_heavy_insert(benchmark, heavy_insert):
    benchmark(_clause_to_dict, heavy_insert)


def test_serde_overhead_large_batch(benchmark, large_batch):
    def _convert():
        for c in large_batch:
            _clause_to_dict(c)

    benchmark(_convert)


# ===========================================================================
# ArithmeticValue compilation
# ===========================================================================


def test_compile_arithmetic_match_python(benchmark, python_compiler, arithmetic_match):
    benchmark(_compile_python, python_compiler, arithmetic_match)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_arithmetic_match_rust(benchmark, rust_compiler, arithmetic_match):
    d = _clause_to_dict(arithmetic_match)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_nested_arithmetic_python(benchmark, python_compiler, nested_arithmetic):
    benchmark(_compile_python, python_compiler, nested_arithmetic)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_nested_arithmetic_rust(benchmark, rust_compiler, nested_arithmetic):
    d = _clause_to_dict(nested_arithmetic)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# MatchLetClause compilation
# ===========================================================================


def test_compile_match_let_single_python(benchmark, python_compiler, match_let_single):
    benchmark(_compile_python, python_compiler, match_let_single)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_match_let_single_rust(benchmark, rust_compiler, match_let_single):
    d = _clause_to_dict(match_let_single)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_match_let_multiple_python(benchmark, python_compiler, match_let_multiple):
    benchmark(_compile_python, python_compiler, match_let_multiple)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_match_let_multiple_rust(benchmark, rust_compiler, match_let_multiple):
    d = _clause_to_dict(match_let_multiple)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# FetchVariable compilation
# ===========================================================================


def test_compile_fetch_variable_python(benchmark, python_compiler, fetch_with_variable):
    benchmark(_compile_python, python_compiler, fetch_with_variable)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_fetch_variable_rust(benchmark, rust_compiler, fetch_with_variable):
    d = _clause_to_dict(fetch_with_variable)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# IsaConstraint compilation
# ===========================================================================


def test_compile_isa_constraint_python(benchmark, python_compiler, isa_constraint_match):
    benchmark(_compile_python, python_compiler, isa_constraint_match)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_isa_constraint_rust(benchmark, rust_compiler, isa_constraint_match):
    d = _clause_to_dict(isa_constraint_match)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Standalone clause types
# ===========================================================================


def test_compile_standalone_insert_python(benchmark, python_compiler, standalone_insert):
    benchmark(_compile_python, python_compiler, standalone_insert)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_standalone_insert_rust(benchmark, rust_compiler, standalone_insert):
    d = _clause_to_dict(standalone_insert)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_standalone_delete_python(benchmark, python_compiler, standalone_delete):
    benchmark(_compile_python, python_compiler, standalone_delete)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_standalone_delete_rust(benchmark, rust_compiler, standalone_delete):
    d = _clause_to_dict(standalone_delete)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_standalone_update_python(benchmark, python_compiler, standalone_update):
    benchmark(_compile_python, python_compiler, standalone_update)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_compile_standalone_update_rust(benchmark, rust_compiler, standalone_update):
    d = _clause_to_dict(standalone_update)

    def _run():
        rust_compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Serde overhead — new AST nodes and standalone clause types
# ===========================================================================


def test_serde_overhead_arithmetic(benchmark, arithmetic_match):
    benchmark(_clause_to_dict, arithmetic_match)


def test_serde_overhead_match_let(benchmark, match_let_multiple):
    benchmark(_clause_to_dict, match_let_multiple)


def test_serde_overhead_fetch_variable(benchmark, fetch_with_variable):
    benchmark(_clause_to_dict, fetch_with_variable)


def test_serde_overhead_standalone_insert(benchmark, standalone_insert):
    benchmark(_clause_to_dict, standalone_insert)


def test_serde_overhead_standalone_delete(benchmark, standalone_delete):
    benchmark(_clause_to_dict, standalone_delete)


def test_serde_overhead_standalone_update(benchmark, standalone_update):
    benchmark(_clause_to_dict, standalone_update)
