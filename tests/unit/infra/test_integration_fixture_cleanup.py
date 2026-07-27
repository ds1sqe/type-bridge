"""Cleanup guarantees for the live integration fixtures."""

import pytest

from tests.integration import conftest as integration_fixtures


@pytest.mark.parametrize(
    ("failing_stage", "expected_events"),
    [
        ("delete", ["connect", "exists", "delete", "close"]),
        ("create", ["connect", "exists", "delete", "create", "close"]),
    ],
)
def test_clean_db_closes_when_setup_fails_before_yield(
    monkeypatch: pytest.MonkeyPatch,
    failing_stage: str,
    expected_events: list[str],
) -> None:
    """A lifecycle error during fixture setup must not leak its native driver."""
    events: list[str] = []

    class FailingDatabase:
        def __init__(self, *args: object, **kwargs: object) -> None:
            del args, kwargs

        def connect(self) -> None:
            events.append("connect")

        def database_exists(self) -> bool:
            events.append("exists")
            return True

        def delete_database(self) -> None:
            events.append("delete")
            if failing_stage == "delete":
                raise RuntimeError("delete setup failure")

        def create_database(self) -> None:
            events.append("create")
            if failing_stage == "create":
                raise RuntimeError("create setup failure")

        def close(self) -> None:
            events.append("close")

    monkeypatch.setattr(integration_fixtures, "Database", FailingDatabase)
    fixture_function = getattr(integration_fixtures.clean_db, "__wrapped__")
    fixture = fixture_function(None, "fixture_cleanup")

    with pytest.raises(RuntimeError, match=f"{failing_stage} setup failure"):
        next(fixture)

    assert events == expected_events
