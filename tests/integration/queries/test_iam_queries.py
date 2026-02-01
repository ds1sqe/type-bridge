"""Tests for IAM (Identity and Access Management) query patterns.

Tests relation inheritance, role specialization, role cardinality,
attribute inheritance, and relations-as-role-players patterns.
Based on TypeDB IAM example schema.
"""

import pytest

from type_bridge import (
    Boolean,
    Card,
    Database,
    DateTime,
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

# =============================================================================
# Shared Attribute Types
# =============================================================================


class Credential(String):
    pass


class IamName(String):
    """Name attribute for IAM entities."""

    pass


class IamEmail(String):
    """Email attribute - can have 1-2 values per entity."""

    pass


class FullName(String):
    pass


class ObjectType(String):
    pass


class OwnershipType(String):
    pass


class ReviewDate(DateTime):
    pass


class Validity(Boolean):
    pass


class FilePath(String):
    pass


class Reference(String):
    pass


# =============================================================================
# Test: Relation Inheritance with Role Specialization
# =============================================================================


@pytest.mark.integration
class TestRelationInheritance:
    """Test relation inheritance and role specialization patterns."""

    @pytest.fixture
    def schema_with_relation_inheritance(self, clean_db: Database):
        """Set up schema with inherited relations and specialized roles."""

        # Entity types first (needed for abstract membership roles)
        class Subject(Entity):
            flags = TypeFlags(name="subject_iam", abstract=True)
            credential: Credential | None = None

        class UserGroup(Subject):
            flags = TypeFlags(name="user_group_iam")
            name: IamName = Flag(Key)

        class User(Subject):
            flags = TypeFlags(name="user_iam", abstract=True)

        class Employee(User):
            flags = TypeFlags(name="employee_iam")
            full_name: FullName = Flag(Key)
            email: list[IamEmail] = Flag(Card(min=1, max=2))

        # Concrete membership - group membership with specialized roles
        class GroupMembership(Relation):
            flags = TypeFlags(name="group_membership_iam")
            user_group: Role[UserGroup] = Role("user_group", UserGroup)
            group_member: Role[Subject] = Role("group_member", Subject)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Subject, User, Employee, UserGroup, GroupMembership)
        schema_manager.sync_schema(force=True)

        return clean_db, Subject, User, Employee, UserGroup, GroupMembership

    def test_insert_concrete_relation_subtype(self, schema_with_relation_inheritance):
        """Insert a concrete relation that is a subtype of abstract relation."""
        db, Subject, User, Employee, UserGroup, GroupMembership = schema_with_relation_inheritance

        # Create a user group
        engineering = UserGroup(name=IamName("Engineering"))
        UserGroup.manager(db).insert(engineering)

        # Create an employee
        alice = Employee(
            full_name=FullName("Alice Smith"),
            email=[IamEmail("alice@company.com")],
        )
        Employee.manager(db).insert(alice)

        # Create group membership
        membership = GroupMembership(
            user_group=engineering,
            group_member=alice,
        )
        GroupMembership.manager(db).insert(membership)

        # Query memberships
        memberships = GroupMembership.manager(db).all()
        assert len(memberships) == 1
        assert str(memberships[0].user_group.name) == "Engineering"

    def test_query_relation_with_polymorphic_member(self, schema_with_relation_inheritance):
        """Query relation where member role accepts abstract type."""
        db, Subject, User, Employee, UserGroup, GroupMembership = schema_with_relation_inheritance

        # Create groups
        eng_group = UserGroup(name=IamName("Engineering"))
        admin_group = UserGroup(name=IamName("Admins"))
        UserGroup.manager(db).insert(eng_group)
        UserGroup.manager(db).insert(admin_group)

        # Create employee
        bob = Employee(
            full_name=FullName("Bob Jones"),
            email=[IamEmail("bob@company.com")],
        )
        Employee.manager(db).insert(bob)

        # Add employee to engineering group
        GroupMembership.manager(db).insert(GroupMembership(user_group=eng_group, group_member=bob))

        # Add engineering group to admins group (group as member)
        eng_fetched = UserGroup.manager(db).get(name="Engineering")[0]
        GroupMembership.manager(db).insert(
            GroupMembership(user_group=admin_group, group_member=eng_fetched)
        )

        # Query all memberships
        memberships = GroupMembership.manager(db).all()
        assert len(memberships) == 2

        # Both Employee and UserGroup can be members (polymorphic role)
        member_types = {type(m.group_member).__name__ for m in memberships}
        assert member_types == {"Employee", "UserGroup"}


