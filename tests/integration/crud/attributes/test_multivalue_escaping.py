"""Integration tests for escaping special characters in multi-value attributes."""

import pytest

from type_bridge import (
    Card,
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
@pytest.mark.order(250)
def test_entity_multivalue_quotes_insert_fetch(clean_db):
    """Test inserting and fetching entity with multi-value attributes containing quotes."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_mv_quotes")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert person with quotes in tags
    person = Person(
        name=Name("Alice"),
        tags=[Tag('skill "Python"'), Tag('role "Engineer"'), Tag('He said "hello"')],
    )
    manager.insert(person)

    # Fetch and verify quotes are preserved
    results = manager.all()
    assert len(results) == 1
    assert len(results[0].tags) == 3
    tag_values = {tag.value for tag in results[0].tags}
    assert 'skill "Python"' in tag_values
    assert 'role "Engineer"' in tag_values
    assert 'He said "hello"' in tag_values


@pytest.mark.integration
@pytest.mark.order(251)
def test_entity_multivalue_backslashes_insert_fetch(clean_db):
    """Test inserting and fetching entity with multi-value attributes containing backslashes."""

    class Name(String):
        pass

    class Path(String):
        pass

    class FileSet(Entity):
        flags = TypeFlags(name="fileset_mv_backslash")
        name: Name = Flag(Key)
        paths: list[Path] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(FileSet)
    schema_manager.sync_schema(force=True)

    manager = FileSet.manager(clean_db)

    # Insert with backslashes
    fileset = FileSet(
        name=Name("ProjectFiles"),
        paths=[Path("C:\\Users\\Alice"), Path("D:\\Projects\\TypeBridge\\src")],
    )
    manager.insert(fileset)

    # Fetch and verify backslashes are preserved
    results = manager.all()
    assert len(results) == 1
    assert len(results[0].paths) == 2
    path_values = {path.value for path in results[0].paths}
    assert "C:\\Users\\Alice" in path_values
    assert "D:\\Projects\\TypeBridge\\src" in path_values


@pytest.mark.integration
@pytest.mark.order(252)
def test_entity_multivalue_mixed_escaping_insert_fetch(clean_db):
    """Test inserting and fetching entity with multi-value attributes containing both quotes and backslashes."""

    class Name(String):
        pass

    class Description(String):
        pass

    class Document(Entity):
        flags = TypeFlags(name="document_mv_mixed")
        name: Name = Flag(Key)
        descriptions: list[Description] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Document)
    schema_manager.sync_schema(force=True)

    manager = Document.manager(clean_db)

    # Insert with mixed escaping
    doc = Document(
        name=Name("README"),
        descriptions=[
            Description(r'Path: "C:\Program Files\App"'),
            Description(r'Quote: "C:\\test\\"'),
            Description('Normal "text" with \\backslash'),
        ],
    )
    manager.insert(doc)

    # Fetch and verify complex strings are preserved
    results = manager.all()
    assert len(results) == 1
    assert len(results[0].descriptions) == 3
    desc_values = {desc.value for desc in results[0].descriptions}
    assert r'Path: "C:\Program Files\App"' in desc_values
    assert r'Quote: "C:\\test\\"' in desc_values
    assert 'Normal "text" with \\backslash' in desc_values


@pytest.mark.integration
@pytest.mark.order(253)
def test_entity_multivalue_insert_many_with_escaping(clean_db):
    """Test insert_many with multi-value attributes containing special characters."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_mv_insert_many")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert multiple people with special characters in tags
    people = [
        Person(name=Name("Alice"), tags=[Tag('skill "Python"'), Tag("path\\to\\file")]),
        Person(name=Name("Bob"), tags=[Tag('quote "here"'), Tag('both "A\\B"')]),
        Person(name=Name("Charlie"), tags=[Tag("normal"), Tag('mixed "test\\"')]),
    ]
    manager.insert_many(people)

    # Fetch all and verify
    results = manager.all()
    assert len(results) == 3

    # Find each person and verify tags
    alice = [p for p in results if p.name.value == "Alice"][0]
    assert len(alice.tags) == 2
    alice_tag_values = {tag.value for tag in alice.tags}
    assert 'skill "Python"' in alice_tag_values
    assert "path\\to\\file" in alice_tag_values

    bob = [p for p in results if p.name.value == "Bob"][0]
    assert len(bob.tags) == 2
    bob_tag_values = {tag.value for tag in bob.tags}
    assert 'quote "here"' in bob_tag_values
    assert 'both "A\\B"' in bob_tag_values

    charlie = [p for p in results if p.name.value == "Charlie"][0]
    assert len(charlie.tags) == 2
    charlie_tag_values = {tag.value for tag in charlie.tags}
    assert "normal" in charlie_tag_values
    assert 'mixed "test\\"' in charlie_tag_values


