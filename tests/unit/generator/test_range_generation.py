"""Tests for range constraint code generation and runtime validation."""

import sys
import tempfile
from pathlib import Path

import pytest
from type_bridge.generator import generate_models


class TestRangeGenerationAndValidation:
    """Integration tests for @range constraint generation and runtime validation."""

    def test_integer_range_validation_from_generated_code(self) -> None:
        """Test that generated Integer with @range validates at runtime."""
        schema = """
            define
            attribute age, value integer @range(0..150);

            define
            entity person,
                owns age;
        """

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"
            generate_models(schema, output)

            # Add to path and import
            sys.path.insert(0, str(tmpdir))
            try:
                import importlib

                attrs = importlib.import_module("models.attributes")

                # Valid values should work
                age_valid = attrs.Age(30)
                assert age_valid.value == 30

                age_min = attrs.Age(0)
                assert age_min.value == 0

                age_max = attrs.Age(150)
                assert age_max.value == 150

                # Invalid values should raise
                with pytest.raises(ValueError, match="below minimum"):
                    attrs.Age(-1)

                with pytest.raises(ValueError, match="above maximum"):
                    attrs.Age(200)

            finally:
                sys.path.remove(str(tmpdir))
                # Clean up module cache
                for mod_name in list(sys.modules.keys()):
                    if mod_name.startswith("models"):
                        del sys.modules[mod_name]

    def test_double_range_validation_from_generated_code(self) -> None:
        """Test that generated Double with @range validates at runtime."""
        schema = """
            define
            attribute temperature, value double @range(-50.0..50.0);

            define
            entity sensor,
                owns temperature;
        """

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"
            generate_models(schema, output)

            # Add to path and import
            sys.path.insert(0, str(tmpdir))
            try:
                import importlib

                attrs = importlib.import_module("models.attributes")

                # Valid values should work
                temp_valid = attrs.Temperature(25.5)
                assert temp_valid.value == 25.5

                temp_min = attrs.Temperature(-50.0)
                assert temp_min.value == -50.0

                temp_max = attrs.Temperature(50.0)
                assert temp_max.value == 50.0

                # Invalid values should raise
                with pytest.raises(ValueError, match="below minimum"):
                    attrs.Temperature(-100.0)

                with pytest.raises(ValueError, match="above maximum"):
                    attrs.Temperature(100.0)

            finally:
                sys.path.remove(str(tmpdir))
                # Clean up module cache
                for mod_name in list(sys.modules.keys()):
                    if mod_name.startswith("models"):
                        del sys.modules[mod_name]

    def test_open_ended_range_min_only(self) -> None:
        """Test @range with only minimum bound (e.g., @range(0..))."""
        schema = """
            define
            attribute score, value integer @range(0..);

            define
            entity game,
                owns score;
        """

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"
            generate_models(schema, output)

            sys.path.insert(0, str(tmpdir))
            try:
                import importlib

                attrs = importlib.import_module("models.attributes")

                # Any non-negative value should work
                assert attrs.Score(0).value == 0
                assert attrs.Score(1000000).value == 1000000

                # Negative should fail
                with pytest.raises(ValueError, match="below minimum"):
                    attrs.Score(-1)

            finally:
                sys.path.remove(str(tmpdir))
                for mod_name in list(sys.modules.keys()):
                    if mod_name.startswith("models"):
                        del sys.modules[mod_name]

    def test_open_ended_range_max_only(self) -> None:
        """Test @range with only maximum bound (e.g., @range(..100))."""
        schema = """
            define
            attribute priority, value integer @range(..10);

            define
            entity task,
                owns priority;
        """

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"
            generate_models(schema, output)

            sys.path.insert(0, str(tmpdir))
            try:
                import importlib

                attrs = importlib.import_module("models.attributes")

                # Any value up to 10 should work
                assert attrs.Priority(-100).value == -100
                assert attrs.Priority(0).value == 0
                assert attrs.Priority(10).value == 10

                # Above 10 should fail
                with pytest.raises(ValueError, match="above maximum"):
                    attrs.Priority(11)

            finally:
                sys.path.remove(str(tmpdir))
                for mod_name in list(sys.modules.keys()):
                    if mod_name.startswith("models"):
                        del sys.modules[mod_name]

    def test_range_in_entity_pydantic_validation(self) -> None:
        """Test that range validation works through Pydantic in entities."""
        schema = """
            define
            attribute name, value string;
            attribute age, value integer @range(0..150);

            define
            entity person,
                owns name @key,
                owns age;
        """

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"
            generate_models(schema, output)

            sys.path.insert(0, str(tmpdir))
            try:
                import importlib

                attrs = importlib.import_module("models.attributes")
                entities = importlib.import_module("models.entities")

                # Valid entity should work
                person = entities.Person(
                    name=attrs.Name("Alice"),
                    age=attrs.Age(30),
                )
                assert person.age.value == 30

                # Invalid age through entity should fail
                with pytest.raises(ValueError, match="above maximum"):
                    entities.Person(
                        name=attrs.Name("Bob"),
                        age=attrs.Age(200),
                    )

            finally:
                sys.path.remove(str(tmpdir))
                for mod_name in list(sys.modules.keys()):
                    if mod_name.startswith("models"):
                        del sys.modules[mod_name]