# =============================================================================
# Test: Multi-Value Attributes with Upper Bounds
# =============================================================================


@pytest.mark.integration
class TestBoundedMultiValueAttributes:
    """Test multi-value attributes with upper bounds (@card(1..2))."""

    @pytest.fixture
    def schema_with_bounded_attrs(self, clean_db: Database):
        """Set up schema with bounded multi-value attributes."""

        class Person(Entity):
            flags = TypeFlags(name="person_bounded")
            name: FullName = Flag(Key)
            # 1-2 emails allowed
            email: list[IamEmail] = Flag(Card(min=1, max=2))

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person)
        schema_manager.sync_schema(force=True)

        return clean_db, Person

    def test_insert_with_minimum_values(self, schema_with_bounded_attrs):
        """Insert entity with minimum required values (1 email)."""
        db, Person = schema_with_bounded_attrs

        person = Person(
            name=FullName("Alice"),
            email=[IamEmail("alice@example.com")],
        )
        Person.manager(db).insert(person)

        result = Person.manager(db).get(name="Alice")[0]
        assert len(result.email) == 1
        assert str(result.email[0]) == "alice@example.com"

    def test_insert_with_maximum_values(self, schema_with_bounded_attrs):
        """Insert entity with maximum allowed values (2 emails)."""
        db, Person = schema_with_bounded_attrs

        person = Person(
            name=FullName("Bob"),
            email=[IamEmail("bob@work.com"), IamEmail("bob@personal.com")],
        )
        Person.manager(db).insert(person)

        result = Person.manager(db).get(name="Bob")[0]
        assert len(result.email) == 2
        emails = {str(e) for e in result.email}
        assert emails == {"bob@work.com", "bob@personal.com"}

    def test_update_multi_value_within_bounds(self, schema_with_bounded_attrs):
        """Update multi-value attribute staying within bounds."""
        db, Person = schema_with_bounded_attrs

        # Insert with 1 email
        person = Person(
            name=FullName("Charlie"),
            email=[IamEmail("charlie@v1.com")],
        )
        Person.manager(db).insert(person)

        # Update to 2 emails
        fetched = Person.manager(db).get(name="Charlie")[0]
        fetched.email = [IamEmail("charlie@v2.com"), IamEmail("charlie@backup.com")]
        Person.manager(db).update(fetched)

        result = Person.manager(db).get(name="Charlie")[0]
        assert len(result.email) == 2


# =============================================================================
# Test: Attribute Inheritance
# =============================================================================


@pytest.mark.integration
class TestAttributeInheritance:
    """Test attribute type inheritance patterns."""

    @pytest.fixture
    def schema_with_attr_inheritance(self, clean_db: Database):
        """Set up schema with inherited attribute types.

        Note: In type-bridge, attribute inheritance is modeled through
        Python class inheritance with AttributeFlags for TypeDB name override.
        """
        from type_bridge import AttributeFlags

        # Base ID type (abstract concept)
        class ResourceId(String):
            """Base resource identifier."""

            flags = AttributeFlags(name="resource_id_iam")

        # Specialized IDs
        class FileId(String):
            """File path identifier."""

            flags = AttributeFlags(name="file_id_iam")

        class RecordId(String):
            """Database record identifier."""

            flags = AttributeFlags(name="record_id_iam")

        # Entities using different ID types
        class File(Entity):
            flags = TypeFlags(name="file_iam")
            path: FileId = Flag(Key)

        class Record(Entity):
            flags = TypeFlags(name="record_iam")
            primary_key: RecordId = Flag(Key)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(File, Record)
        schema_manager.sync_schema(force=True)

        return clean_db, File, Record, FileId, RecordId

    def test_different_id_types_are_distinct(self, schema_with_attr_inheritance):
        """Different ID types remain distinct in the database."""
        db, File, Record, FileId, RecordId = schema_with_attr_inheritance

        # Insert file
        file = File(path=FileId("/home/user/doc.txt"))
        File.manager(db).insert(file)

        # Insert record with same string value
        record = Record(primary_key=RecordId("/home/user/doc.txt"))
        Record.manager(db).insert(record)

        # Both should exist independently
        files = File.manager(db).all()
        records = Record.manager(db).all()
        assert len(files) == 1
        assert len(records) == 1

    def test_query_by_specialized_id(self, schema_with_attr_inheritance):
        """Query entities by their specialized ID attribute."""
        db, File, Record, FileId, RecordId = schema_with_attr_inheritance

        # Insert files
        File.manager(db).insert(File(path=FileId("/etc/config")))
        File.manager(db).insert(File(path=FileId("/var/log/app.log")))

        # Query specific file
        result = File.manager(db).get(path="/etc/config")
        assert len(result) == 1
        assert str(result[0].path) == "/etc/config"


