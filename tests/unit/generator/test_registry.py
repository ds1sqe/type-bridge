"""Unit tests for generated registry output."""

from __future__ import annotations

import tempfile
from pathlib import Path

from type_bridge.generator import generate_models


class TestGenerateModelsWithRegistry:
    """Integration tests for generate_models with registry."""

    def test_generates_registry_file(self) -> None:
        """generate_models creates registry.py."""
        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"

            generate_models(
                """
define
entity person, owns name @key;
attribute name, value string;
""",
                output,
            )

            assert (output / "registry.py").exists()

            init_content = (output / "__init__.py").read_text()
            assert "registry" in init_content

    def test_registry_code_is_valid(self) -> None:
        """Generated registry compiles and contains expected metadata."""
        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"

            generate_models(
                """
define
# @prefix(P)
entity person, owns name @key, plays friendship:friend;
attribute name, value string;
relation friendship, relates friend;
""",
                output,
            )

            for filename in ["registry.py", "attributes.py", "entities.py", "relations.py"]:
                content = (output / filename).read_text()
                compile(content, filename, "exec")

            registry_content = (output / "registry.py").read_text()
            assert "ENTITY_TYPES" in registry_content
            assert "EntityType" in registry_content
            assert "ENTITY_MAP" in registry_content
            assert "RELATION_ROLES" in registry_content
            assert '"prefix": "P"' in registry_content
