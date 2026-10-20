"""Benchmarks: query compilation in the Rust core, with and without marshalling."""

from __future__ import annotations

from type_bridge.query.compiler import _clause_to_dict

# ---------------------------------------------------------------------------
# Simple match: $p isa person
# ---------------------------------------------------------------------------


def test_compile_simple_match(benchmark, compiler, simple_match):
    d = _clause_to_dict(simple_match)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


# ---------------------------------------------------------------------------
# Match with constraints (IID + 2 has)
# ---------------------------------------------------------------------------


def test_compile_constrained_match(benchmark, compiler, match_with_constraints):
    d = _clause_to_dict(match_with_constraints)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


# ---------------------------------------------------------------------------
# Complex query: 10 patterns (entity, relation, not, or, has, comparison)
# ---------------------------------------------------------------------------


def test_compile_complex_query(benchmark, compiler, complex_query):
    d = _clause_to_dict(complex_query)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


# ---------------------------------------------------------------------------
# Relation insert with role players + inline attributes
# ---------------------------------------------------------------------------


def test_compile_relation_insert(benchmark, compiler, relation_insert):
    d = _clause_to_dict(relation_insert)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


# ---------------------------------------------------------------------------
# Batch: 50 mixed clauses
# ---------------------------------------------------------------------------


def test_compile_batch(benchmark, compiler, batch_clauses):
    dicts = [_clause_to_dict(c) for c in batch_clauses]

    def _run():
        compiler.compile_dicts(dicts)

    benchmark(_run)


# ---------------------------------------------------------------------------
# Serde conversion overhead (dict building only, no native call)
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
# End-to-end: serde conversion + native compilation (measures real-world cost)
# ---------------------------------------------------------------------------


def test_compile_e2e_complex(benchmark, compiler, complex_query):
    """Measures dict conversion + Rust deserialization + compilation together."""

    def _run():
        d = _clause_to_dict(complex_query)
        compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_e2e_batch(benchmark, compiler, batch_clauses):
    """Measures dict conversion + Rust deserialization + compilation for 50 clauses."""

    def _run():
        dicts = [_clause_to_dict(c) for c in batch_clauses]
        compiler.compile_dicts(dicts)

    benchmark(_run)


# ===========================================================================
# Long query: 30 patterns (wide join)
# ===========================================================================


def test_compile_long_query(benchmark, compiler, long_query):
    d = _clause_to_dict(long_query)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_long_query_e2e(benchmark, compiler, long_query):
    def _run():
        d = _clause_to_dict(long_query)
        compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Deeply nested query: or-of-not-of-or, 3 levels
# ===========================================================================


def test_compile_deeply_nested(benchmark, compiler, deeply_nested_query):
    d = _clause_to_dict(deeply_nested_query)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_deeply_nested_e2e(benchmark, compiler, deeply_nested_query):
    def _run():
        d = _clause_to_dict(deeply_nested_query)
        compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Graph traversal: 8-hop chain of relations
# ===========================================================================


def test_compile_graph_traversal(benchmark, compiler, graph_traversal_query):
    d = _clause_to_dict(graph_traversal_query)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_graph_traversal_e2e(benchmark, compiler, graph_traversal_query):
    def _run():
        d = _clause_to_dict(graph_traversal_query)
        compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Heavy insert: 100 entities x 5 attributes = 600 statements
# ===========================================================================


def test_compile_heavy_insert(benchmark, compiler, heavy_insert):
    d = _clause_to_dict(heavy_insert)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_heavy_insert_e2e(benchmark, compiler, heavy_insert):
    def _run():
        d = _clause_to_dict(heavy_insert)
        compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Large fetch: 20 mixed fetch items
# ===========================================================================


def test_compile_large_fetch(benchmark, compiler, large_fetch):
    d = _clause_to_dict(large_fetch)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Reduce: 5 aggregations with groupby
# ===========================================================================


def test_compile_reduce(benchmark, compiler, reduce_query):
    d = _clause_to_dict(reduce_query)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Realistic pipeline: match (9 patterns) + fetch (5 items) — 2 clauses
# ===========================================================================


def test_compile_realistic_pipeline(benchmark, compiler, realistic_pipeline):
    dicts = [_clause_to_dict(c) for c in realistic_pipeline]

    def _run():
        compiler.compile_dicts(dicts)

    benchmark(_run)


def test_compile_realistic_pipeline_e2e(benchmark, compiler, realistic_pipeline):
    def _run():
        dicts = [_clause_to_dict(c) for c in realistic_pipeline]
        compiler.compile_dicts(dicts)

    benchmark(_run)


# ===========================================================================
# Large batch: 200 mixed clauses
# ===========================================================================


def test_compile_large_batch(benchmark, compiler, large_batch):
    dicts = [_clause_to_dict(c) for c in large_batch]

    def _run():
        compiler.compile_dicts(dicts)

    benchmark(_run)


def test_compile_large_batch_e2e(benchmark, compiler, large_batch):
    def _run():
        dicts = [_clause_to_dict(c) for c in large_batch]
        compiler.compile_dicts(dicts)

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


def test_compile_arithmetic_match(benchmark, compiler, arithmetic_match):
    d = _clause_to_dict(arithmetic_match)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_nested_arithmetic(benchmark, compiler, nested_arithmetic):
    d = _clause_to_dict(nested_arithmetic)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# MatchLetClause compilation
# ===========================================================================


def test_compile_match_let_single(benchmark, compiler, match_let_single):
    d = _clause_to_dict(match_let_single)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_match_let_multiple(benchmark, compiler, match_let_multiple):
    d = _clause_to_dict(match_let_multiple)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# FetchVariable compilation
# ===========================================================================


def test_compile_fetch_variable(benchmark, compiler, fetch_with_variable):
    d = _clause_to_dict(fetch_with_variable)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# IsaConstraint compilation
# ===========================================================================


def test_compile_isa_constraint(benchmark, compiler, isa_constraint_match):
    d = _clause_to_dict(isa_constraint_match)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


# ===========================================================================
# Standalone clause types
# ===========================================================================


def test_compile_standalone_insert(benchmark, compiler, standalone_insert):
    d = _clause_to_dict(standalone_insert)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_standalone_delete(benchmark, compiler, standalone_delete):
    d = _clause_to_dict(standalone_delete)

    def _run():
        compiler.compile_dicts([d])

    benchmark(_run)


def test_compile_standalone_update(benchmark, compiler, standalone_update):
    d = _clause_to_dict(standalone_update)

    def _run():
        compiler.compile_dicts([d])

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