# =============================================================================
# Test: Relations as Role Players
# =============================================================================


@pytest.mark.integration
class TestRelationsAsRolePlayers:
    """Test relations playing roles in other relations."""

    @pytest.fixture
    def schema_with_relation_as_player(self, clean_db: Database):
        """Set up schema where a relation plays a role in another relation."""

        class ResourceName(String):
            pass

        class ActionName(String):
            pass

        # Object (resource) entity
        class Resource(Entity):
            flags = TypeFlags(name="resource_rap")
            name: ResourceName = Flag(Key)

        # Action entity
        class Action(Entity):
            flags = TypeFlags(name="action_rap")
            name: ActionName = Flag(Key)

        # Access relation - links resource to valid action
        class Access(Relation):
            flags = TypeFlags(name="access_rap")
            accessed_object: Role[Resource] = Role("accessed_object", Resource)
            valid_action: Role[Action] = Role("valid_action", Action)

        # Subject entity
        class SubjectEntity(Entity):
            flags = TypeFlags(name="subject_rap")
            name: IamName = Flag(Key)

        # Permission relation - uses Access as a role player!
        class Permission(Relation):
            flags = TypeFlags(name="permission_rap")
            permitted_subject: Role[SubjectEntity] = Role("permitted_subject", SubjectEntity)
            permitted_access: Role[Access] = Role("permitted_access", Access)
            validity: Validity | None = None

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Resource, Action, Access, SubjectEntity, Permission)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            Resource,
            Action,
            Access,
            SubjectEntity,
            Permission,
            ResourceName,
            ActionName,
        )

    def test_insert_relation_with_relation_as_player(self, schema_with_relation_as_player):
        """Insert a relation where one role player is itself a relation."""
        db, Resource, Action, Access, SubjectEntity, Permission, ResourceName, ActionName = (
            schema_with_relation_as_player
        )

        # Create entities
        file = Resource(name=ResourceName("secret.txt"))
        read_action = Action(name=ActionName("read"))
        alice = SubjectEntity(name=IamName("Alice"))

        Resource.manager(db).insert(file)
        Action.manager(db).insert(read_action)
        SubjectEntity.manager(db).insert(alice)

        # Create access relation
        file_fetched = Resource.manager(db).get(name="secret.txt")[0]
        read_fetched = Action.manager(db).get(name="read")[0]
        access = Access(accessed_object=file_fetched, valid_action=read_fetched)
        Access.manager(db).insert(access)

        # Create permission using the access relation as a role player
        access_fetched = Access.manager(db).all()[0]
        alice_fetched = SubjectEntity.manager(db).get(name="Alice")[0]

        permission = Permission(
            permitted_subject=alice_fetched,
            permitted_access=access_fetched,
            validity=Validity(True),
        )
        Permission.manager(db).insert(permission)

        # Query permissions
        permissions = Permission.manager(db).all()
        assert len(permissions) == 1
        assert permissions[0].validity is not None
        assert bool(permissions[0].validity) is True

    def test_query_permission_chain(self, schema_with_relation_as_player):
        """Query through the permission chain: subject -> access -> resource."""
        db, Resource, Action, Access, SubjectEntity, Permission, ResourceName, ActionName = (
            schema_with_relation_as_player
        )

        # Setup: Create multiple resources and permissions
        Resource.manager(db).insert(Resource(name=ResourceName("doc1.txt")))
        Resource.manager(db).insert(Resource(name=ResourceName("doc2.txt")))
        Action.manager(db).insert(Action(name=ActionName("read")))
        Action.manager(db).insert(Action(name=ActionName("write")))
        SubjectEntity.manager(db).insert(SubjectEntity(name=IamName("Bob")))

        # Create accesses
        doc1 = Resource.manager(db).get(name="doc1.txt")[0]
        doc2 = Resource.manager(db).get(name="doc2.txt")[0]
        read = Action.manager(db).get(name="read")[0]
        write = Action.manager(db).get(name="write")[0]
        bob = SubjectEntity.manager(db).get(name="Bob")[0]

        Access.manager(db).insert(Access(accessed_object=doc1, valid_action=read))
        Access.manager(db).insert(Access(accessed_object=doc2, valid_action=write))

        # Grant permissions
        accesses = Access.manager(db).all()
        for access in accesses:
            Permission.manager(db).insert(
                Permission(permitted_subject=bob, permitted_access=access, validity=Validity(True))
            )

        # Query Bob's permissions
        permissions = Permission.manager(db).filter(permitted_subject=bob).execute()
        assert len(permissions) == 2


