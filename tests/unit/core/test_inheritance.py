"""Tests for inheritance edge cases and built-in type name collisions."""

import warnings

import pytest

import type_bridge as tbg
from type_bridge import Flag, Integer, Key, Role, String, TypeFlags


class TestInheritanceEdgeCases:
    """Test edge cases with multi-level inheritance and naming collisions."""

    def test_builtin_type_name_collision_entity(self):
        """Test that using 'entity' as a type name raises an error."""
        with pytest.raises(ValueError, match="conflicts with TypeDB built-in type"):

            class BadEntity(tbg.Entity):
                flags = TypeFlags(name="entity")  # Explicit collision

    def test_builtin_type_name_collision_relation(self):
        """Test that using 'relation' as a type name raises an error."""
        with pytest.raises(ValueError, match="conflicts with TypeDB built-in type"):

            class BadRelation(tbg.Relation):
                flags = TypeFlags(name="relation")  # Explicit collision

    def test_builtin_type_name_collision_attribute(self):
        """Test that using 'attribute' as an attribute name raises an error."""
        with pytest.raises(ValueError, match="conflicts with TypeDB built-in type"):
            # Class name directly conflicts (will lowercase to "attribute")
            class Attribute(String):
                pass

    def test_builtin_type_name_collision_thing(self):
        """Test that using 'thing' as a type name raises an error."""
        with pytest.raises(ValueError, match="conflicts with TypeDB built-in type"):

            class Thing(tbg.Entity):
                flags = TypeFlags(name="thing")  # Explicit collision

    def test_intermediate_base_class_without_flags(self):
        """Test multi-level inheritance with intermediate class."""
        # This is the edge case: creating a custom base class that inherits from Entity
        # Should work if we use abstract=True

        class BaseEntity(tbg.Entity):
            flags = TypeFlags(abstract=True, name="base_entity")

        class Name(String):
            pass

        class ConcreteEntity(BaseEntity):
            flags = TypeFlags(name="concrete_entity")
            name: Name

        # Should generate correct schema
        schema = ConcreteEntity.to_schema_definition()
        assert schema is not None
        assert "entity concrete_entity, sub base_entity" in schema
        assert "owns Name" in schema  # Name uses CLASS_NAME default

        # Base entity should also have correct schema
        base_schema = BaseEntity.to_schema_definition()
        assert base_schema is not None
        assert "entity base_entity @abstract" in base_schema

    def test_intermediate_base_class_name_default(self):
        """Test that intermediate class with default name gets validated."""
        # This should raise an error because the class name is "Entity"
        # which would default to name="Entity" (which lowercases to "entity", collision)
        with pytest.raises(ValueError, match="'Entity'.*conflicts with TypeDB built-in"):
            # Intentionally name the class "Entity" to trigger the edge case
            class Entity(tbg.Entity):
                pass  # No flags - would default to name="Entity" (CLASS_NAME)

    def test_multi_level_inheritance_chain(self):
        """Test deep inheritance chain works correctly."""

        class Animal(tbg.Entity):
            flags = TypeFlags(abstract=True, name="animal")

        class Mammal(Animal):
            flags = TypeFlags(abstract=True, name="mammal")

        class Name(String):
            pass

        class Dog(Mammal):
            flags = TypeFlags(name="dog")
            name: Name

        # Check supertype chain
        assert Dog.get_supertype() == "mammal"
        assert Mammal.get_supertype() == "animal"
        assert Animal.get_supertype() is None

        # Check schema generation
        dog_schema = Dog.to_schema_definition()
        assert dog_schema is not None
        assert "entity dog, sub mammal" in dog_schema

        mammal_schema = Mammal.to_schema_definition()
        assert mammal_schema is not None
        assert "entity mammal @abstract, sub animal" in mammal_schema
        assert "abstract" in mammal_schema

        animal_schema = Animal.to_schema_definition()
        assert animal_schema is not None
        assert "entity animal @abstract" in animal_schema

    def test_implicit_type_name_with_safe_class_name(self):
        """Test that implicit type names work when class name is safe."""

        class Name(String):
            pass

        class Person(tbg.Entity):
            # No explicit type_name - should use "Person" (CLASS_NAME default)
            name: Name

        assert Person.get_type_name() == "Person"  # CLASS_NAME default
        schema = Person.to_schema_definition()
        assert schema is not None
        assert "entity Person" in schema  # CLASS_NAME default

    def test_relation_inheritance_edge_cases(self):
        """Test relation inheritance with built-in name collision."""
        with pytest.raises(ValueError, match="'Relation'.*conflicts with TypeDB built-in"):

            class Relation(tbg.Relation):
                pass  # Would default to name="Relation" (CLASS_NAME)

    def test_case_sensitivity_in_builtin_check(self):
        """Test that built-in type check is case-insensitive to match TypeDB behavior."""

        class Name(String):
            pass

        # TypeDB type names are case-insensitive in the validation (type_name.lower())
        # So "ENTITY" conflicts with "entity"
        with pytest.raises(ValueError, match="conflicts with TypeDB built-in"):

            class ENTITY(tbg.Entity):
                flags = TypeFlags(name="ENTITY")  # Still conflicts (case-insensitive)
                name: Name

        # But these should be allowed (different names)
        class EntityType(tbg.Entity):
            flags = TypeFlags(name="entity_type")  # Has suffix - safe
            name: Name

        # Should not raise errors
        assert EntityType.get_type_name() == "entity_type"


