from type_bridge import Database, Entity, Relation


def assert_entity_exists(db: Database, entity_type: type[Entity], **attrs) -> Entity:
    """Assert an entity with given attributes exists and return it."""
    manager = entity_type.manager(db)
    result = manager.filter(**attrs).first()
    assert result is not None, f"Expected {entity_type.__name__} with {attrs} to exist"
    return result


def assert_entity_count(db: Database, entity_type: type[Entity], expected: int) -> None:
    """Assert the count of entities matches expected."""
    manager = entity_type.manager(db)
    actual = len(manager.all())
    assert actual == expected, f"Expected {expected} {entity_type.__name__}, got {actual}"


def assert_relation_exists(db: Database, relation_type: type[Relation], **role_players) -> Relation:
    """Assert a relation with given role players exists."""
    manager = relation_type.manager(db)
    result = manager.filter(**role_players).first()
    assert result is not None, f"Expected {relation_type.__name__} with {role_players} to exist"
    return result


def assert_relation_count(db: Database, relation_type: type[Relation], expected: int) -> None:
    """Assert the count of relations matches expected."""
    manager = relation_type.manager(db)
    actual = len(manager.all())
    assert actual == expected, f"Expected {expected} {relation_type.__name__}, got {actual}"
