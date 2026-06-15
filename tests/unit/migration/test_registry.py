from __future__ import annotations

import sys
from pathlib import Path

from type_bridge.migration.registry import ModelRegistry


def test_discover_accepts_generated_package_exports(tmp_path: Path) -> None:
    package = tmp_path / "generated_models"
    package.mkdir()
    (package / "attributes.py").write_text(
        """
from type_bridge import String
from type_bridge.attribute import AttributeFlags


class GeneratedName(String):
    flags = AttributeFlags(name="generated-name")
""".lstrip()
    )
    (package / "entities.py").write_text(
        """
from type_bridge import Entity, Flag, Key, TypeFlags
from .attributes import GeneratedName


class GeneratedPerson(Entity):
    flags = TypeFlags(name="generated-person")
    name: GeneratedName = Flag(Key)
""".lstrip()
    )
    (package / "__init__.py").write_text(
        """
from .entities import GeneratedPerson

ENTITIES = [GeneratedPerson]
RELATIONS = []
""".lstrip()
    )

    sys.path.insert(0, str(tmp_path))
    try:
        ModelRegistry.clear()
        models = ModelRegistry.discover("generated_models", register=False)
    finally:
        sys.path.remove(str(tmp_path))
        ModelRegistry.clear()

    assert [model.__name__ for model in models] == ["GeneratedPerson"]