# =============================================================================
# Test: Role Cardinality Constraints
# =============================================================================


@pytest.mark.integration
class TestRoleCardinalityConstraints:
    """Test role cardinality constraints (@card on roles)."""

    @pytest.fixture
    def schema_with_role_cardinality(self, clean_db: Database):
        """Set up schema with role cardinality constraints."""

        class PolicyName(String):
            pass

        class ActionCardName(String):
            """Action name attribute for cardinality tests."""

            pass

        class ActionItem(Entity):
            flags = TypeFlags(name="action_item_card")
            name: ActionCardName = Flag(Key)

        # Segregation policy requires EXACTLY 2 segregated actions
        class SegregationPolicy(Relation):
            flags = TypeFlags(name="segregation_policy_card")
            segregated_action: Role[ActionItem] = Role(
                "segregated_action", ActionItem, cardinality=Card(2, 2)
            )
            name: PolicyName

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(ActionItem, SegregationPolicy)
        schema_manager.sync_schema(force=True)

        return clean_db, ActionItem, SegregationPolicy, PolicyName, ActionCardName

    def test_insert_with_exact_role_cardinality(self, schema_with_role_cardinality):
        """Insert relation with exactly required number of role players."""
        db, ActionItem, SegregationPolicy, PolicyName, ActionCardName = schema_with_role_cardinality

        # Create two actions
        approve = ActionItem(name=ActionCardName("approve"))
        reject = ActionItem(name=ActionCardName("reject"))
        ActionItem.manager(db).insert(approve)
        ActionItem.manager(db).insert(reject)

        # Fetch for IID
        approve_fetched = ActionItem.manager(db).get(name="approve")[0]
        reject_fetched = ActionItem.manager(db).get(name="reject")[0]

        # Create segregation policy with exactly 2 actions
        policy = SegregationPolicy(
            segregated_action=[approve_fetched, reject_fetched],
            name=PolicyName("Approve-Reject Segregation"),
        )
        SegregationPolicy.manager(db).insert(policy)

        # Query
        policies = SegregationPolicy.manager(db).all()
        assert len(policies) == 1
        assert len(policies[0].segregated_action) == 2

    def test_query_segregation_policy_actions(self, schema_with_role_cardinality):
        """Query actions involved in segregation policies."""
        db, ActionItem, SegregationPolicy, PolicyName, ActionCardName = schema_with_role_cardinality

        # Create actions
        for action_name in ["create", "delete", "approve", "execute"]:
            ActionItem.manager(db).insert(ActionItem(name=ActionCardName(action_name)))

        # Create policies
        create = ActionItem.manager(db).get(name="create")[0]
        delete = ActionItem.manager(db).get(name="delete")[0]
        approve = ActionItem.manager(db).get(name="approve")[0]
        execute = ActionItem.manager(db).get(name="execute")[0]

        SegregationPolicy.manager(db).insert(
            SegregationPolicy(
                segregated_action=[create, delete],
                name=PolicyName("Create-Delete Segregation"),
            )
        )
        SegregationPolicy.manager(db).insert(
            SegregationPolicy(
                segregated_action=[approve, execute],
                name=PolicyName("Approve-Execute Segregation"),
            )
        )

        # Query all policies
        policies = SegregationPolicy.manager(db).all()
        assert len(policies) == 2

        # Each policy has exactly 2 actions
        for policy in policies:
            assert len(policy.segregated_action) == 2


# =============================================================================
# Test: Deep Entity Inheritance
# =============================================================================


