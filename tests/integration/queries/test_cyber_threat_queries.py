"""Tests for Cyber Threat Intelligence query patterns.

Tests STIX-style patterns including relation inheritance with role specialization,
multi-role relations, entity hierarchies, and complex threat modeling patterns.
Based on TypeDB Cyber Threat Intelligence example schema (STIX 2.1).
"""

from datetime import UTC, datetime

import pytest

from type_bridge import (
    AttributeFlags,
    Card,
    Database,
    DateTimeTZ,
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
# Test: STIX Domain Object Hierarchy
# =============================================================================


@pytest.mark.integration
class TestStixDomainObjectHierarchy:
    """Test STIX domain object entity hierarchy.

    stix-object (abstract) -> stix-domain-object (abstract) -> attack-pattern, campaign, etc.
    """

    @pytest.fixture
    def schema_with_stix_hierarchy(self, clean_db: Database):
        """Set up schema with STIX domain object hierarchy."""

        class StixId(String):
            """STIX identifier."""

            flags = AttributeFlags(name="stix_id_cti")

        class StixType(String):
            """STIX object type."""

            flags = AttributeFlags(name="stix_type_cti")

        class StixName(String):
            flags = AttributeFlags(name="stix_name_cti")

        class StixDescription(String):
            flags = AttributeFlags(name="stix_description_cti")

        class CreatedTime(DateTimeTZ):
            flags = AttributeFlags(name="created_cti")

        class ModifiedTime(DateTimeTZ):
            flags = AttributeFlags(name="modified_cti")

        class StixAlias(String):
            """STIX alias - can have multiple."""

            flags = AttributeFlags(name="stix_alias_cti")

        # Abstract base (stix-domain-object)
        class StixDomainObject(Entity):
            flags = TypeFlags(name="stix_domain_object_cti", abstract=True)
            stix_id: StixId = Flag(Key)
            stix_type: StixType
            created: CreatedTime
            modified: ModifiedTime

        # Concrete STIX types
        class AttackPattern(StixDomainObject):
            flags = TypeFlags(name="attack_pattern_cti")
            name: StixName
            description: StixDescription | None = None
            aliases: list[StixAlias] = Flag(Card(min=0))

        class Campaign(StixDomainObject):
            flags = TypeFlags(name="campaign_cti")
            name: StixName
            description: StixDescription | None = None
            aliases: list[StixAlias] = Flag(Card(min=0))

        class ThreatActor(StixDomainObject):
            flags = TypeFlags(name="threat_actor_cti")
            name: StixName
            description: StixDescription | None = None
            aliases: list[StixAlias] = Flag(Card(min=0))

        class Indicator(StixDomainObject):
            flags = TypeFlags(name="indicator_cti")
            name: StixName | None = None
            pattern: StixDescription  # Simplified - STIX has specific pattern syntax

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(StixDomainObject, AttackPattern, Campaign, ThreatActor, Indicator)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            StixDomainObject,
            AttackPattern,
            Campaign,
            ThreatActor,
            Indicator,
            StixId,
            StixType,
            StixName,
            StixDescription,
            CreatedTime,
            ModifiedTime,
            StixAlias,
        )

    def test_insert_attack_pattern(self, schema_with_stix_hierarchy):
        """Insert STIX attack pattern entity."""
        (
            db,
            StixDomainObject,
            AttackPattern,
            Campaign,
            ThreatActor,
            Indicator,
            StixId,
            StixType,
            StixName,
            StixDescription,
            CreatedTime,
            ModifiedTime,
            StixAlias,
        ) = schema_with_stix_hierarchy

        now = datetime.now(UTC)
        attack = AttackPattern(
            stix_id=StixId("attack-pattern--abc123"),
            stix_type=StixType("attack-pattern"),
            created=CreatedTime(now),
            modified=ModifiedTime(now),
            name=StixName("Spear Phishing"),
            description=StixDescription("Targeted email attacks"),
            aliases=[StixAlias("Phishing"), StixAlias("Social Engineering")],
        )
        AttackPattern.manager(db).insert(attack)

        result = AttackPattern.manager(db).get(stix_id="attack-pattern--abc123")
        assert len(result) == 1
        assert str(result[0].name) == "Spear Phishing"
        assert len(result[0].aliases) == 2

    def test_insert_multiple_stix_types(self, schema_with_stix_hierarchy):
        """Insert different STIX domain object types."""
        (
            db,
            StixDomainObject,
            AttackPattern,
            Campaign,
            ThreatActor,
            Indicator,
            StixId,
            StixType,
            StixName,
            StixDescription,
            CreatedTime,
            ModifiedTime,
            StixAlias,
        ) = schema_with_stix_hierarchy

        now = datetime.now(UTC)

        # Attack pattern
        AttackPattern.manager(db).insert(
            AttackPattern(
                stix_id=StixId("attack-pattern--001"),
                stix_type=StixType("attack-pattern"),
                created=CreatedTime(now),
                modified=ModifiedTime(now),
                name=StixName("SQL Injection"),
            )
        )

        # Campaign
        Campaign.manager(db).insert(
            Campaign(
                stix_id=StixId("campaign--001"),
                stix_type=StixType("campaign"),
                created=CreatedTime(now),
                modified=ModifiedTime(now),
                name=StixName("Operation Sunrise"),
            )
        )

        # Threat actor
        ThreatActor.manager(db).insert(
            ThreatActor(
                stix_id=StixId("threat-actor--001"),
                stix_type=StixType("threat-actor"),
                created=CreatedTime(now),
                modified=ModifiedTime(now),
                name=StixName("APT28"),
                aliases=[StixAlias("Fancy Bear"), StixAlias("Sofacy")],
            )
        )

        # Verify each type
        assert len(AttackPattern.manager(db).all()) == 1
        assert len(Campaign.manager(db).all()) == 1
        assert len(ThreatActor.manager(db).all()) == 1


