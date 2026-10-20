"""The AST dependency must not become part of a released signature."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    "check_typeql_public_api", ROOT / "scripts/ci/check_typeql_public_api.py"
)
assert SPEC and SPEC.loader
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)


@pytest.mark.parametrize(
    "source",
    [
        "pub fn leak() -> typeql::Query {}",
        "use typeql::Query as Official; pub fn leak(query: Official) {}",
        "pub struct Leak { pub query: typeql::Query }",
        "pub struct Leak(pub typeql::Query);",
        "pub enum Leak { Query(typeql::Query) }",
        "pub type Leak = typeql::Query;",
        "pub use typeql::Query;",
    ],
)
def test_rejects_public_upstream_types(source: str) -> None:
    assert PROBE.violations(source)


def test_private_fields_and_private_helpers_are_allowed() -> None:
    assert not PROBE.violations(
        "pub struct Query { pub(crate) inner: typeql::Query }\n"
        "pub struct PrivateTuple(typeql::Query);\n"
        "pub(crate) fn helper() -> typeql::Query {}\n"
        'pub fn safe() { let diagnostic = "typeql::Query"; }'
    )