@pytest.mark.integration
class TestDeepEntityInheritance:
    """Test deep entity inheritance hierarchies."""

    @pytest.fixture
    def schema_with_deep_inheritance(self, clean_db: Database):
        """Set up schema with 4-level entity inheritance."""

        class BaseId(String):
            pass

        class SpecialField(String):
            pass

        # Level 1: Abstract base
        class BaseSubject(Entity):
            flags = TypeFlags(name="base_subject_deep", abstract=True)
            base_id: BaseId = Flag(Key)

        # Level 2: Abstract user
        class AbstractUser(BaseSubject):
            flags = TypeFlags(name="abstract_user_deep", abstract=True)
            credential: Credential | None = None

        # Level 3: Concrete internal user
        class InternalUser(AbstractUser):
            flags = TypeFlags(name="internal_user_deep")
            full_name: FullName

        # Level 3: Concrete external user (sibling)
        class ExternalUser(AbstractUser):
            flags = TypeFlags(name="external_user_deep")
            company: SpecialField

        # Level 4: Admin user (extends InternalUser)
        class AdminUser(InternalUser):
            flags = TypeFlags(name="admin_user_deep")
            admin_level: Integer | None = None

        class AdminLevel(Integer):
            pass

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(BaseSubject, AbstractUser, InternalUser, ExternalUser, AdminUser)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            BaseSubject,
            AbstractUser,
            InternalUser,
            ExternalUser,
            AdminUser,
            BaseId,
            SpecialField,
        )

    def test_query_deep_subtype_has_all_inherited_attrs(self, schema_with_deep_inheritance):
        """Deepest subtype has all inherited attributes accessible."""
        (
            db,
            _BaseSubject,
            _AbstractUser,
            _InternalUser,
            _ExternalUser,
            AdminUser,
            BaseId,
            _SpecialField,
        ) = schema_with_deep_inheritance

        # Insert admin user (level 4)
        admin = AdminUser(
            base_id=BaseId("admin-001"),
            credential=Credential("secret-token"),
            full_name=FullName("Super Admin"),
        )
        AdminUser.manager(db).insert(admin)

        # Query and verify all inherited attributes
        result = AdminUser.manager(db).get(base_id="admin-001")[0]
        assert str(result.base_id) == "admin-001"  # From BaseSubject (level 1)
        assert str(result.credential) == "secret-token"  # From AbstractUser (level 2)
        assert str(result.full_name) == "Super Admin"  # From InternalUser (level 3)

    def test_sibling_types_have_different_attrs(self, schema_with_deep_inheritance):
        """Sibling types at same level have different specific attributes."""
        (
            db,
            _BaseSubject,
            _AbstractUser,
            InternalUser,
            ExternalUser,
            _AdminUser,
            BaseId,
            SpecialField,
        ) = schema_with_deep_inheritance

        # Insert internal user
        internal = InternalUser(
            base_id=BaseId("int-001"),
            full_name=FullName("Internal Employee"),
        )
        InternalUser.manager(db).insert(internal)

        # Insert external user
        external = ExternalUser(
            base_id=BaseId("ext-001"),
            company=SpecialField("Partner Corp"),
        )
        ExternalUser.manager(db).insert(external)

        # Query each
        internal_result = InternalUser.manager(db).all()
        external_result = ExternalUser.manager(db).all()

        assert len(internal_result) == 1
        assert len(external_result) == 1

        # Internal has full_name, External has company
        assert hasattr(internal_result[0], "full_name")
        assert hasattr(external_result[0], "company")

    def test_filter_on_inherited_attribute(self, schema_with_deep_inheritance):
        """Filter deepest subtype by inherited attribute."""
        (
            db,
            _BaseSubject,
            _AbstractUser,
            _InternalUser,
            _ExternalUser,
            AdminUser,
            BaseId,
            _SpecialField,
        ) = schema_with_deep_inheritance

        # Insert multiple admins
        AdminUser.manager(db).insert(
            AdminUser(
                base_id=BaseId("admin-a"),
                credential=Credential("token-a"),
                full_name=FullName("Admin A"),
            )
        )
        AdminUser.manager(db).insert(
            AdminUser(
                base_id=BaseId("admin-b"),
                credential=Credential("token-b"),
                full_name=FullName("Admin B"),
            )
        )

        # Filter by inherited attribute (credential from level 2)
        result = AdminUser.manager(db).filter(credential="token-a").execute()
        assert len(result) == 1
        assert str(result[0].full_name) == "Admin A"