# =============================================================================
# Test: STIX Relationship Objects
# =============================================================================


@pytest.mark.integration
class TestStixRelationships:
    """Test STIX relationship object patterns.

    STIX uses typed relationships (uses, targets, indicates, etc.)
    with source and target roles.
    """

    @pytest.fixture
    def schema_with_stix_relations(self, clean_db: Database):
        """Set up schema with STIX relationship types."""

        class StixId(String):
            flags = AttributeFlags(name="stix_id_rel")

        class StixName(String):
            flags = AttributeFlags(name="stix_name_rel")

        class RelationshipType(String):
            flags = AttributeFlags(name="relationship_type_rel")

        # Simplified STIX entities
        class StixThreatActor(Entity):
            flags = TypeFlags(name="threat_actor_rel")
            stix_id: StixId = Flag(Key)
            name: StixName

        class StixMalware(Entity):
            flags = TypeFlags(name="malware_rel")
            stix_id: StixId = Flag(Key)
            name: StixName

        class StixVulnerability(Entity):
            flags = TypeFlags(name="vulnerability_rel")
            stix_id: StixId = Flag(Key)
            name: StixName

        class StixInfrastructure(Entity):
            flags = TypeFlags(name="infrastructure_rel")
            stix_id: StixId = Flag(Key)
            name: StixName

        # Abstract STIX relationship (could be any STIX object as source/target)
        # For simplicity, we create typed relationships

        # "uses" relationship: threat-actor uses malware/tool
        class Uses(Relation):
            flags = TypeFlags(name="uses_rel")
            using_source: Role[StixThreatActor] = Role("using_source", StixThreatActor)
            used_target: Role[StixMalware] = Role("used_target", StixMalware)
            relationship_type: RelationshipType | None = None

        # "targets" relationship: malware targets vulnerability
        class Targets(Relation):
            flags = TypeFlags(name="targets_rel")
            targeting_source: Role[StixMalware] = Role("targeting_source", StixMalware)
            targeted_target: Role[StixVulnerability] = Role("targeted_target", StixVulnerability)

        # "hosts" relationship: infrastructure hosts malware
        class Hosts(Relation):
            flags = TypeFlags(name="hosts_rel")
            hosting_source: Role[StixInfrastructure] = Role("hosting_source", StixInfrastructure)
            hosted_target: Role[StixMalware] = Role("hosted_target", StixMalware)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(
            StixThreatActor,
            StixMalware,
            StixVulnerability,
            StixInfrastructure,
            Uses,
            Targets,
            Hosts,
        )
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            StixThreatActor,
            StixMalware,
            StixVulnerability,
            StixInfrastructure,
            Uses,
            Targets,
            Hosts,
            StixId,
            StixName,
            RelationshipType,
        )

    def test_threat_actor_uses_malware(self, schema_with_stix_relations):
        """Create 'uses' relationship between threat actor and malware."""
        (
            db,
            StixThreatActor,
            StixMalware,
            StixVulnerability,
            StixInfrastructure,
            Uses,
            Targets,
            Hosts,
            StixId,
            StixName,
            RelationshipType,
        ) = schema_with_stix_relations

        # Create entities
        actor = StixThreatActor(stix_id=StixId("threat-actor--apt29"), name=StixName("APT29"))
        malware = StixMalware(stix_id=StixId("malware--cozy-bear"), name=StixName("CozyDuke"))

        StixThreatActor.manager(db).insert(actor)
        StixMalware.manager(db).insert(malware)

        # Fetch
        actor_f = StixThreatActor.manager(db).get(stix_id="threat-actor--apt29")[0]
        malware_f = StixMalware.manager(db).get(stix_id="malware--cozy-bear")[0]

        # Create relationship
        Uses.manager(db).insert(
            Uses(
                using_source=actor_f,
                used_target=malware_f,
                relationship_type=RelationshipType("uses"),
            )
        )

        # Verify
        uses_rels = Uses.manager(db).all()
        assert len(uses_rels) == 1
        assert str(uses_rels[0].using_source.name) == "APT29"
        assert str(uses_rels[0].used_target.name) == "CozyDuke"

    def test_threat_intelligence_chain(self, schema_with_stix_relations):
        """Create chain: actor -> uses -> malware -> targets -> vulnerability."""
        (
            db,
            StixThreatActor,
            StixMalware,
            StixVulnerability,
            StixInfrastructure,
            Uses,
            Targets,
            Hosts,
            StixId,
            StixName,
            RelationshipType,
        ) = schema_with_stix_relations

        # Create entities
        actor = StixThreatActor(stix_id=StixId("threat-actor--x"), name=StixName("Threat Group X"))
        malware = StixMalware(stix_id=StixId("malware--y"), name=StixName("Malware Y"))
        vuln = StixVulnerability(stix_id=StixId("vulnerability--z"), name=StixName("CVE-2023-1234"))

        StixThreatActor.manager(db).insert(actor)
        StixMalware.manager(db).insert(malware)
        StixVulnerability.manager(db).insert(vuln)

        # Fetch
        actor_f = StixThreatActor.manager(db).get(stix_id="threat-actor--x")[0]
        malware_f = StixMalware.manager(db).get(stix_id="malware--y")[0]
        vuln_f = StixVulnerability.manager(db).get(stix_id="vulnerability--z")[0]

        # Create chain
        Uses.manager(db).insert(Uses(using_source=actor_f, used_target=malware_f))
        Targets.manager(db).insert(Targets(targeting_source=malware_f, targeted_target=vuln_f))

        # Verify chain
        assert len(Uses.manager(db).all()) == 1
        assert len(Targets.manager(db).all()) == 1


