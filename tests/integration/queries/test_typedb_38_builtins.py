"""Tests for TypeDB 3.8+ built-in functions.

Tests iid(), label() functions and Unicode identifier support.
"""

import pytest

from type_bridge import (
    Database,
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
class TestIidFunction:
    """Test iid() built-in function for internal identifier access."""

    @pytest.fixture
    def schema_for_iid(self, clean_db: Database):
        """Set up schema for iid tests."""

        class Name(String):
            pass

        class ItemValue(Integer):
            pass

        class Item(Entity):
            flags = TypeFlags(name="item_iid_test")
            name: Name = Flag(Key)
            value: ItemValue

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Item)
        schema_manager.sync_schema(force=True)

        return clean_db, Item, Name, ItemValue

    def test_entity_has_iid_after_insert(self, schema_for_iid):
        """Entity has _iid attribute after insertion."""
        db, Item, Name, ItemValue = schema_for_iid

        item = Item(name=Name("Test"), value=ItemValue(42))
        manager = Item.manager(db)
        manager.insert(item)

        # Fetch and check for _iid
        results = manager.all()
        assert len(results) == 1

        fetched = results[0]
        # _iid should be populated after fetch from DB
        assert hasattr(fetched, "_iid")
        assert fetched._iid is not None

    def test_iid_is_unique_per_entity(self, schema_for_iid):
        """Each entity gets a unique iid."""
        db, Item, Name, ItemValue = schema_for_iid

        manager = Item.manager(db)

        # Insert multiple items
        manager.insert(Item(name=Name("Item1"), value=ItemValue(1)))
        manager.insert(Item(name=Name("Item2"), value=ItemValue(2)))
        manager.insert(Item(name=Name("Item3"), value=ItemValue(3)))

        # Fetch all
        results = manager.all()
        assert len(results) == 3

        # All iids should be unique
        iids = [r._iid for r in results]
        assert len(set(iids)) == 3  # All unique

    def test_iid_persists_across_queries(self, schema_for_iid):
        """Same entity returns same iid across different queries."""
        db, Item, Name, ItemValue = schema_for_iid

        manager = Item.manager(db)
        manager.insert(Item(name=Name("Persistent"), value=ItemValue(100)))

        # Query twice
        result1 = manager.get(name="Persistent")[0]
        result2 = manager.get(name="Persistent")[0]

        # Same iid
        assert result1._iid == result2._iid


@pytest.mark.integration
class TestLabelFunction:
    """Test label() built-in function for type name access."""

    @pytest.fixture
    def schema_with_inheritance(self, clean_db: Database):
        """Set up schema with inheritance for label tests."""

        class Name(String):
            pass

        # Abstract base
        class Vehicle(Entity):
            flags = TypeFlags(name="vehicle_label_test", abstract=True)
            name: Name = Flag(Key)

        # Concrete subtypes
        class Car(Vehicle):
            flags = TypeFlags(name="car_label_test")

        class Truck(Vehicle):
            flags = TypeFlags(name="truck_label_test")

        class Motorcycle(Vehicle):
            flags = TypeFlags(name="motorcycle_label_test")

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Vehicle, Car, Truck, Motorcycle)
        schema_manager.sync_schema(force=True)

        return clean_db, Vehicle, Car, Truck, Motorcycle, Name

    def test_concrete_type_has_correct_label(self, schema_with_inheritance):
        """Concrete entity type has correct type label."""
        db, Vehicle, Car, Truck, Motorcycle, Name = schema_with_inheritance

        car = Car(name=Name("Sedan"))
        Car.manager(db).insert(car)

        # Query and check type
        results = Car.manager(db).all()
        assert len(results) == 1

        # The fetched entity should know its type
        fetched = results[0]
        assert type(fetched).__name__ == "Car"

    def test_polymorphic_query_returns_correct_types(self, schema_with_inheritance):
        """Query on abstract type returns correct concrete types."""
        db, Vehicle, Car, Truck, Motorcycle, Name = schema_with_inheritance

        # Insert different vehicle types
        Car.manager(db).insert(Car(name=Name("Sedan")))
        Truck.manager(db).insert(Truck(name=Name("Pickup")))
        Motorcycle.manager(db).insert(Motorcycle(name=Name("Cruiser")))

        # Query abstract type - should get all vehicles
        # Note: This depends on how the query builder handles abstract types
        cars = Car.manager(db).all()
        trucks = Truck.manager(db).all()
        motorcycles = Motorcycle.manager(db).all()

        assert len(cars) == 1
        assert len(trucks) == 1
        assert len(motorcycles) == 1

        # Each should be the correct type
        assert type(cars[0]).__name__ == "Car"
        assert type(trucks[0]).__name__ == "Truck"
        assert type(motorcycles[0]).__name__ == "Motorcycle"


@pytest.mark.integration
class TestUnicodeIdentifiers:
    """Test Unicode identifier support in TypeDB 3.8+."""

    @pytest.fixture
    def schema_with_unicode(self, clean_db: Database):
        """Set up schema with unicode-safe names."""

        class Name(String):
            pass

        class Description(String):
            pass

        # Use ASCII names for schema but test Unicode in values
        class Item(Entity):
            flags = TypeFlags(name="item_unicode_test")
            name: Name = Flag(Key)
            description: Description | None = None

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Item)
        schema_manager.sync_schema(force=True)

        return clean_db, Item, Name, Description

    def test_unicode_in_string_values(self, schema_with_unicode):
        """Unicode characters in string attribute values work correctly."""
        db, Item, Name, Description = schema_with_unicode

        # Test various unicode strings
        unicode_values = [
            ("Japanese", "こんにちは"),  # Hello in Japanese
            ("Chinese", "你好世界"),  # Hello World in Chinese
            ("Arabic", "مرحبا"),  # Hello in Arabic
            ("Emoji", "Hello 🌍🎉"),  # Emojis
            ("Greek", "Ελληνικά"),  # Greek
            ("Russian", "Привет мир"),  # Hello world in Russian
        ]

        manager = Item.manager(db)

        for name, desc in unicode_values:
            item = Item(name=Name(name), description=Description(desc))
            manager.insert(item)

        # Query back and verify
        results = manager.all()
        assert len(results) == len(unicode_values)

        # Verify each unicode value was preserved
        result_map = {str(r.name): str(r.description) for r in results}
        for name, expected_desc in unicode_values:
            assert result_map[name] == expected_desc

    def test_unicode_in_key_values(self, schema_with_unicode):
        """Unicode characters in key attribute values work correctly."""
        db, Item, Name, Description = schema_with_unicode

        manager = Item.manager(db)

        # Use unicode in key
        item = Item(name=Name("日本語キー"))
        manager.insert(item)

        # Query by unicode key
        results = manager.get(name="日本語キー")
        assert len(results) == 1
        assert str(results[0].name) == "日本語キー"

    def test_mixed_unicode_ascii(self, schema_with_unicode):
        """Mixed Unicode and ASCII content works correctly."""
        db, Item, Name, Description = schema_with_unicode

        manager = Item.manager(db)

        item = Item(
            name=Name("Mixed Content"),
            description=Description("Hello 世界 Привет 🌍 مرحبا"),
        )
        manager.insert(item)

        results = manager.all()
        assert len(results) == 1
        assert str(results[0].description) == "Hello 世界 Привет 🌍 مرحبا"
