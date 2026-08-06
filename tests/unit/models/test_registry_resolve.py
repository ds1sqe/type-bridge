"""Tests for ModelRegistry.resolve() unified polymorphic resolution."""

from tests.utils.handwritten import Entity, Flag, Key, ModelRegistry, String, TypeFlags


class Name(String):
    pass


class BasePerson(Entity):
    flags = TypeFlags(name="base_person", abstract=True)
    name: Name = Flag(Key)


class Employee(BasePerson):
    flags = TypeFlags(name="employee")


class Manager(Employee):
    flags = TypeFlags(name="manager")


class Contractor(BasePerson):
    flags = TypeFlags(name="contractor")


class TestRegistryResolve:
    """Tests for ModelRegistry.resolve()."""

    def setup_method(self) -> None:
        """Clear registry before each test."""
        ModelRegistry.clear()
        # Re-register test classes
        ModelRegistry.register(BasePerson)
        ModelRegistry.register(Employee)
        ModelRegistry.register(Manager)
        ModelRegistry.register(Contractor)

    def test_resolve_from_registry(self) -> None:
        """Test resolving a type that's in the registry."""
        result = ModelRegistry.resolve("employee", (BasePerson,))
        assert result is Employee

    def test_resolve_subclass(self) -> None:
        """Test resolving a subclass via inheritance search."""
        # Clear registry so it falls back to subclass search
        ModelRegistry.clear()
        result = ModelRegistry.resolve("manager", (BasePerson,))
        assert result is Manager

    def test_resolve_none_label(self) -> None:
        """Test resolving with None label returns first allowed class."""
        result = ModelRegistry.resolve(None, (BasePerson, Employee))
        assert result is BasePerson

    def test_resolve_fallback_to_allowed(self) -> None:
        """Test that unknown labels fall back to first allowed class."""
        result = ModelRegistry.resolve("unknown_type", (Employee, Contractor))
        assert result is Employee

    def test_resolve_caches_result(self) -> None:
        """Test that resolved results are cached."""
        # First call should search and cache
        result1 = ModelRegistry.resolve("employee", (BasePerson,))
        # Second call should hit cache
        result2 = ModelRegistry.resolve("employee", (BasePerson,))
        assert result1 is result2 is Employee

    def test_resolve_direct_match_in_allowed(self) -> None:
        """Test direct match when type is in allowed classes."""
        result = ModelRegistry.resolve("contractor", (Employee, Contractor))
        assert result is Contractor

    def test_resolve_deep_subclass(self) -> None:
        """Test resolving a class two levels deep in hierarchy."""
        ModelRegistry.clear()
        result = ModelRegistry.resolve("manager", (BasePerson,))
        assert result is Manager

    def test_clear_clears_cache(self) -> None:
        """Test that clear() also clears the resolution cache."""
        # Populate cache
        ModelRegistry.resolve("employee", (BasePerson,))
        # Clear everything
        ModelRegistry.clear()
        # Registry should be empty
        assert ModelRegistry.get("employee") is None
        # Cache should be empty (no error on resolve, but needs subclass search)
        result = ModelRegistry.resolve("employee", (BasePerson,))
        assert result is Employee  # Found via subclass search