# =============================================================================
# Test: Sighting Pattern (Multi-role Relation)
# =============================================================================


@pytest.mark.integration
class TestSightingPattern:
    """Test STIX sighting pattern - relation with multiple optional roles.

    Sighting: sighting-of (what was seen), observed-data (evidence), where-sighted (location)
    """

    @pytest.fixture
    def schema_with_sighting(self, clean_db: Database):
        """Set up schema with sighting relation pattern."""

        class StixId(String):
            flags = AttributeFlags(name="stix_id_sight")

        class StixName(String):
            flags = AttributeFlags(name="stix_name_sight")

        class SightingCount(Integer):
            flags = AttributeFlags(name="count_sight")

        class FirstSeen(DateTimeTZ):
            flags = AttributeFlags(name="first_seen_sight")

        class LastSeen(DateTimeTZ):
            flags = AttributeFlags(name="last_seen_sight")

        # What can be sighted
        class SightIndicator(Entity):
            flags = TypeFlags(name="indicator_sight")
            stix_id: StixId = Flag(Key)
            name: StixName

        # Evidence of sighting
        class SightObservedData(Entity):
            flags = TypeFlags(name="observed_data_sight")
            stix_id: StixId = Flag(Key)

        # Where it was sighted
        class SightIdentity(Entity):
            flags = TypeFlags(name="identity_sight")
            stix_id: StixId = Flag(Key)
            name: StixName

        # Sighting relation - links indicator to evidence and location
        class Sighting(Relation):
            flags = TypeFlags(name="sighting_rel")
            sighting_of: Role[SightIndicator] = Role("sighting_of", SightIndicator)
            observed_data: Role[SightObservedData] = Role("observed_data", SightObservedData)
            where_sighted: Role[SightIdentity] = Role("where_sighted", SightIdentity)
            count: SightingCount | None = None
            first_seen: FirstSeen | None = None
            last_seen: LastSeen | None = None

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(SightIndicator, SightObservedData, SightIdentity, Sighting)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            SightIndicator,
            SightObservedData,
            SightIdentity,
            Sighting,
            StixId,
            StixName,
            SightingCount,
            FirstSeen,
            LastSeen,
        )

    def test_create_sighting_with_all_roles(self, schema_with_sighting):
        """Create sighting with all three roles populated."""
        (
            db,
            SightIndicator,
            SightObservedData,
            SightIdentity,
            Sighting,
            StixId,
            StixName,
            SightingCount,
            FirstSeen,
            LastSeen,
        ) = schema_with_sighting

        # Create entities
        indicator = SightIndicator(
            stix_id=StixId("indicator--malware-hash"), name=StixName("Malware Hash IOC")
        )
        observed = SightObservedData(stix_id=StixId("observed-data--log-entry"))
        location = SightIdentity(stix_id=StixId("identity--org-a"), name=StixName("Organization A"))

        SightIndicator.manager(db).insert(indicator)
        SightObservedData.manager(db).insert(observed)
        SightIdentity.manager(db).insert(location)

        # Fetch
        indicator_f = SightIndicator.manager(db).get(stix_id="indicator--malware-hash")[0]
        observed_f = SightObservedData.manager(db).get(stix_id="observed-data--log-entry")[0]
        location_f = SightIdentity.manager(db).get(stix_id="identity--org-a")[0]

        now = datetime.now(UTC)
        sighting = Sighting(
            sighting_of=indicator_f,
            observed_data=observed_f,
            where_sighted=location_f,
            count=SightingCount(5),
            first_seen=FirstSeen(now),
            last_seen=LastSeen(now),
        )
        Sighting.manager(db).insert(sighting)

        # Verify
        sightings = Sighting.manager(db).all()
        assert len(sightings) == 1
        assert int(sightings[0].count) == 5
        assert str(sightings[0].where_sighted.name) == "Organization A"