@pytest.mark.integration
@pytest.mark.order(254)
def test_entity_multivalue_update_with_escaping(clean_db):
    """Test updating multi-value attributes with special characters."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_mv_update_escape")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert person
    person = Person(name=Name("Alice"), tags=[Tag("initial")])
    manager.insert(person)

    # Fetch and update with special characters
    fetched = manager.all()[0]
    fetched.tags = [Tag('skill "Python"'), Tag("path\\to\\code"), Tag('both "A\\B"')]
    manager.update(fetched)

    # Verify update
    updated = manager.all()[0]
    assert len(updated.tags) == 3
    updated_tag_values = {tag.value for tag in updated.tags}
    assert 'skill "Python"' in updated_tag_values
    assert "path\\to\\code" in updated_tag_values
    assert 'both "A\\B"' in updated_tag_values


@pytest.mark.integration
@pytest.mark.order(255)
def test_relation_multivalue_quotes_insert_fetch(clean_db):
    """Test inserting and fetching relation with multi-value attributes containing quotes."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Salary(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_rel_mv_quotes")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company_rel_mv_quotes")
        name: Name = Flag(Key)

    class Employment(Relation):
        flags = TypeFlags(name="employment_mv_quotes")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        salary: Salary
        tags: list[Tag] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    emp_mgr = Employment.manager(clean_db)

    # Insert entities
    alice = Person(name=Name("Alice"))
    techcorp = Company(name=Name("TechCorp"))
    person_mgr.insert(alice)
    company_mgr.insert(techcorp)

    # Insert relation with quotes in tags
    emp = Employment(
        employee=alice,
        employer=techcorp,
        salary=Salary(100000),
        tags=[Tag('skill "Python"'), Tag('role "Senior Engineer"')],
    )
    emp_mgr.insert(emp)

    # Fetch and verify quotes are preserved
    results = emp_mgr.all()
    assert len(results) == 1
    assert len(results[0].tags) == 2
    tag_values = {tag.value for tag in results[0].tags}
    assert 'skill "Python"' in tag_values
    assert 'role "Senior Engineer"' in tag_values


@pytest.mark.integration
@pytest.mark.order(256)
def test_relation_multivalue_insert_many_with_escaping(clean_db):
    """Test insert_many for relations with multi-value attributes containing special characters."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Salary(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_rel_many_escape")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company_rel_many_escape")
        name: Name = Flag(Key)

    class Employment(Relation):
        flags = TypeFlags(name="employment_many_escape")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        salary: Salary
        tags: list[Tag] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    emp_mgr = Employment.manager(clean_db)

    # Insert entities
    alice = Person(name=Name("Alice"))
    bob = Person(name=Name("Bob"))
    techcorp = Company(name=Name("TechCorp"))
    person_mgr.insert_many([alice, bob])
    company_mgr.insert(techcorp)

    # Insert multiple relations with special characters
    employments = [
        Employment(
            employee=alice,
            employer=techcorp,
            salary=Salary(100000),
            tags=[Tag('skill "Python"'), Tag("path\\to\\project")],
        ),
        Employment(
            employee=bob,
            employer=techcorp,
            salary=Salary(110000),
            tags=[Tag('both "A\\B"'), Tag('quote "end"')],
        ),
    ]
    emp_mgr.insert_many(employments)

    # Fetch and verify
    results = emp_mgr.all()
    assert len(results) == 2

    # Find each employment and verify tags
    alice_emp = [e for e in results if e.employee.name.value == "Alice"][0]
    assert len(alice_emp.tags) == 2
    alice_tag_values = {tag.value for tag in alice_emp.tags}
    assert 'skill "Python"' in alice_tag_values
    assert "path\\to\\project" in alice_tag_values

    bob_emp = [e for e in results if e.employee.name.value == "Bob"][0]
    assert len(bob_emp.tags) == 2
    bob_tag_values = {tag.value for tag in bob_emp.tags}
    assert 'both "A\\B"' in bob_tag_values
    assert 'quote "end"' in bob_tag_values


@pytest.mark.integration
@pytest.mark.order(257)
def test_relation_multivalue_update_with_escaping(clean_db):
    """Test update_with for relations with multi-value attributes containing special characters."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Salary(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_rel_update_escape")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company_rel_update_escape")
        name: Name = Flag(Key)

    class Employment(Relation):
        flags = TypeFlags(name="employment_update_escape")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        salary: Salary
        tags: list[Tag] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    emp_mgr = Employment.manager(clean_db)

    # Insert entities
    alice = Person(name=Name("Alice"))
    techcorp = Company(name=Name("TechCorp"))
    person_mgr.insert(alice)
    company_mgr.insert(techcorp)

    # Insert relation
    emp = Employment(
        employee=alice,
        employer=techcorp,
        salary=Salary(100000),
        tags=[Tag("initial")],
    )
    emp_mgr.insert(emp)

    # Update with special characters using update_with
    def add_special_tags(employment):
        current_tags = list(employment.tags) if employment.tags else []
        current_tags.extend([Tag('skill "Python"'), Tag("path\\to\\code")])
        employment.tags = current_tags

    emp_mgr.filter(Salary.eq(Salary(100000))).update_with(add_special_tags)

    # Verify update
    results = emp_mgr.all()
    assert len(results) == 1
    assert len(results[0].tags) == 3
    tag_values = {tag.value for tag in results[0].tags}
    assert "initial" in tag_values
    assert 'skill "Python"' in tag_values
    assert "path\\to\\code" in tag_values


@pytest.mark.integration
@pytest.mark.order(258)
def test_multivalue_empty_string_escaping(clean_db):
    """Test multi-value attributes with empty strings and edge cases."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Item(Entity):
        flags = TypeFlags(name="item_mv_empty")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Item)
    schema_manager.sync_schema(force=True)

    manager = Item.manager(clean_db)

    # Insert with empty string and special characters
    item = Item(
        name=Name("TestItem"),
        tags=[Tag(""), Tag('quote "here"'), Tag("path\\to\\file")],
    )
    manager.insert(item)

    # Fetch and verify
    results = manager.all()
    assert len(results) == 1
    assert len(results[0].tags) == 3
    tag_values = [tag.value for tag in results[0].tags]
    # Check for empty string
    assert "" in tag_values
    # Check for other values
    assert 'quote "here"' in tag_values
    assert "path\\to\\file" in tag_values
