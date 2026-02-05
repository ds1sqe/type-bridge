"""Integration tests for transaction context manager reuse."""

import pytest
from typedb.driver import TransactionType

from type_bridge import (
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(305)
def test_transaction_context_commits_all_operations(clean_db):
    """Managers created from a transaction context share a single commit."""

    class Prefix(String):
        flags = TypeFlags(name="prefix")

    class NextValue(Integer):
        flags = TypeFlags(name="next_value")

    class Counter(Entity):
        flags = TypeFlags(name="counter")
        prefix: Prefix = Flag(Key)
        next_value: NextValue

    class DisplayId(String):
        flags = TypeFlags(name="display_id")

    class Artifact(Entity):
        flags = TypeFlags(name="artifact")
        display_id: DisplayId = Flag(Key)

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Counter, Artifact)
    schema_manager.sync_schema(force=True)

    with clean_db.transaction(TransactionType.WRITE) as tx:
        counter_mgr = Counter.manager(tx)
        artifact_mgr = Artifact.manager(tx)

        counter = Counter(prefix=Prefix("US"), next_value=NextValue(1))
        counter_mgr.insert(counter)

        artifact = Artifact(display_id=DisplayId("US-1"))
        artifact_mgr.insert(artifact)

    counters = Counter.manager(clean_db).get(prefix=Prefix("US"))
    artifacts = Artifact.manager(clean_db).get(display_id=DisplayId("US-1"))

    assert len(counters) == 1
    assert counters[0].next_value.value == 1
    assert len(artifacts) == 1
    assert artifacts[0].display_id.value == "US-1"


@pytest.mark.integration
@pytest.mark.order(306)
def test_transaction_context_rolls_back_on_exception(clean_db):
    """Rollback on exception inside context leaves no partial writes."""

    class Name(String):
        flags = TypeFlags(name="name")

    class Counter(Entity):
        flags = TypeFlags(name="rollback_counter")
        name: Name = Flag(Key)

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Counter)
    schema_manager.sync_schema(force=True)

    with pytest.raises(RuntimeError):
        with clean_db.transaction(TransactionType.WRITE) as tx:
            mgr = Counter.manager(tx)
            mgr.insert(Counter(name=Name("temp")))
            raise RuntimeError("force rollback")

    remaining = Counter.manager(clean_db).get()
    assert remaining == []
