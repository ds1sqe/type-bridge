"""Integration tests for schema inheritance."""

import pytest

from type_bridge import (
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(6)
def test_schema_inheritance(clean_db):
    """Test schema creation with entity inheritance."""

    class Name(String):
        pass

    class Animal(Entity):
        flags = TypeFlags(name="animal", abstract=True)
        name: Name = Flag(Key)

    class Species(String):
        pass

    class Dog(Animal):
        flags = TypeFlags(name="dog")
        species: Species

    # Create and sync schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Animal, Dog)
    schema_manager.sync_schema(force=True)

    # Verify schema
    schema_info = schema_manager.collect_schema_info()

    entity_names = {e.get_type_name() for e in schema_info.entities}
    assert "animal" in entity_names
    assert "dog" in entity_names

    # Verify dog inherits from animal
    dog_entity = [e for e in schema_info.entities if e.get_type_name() == "dog"][0]
    dog_owned_attrs = dog_entity.get_owned_attributes()
    # Dog only directly owns species - name is inherited from Animal
    assert "name" not in dog_owned_attrs  # Inherited from Animal
    assert "species" in dog_owned_attrs  # Direct attribute


@pytest.mark.integration
@pytest.mark.order(7)
def test_multi_level_abstract_inheritance(clean_db):
    """Test multi-level inheritance with abstract entities.

    This tests the TypeDB 3.x syntax: entity name @abstract, sub parent,
    Similar to TypeDB's social network schema example.
    """

    # Base attribute types
    class Id(String):
        pass

    class PageId(String):
        pass

    class Username(String):
        pass

    class Bio(String):
        pass

    # Level 1: Abstract base entity
    class Content(Entity):
        flags = TypeFlags(name="content", abstract=True)
        id: Id = Flag(Key)

    # Level 2: Abstract entity inheriting from abstract
    class Page(Content):
        flags = TypeFlags(name="page", abstract=True)
        page_id: PageId
        bio: Bio | None

    # Level 3: Abstract entity inheriting from abstract
    class Profile(Page):
        flags = TypeFlags(name="profile", abstract=True)
        username: Username

    # Level 4: Concrete entity inheriting from abstract
    class Person(Profile):
        flags = TypeFlags(name="person")

    # Create and sync schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Content, Page, Profile, Person)
    schema_manager.sync_schema(force=True)

    # Verify schema was created successfully
    schema_info = schema_manager.collect_schema_info()

    entity_names = {e.get_type_name() for e in schema_info.entities}
    assert "content" in entity_names
    assert "page" in entity_names
    assert "profile" in entity_names
    assert "person" in entity_names

    # Verify inheritance chain
    assert Page.get_supertype() == "content"
    assert Profile.get_supertype() == "page"
    assert Person.get_supertype() == "profile"

    # Verify Person has only direct attributes (inherited ones come from parents)
    person_entity = [e for e in schema_info.entities if e.get_type_name() == "person"][0]
    person_attrs = person_entity.get_owned_attributes()
    # Person has no direct attributes - all are inherited
    assert "id" not in person_attrs  # Inherited from Content
    assert "page_id" not in person_attrs  # Inherited from Page
    assert "bio" not in person_attrs  # Inherited from Page
    assert "username" not in person_attrs  # Inherited from Profile
    assert len(person_attrs) == 0


@pytest.mark.integration
@pytest.mark.order(8)
def test_relation_inheritance_with_abstract(clean_db):
    """Test relation inheritance with abstract relations.

    Similar to TypeDB's social-relation pattern.
    """

    class Name(String):
        pass

    class StartDate(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    # Abstract base relation
    class SocialRelation(Relation):
        flags = TypeFlags(name="social_relation", abstract=True)
        related: Role[Person] = Role("related", Person)

    # Concrete relation inheriting from abstract
    class Friendship(SocialRelation):
        flags = TypeFlags(name="friendship")
        friend: Role[Person] = Role("friend", Person)

    # Another concrete relation with attribute
    class Relationship(SocialRelation):
        flags = TypeFlags(name="relationship")
        partner: Role[Person] = Role("partner", Person)
        start_date: StartDate | None

    # Create and sync schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, SocialRelation, Friendship, Relationship)
    schema_manager.sync_schema(force=True)

    # Verify schema
    schema_info = schema_manager.collect_schema_info()

    relation_names = {r.get_type_name() for r in schema_info.relations}
    assert "social_relation" in relation_names
    assert "friendship" in relation_names
    assert "relationship" in relation_names

    # Verify inheritance
    assert Friendship.get_supertype() == "social_relation"
    assert Relationship.get_supertype() == "social_relation"


@pytest.mark.integration
@pytest.mark.order(9)
def test_ast_hierarchy_integration(clean_db):
    """Test AST-like deep inheritance hierarchy with actual TypeDB.

    Tests 4-level entity inheritance: ASTNode -> Statement/Expression -> concrete types
    """

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
    class ASTNode(Entity):
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

    # Create and sync schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(
        ASTNode,
        Statement,
        LetStatement,
        ReturnStatement,
        Expression,
        BinaryExpr,
        LiteralExpr,
    )
    schema_manager.sync_schema(force=True)

    # Verify schema was created
    schema_info = schema_manager.collect_schema_info()
    entity_names = {e.get_type_name() for e in schema_info.entities}

    assert "ast-node" in entity_names
    assert "statement" in entity_names
    assert "let-statement" in entity_names
    assert "return-statement" in entity_names
    assert "expression" in entity_names
    assert "binary-expr" in entity_names
    assert "literal-expr" in entity_names

    # Verify inheritance chain
    assert Statement.get_supertype() == "ast-node"
    assert LetStatement.get_supertype() == "statement"
    assert ReturnStatement.get_supertype() == "statement"
    assert Expression.get_supertype() == "ast-node"
    assert BinaryExpr.get_supertype() == "expression"

    # Verify direct attributes only
    let_attrs = LetStatement.get_owned_attributes()
    assert "var_name" in let_attrs
    assert "node_id" not in let_attrs  # Inherited
    assert "line_number" not in let_attrs  # Inherited

    return_attrs = ReturnStatement.get_owned_attributes()
    assert len(return_attrs) == 0  # No direct attributes

    # Insert test data
    let_mgr = LetStatement.manager(clean_db)
    let_stmt = LetStatement(
        node_id=NodeId("let-1"),
        line_number=LineNumber(10),
        var_name=VarName("x"),
        type_annotation=TypeAnnotation("int"),
    )
    let_mgr.insert(let_stmt)

    # Fetch and verify
    results = let_mgr.get(node_id="let-1")
    assert len(results) == 1
    assert results[0].var_name.value == "x"
    assert results[0].line_number.value == 10


@pytest.mark.integration
@pytest.mark.order(10)
def test_relation_hierarchy_integration(clean_db):
    """Test multi-level relation inheritance with actual TypeDB.

    Tests: Interaction -> Communication/Collaboration -> concrete relations
    """

    # Attribute types
    class RelId(String):
        pass

    class Timestamp(String):
        pass

    class Priority(Integer):
        pass

    class MessageContent(String):
        pass

    class PersonName(String):
        pass

    # Entity for role players
    class Person(Entity):
        flags = TypeFlags(name="person")
        name: PersonName = Flag(Key)

    # Level 1: Abstract base relation
    class Interaction(Relation):
        flags = TypeFlags(name="interaction", abstract=True)
        participant: Role[Person] = Role("participant", Person)
        rel_id: RelId = Flag(Key)

    # Level 2: Abstract communication
    class Communication(Interaction):
        flags = TypeFlags(name="communication", abstract=True)
        timestamp: Timestamp

    # Level 3: Concrete communication
    class DirectMessage(Communication):
        flags = TypeFlags(name="direct-message")
        sender: Role[Person] = Role("sender", Person)
        receiver: Role[Person] = Role("receiver", Person)
        message: MessageContent

    # Level 2: Abstract collaboration
    class Collaboration(Interaction):
        flags = TypeFlags(name="collaboration", abstract=True)
        priority: Priority | None

    # Level 3: Concrete collaboration
    class Project(Collaboration):
        flags = TypeFlags(name="project")
        team_member: Role[Person] = Role("team-member", Person)

    # Create and sync schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(
        Person,
        Interaction,
        Communication,
        DirectMessage,
        Collaboration,
        Project,
    )
    schema_manager.sync_schema(force=True)

    # Verify schema
    schema_info = schema_manager.collect_schema_info()
    relation_names = {r.get_type_name() for r in schema_info.relations}

    assert "interaction" in relation_names
    assert "communication" in relation_names
    assert "direct-message" in relation_names
    assert "collaboration" in relation_names
    assert "project" in relation_names

    # Verify inheritance
    assert Communication.get_supertype() == "interaction"
    assert DirectMessage.get_supertype() == "communication"
    assert Collaboration.get_supertype() == "interaction"
    assert Project.get_supertype() == "collaboration"

    # Verify direct attributes only
    dm_attrs = DirectMessage.get_owned_attributes()
    assert "message" in dm_attrs
    assert "rel_id" not in dm_attrs  # Inherited
    assert "timestamp" not in dm_attrs  # Inherited

    project_attrs = Project.get_owned_attributes()
    assert len(project_attrs) == 0  # No direct attributes

    # Insert test data
    person_mgr = Person.manager(clean_db)
    alice = Person(name=PersonName("Alice"))
    bob = Person(name=PersonName("Bob"))
    person_mgr.insert_many([alice, bob])

    dm_mgr = DirectMessage.manager(clean_db)
    dm = DirectMessage(
        rel_id=RelId("dm-1"),
        timestamp=Timestamp("2024-01-01T10:00:00"),
        message=MessageContent("Hello!"),
        sender=alice,
        receiver=bob,
    )
    dm_mgr.insert(dm)

    # Fetch and verify
    results = dm_mgr.get(rel_id="dm-1")
    assert len(results) == 1
    assert results[0].message.value == "Hello!"