# =============================================================================
# Test: Cyber Observable Objects
# =============================================================================


@pytest.mark.integration
class TestCyberObservables:
    """Test STIX cyber observable object patterns (SCOs)."""

    @pytest.fixture
    def schema_with_observables(self, clean_db: Database):
        """Set up schema with cyber observable objects."""

        class StixId(String):
            flags = AttributeFlags(name="stix_id_sco")

        class IpValue(String):
            flags = AttributeFlags(name="ip_value_sco")

        class DomainValue(String):
            flags = AttributeFlags(name="domain_value_sco")

        class FileName(String):
            flags = AttributeFlags(name="file_name_sco")

        class FileHash(String):
            flags = AttributeFlags(name="file_hash_sco")

        class FileSize(Integer):
            flags = AttributeFlags(name="file_size_sco")

        # Abstract SCO base
        class CyberObservable(Entity):
            flags = TypeFlags(name="cyber_observable_sco", abstract=True)
            stix_id: StixId = Flag(Key)

        class Ipv4Address(CyberObservable):
            flags = TypeFlags(name="ipv4_addr_sco")
            value: IpValue

        class DomainName(CyberObservable):
            flags = TypeFlags(name="domain_name_sco")
            value: DomainValue

        class FileObject(CyberObservable):
            flags = TypeFlags(name="file_sco")
            name: FileName | None = None
            hashes: list[FileHash] = Flag(Card(min=0))
            size: FileSize | None = None

        # Resolves-to relationship (domain -> IP)
        class ResolvesTo(Relation):
            flags = TypeFlags(name="resolves_to_sco")
            resolving_source: Role[DomainName] = Role("resolving_source", DomainName)
            resolved_target: Role[Ipv4Address] = Role("resolved_target", Ipv4Address)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(CyberObservable, Ipv4Address, DomainName, FileObject, ResolvesTo)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            CyberObservable,
            Ipv4Address,
            DomainName,
            FileObject,
            ResolvesTo,
            StixId,
            IpValue,
            DomainValue,
            FileName,
            FileHash,
            FileSize,
        )

    def test_insert_ip_address(self, schema_with_observables):
        """Insert IPv4 address observable."""
        (
            db,
            CyberObservable,
            Ipv4Address,
            DomainName,
            FileObject,
            ResolvesTo,
            StixId,
            IpValue,
            DomainValue,
            FileName,
            FileHash,
            FileSize,
        ) = schema_with_observables

        ip = Ipv4Address(stix_id=StixId("ipv4-addr--192-168-1-1"), value=IpValue("192.168.1.1"))
        Ipv4Address.manager(db).insert(ip)

        result = Ipv4Address.manager(db).all()
        assert len(result) == 1
        assert str(result[0].value) == "192.168.1.1"

    def test_file_with_multiple_hashes(self, schema_with_observables):
        """Insert file observable with multiple hash values."""
        (
            db,
            CyberObservable,
            Ipv4Address,
            DomainName,
            FileObject,
            ResolvesTo,
            StixId,
            IpValue,
            DomainValue,
            FileName,
            FileHash,
            FileSize,
        ) = schema_with_observables

        file_obj = FileObject(
            stix_id=StixId("file--malware-sample"),
            name=FileName("malware.exe"),
            hashes=[
                FileHash("d41d8cd98f00b204e9800998ecf8427e"),  # MD5
                FileHash("da39a3ee5e6b4b0d3255bfef95601890afd80709"),  # SHA1
            ],
            size=FileSize(1024),
        )
        FileObject.manager(db).insert(file_obj)

        result = FileObject.manager(db).get(stix_id="file--malware-sample")
        assert len(result) == 1
        assert len(result[0].hashes) == 2
        assert int(result[0].size) == 1024

    def test_domain_resolves_to_ip(self, schema_with_observables):
        """Create resolves-to relationship between domain and IP."""
        (
            db,
            CyberObservable,
            Ipv4Address,
            DomainName,
            FileObject,
            ResolvesTo,
            StixId,
            IpValue,
            DomainValue,
            FileName,
            FileHash,
            FileSize,
        ) = schema_with_observables

        # Create observables
        domain = DomainName(stix_id=StixId("domain--evil-com"), value=DomainValue("evil.com"))
        ip = Ipv4Address(stix_id=StixId("ipv4-addr--evil-ip"), value=IpValue("10.0.0.1"))

        DomainName.manager(db).insert(domain)
        Ipv4Address.manager(db).insert(ip)

        # Fetch
        domain_f = DomainName.manager(db).get(stix_id="domain--evil-com")[0]
        ip_f = Ipv4Address.manager(db).get(stix_id="ipv4-addr--evil-ip")[0]

        # Create relationship
        ResolvesTo.manager(db).insert(ResolvesTo(resolving_source=domain_f, resolved_target=ip_f))

        # Verify
        resolutions = ResolvesTo.manager(db).all()
        assert len(resolutions) == 1
        assert str(resolutions[0].resolving_source.value) == "evil.com"
        assert str(resolutions[0].resolved_target.value) == "10.0.0.1"


