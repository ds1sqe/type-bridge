"""The ownership probe catches syntax construction without flagging data."""

import json
from pathlib import Path

import pytest

from scripts.ci.check_typeql_text import python_violations, rust_violations


@pytest.mark.parametrize(
    "source",
    [
        'query = f"match {pattern};"',
        'query = "insert " + statement',
        'query = f"{variable} isa {label}"',
        'query = f"{variable} has {attribute} {value}"',
        'query = f"fetch {{ {fields} }};"',
        'query = f"sort {variable} desc;"',
        'query = f"@card({minimum}..{maximum})"',
    ],
)
def test_python_builders_are_rejected(source: str) -> None:
    assert python_violations(source) == [1]


@pytest.mark.parametrize(
    "source",
    [
        'query = "match $p isa person;"',
        'raise ValueError(f"Cannot insert {kind}: no @key attributes found")',
        'logger.debug(f"match {count} rows; query was {query}")',
        'message = f"Cannot update {kind}: no @key attributes found"',
        'identity = f"owns {owner} {attribute}"',
    ],
)
def test_python_inputs_and_diagnostics_are_allowed(source: str) -> None:
    assert python_violations(source) == []


@pytest.mark.parametrize(
    "source",
    [
        'let q = format!("match {pattern};");',
        'q.push_str(" has ");',
        'let q = concat!("ma", "tch ");',
        'let q = format!(r#"fetch {{"value": {value}}};"#);',
        'let q = format!("{variable} links ({players})");',
        'let q = format!("@doc({value})");',
    ],
)
def test_rust_builders_are_rejected(source: str) -> None:
    assert rust_violations(source) == [1]


@pytest.mark.parametrize(
    "source",
    [
        'let q = "match $p isa person;";',
        'let error = Err(format!("match {count} rows; invalid"));',
        'let value = path.ok_or_else(|| format!("{field} has no file name"));',
        'let value = path.map_err(|_| format!("{field} has no filesystem anchor"));',
        'let error = Error { message: format!("match {count}; invalid") };',
        'warning("limit", format!("@card permits {maximum}"), path);',
        'let diagnostic = format!("selected row has {actual} slots; expected {expected}");',
        'output.push_str("match __tb_token.as_str() { _ => None };\\n");',
        'let c = format!("#ifndef {guard}\\n#define {guard}\\n");',
        'let identity = format!("owns {owner} {attribute}");',
        '/* outer /* nested */ format!("match {x}"); */ fn f() {}',
        'let text = "format!(\\"match {pattern}\\")";',
    ],
)
def test_rust_inputs_diagnostics_and_generated_code_are_allowed(source: str) -> None:
    assert rust_violations(source) == []


def test_rust_test_scope_and_named_constant_are_bounded() -> None:
    source = """#[cfg(test)]
mod tests { fn example() { format!("match {pattern};"); } }
fn production() { format!("match {pattern};"); }
"""
    assert rust_violations(source) == [3]
    source = """const FROZEN: &str = concat!("define ", "entity person;");
const NEW: &str = concat!("define ", "entity company;");"""
    assert rust_violations(source, {"FROZEN"}) == [2]


def test_legacy_waivers_have_the_requested_removal_release() -> None:
    root = Path(__file__).resolve().parents[3]
    allowlist = json.loads((root / "scripts/ci/typeql_text_allowlist.json").read_text())
    assert set(allowlist["legacy_emitters"]) == {
        f"type-bridge-core/crates/orm/src/{name}"
        for name in (
            "manager/dynamic.rs",
            "manager/query_builder.rs",
            "dynamic.rs",
            "projected_batch_executor.rs",
            "session/legacy_writer.rs",
        )
    }
    assert all(
        entry["remove_in"] == "3.0" and entry["reason"]
        for entry in allowlist["legacy_emitters"].values()
    )
    assert "python scripts/ci/check_typeql_text.py" in (root / "scripts/check.sh").read_text()
