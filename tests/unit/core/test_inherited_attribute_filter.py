"""Private-engine tests for inherited attribute discovery."""

from tests.utils.handwritten import Entity, Flag, Integer, Key, String, TypeFlags


# Test model hierarchy
class LivingName(String):
    """Name attribute for Living entities."""

    pass


class LivingAge(Integer):
    """Age attribute for Living entities."""

    pass


class Living(Entity):
    """Abstract base entity with name key."""

    flags = TypeFlags(abstract=True, name="living")
    name: LivingName = Flag(Key)


class Animal(Living):
    """Abstract animal entity inheriting from Living."""

    flags = TypeFlags(abstract=True, name="animal")


class Dog(Animal):
    """Concrete Dog entity - inherits name from Living."""

    flags = TypeFlags(name="dog")
    age: LivingAge | None = None


class Cat(Animal):
    """Concrete Cat entity - inherits name from Living."""

    flags = TypeFlags(name="cat")


class TestGetAllAttributesVsGetOwnedAttributes:
    """Test that get_all_attributes includes inherited attributes."""

    def test_get_owned_attributes_excludes_inherited(self):
        """get_owned_attributes() should only return directly-owned attributes."""
        # Living owns 'name'
        living_attrs = Living.get_owned_attributes()
        assert "name" in living_attrs

        # Animal inherits 'name' but doesn't own any new attributes
        animal_attrs = Animal.get_owned_attributes()
        assert "name" not in animal_attrs
        assert len(animal_attrs) == 0

        # Dog inherits 'name' from Living and owns 'age'
        dog_attrs = Dog.get_owned_attributes()
        assert "name" not in dog_attrs  # Inherited, not owned
        assert "age" in dog_attrs  # Directly owned

        # Cat inherits 'name' and owns nothing
        cat_attrs = Cat.get_owned_attributes()
        assert "name" not in cat_attrs
        assert len(cat_attrs) == 0

    def test_get_all_attributes_includes_inherited(self):
        """get_all_attributes() should return all attributes including inherited."""
        # Living owns 'name'
        living_all = Living.get_all_attributes()
        assert "name" in living_all

        # Animal inherits 'name'
        animal_all = Animal.get_all_attributes()
        assert "name" in animal_all

        # Dog inherits 'name' and owns 'age'
        dog_all = Dog.get_all_attributes()
        assert "name" in dog_all  # Inherited
        assert "age" in dog_all  # Owned

        # Cat inherits 'name'
        cat_all = Cat.get_all_attributes()
        assert "name" in cat_all

    def test_get_all_attributes_mro_order(self):
        """get_all_attributes() should traverse MRO in correct order."""

        class ParentId(String):
            pass

        class ChildId(String):
            pass

        class ParentEntity(Entity):
            flags = TypeFlags(abstract=True, name="parent_mro")
            parent_id: ParentId = Flag(Key)

        class ChildEntity(ParentEntity):
            flags = TypeFlags(name="child_mro")
            child_id: ChildId

        child_all = ChildEntity.get_all_attributes()
        # Should have both parent and child attributes
        assert "parent_id" in child_all
        assert "child_id" in child_all
        assert child_all["parent_id"].typ == ParentId
        assert child_all["child_id"].typ == ChildId


class TestDeepInheritanceChain:
    """Test inherited attribute filtering with deeper inheritance chains."""

    def test_three_level_inheritance(self):
        """Test that attributes from grandparent are accessible."""

        class GrandparentName(String):
            pass

        class ParentAge(Integer):
            pass

        class ChildScore(Integer):
            pass

        class Grandparent(Entity):
            flags = TypeFlags(abstract=True, name="grandparent")
            name: GrandparentName = Flag(Key)

        class Parent(Grandparent):
            flags = TypeFlags(abstract=True, name="parent")
            age: ParentAge

        class Child(Parent):
            flags = TypeFlags(name="child")
            score: ChildScore

        # Child should have all three attributes
        child_all = Child.get_all_attributes()
        assert "name" in child_all  # From Grandparent
        assert "age" in child_all  # From Parent
        assert "score" in child_all  # Own attribute

        # get_owned_attributes should only have 'score'
        child_owned = Child.get_owned_attributes()
        assert "name" not in child_owned
        assert "age" not in child_owned
        assert "score" in child_owned