# =============================================================================
# Test: Marking Definitions and References
# =============================================================================


@pytest.mark.integration
class TestMarkingDefinitions:
    """Test STIX marking definition patterns (TLP, etc.)."""

    @pytest.fixture
    def schema_with_markings(self, clean_db: Database):
        """Set up schema with marking definitions."""

        class StixId(String):
            flags = AttributeFlags(name="stix_id_mark")

        class StixName(String):
            flags = AttributeFlags(name="stix_name_mark")

        class TlpLevel(String):
            """Traffic Light Protocol level."""

            flags = AttributeFlags(name="tlp_mark")

        class Statement(String):
            """Marking statement."""

            flags = AttributeFlags(name="statement_mark")

        # Marking definition
        class MarkingDefinition(Entity):
            flags = TypeFlags(name="marking_definition")
            stix_id: StixId = Flag(Key)
            name: StixName | None = None
            tlp: TlpLevel | None = None  # For TLP markings
            statement: Statement | None = None  # For statement markings

        # Object that can be marked
        class MarkedObject(Entity):
            flags = TypeFlags(name="marked_object")
            stix_id: StixId = Flag(Key)
            name: StixName

        # Object marking relation
        class ObjectMarking(Relation):
            flags = TypeFlags(name="object_marking")
            marked_object: Role[MarkedObject] = Role("marked_object", MarkedObject)
            marking: Role[MarkingDefinition] = Role("marking", MarkingDefinition)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(MarkingDefinition, MarkedObject, ObjectMarking)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            MarkingDefinition,
            MarkedObject,
            ObjectMarking,
            StixId,
            StixName,
            TlpLevel,
            Statement,
        )

    def test_create_tlp_marking(self, schema_with_markings):
        """Create TLP marking definition."""
        (
            db,
            MarkingDefinition,
            MarkedObject,
            ObjectMarking,
            StixId,
            StixName,
            TlpLevel,
            Statement,
        ) = schema_with_markings

        # Create TLP:RED marking
        tlp_red = MarkingDefinition(
            stix_id=StixId("marking-definition--tlp-red"),
            name=StixName("TLP:RED"),
            tlp=TlpLevel("red"),
        )
        MarkingDefinition.manager(db).insert(tlp_red)

        result = MarkingDefinition.manager(db).get(stix_id="marking-definition--tlp-red")
        assert len(result) == 1
        assert str(result[0].tlp) == "red"

    def test_apply_marking_to_object(self, schema_with_markings):
        """Apply marking definition to an object."""
        (
            db,
            MarkingDefinition,
            MarkedObject,
            ObjectMarking,
            StixId,
            StixName,
            TlpLevel,
            Statement,
        ) = schema_with_markings

        # Create marking and object
        marking = MarkingDefinition(
            stix_id=StixId("marking-definition--tlp-amber"),
            name=StixName("TLP:AMBER"),
            tlp=TlpLevel("amber"),
        )
        obj = MarkedObject(
            stix_id=StixId("indicator--sensitive"),
            name=StixName("Sensitive Indicator"),
        )

        MarkingDefinition.manager(db).insert(marking)
        MarkedObject.manager(db).insert(obj)

        # Fetch and apply marking
        marking_f = MarkingDefinition.manager(db).get(stix_id="marking-definition--tlp-amber")[0]
        obj_f = MarkedObject.manager(db).get(stix_id="indicator--sensitive")[0]

        ObjectMarking.manager(db).insert(ObjectMarking(marked_object=obj_f, marking=marking_f))

        # Verify
        markings = ObjectMarking.manager(db).all()
        assert len(markings) == 1
        assert str(markings[0].marking.tlp) == "amber"

    def test_multiple_markings_on_object(self, schema_with_markings):
        """Apply multiple markings to same object."""
        (
            db,
            MarkingDefinition,
            MarkedObject,
            ObjectMarking,
            StixId,
            StixName,
            TlpLevel,
            Statement,
        ) = schema_with_markings

        # Create markings
        tlp = MarkingDefinition(
            stix_id=StixId("marking--tlp"),
            name=StixName("TLP:GREEN"),
            tlp=TlpLevel("green"),
        )
        statement = MarkingDefinition(
            stix_id=StixId("marking--statement"),
            name=StixName("Copyright Notice"),
            statement=Statement("Copyright 2023"),
        )
        MarkingDefinition.manager(db).insert(tlp)
        MarkingDefinition.manager(db).insert(statement)

        # Create object
        obj = MarkedObject(stix_id=StixId("report--001"), name=StixName("Threat Report"))
        MarkedObject.manager(db).insert(obj)

        # Fetch all
        tlp_f = MarkingDefinition.manager(db).get(stix_id="marking--tlp")[0]
        statement_f = MarkingDefinition.manager(db).get(stix_id="marking--statement")[0]
        obj_f = MarkedObject.manager(db).get(stix_id="report--001")[0]

        # Apply both markings
        ObjectMarking.manager(db).insert(ObjectMarking(marked_object=obj_f, marking=tlp_f))
        ObjectMarking.manager(db).insert(ObjectMarking(marked_object=obj_f, marking=statement_f))

        # Verify
        markings = ObjectMarking.manager(db).all()
        assert len(markings) == 2