class TestComplexInheritanceHierarchy:
    """Test complex multi-level inheritance hierarchies."""

    def test_ast_hierarchy_entity_inheritance(self):
        """Test AST-like deep inheritance hierarchy for entities."""

        # Attribute types
        class NodeId(String):
            pass

        class LineNumber(Integer):
            pass

        class VarName(String):
            pass

        class TypeAnnotation(String):
            pass

        class Operator(String):
            pass

        # Level 1: Abstract base
        class ASTNode(tbg.Entity):
            flags = TypeFlags(name="ast-node", abstract=True)
            node_id: NodeId = Flag(Key)

        # Level 2: Abstract statement
        class Statement(ASTNode):
            flags = TypeFlags(name="statement", abstract=True)
            line_number: LineNumber

        # Level 3: Concrete statements
        class LetStatement(Statement):
            flags = TypeFlags(name="let-statement")
            var_name: VarName
            type_annotation: TypeAnnotation | None

        class ReturnStatement(Statement):
            flags = TypeFlags(name="return-statement")

        # Level 2: Abstract expression (sibling to Statement)
        class Expression(ASTNode):
            flags = TypeFlags(name="expression", abstract=True)

        # Level 3: Concrete expressions
        class BinaryExpr(Expression):
            flags = TypeFlags(name="binary-expr")
            operator: Operator

        class LiteralExpr(Expression):
            flags = TypeFlags(name="literal-expr")

        # Verify inheritance chain
        assert ASTNode.get_supertype() is None
        assert Statement.get_supertype() == "ast-node"
        assert LetStatement.get_supertype() == "statement"
        assert ReturnStatement.get_supertype() == "statement"
        assert Expression.get_supertype() == "ast-node"
        assert BinaryExpr.get_supertype() == "expression"
        assert LiteralExpr.get_supertype() == "expression"

        # Verify owned attributes (only direct, not inherited)
        assert "node_id" in ASTNode.get_owned_attributes()
        assert "line_number" in Statement.get_owned_attributes()
        assert "node_id" not in Statement.get_owned_attributes()

        let_attrs = LetStatement.get_owned_attributes()
        assert "var_name" in let_attrs
        assert "type_annotation" in let_attrs
        assert "node_id" not in let_attrs
        assert "line_number" not in let_attrs

        return_attrs = ReturnStatement.get_owned_attributes()
        assert len(return_attrs) == 0  # No direct attributes

        binary_attrs = BinaryExpr.get_owned_attributes()
        assert "operator" in binary_attrs
        assert "node_id" not in binary_attrs

        # Verify schema generation
        ast_schema = ASTNode.to_schema_definition()
        assert ast_schema is not None
        assert "entity ast-node @abstract" in ast_schema
        assert "owns NodeId @key" in ast_schema

        stmt_schema = Statement.to_schema_definition()
        assert stmt_schema is not None
        assert "entity statement @abstract, sub ast-node" in stmt_schema
        assert "owns LineNumber" in stmt_schema
        assert "owns NodeId" not in stmt_schema

        let_schema = LetStatement.to_schema_definition()
        assert let_schema is not None
        assert "entity let-statement, sub statement" in let_schema
        assert "owns VarName" in let_schema
        assert "owns TypeAnnotation" in let_schema
        assert "owns NodeId" not in let_schema
        assert "owns LineNumber" not in let_schema

        return_schema = ReturnStatement.to_schema_definition()
        assert return_schema is not None
        assert "entity return-statement, sub statement" in return_schema
        assert "owns" not in return_schema  # No direct attributes

    def test_relation_inheritance_hierarchy(self):
        """Test multi-level inheritance hierarchy for relations."""

        # Attribute types
        class RelId(String):
            pass

        class Timestamp(String):
            pass

        class Priority(Integer):
            pass

        class Message(String):
            pass

        # Entity for role players
        class Name(String):
            pass

        class Person(tbg.Entity):
            flags = TypeFlags(name="person")
            name: Name

        # Level 1: Abstract base relation
        class Interaction(tbg.Relation):
            flags = TypeFlags(name="interaction", abstract=True)
            participant: Role[Person] = Role("participant", Person)
            rel_id: RelId = Flag(Key)

        # Level 2: Abstract communication
        class Communication(Interaction):
            flags = TypeFlags(name="communication", abstract=True)
            timestamp: Timestamp

        # Level 3: Concrete communications
        class DirectMessage(Communication):
            flags = TypeFlags(name="direct-message")
            sender: Role[Person] = Role("sender", Person)
            receiver: Role[Person] = Role("receiver", Person)
            message: Message

        class GroupChat(Communication):
            flags = TypeFlags(name="group-chat")
            members: Role[Person] = Role("members", Person)

        # Level 2: Abstract collaboration (sibling to Communication)
        class Collaboration(Interaction):
            flags = TypeFlags(name="collaboration", abstract=True)
            priority: Priority | None

        # Level 3: Concrete collaboration
        class Project(Collaboration):
            flags = TypeFlags(name="project")
            team_member: Role[Person] = Role("team-member", Person)

        # Verify inheritance chain
        assert Interaction.get_supertype() is None
        assert Communication.get_supertype() == "interaction"
        assert DirectMessage.get_supertype() == "communication"
        assert GroupChat.get_supertype() == "communication"
        assert Collaboration.get_supertype() == "interaction"
        assert Project.get_supertype() == "collaboration"

        # Verify owned attributes (only direct, not inherited)
        assert "rel_id" in Interaction.get_owned_attributes()
        assert "timestamp" in Communication.get_owned_attributes()
        assert "rel_id" not in Communication.get_owned_attributes()

        dm_attrs = DirectMessage.get_owned_attributes()
        assert "message" in dm_attrs
        assert "rel_id" not in dm_attrs
        assert "timestamp" not in dm_attrs

        project_attrs = Project.get_owned_attributes()
        assert len(project_attrs) == 0  # No direct attributes

        # Verify schema generation
        interaction_schema = Interaction.to_schema_definition()
        assert interaction_schema is not None
        assert "relation interaction @abstract" in interaction_schema
        assert "owns RelId @key" in interaction_schema

        comm_schema = Communication.to_schema_definition()
        assert comm_schema is not None
        assert "relation communication @abstract, sub interaction" in comm_schema
        assert "owns Timestamp" in comm_schema
        assert "owns RelId" not in comm_schema

        dm_schema = DirectMessage.to_schema_definition()
        assert dm_schema is not None
        assert "relation direct-message, sub communication" in dm_schema
        assert "owns Message" in dm_schema
        assert "owns RelId" not in dm_schema
        assert "owns Timestamp" not in dm_schema

    def test_mixed_base_and_normal_inheritance(self):
        """Test combination of base=True and normal TypeDB inheritance."""

        # Attribute types
        class Id(String):
            pass

        class CreatedAt(String):
            pass

        class Name(String):
            pass

        class Email(String):
            pass

        # Python-only base class (base=True)
        class Timestamped(tbg.Entity):
            flags = TypeFlags(base=True)
            created_at: CreatedAt

        # Abstract TypeDB entity that also inherits from Python base
        # Pydantic warns about field shadowing when base=True attrs are absorbed
        with warnings.catch_warnings():
            warnings.filterwarnings("ignore", message="Field name.*shadows", category=UserWarning)

            class Resource(Timestamped):
                flags = TypeFlags(name="resource", abstract=True)
                id: Id = Flag(Key)

        # Concrete entity
        class User(Resource):
            flags = TypeFlags(name="user")
            name: Name
            email: Email

        # Verify Timestamped doesn't appear in schema
        assert Timestamped.to_schema_definition() is None

        # Verify Resource includes created_at from base=True parent
        resource_attrs = Resource.get_owned_attributes()
        assert "id" in resource_attrs
        assert "created_at" in resource_attrs  # From base=True parent

        resource_schema = Resource.to_schema_definition()
        assert resource_schema is not None
        assert "entity resource @abstract" in resource_schema
        assert "owns Id @key" in resource_schema
        assert "owns CreatedAt" in resource_schema

        # Verify User only has direct attributes
        user_attrs = User.get_owned_attributes()
        assert "name" in user_attrs
        assert "email" in user_attrs
        assert "id" not in user_attrs  # Inherited from Resource
        assert "created_at" not in user_attrs  # Inherited from Resource

        user_schema = User.to_schema_definition()
        assert user_schema is not None
        assert "entity user, sub resource" in user_schema
        assert "owns Name" in user_schema
        assert "owns Email" in user_schema
        assert "owns Id" not in user_schema
        assert "owns CreatedAt" not in user_schema


