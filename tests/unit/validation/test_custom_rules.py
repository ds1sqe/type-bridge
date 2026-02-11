"""Tests for custom validation rules — JSON DSL (issue #108)."""

from __future__ import annotations

import json
from typing import Any

import pytest

from type_bridge.rules import RuleBuilder

HAS_RUST_CORE = False
ValidationEngine: Any = None
try:
    from type_bridge_core import ValidationEngine  # type: ignore[no-redef]

    HAS_RUST_CORE = True
except ImportError:
    pass


# ---------------------------------------------------------------------------
# Python-only tests (RuleBuilder serialization)
# ---------------------------------------------------------------------------


class TestRuleBuilder:
    """Tests for the Python RuleBuilder API."""

    def test_required_rule(self) -> None:
        rules = RuleBuilder().required("r1", entity="person", attribute="name").to_json()
        parsed = json.loads(rules)
        assert len(parsed["rules"]) == 1
        r = parsed["rules"][0]
        assert r["id"] == "r1"
        assert r["rule_type"]["type"] == "Required"
        assert r["target"]["type"] == "EntityAttribute"
        assert r["target"]["data"]["entity"] == "person"

    def test_regex_rule(self) -> None:
        rules = RuleBuilder().regex("r1", attribute="email", pattern=r"^.+@.+$").to_json()
        parsed = json.loads(rules)
        r = parsed["rules"][0]
        assert r["rule_type"]["type"] == "Regex"
        assert r["rule_type"]["data"]["pattern"] == r"^.+@.+$"
        assert r["target"]["type"] == "Attribute"

    def test_range_rule(self) -> None:
        rules = RuleBuilder().range("r1", attribute="age", min=0, max=150).to_json()
        parsed = json.loads(rules)
        r = parsed["rules"][0]
        assert r["rule_type"]["type"] == "Range"
        assert r["rule_type"]["data"]["min"] == 0
        assert r["rule_type"]["data"]["max"] == 150

    def test_values_rule(self) -> None:
        rules = RuleBuilder().values("r1", attribute="status", allowed=["a", "b"]).to_json()
        parsed = json.loads(rules)
        r = parsed["rules"][0]
        assert r["rule_type"]["type"] == "Values"
        assert r["rule_type"]["data"]["allowed"] == ["a", "b"]

    def test_cardinality_rule(self) -> None:
        rules = (
            RuleBuilder()
            .cardinality("r1", entity="person", attribute="tags", min=1, max=5)
            .to_json()
        )
        parsed = json.loads(rules)
        r = parsed["rules"][0]
        assert r["rule_type"]["type"] == "Cardinality"
        assert r["rule_type"]["data"]["min"] == 1
        assert r["rule_type"]["data"]["max"] == 5

    def test_length_rule(self) -> None:
        rules = RuleBuilder().length("r1", attribute="name", min=1, max=255).to_json()
        parsed = json.loads(rules)
        r = parsed["rules"][0]
        assert r["rule_type"]["type"] == "Length"

    def test_chaining(self) -> None:
        rules = (
            RuleBuilder()
            .required("r1", entity="person", attribute="name")
            .regex("r2", attribute="email", pattern=r"^.+@.+$")
            .range("r3", attribute="age", min=0, max=150)
        ).to_json()
        parsed = json.loads(rules)
        assert len(parsed["rules"]) == 3

    def test_custom_message(self) -> None:
        rules = (
            RuleBuilder()
            .required("r1", entity="person", attribute="name", message="Name is mandatory")
            .to_json()
        )
        parsed = json.loads(rules)
        assert parsed["rules"][0]["error_message"] == "Name is mandatory"

    def test_json_roundtrip(self) -> None:
        builder = RuleBuilder().required("r1", entity="person", attribute="name")
        json_str = builder.to_json()
        parsed = json.loads(json_str)
        json_str2 = json.dumps(parsed, indent=2)
        assert json.loads(json_str) == json.loads(json_str2)

    def test_entity_scoped_regex(self) -> None:
        rules = (
            RuleBuilder()
            .regex("r1", attribute="email", pattern=r"^.+@.+$", entity="employee")
            .to_json()
        )
        parsed = json.loads(rules)
        assert parsed["rules"][0]["target"]["type"] == "EntityAttribute"
        assert parsed["rules"][0]["target"]["data"]["entity"] == "employee"


# ---------------------------------------------------------------------------
# Rust integration tests (require type_bridge_core)
# ---------------------------------------------------------------------------


