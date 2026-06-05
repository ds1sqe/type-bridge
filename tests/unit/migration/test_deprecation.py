"""Unit tests for SimpleMigrationManager deprecation warning (P3b D6)."""

from __future__ import annotations

import pytest


def test_simple_migration_manager_raises_deprecation_warning() -> None:
    """Instantiating SimpleMigrationManager (MigrationManager) emits DeprecationWarning.

    Requirement: P3b D6 — the class must warn on construction so callers
    have a migration path to MigrationExecutor + MigrationGenerator.
    """
    from unittest.mock import MagicMock

    from type_bridge.migration import SimpleMigrationManager

    mock_db = MagicMock()
    with pytest.warns(DeprecationWarning, match="MigrationManager"):
        SimpleMigrationManager(mock_db)