class TestInheritanceAttributePropagation:
    """Test that attributes are inherited correctly."""

    def test_child_inherits_parent_attributes(self):
        """Test that child entities inherit parent attributes via TypeDB inheritance."""
        # In TypeDB, inheritance means the child is a subtype of the parent
        # Child entities automatically inherit parent attributes - no need to redeclare
        # get_owned_attributes() returns only DIRECT attributes (not inherited ones)

        class Name(String):
            pass

        class Age(Integer):
            pass

        class Animal(tbg.Entity):
            flags = TypeFlags(abstract=True, name="animal")
            name: Name

        class Dog(Animal):
            flags = TypeFlags(name="dog")
            age: Age

        # Check owned attributes - only DIRECT attributes, not inherited
        animal_attrs = Animal.get_owned_attributes()
        dog_attrs = Dog.get_owned_attributes()

        assert "name" in animal_attrs
        assert "age" in dog_attrs
        # Dog does NOT include "name" in get_owned_attributes() because it's inherited
        # TypeDB handles inheritance automatically
        assert "name" not in dog_attrs

        # Check schema generation
        animal_schema = Animal.to_schema_definition()
        assert animal_schema is not None
        assert "owns Name" in animal_schema  # Name uses CLASS_NAME default

        dog_schema = Dog.to_schema_definition()
        assert dog_schema is not None
        assert "entity dog, sub animal" in dog_schema
        assert "owns Age" in dog_schema  # Age uses CLASS_NAME default
        # Dog schema does NOT include "owns Name" - it's inherited from Animal
        assert "owns Name" not in dog_schema