@pytest.mark.skipif(not HAS_RUST_CORE, reason="Rust core not available")
class TestRustValidation:
    """Tests that validate entity data via the Rust engine."""

    @pytest.fixture
    def engine(self) -> Any:
        return ValidationEngine()

    def test_load_and_validate_valid(self, engine: Any) -> None:
        rules = RuleBuilder().required("r1", entity="person", attribute="name").to_json()
        engine.load_rules(rules)
        result = engine.validate_entity({"__type__": "person", "name": "Alice"}, None)
        assert result["is_valid"] is True

    def test_load_and_validate_invalid(self, engine: Any) -> None:
        rules = RuleBuilder().required("r1", entity="person", attribute="name").to_json()
        engine.load_rules(rules)
        result = engine.validate_entity({"__type__": "person"}, None)
        assert result["is_valid"] is False
        assert any(e["code"] == "RULE_REQUIRED" for e in result["errors"])

    def test_regex_validation(self, engine: Any) -> None:
        rules = RuleBuilder().regex("r1", attribute="email", pattern=r"^.+@.+\..+$").to_json()
        engine.load_rules(rules)

        valid = engine.validate_entity({"__type__": "person", "email": "a@b.com"}, None)
        assert valid["is_valid"] is True

        invalid = engine.validate_entity({"__type__": "person", "email": "nope"}, None)
        assert invalid["is_valid"] is False
        assert any(e["code"] == "RULE_REGEX_MISMATCH" for e in invalid["errors"])

    def test_range_validation(self, engine: Any) -> None:
        rules = RuleBuilder().range("r1", attribute="age", min=0, max=150).to_json()
        engine.load_rules(rules)

        valid = engine.validate_entity({"__type__": "person", "age": 30}, None)
        assert valid["is_valid"] is True

        invalid = engine.validate_entity({"__type__": "person", "age": -1}, None)
        assert invalid["is_valid"] is False
        assert any(e["code"] == "RULE_RANGE_VIOLATION" for e in invalid["errors"])

    def test_values_validation(self, engine: Any) -> None:
        rules = (
            RuleBuilder().values("r1", attribute="status", allowed=["active", "inactive"]).to_json()
        )
        engine.load_rules(rules)

        valid = engine.validate_entity({"__type__": "person", "status": "active"}, None)
        assert valid["is_valid"] is True

        invalid = engine.validate_entity({"__type__": "person", "status": "deleted"}, None)
        assert invalid["is_valid"] is False
        assert any(e["code"] == "RULE_VALUES_VIOLATION" for e in invalid["errors"])

    def test_cardinality_validation(self, engine: Any) -> None:
        rules = (
            RuleBuilder()
            .cardinality("r1", entity="person", attribute="tags", min=1, max=3)
            .to_json()
        )
        engine.load_rules(rules)

        valid = engine.validate_entity({"__type__": "person", "tags": ["a", "b"]}, None)
        assert valid["is_valid"] is True

        too_few = engine.validate_entity({"__type__": "person", "tags": []}, None)
        assert too_few["is_valid"] is False

        too_many = engine.validate_entity(
            {"__type__": "person", "tags": ["a", "b", "c", "d"]}, None
        )
        assert too_many["is_valid"] is False

    def test_length_validation(self, engine: Any) -> None:
        rules = RuleBuilder().length("r1", attribute="name", min=1, max=10).to_json()
        engine.load_rules(rules)

        valid = engine.validate_entity({"__type__": "person", "name": "Alice"}, None)
        assert valid["is_valid"] is True

        too_short = engine.validate_entity({"__type__": "person", "name": ""}, None)
        assert too_short["is_valid"] is False

        too_long = engine.validate_entity({"__type__": "person", "name": "A" * 20}, None)
        assert too_long["is_valid"] is False

    def test_multiple_errors(self, engine: Any) -> None:
        rules = (
            RuleBuilder()
            .required("r1", entity="person", attribute="name")
            .regex("r2", attribute="email", pattern=r"^.+@.+$")
            .range("r3", attribute="age", min=0, max=150)
        ).to_json()
        engine.load_rules(rules)

        data = {"__type__": "person", "email": "nope", "age": 200}
        result = engine.validate_entity(data, None)
        assert result["is_valid"] is False
        codes = {e["code"] for e in result["errors"]}
        assert "RULE_REQUIRED" in codes
        assert "RULE_REGEX_MISMATCH" in codes
        assert "RULE_RANGE_VIOLATION" in codes

    def test_export_roundtrip(self, engine: Any) -> None:
        rules = RuleBuilder().required("r1", entity="person", attribute="name").to_json()
        engine.load_rules(rules)
        exported = engine.export_rules()

        engine2 = ValidationEngine()
        engine2.load_rules(exported)
        assert engine2.rule_count() == 1

    def test_clear_rules(self, engine: Any) -> None:
        rules = RuleBuilder().required("r1", entity="person", attribute="name").to_json()
        engine.load_rules(rules)
        assert engine.rule_count() == 1
        engine.clear_rules()
        assert engine.rule_count() == 0

    def test_entity_scoping(self, engine: Any) -> None:
        """Rule for person.name should not fire on company entities."""
        rules = RuleBuilder().required("r1", entity="person", attribute="name").to_json()
        engine.load_rules(rules)
        result = engine.validate_entity({"__type__": "company"}, None)
        assert result["is_valid"] is True


@pytest.mark.skipif(not HAS_RUST_CORE, reason="Rust core not available")
class TestPythonWrapper:
    """Tests for the validate_entity_data Python wrapper."""

    def test_wrapper_valid(self) -> None:
        from type_bridge.validation import validate_entity_data

        rules = RuleBuilder().required("r1", entity="person", attribute="name").to_json()
        result = validate_entity_data({"__type__": "person", "name": "Alice"}, rules)
        assert result["is_valid"] is True

    def test_wrapper_invalid(self) -> None:
        from type_bridge.validation import validate_entity_data

        rules = RuleBuilder().required("r1", entity="person", attribute="name").to_json()
        result = validate_entity_data({"__type__": "person"}, rules)
        assert result["is_valid"] is False