# =============================================================================
# Test: Advanced Threat Intelligence Queries
# =============================================================================


@pytest.mark.integration
class TestThreatIntelligenceAggregations:
    """Test aggregations and complex queries on threat intelligence data."""

    @pytest.fixture
    def schema_with_campaign_data(self, clean_db: Database):
        """Schema with campaigns, threat actors, and attack patterns for analytics."""

        class StixId(String):
            pass

        class StixName(String):
            pass

        class SeverityLevel(Integer):
            pass

        class FirstSeen(DateTimeTZ):
            pass

        class ThreatActor(Entity):
            flags = TypeFlags(name="threat_actor_camp")
            stix_id: StixId = Flag(Key)
            name: StixName
            sophistication: SeverityLevel  # 1-5 scale

        class Campaign(Entity):
            flags = TypeFlags(name="campaign_camp")
            stix_id: StixId = Flag(Key)
            name: StixName
            first_seen: FirstSeen

        class AttackPattern(Entity):
            flags = TypeFlags(name="attack_pattern_camp")
            stix_id: StixId = Flag(Key)
            name: StixName
            severity: SeverityLevel  # 1-10 scale

        class Attribution(Relation):
            """Threat actor attributed to campaign."""

            flags = TypeFlags(name="attribution_camp")
            actor: Role[ThreatActor] = Role("actor", ThreatActor)
            campaign: Role[Campaign] = Role("campaign", Campaign)

        class Uses(Relation):
            """Campaign uses attack pattern."""

            flags = TypeFlags(name="uses_camp")
            campaign: Role[Campaign] = Role("campaign", Campaign)
            attack_pattern: Role[AttackPattern] = Role("attack_pattern", AttackPattern)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(ThreatActor, Campaign, AttackPattern, Attribution, Uses)
        schema_manager.sync_schema(force=True)

        return (
            clean_db,
            ThreatActor,
            Campaign,
            AttackPattern,
            Attribution,
            Uses,
            StixId,
            StixName,
            SeverityLevel,
            FirstSeen,
        )

    def test_count_campaigns_per_threat_actor(self, schema_with_campaign_data):
        """Count number of campaigns attributed to each threat actor."""
        (
            db,
            ThreatActor,
            Campaign,
            AttackPattern,
            Attribution,
            Uses,
            StixId,
            StixName,
            SeverityLevel,
            FirstSeen,
        ) = schema_with_campaign_data

        # Create threat actors with varying activity levels
        ThreatActor.manager(db).insert(
            ThreatActor(
                stix_id=StixId("threat-actor--apt28"),
                name=StixName("APT28"),
                sophistication=SeverityLevel(5),
            )
        )
        ThreatActor.manager(db).insert(
            ThreatActor(
                stix_id=StixId("threat-actor--apt29"),
                name=StixName("APT29"),
                sophistication=SeverityLevel(5),
            )
        )
        ThreatActor.manager(db).insert(
            ThreatActor(
                stix_id=StixId("threat-actor--script-kiddie"),
                name=StixName("Script Kiddie"),
                sophistication=SeverityLevel(1),
            )
        )

        apt28 = ThreatActor.manager(db).get(stix_id="threat-actor--apt28")[0]
        apt29 = ThreatActor.manager(db).get(stix_id="threat-actor--apt29")[0]
        script_kiddie = ThreatActor.manager(db).get(stix_id="threat-actor--script-kiddie")[0]

        now = datetime.now(UTC)

        # APT28 runs many campaigns
        for i in range(5):
            Campaign.manager(db).insert(
                Campaign(
                    stix_id=StixId(f"campaign--apt28-{i}"),
                    name=StixName(f"APT28 Campaign {i}"),
                    first_seen=FirstSeen(now),
                )
            )
            campaign = Campaign.manager(db).get(stix_id=f"campaign--apt28-{i}")[0]
            Attribution.manager(db).insert(Attribution(actor=apt28, campaign=campaign))

        # APT29 runs fewer campaigns
        for i in range(2):
            Campaign.manager(db).insert(
                Campaign(
                    stix_id=StixId(f"campaign--apt29-{i}"),
                    name=StixName(f"APT29 Campaign {i}"),
                    first_seen=FirstSeen(now),
                )
            )
            campaign = Campaign.manager(db).get(stix_id=f"campaign--apt29-{i}")[0]
            Attribution.manager(db).insert(Attribution(actor=apt29, campaign=campaign))

        # Script kiddie runs one campaign
        Campaign.manager(db).insert(
            Campaign(
                stix_id=StixId("campaign--noob"),
                name=StixName("Script Kiddie Attack"),
                first_seen=FirstSeen(now),
            )
        )
        noob_campaign = Campaign.manager(db).get(stix_id="campaign--noob")[0]
        Attribution.manager(db).insert(Attribution(actor=script_kiddie, campaign=noob_campaign))

        # Count campaigns per actor
        apt28_count = Attribution.manager(db).filter(actor=apt28).count()
        apt29_count = Attribution.manager(db).filter(actor=apt29).count()
        noob_count = Attribution.manager(db).filter(actor=script_kiddie).count()

        assert apt28_count == 5
        assert apt29_count == 2
        assert noob_count == 1

    def test_filter_high_severity_attack_patterns(self, schema_with_campaign_data):
        """Filter attack patterns by severity level."""
        (
            db,
            ThreatActor,
            Campaign,
            AttackPattern,
            Attribution,
            Uses,
            StixId,
            StixName,
            SeverityLevel,
            FirstSeen,
        ) = schema_with_campaign_data

        # Create attack patterns with varying severity
        patterns = [
            ("Phishing", 3),
            ("Spear Phishing", 6),
            ("Zero Day Exploit", 10),
            ("Credential Stuffing", 4),
            ("Supply Chain Attack", 9),
        ]

        for name, severity in patterns:
            AttackPattern.manager(db).insert(
                AttackPattern(
                    stix_id=StixId(f"attack-pattern--{name.lower().replace(' ', '-')}"),
                    name=StixName(name),
                    severity=SeverityLevel(severity),
                )
            )

        # Filter by high severity (>= 7)
        high_severity = (
            AttackPattern.manager(db).filter(SeverityLevel.gte(SeverityLevel(7))).execute()
        )
        assert len(high_severity) == 2
        high_severity_names = {p.name.value for p in high_severity}
        assert high_severity_names == {"Zero Day Exploit", "Supply Chain Attack"}

        # Filter by low severity (< 5)
        low_severity = (
            AttackPattern.manager(db).filter(SeverityLevel.lt(SeverityLevel(5))).execute()
        )
        assert len(low_severity) == 2
        low_severity_names = {p.name.value for p in low_severity}
        assert low_severity_names == {"Phishing", "Credential Stuffing"}

    def test_order_threat_actors_by_sophistication(self, schema_with_campaign_data):
        """Order threat actors by sophistication level."""
        (
            db,
            ThreatActor,
            Campaign,
            AttackPattern,
            Attribution,
            Uses,
            StixId,
            StixName,
            SeverityLevel,
            FirstSeen,
        ) = schema_with_campaign_data

        actors = [
            ("Nation State", 5),
            ("Hacktivist", 2),
            ("Organized Crime", 4),
            ("Script Kiddie", 1),
            ("APT Group", 5),
        ]

        for name, soph in actors:
            ThreatActor.manager(db).insert(
                ThreatActor(
                    stix_id=StixId(f"threat-actor--{name.lower().replace(' ', '-')}"),
                    name=StixName(name),
                    sophistication=SeverityLevel(soph),
                )
            )

        # Order by sophistication descending
        ordered = ThreatActor.manager(db).filter().order_by("-sophistication").execute()
        levels = [a.sophistication.value for a in ordered]
        assert levels == sorted(levels, reverse=True)

        # Get top 2 most sophisticated
        top2 = ThreatActor.manager(db).filter().order_by("-sophistication").limit(2).execute()
        assert len(top2) == 2
        assert all(a.sophistication.value == 5 for a in top2)


