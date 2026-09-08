"""Tests for base flag functionality."""

import warnings

import tests.utils.handwritten as tbg
from tests.utils.handwritten import Role, String, TypeFlags


class TestBaseFlag:
    """Test base flag for Python-only base classes."""

    def test_entity_base_flag_with_conflicting_name(self):
        """Test that base=True allows conflicting names like 'Entity'."""

        # This should work now with base=True
        class Entity(tbg.Entity):
            flags = TypeFlags(base=True)

        # Verify it's marked as base
        assert Entity.is_base() is True
        assert Entity.is_abstract() is False

        # Base classes don't generate schema
        assert Entity.to_schema_definition() is None

    def test_relation_base_flag_with_conflicting_name(self):
        """Test that base=True allows conflicting names like 'Relation'."""

        # This should work now with base=True
        class Relation(tbg.Relation):
            flags = TypeFlags(base=True)

        # Verify it's marked as base
        assert Relation.is_base() is True
        assert Relation.is_abstract() is False

        # Base classes don't generate schema
        assert Relation.to_schema_definition() is None

    def test_inheritance_skips_base_classes(self):
        """Test that children skip base classes in TypeDB hierarchy."""

        class Name(String):
            pass

        # Create a Python base class
        class Entity(tbg.Entity):
            flags = TypeFlags(base=True)

        # Create concrete entities
        class XX(Entity):
            flags = TypeFlags(name="xx")
            name: Name

        class YY(XX):
            flags = TypeFlags(name="yy")

        # XX should not have Entity as supertype (Entity is base class)
        assert XX.get_supertype() is None  # Skips base class, no other parents

        # YY should have XX as supertype
        assert YY.get_supertype() == "xx"

        # Check schema generation
        xx_schema = XX.to_schema_definition()
        assert xx_schema is not None
        assert "entity xx" in xx_schema
        assert "sub" not in xx_schema  # No supertype

        yy_schema = YY.to_schema_definition()
        assert yy_schema is not None
        assert "entity yy, sub xx" in yy_schema

    def test_multi_level_base_classes(self):
        """Test inheritance chain with multiple base classes."""

        class Name(String):
            pass

        # Python-only base class 1
        class BaseEntity(tbg.Entity):
            flags = TypeFlags(base=True)

        # Python-only base class 2
        class AnotherBase(BaseEntity):
            flags = TypeFlags(base=True)

        # Concrete entity
        class ConcreteEntity(AnotherBase):
            flags = TypeFlags(name="concrete")
            name: Name

        # Should skip all base classes
        assert ConcreteEntity.get_supertype() is None

        # Schema should not reference base classes
        schema = ConcreteEntity.to_schema_definition()
        assert schema is not None
        assert "entity concrete" in schema
        assert "sub" not in schema  # No supertype

    def test_base_and_abstract_together(self):
        """Test that base=True and abstract=True can be used together."""

        # Base + abstract is valid (though redundant)
        class AbstractBase(tbg.Entity):
            flags = TypeFlags(base=True, abstract=True)

        assert AbstractBase.is_base() is True
        assert AbstractBase.is_abstract() is True
        assert AbstractBase.to_schema_definition() is None

    def test_normal_abstract_still_generates_schema(self):
        """Test that abstract=True without base=True still generates schema."""

        class Name(String):
            pass

        class AbstractEntity(tbg.Entity):
            flags = TypeFlags(name="abstract_entity", abstract=True)
            name: Name

        # Abstract entities still generate schema (they appear in TypeDB)
        schema = AbstractEntity.to_schema_definition()
        assert schema is not None
        assert "entity abstract_entity @abstract" in schema
        assert "owns Name" in schema  # Name uses default CLASS_NAME case

    def test_base_class_with_attributes(self):
        """Test that base classes can have attributes (for Python inheritance)."""

        class Name(String):
            pass

        class BaseWithAttrs(tbg.Entity):
            flags = TypeFlags(base=True)
            name: Name

        # Pydantic warns about field shadowing when base=True attrs are absorbed
        with warnings.catch_warnings():
            warnings.filterwarnings("ignore", message="Field name.*shadows", category=UserWarning)

            class Child(BaseWithAttrs):
                flags = TypeFlags(name="child")

        # Base class doesn't generate schema
        assert BaseWithAttrs.to_schema_definition() is None

        # Child inherits attributes from Python base
        child_attrs = Child.get_owned_attributes()
        assert "name" in child_attrs

        # Child generates schema with inherited attribute
        schema = Child.to_schema_definition()
        assert schema is not None
        assert "entity child" in schema
        assert "owns Name" in schema  # Name uses CLASS_NAME default

    def test_relation_base_inheritance(self):
        """Test base flag with relation inheritance."""

        class Name(String):
            pass

        class Person(tbg.Entity):
            flags = TypeFlags(name="person")
            name: Name

        # Base relation
        class Relation(tbg.Relation):
            flags = TypeFlags(base=True)

        # Concrete relation
        class Friendship(Relation):
            flags = TypeFlags(name="friendship")
            friend1: Role[Person] = Role("friend1", Person)
            friend2: Role[Person] = Role("friend2", Person)

        # Friendship should not have Relation as supertype
        assert Friendship.get_supertype() is None

        # Check schema
        schema = Friendship.to_schema_definition()
        assert schema is not None
        assert "relation friendship" in schema
        assert "relates friend1" in schema
        assert "relates friend2" in schema
        assert "sub" not in schema  # No supertype

    def test_is_base_default_false(self):
        """Test that base defaults to False for normal entities."""

        class Name(String):
            pass

        class NormalEntity(tbg.Entity):
            flags = TypeFlags(name="normal")
            name: Name

        assert NormalEntity.is_base() is False
        assert NormalEntity.to_schema_definition() is not None

    def test_base_class_inheritance_with_mixed_hierarchy(self):
        """Test complex hierarchy with both base and non-base classes."""

        class Name(String):
            pass

        # Base class
        class Entity(tbg.Entity):
            flags = TypeFlags(base=True)

        # Non-base abstract
        class LivingThing(Entity):
            flags = TypeFlags(name="living_thing", abstract=True)
            name: Name

        # Another base
        class AnotherBase(LivingThing):
            flags = TypeFlags(base=True)

        # Concrete
        class Animal(AnotherBase):
            flags = TypeFlags(name="animal")

        # Animal should skip base classes and find living_thing
        assert Animal.get_supertype() == "living_thing"

        # Schema should show proper hierarchy
        schema = Animal.to_schema_definition()
        assert schema is not None
        assert "entity animal, sub living_thing" in schema

    def test_inheritance_with_default_classname_case(self):
        """Test that inheritance works with default CLASS_NAME formatting."""

        class Name(String):
            pass

        # Create a Python base class
        class Entity(tbg.Entity):
            flags = TypeFlags(base=True)

        # Create concrete entities WITHOUT explicit flags
        # They should automatically get default flags with CLASS_NAME: XX → "XX", YY → "YY"
        class XX(Entity):
            name: Name

        class YY(XX):
            other_name: Name

        # Check type names use CLASS_NAME default
        assert XX.get_type_name() == "XX"
        assert YY.get_type_name() == "YY"

        # XX should not have Entity as supertype (Entity is base class)
        assert XX.get_supertype() is None  # Skips base class, no other parents

        # YY should have XX as supertype
        assert YY.get_supertype() == "XX"

        # Check schema generation
        xx_schema = XX.to_schema_definition()
        assert xx_schema is not None
        assert "entity XX" in xx_schema  # CLASS_NAME default
        assert "sub" not in xx_schema  # No supertype
        assert "owns Name" in xx_schema  # Name attribute uses CLASS_NAME default

        yy_schema = YY.to_schema_definition()
        assert yy_schema is not None
        assert "entity YY, sub XX" in yy_schema  # Both use CLASS_NAME default
        assert "owns Name" in yy_schema  # Inherited from XX
        assert "owns Name" in yy_schema  # YY's other_name field (same attribute type)