# =============================================================================
# Test: Complex STIX Relationship Queries
# =============================================================================


@pytest.mark.integration
class TestComplexStixRelationships:
    """Test complex queries on STIX relationship patterns."""

    @pytest.fixture
    def schema_with_attack_chain(self, clean_db: Database):
        """Schema for modeling attack chains (kill chain phases)."""

        class StixId(String):
            pass

        class StixName(String):
            pass

        class PhaseOrder(Integer):
            pass

        class KillChainPhase(Entity):
            flags = TypeFlags(name="kill_chain_phase")
            stix_id: StixId = Flag(Key)
            name: StixName
            order: PhaseOrder

        class AttackPattern(Entity):
            flags = TypeFlags(name="attack_pattern_kc")
            stix_id: StixId = Flag(Key)
            name: StixName

        class PhaseMapping(Relation):
            """Maps attack patterns to kill chain phases."""

            flags = TypeFlags(name="phase_mapping")
            attack_pattern: Role[AttackPattern] = Role("attack_pattern", AttackPattern)
            phase: Role[KillChainPhase] = Role("phase", KillChainPhase)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(KillChainPhase, AttackPattern, PhaseMapping)
        schema_manager.sync_schema(force=True)

        return clean_db, KillChainPhase, AttackPattern, PhaseMapping, StixId, StixName, PhaseOrder

    def test_query_attack_patterns_by_kill_chain_phase(self, schema_with_attack_chain):
        """Find all attack patterns in a specific kill chain phase."""
        db, KillChainPhase, AttackPattern, PhaseMapping, StixId, StixName, PhaseOrder = (
            schema_with_attack_chain
        )

        # Create kill chain phases (Lockheed Martin Cyber Kill Chain)
        phases = [
            ("Reconnaissance", 1),
            ("Weaponization", 2),
            ("Delivery", 3),
            ("Exploitation", 4),
            ("Installation", 5),
            ("Command & Control", 6),
            ("Actions on Objectives", 7),
        ]

        for name, order in phases:
            KillChainPhase.manager(db).insert(
                KillChainPhase(
                    stix_id=StixId(f"phase--{order}"),
                    name=StixName(name),
                    order=PhaseOrder(order),
                )
            )

        # Create attack patterns
        attack_patterns = [
            ("Port Scanning", "Reconnaissance"),
            ("OSINT Gathering", "Reconnaissance"),
            ("Malware Development", "Weaponization"),
            ("Phishing Email", "Delivery"),
            ("Spear Phishing", "Delivery"),
            ("Drive-by Download", "Delivery"),
            ("Buffer Overflow", "Exploitation"),
            ("Backdoor Installation", "Installation"),
            ("DNS Tunneling", "Command & Control"),
            ("Data Exfiltration", "Actions on Objectives"),
        ]

        for pattern_name, phase_name in attack_patterns:
            AttackPattern.manager(db).insert(
                AttackPattern(
                    stix_id=StixId(f"attack-pattern--{pattern_name.lower().replace(' ', '-')}"),
                    name=StixName(pattern_name),
                )
            )
            pattern = AttackPattern.manager(db).get(
                stix_id=f"attack-pattern--{pattern_name.lower().replace(' ', '-')}"
            )[0]

            phase = next(p for p in KillChainPhase.manager(db).all() if p.name.value == phase_name)
            PhaseMapping.manager(db).insert(PhaseMapping(attack_pattern=pattern, phase=phase))

        # Query patterns in "Delivery" phase
        delivery_phase = (
            KillChainPhase.manager(db).filter(StixName.eq(StixName("Delivery"))).execute()[0]
        )

        delivery_mappings = PhaseMapping.manager(db).filter(phase=delivery_phase).execute()
        assert len(delivery_mappings) == 3

        delivery_pattern_names = {m.attack_pattern.name.value for m in delivery_mappings}
        assert delivery_pattern_names == {"Phishing Email", "Spear Phishing", "Drive-by Download"}

    def test_count_patterns_per_phase(self, schema_with_attack_chain):
        """Count attack patterns in each kill chain phase."""
        db, KillChainPhase, AttackPattern, PhaseMapping, StixId, StixName, PhaseOrder = (
            schema_with_attack_chain
        )

        # Create phases
        for name, order in [("Phase1", 1), ("Phase2", 2), ("Phase3", 3)]:
            KillChainPhase.manager(db).insert(
                KillChainPhase(
                    stix_id=StixId(f"phase--{order}"),
                    name=StixName(name),
                    order=PhaseOrder(order),
                )
            )

        phases = {p.name.value: p for p in KillChainPhase.manager(db).all()}

        # Create patterns with varying distribution
        # Phase1: 5 patterns, Phase2: 3 patterns, Phase3: 2 patterns
        pattern_counts = {"Phase1": 5, "Phase2": 3, "Phase3": 2}

        pattern_id = 0
        for phase_name, count in pattern_counts.items():
            for _ in range(count):
                AttackPattern.manager(db).insert(
                    AttackPattern(
                        stix_id=StixId(f"pattern--{pattern_id}"),
                        name=StixName(f"Pattern {pattern_id}"),
                    )
                )
                pattern = AttackPattern.manager(db).get(stix_id=f"pattern--{pattern_id}")[0]
                PhaseMapping.manager(db).insert(
                    PhaseMapping(attack_pattern=pattern, phase=phases[phase_name])
                )
                pattern_id += 1

        # Verify counts per phase
        for phase_name, expected_count in pattern_counts.items():
            actual_count = PhaseMapping.manager(db).filter(phase=phases[phase_name]).count()
            assert actual_count == expected_count, (
                f"Phase {phase_name}: expected {expected_count}, got {actual_count}"
            )

    def test_order_phases_and_paginate(self, schema_with_attack_chain):
        """Order kill chain phases and use pagination."""
        db, KillChainPhase, AttackPattern, PhaseMapping, StixId, StixName, PhaseOrder = (
            schema_with_attack_chain
        )

        # Create 7 phases
        for i in range(1, 8):
            KillChainPhase.manager(db).insert(
                KillChainPhase(
                    stix_id=StixId(f"phase--{i}"),
                    name=StixName(f"Phase {i}"),
                    order=PhaseOrder(i),
                )
            )

        # Order by phase order
        ordered = KillChainPhase.manager(db).filter().order_by("order").execute()
        orders = [p.order.value for p in ordered]
        assert orders == list(range(1, 8))

        # Get middle 3 phases (offset 2, limit 3)
        middle = KillChainPhase.manager(db).filter().order_by("order").offset(2).limit(3).execute()
        assert len(middle) == 3
        middle_orders = [p.order.value for p in middle]
        assert middle_orders == [3, 4, 5]
